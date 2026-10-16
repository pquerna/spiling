// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Import enclosure and denominator admission share one whole-domain pass.
//! Source knot spans qualify jets; only their position bounds are used here.
//! Mixed weights require actual denominator exclusion, never a coefficient hull.
use super::{admission_error, error, invalid};
use crate::{
    GeometryWorkBudget, check_cancel,
    parametric::{
        Interval, curve_enclosure, curve_parameter_spans, native_error, surface_enclosure,
        surface_parameter_boxes,
    },
};
use monstertruck_modeling::{
    BoundedCurve, Curve, Curve2D, ParametricSurface, Solid, Surface, ValidatedGeometryBudget,
    bounding::validated_face_bounding_box_with_boundary_enclosure,
};
use spiling_contracts::geometry::{AabbMm, FaceInfo, GeometryError, GeometryErrorCode};
use std::sync::atomic::AtomicBool;

const MAX_IMPORT_BOUND_WORK: u64 = 20_000_000;
const MAX_CONDITIONING_DEPTH: u8 = 48;
const MAX_IMPORT_BOUND_BYTES: usize = 32 * 1024 * 1024;

fn include<const N: usize>(
    bounds: &mut Option<[Interval; N]>,
    value: [Interval; N],
) -> Result<(), GeometryError> {
    if value
        .iter()
        .any(|v| !v.lo.is_finite() || !v.hi.is_finite() || v.lo > v.hi)
    {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "native import enclosure is nonfinite",
        ));
    }
    *bounds = Some(match *bounds {
        Some(previous) => std::array::from_fn(|axis| previous[axis].hull(value[axis])),
        None => value,
    });
    Ok(())
}

// Depth-first refinement owns at most one pending sibling per depth. All native
// attempts, including unsuccessful attempts and cancellation, consume one budget.
fn refine<const D: usize, const N: usize>(
    domain: [[f64; 2]; D],
    bounds: &mut Option<[Interval; N]>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut enclose: impl FnMut(
        [[f64; 2]; D],
        &mut GeometryWorkBudget,
    ) -> Result<[Interval; N], GeometryError>,
) -> Result<(), GeometryError> {
    for range in domain {
        Interval::new(range[0], range[1])?;
    }
    let mut stack = [(domain, 0u8); MAX_CONDITIONING_DEPTH as usize + 1];
    let mut live = 1;
    while live != 0 {
        live -= 1;
        let (cell, depth) = stack[live];
        work.charge(1, cancel)?;
        match enclose(cell, work) {
            Ok(value) => include(bounds, value)?,
            Err(cause) if cause.code == GeometryErrorCode::ConditioningFailure => {
                let axis = (0..D)
                    .max_by(|&a, &b| {
                        (cell[a][1] - cell[a][0]).total_cmp(&(cell[b][1] - cell[b][0]))
                    })
                    .unwrap();
                let mid = cell[axis][0] * 0.5 + cell[axis][1] * 0.5;
                if depth == MAX_CONDITIONING_DEPTH || mid <= cell[axis][0] || mid >= cell[axis][1] {
                    return Err(error(
                        GeometryErrorCode::ConditioningFailure,
                        "source denominator remains unresolved over a bounded native interval",
                    ));
                }
                let mut left = cell;
                let mut right = cell;
                left[axis][1] = mid;
                right[axis][0] = mid;
                stack[live] = (right, depth + 1);
                stack[live + 1] = (left, depth + 1);
                live += 2;
            }
            Err(cause) => return Err(cause),
        }
    }
    Ok(())
}

fn curve_bounds(
    curve: &Curve,
    bounds: &mut Option<[Interval; 3]>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let mut basis = curve;
    let mut depth = 0;
    while let Curve::TrimmedCurve(wrapper) = basis {
        work.charge(1, cancel)?;
        if depth == 32 {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "native import curve nesting exceeds bound",
            ));
        }
        basis = wrapper.entity().curve().as_ref();
        depth += 1;
    }
    let range = curve.range_tuple();
    if matches!(basis, Curve::IntersectionCurve(_)) {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "original implicit import bounds require its admitted retained continuation",
        ));
    }
    for span in curve_parameter_spans(curve, [range.0, range.1], work, cancel)? {
        refine([span], bounds, work, cancel, |cell, work| {
            Ok(curve_enclosure(curve, (cell[0][0], cell[0][1]), work, cancel)?.position)
        })?;
    }
    Ok(())
}

fn surface_bounds(
    surface: &Surface,
    domain: [[f64; 2]; 2],
    bounds: &mut Option<[Interval; 3]>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    for cell in surface_parameter_boxes(surface, domain, work, cancel)? {
        refine(cell, bounds, work, cancel, |cell, work| {
            Ok(surface_enclosure(surface, cell, work, cancel)?.position)
        })?;
    }
    Ok(())
}

/// Coarse per-face enclosures remain valid even when exact trim preparation is
/// unavailable. Carry their original work/allocation authority into the later
/// Definition-ready tightening pass; never replenish either allowance.
pub(super) struct ImportBoundsRefinement {
    faces: Vec<[Interval; 3]>,
    work: GeometryWorkBudget,
    allocated_bytes: usize,
}

type OriginalBoundarySlot = (
    monstertruck_topology::compress::CompressedEdgeIndex,
    (usize, usize),
);

/// Aligned original/native geometry tables admitted for one definition.
pub(super) struct ImportBoundsInput<'a> {
    pub solid: &'a Solid,
    pub faces: &'a [FaceInfo],
    pub source_trims: &'a [Vec<Vec<Option<Curve2D>>>],
    pub parameters: &'a [crate::kernel::FaceParameterWitness],
    pub continuations: &'a [Option<crate::kernel::OriginalBoundaryContinuation>],
    pub proofs: &'a [monstertruck_modeling::PreparedValidatedCurve<'static>],
    pub edge_count: usize,
    pub source_slots: &'a [Vec<Vec<OriginalBoundarySlot>>],
}

pub(super) fn import_bounds(
    input: ImportBoundsInput<'_>,
    cancel: &AtomicBool,
) -> Result<(AabbMm, ImportBoundsRefinement), GeometryError> {
    let ImportBoundsInput {
        solid,
        faces,
        source_trims,
        parameters,
        continuations,
        proofs,
        edge_count,
        source_slots,
    } = input;
    let mut work = GeometryWorkBudget::new(MAX_IMPORT_BOUND_WORK);
    let mut bounds = None;
    let face_count = solid
        .boundaries()
        .iter()
        .map(|shell| shell.len())
        .sum::<usize>();
    if face_count != faces.len()
        || face_count != source_trims.len()
        || face_count != parameters.len()
        || face_count != source_slots.len()
    {
        return Err(invalid(
            "native import bounds lost original face correspondence",
        ));
    }
    let face_bytes = face_count
        .checked_mul(std::mem::size_of::<[Interval; 3]>())
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "import retained face enclosure capacity exhausted",
            )
        })?;
    let allocated_bytes = edge_count
        .checked_mul(std::mem::size_of::<Option<[Interval; 3]>>())
        .and_then(|bytes| bytes.checked_add(face_bytes))
        .filter(|bytes| *bytes <= MAX_IMPORT_BOUND_BYTES)
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "import retained boundary enclosure capacity exhausted",
            )
        })?;
    work.charge(edge_count as u64 + face_count as u64, cancel)?;
    let mut edge_bounds = vec![None; edge_count];
    let mut face_enclosures = Vec::with_capacity(face_count);
    for (face_index, (((face, _), parameter), slots)) in solid
        .boundaries()
        .iter()
        .flat_map(|shell| shell.iter())
        .zip(source_trims)
        .zip(parameters)
        .zip(source_slots)
        .enumerate()
    {
        let info = &faces[face_index];
        work.charge(1, cancel).map_err(|cause| {
            admission_error(
                cause,
                "import-bound-face",
                &work,
                format_args!(
                    "face {} (source #{})",
                    info.face_id.as_str(),
                    info.surface_face_entity
                ),
            )
        })?;
        let mut has_original_implicit = false;
        let mut boundary = None;
        let wires = face.absolute_boundaries();
        if wires.len() != slots.len() {
            return Err(invalid(
                "import carrier bounds lost original wire correspondence",
            ));
        }
        for (wire, slots) in wires.iter().zip(slots) {
            if wire.len() != slots.len() {
                return Err(invalid(
                    "import carrier bounds lost original coedge correspondence",
                ));
            }
            for (edge, (slot, _)) in wire.iter().zip(slots) {
                work.charge(1, cancel).map_err(|cause| {
                    admission_error(
                        cause,
                        "import-bound-edge",
                        &work,
                        format_args!(
                            "face {} (source #{}), native edge {}",
                            info.face_id.as_str(),
                            info.surface_face_entity,
                            slot.index,
                        ),
                    )
                })?;
                let local = edge_bounds
                    .get_mut(slot.index)
                    .ok_or_else(|| invalid("import edge bound slot is missing"))?;
                let branch = continuations.get(slot.index).and_then(Option::as_ref);
                has_original_implicit |= branch.is_some();
                if local.is_none() {
                    if let Some(branch) = branch {
                        let native = proofs.get(branch.prepared).ok_or_else(|| {
                            invalid("import bounds lost original continuation owner")
                        })?;
                        let prepared =
                            crate::parametric::PreparedCurveEnclosure::borrowed_restricted(
                                native,
                                branch.scale_to_mm,
                                branch.source_range,
                            );
                        refine(
                            [[branch.source_range.0, branch.source_range.1]],
                            local,
                            &mut work,
                            cancel,
                            |cell, work| {
                                prepared.position_bounds((cell[0][0], cell[0][1]), work, cancel)
                            },
                        )
                        .map_err(|cause| {
                            admission_error(
                                cause,
                                "import-bound-continuation",
                                &work,
                                format_args!(
                                    "face {} (source #{}), native edge {}",
                                    info.face_id.as_str(),
                                    info.surface_face_entity,
                                    slot.index,
                                ),
                            )
                        })?;
                    } else {
                        edge.with_curve(|curve| curve_bounds(curve, local, &mut work, cancel))
                            .map_err(|cause| {
                                admission_error(
                                    cause,
                                    "import-bound-curve",
                                    &work,
                                    format_args!(
                                        "face {} (source #{}), native edge {}",
                                        info.face_id.as_str(),
                                        info.surface_face_entity,
                                        slot.index,
                                    ),
                                )
                            })?;
                    }
                }
                include(
                    &mut boundary,
                    local.ok_or_else(|| {
                        invalid("native face boundary has no retained whole image enclosure")
                    })?,
                )?;
            }
        }
        let mut face_bounds = boundary;
        // Release the surface lock before the native face theorem locks it.
        // Its candidate apex is intentionally retained until a sealed native
        // trim-domain proof establishes actual bounded material coverage.
        let needs_native_trim_bound = face.with_surface(|surface| {
            matches!(
                surface,
                Surface::Plane(_) | Surface::ExtrusionSurface(_) | Surface::RevolutionSurface(_)
            )
        });
        if needs_native_trim_bound {
            let mut native = ValidatedGeometryBudget::new(
                usize::try_from(work.remaining()).unwrap_or(usize::MAX),
            );
            let boundary = boundary
                .ok_or_else(|| invalid("native face has no bounded original boundary"))?
                .map(|value| monstertruck_modeling::ValidatedInterval {
                    lower: value.lo,
                    upper: value.hi,
                });
            let result = validated_face_bounding_box_with_boundary_enclosure(
                face,
                boundary,
                &mut native,
                cancel,
            );
            work.charge(native.consumed() as u64, &AtomicBool::new(false))
                .map_err(|cause| {
                    admission_error(
                        cause,
                        "import-bound-carrier-debit",
                        &work,
                        format_args!(
                            "face {} (source #{})",
                            info.face_id.as_str(),
                            info.surface_face_entity
                        ),
                    )
                })?;
            check_cancel(cancel)?;
            match result {
                Ok(native) => {
                    let min: [f64; 3] = native.min().into();
                    let max: [f64; 3] = native.max().into();
                    include(
                        &mut face_bounds,
                        std::array::from_fn(|axis| Interval {
                            lo: min[axis],
                            hi: max[axis],
                        }),
                    )?;
                    let face_bounds = face_bounds
                        .ok_or_else(|| invalid("native face has no whole image enclosure"))?;
                    include(&mut bounds, face_bounds)?;
                    face_enclosures.push(face_bounds);
                    continue;
                }
                // Capability is decided by the native theorem, not a bare
                // Line variant gate that silently excludes exact wrappers.
                Err(monstertruck_modeling::ValidatedGeometryError::Unsupported) => {}
                Err(cause) => {
                    return Err(admission_error(
                        native_error(cause),
                        "import-bound-carrier",
                        &work,
                        format_args!(
                            "face {} (source #{})",
                            info.face_id.as_str(),
                            info.surface_face_entity
                        ),
                    ));
                }
            }
        }
        face.with_surface(|surface| {
            let domain = if has_original_implicit || matches!(surface, Surface::RevolutionSurface(_)) {
                // A candidate UV residual is not an inverse-domain enclosure.
                // Only an actual bounded native carrier or independently
                // declared bounded sweep profile authorizes this full box.
                let bounded=matches!(surface,Surface::BsplineSurface(_)|Surface::NurbsSurface(_)
                    |Surface::SphericalSurface(_)|Surface::ToroidalSurface(_))
                    ||matches!(parameter.conversion,
                        monstertruck_io::step::load::step_geometry::SurfaceParameterMap::Revolution {profile,..}
                        if profile.source_domain==monstertruck_io::step::load::step_geometry::CurveParameterDomain::Bounded);
                if !bounded {
                    return Err(error(GeometryErrorCode::ParameterCorrespondenceUnavailable,
                        "trimmed carrier has no retained-boundary theorem or whole declared bounded domain"));
                }
                let (Some(u),Some(v))=surface.try_range_tuple() else {
                    return Err(error(GeometryErrorCode::ParameterCorrespondenceUnavailable,
                        "source carrier bounded domain is unavailable"));
                };
                [[u.0,u.1],[v.0,v.1]]
            } else {
                let (u, v) = surface.try_range_tuple();
                let (Some(u), Some(v)) = (u, v) else {
                    return Err(error(GeometryErrorCode::ParameterCorrespondenceUnavailable,
                        "native source carrier lacks a bounded exact parameter chart"));
                };
                [[u.0, u.1], [v.0, v.1]]
            };
            surface_bounds(surface, domain, &mut face_bounds, &mut work, cancel)
        })
        .map_err(|cause| {
            admission_error(
                cause,
                "import-bound-surface",
                &work,
                format_args!("face {} (source #{})", info.face_id.as_str(), info.surface_face_entity),
            )
        })?;
        let face_bounds =
            face_bounds.ok_or_else(|| invalid("native face has no whole image enclosure"))?;
        include(&mut bounds, face_bounds)?;
        face_enclosures.push(face_bounds);
    }
    check_cancel(cancel)?;
    let bounds = bounds.ok_or_else(|| invalid("native solid has no bounded geometry"))?;
    let result = AabbMm {
        min: bounds.map(|v| v.lo),
        max: bounds.map(|v| v.hi),
    };
    result.validate()?;
    Ok((
        result,
        ImportBoundsRefinement {
            faces: face_enclosures,
            work,
            allocated_bytes,
        },
    ))
}

/// Tighten only after the Definition binds original oriented uses, source-chart
/// certification/uncertainty and native identities. A collection of boundary
/// curves is not a material-domain theorem: a cone cap can contain its apex
/// outside the boundary's constant profile coordinate.
pub(super) fn tighten_import_bounds(
    definition: &mut crate::Definition,
    mut refinement: ImportBoundsRefinement,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    if definition.solid.face_iter().count() != refinement.faces.len() {
        return Err(invalid(
            "import refinement lost original face correspondence",
        ));
    }
    let Some(mut allocated_bytes) = refinement
        .allocated_bytes
        .checked_add(definition.retained_query_proof_bytes())
        .filter(|bytes| *bytes <= MAX_IMPORT_BOUND_BYTES)
    else {
        // The independently sound coarse enclosure remains authoritative.
        return check_cancel(cancel);
    };
    let mut aggregate = None;
    for (face_index, (face, coarse)) in definition
        .solid
        .face_iter()
        .zip(refinement.faces)
        .enumerate()
    {
        if let Err(cause) = refinement.work.charge(1, cancel) {
            return if cause.code == GeometryErrorCode::Cancelled {
                Err(cause)
            } else {
                check_cancel(cancel)
            };
        }
        let needs_chart = face.with_surface(|surface| {
            !matches!(surface, Surface::Plane(_) | Surface::ExtrusionSurface(_))
        });
        let refined = if needs_chart {
            // Preparation may acquire this face's surface mutex. Never invoke
            // it from inside the surface callback used for interval evaluation.
            let attempt = (|| {
                let trim = crate::kernel::prepare_face_trim_domain_with_byte_limit(
                    definition,
                    face_index,
                    MAX_IMPORT_BOUND_BYTES - allocated_bytes,
                    &mut refinement.work,
                    cancel,
                )?;
                allocated_bytes = allocated_bytes
                    .checked_add(trim.retained_bytes())
                    .filter(|bytes| *bytes <= MAX_IMPORT_BOUND_BYTES)
                    .ok_or_else(|| {
                        error(
                            GeometryErrorCode::ResourceLimit,
                            "import trim proof exhausts enclosure allocation cap",
                        )
                    })?;
                let mut bounded = None;
                face.with_surface(|surface| {
                    surface_bounds(
                        surface,
                        trim.uv_bounds(),
                        &mut bounded,
                        &mut refinement.work,
                        cancel,
                    )
                })?;
                bounded.ok_or_else(|| invalid("sealed native trim has no whole image enclosure"))
            })();
            match attempt {
                Ok(bounded) => std::array::from_fn(|axis| Interval {
                    lo: coarse[axis].lo.max(bounded[axis].lo),
                    hi: coarse[axis].hi.min(bounded[axis].hi),
                }),
                Err(cause) if cause.code == GeometryErrorCode::Cancelled => return Err(cause),
                // Refinement is optional: resource exhaustion cannot authorize
                // a narrower box or replenish the original import allowance.
                // Keep the complete original aggregate when no work remains.
                Err(cause) if cause.code == GeometryErrorCode::ResourceLimit => {
                    return check_cancel(cancel);
                }
                Err(_) => coarse,
            }
        } else {
            coarse
        };
        include(&mut aggregate, refined)?;
    }
    check_cancel(cancel)?;
    let aggregate = aggregate.ok_or_else(|| invalid("native solid has no bounded geometry"))?;
    let bounds = AabbMm {
        min: aggregate.map(|v| v.lo),
        max: aggregate.map(|v| v.hi),
    };
    bounds.validate()?;
    definition.bounds = bounds;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use monstertruck_modeling::{
        BsplineCurve, BsplineSurface, ExtrusionSurface, Face, KnotVector, Line, NurbsCurve,
        NurbsSurface, Point3, Vector3, Vector4, Vertex, Wire,
        bounding::validated_face_bounding_box, builder,
    };

    fn contains(bounds: [Interval; 3], point: [f64; 3]) {
        for axis in 0..3 {
            assert!(bounds[axis].lo <= point[axis] && point[axis] <= bounds[axis].hi);
        }
    }

    #[test]
    fn frozen_perforated_plate_admission_preserves_every_hole_with_frozen_work_caps() {
        let bytes = include_bytes!("../../../../fixtures/geometry/perforated-plate.step");
        let definition = crate::import_step(bytes, &AtomicBool::new(false)).unwrap();
        assert_eq!(definition.faces.len(), 1030);
        assert_eq!(
            definition
                .faces
                .iter()
                .filter(|face| { face.source_surface_entity_kind == "CYLINDRICAL_SURFACE" })
                .count(),
            1024,
        );
        let mut caps = 0;
        for (face, boundary) in definition.faces.iter().zip(&definition.face_boundaries) {
            if face.geometry.domain.inner_loop_count == 0 {
                continue;
            }
            caps += 1;
            assert_eq!(face.geometry.domain.inner_loop_count, 512);
            assert_eq!(face.geometry.domain.trim_loop_count, 513);
            assert_eq!(boundary.wires.len(), 513);
            assert!(boundary.wires.iter().all(|wire| {
                wire.iter()
                    .all(|coedge| coedge.source_chart_certificate.is_certified())
            }));
            assert!(
                boundary
                    .wires
                    .iter()
                    .flatten()
                    .all(|coedge| { coedge.source_chart_certificate.has_regular_inverse() }),
                "planar hole caps retain ordinary source chart coordinates, not pole graph authority",
            );
        }
        assert_eq!(caps, 2);
        for (axis, expected) in [[0.0, 1640.0], [0.0, 840.0], [0.0, 20.0]]
            .into_iter()
            .enumerate()
        {
            assert!(definition.bounds.min[axis] <= expected[0]);
            assert!(definition.bounds.max[axis] >= expected[1]);
        }
    }

    #[test]
    fn independent_fitting_import_and_directional_bounds_enclose_physical_height() {
        // This unchanged OCCT export has a six-to-four outer taper, a radius
        // two bore and physical end planes at zero and twelve millimetres.
        // Its cone apex is outside the admitted face, not a height witness.
        let bytes = include_bytes!(
            "../../../../fixtures/geometry/independent-exporters/occt-tapered-fitting-mm.step"
        );
        let cancel = AtomicBool::new(false);
        let definition = crate::import_step(bytes, &cancel).unwrap();
        let tolerance = 0.00001;
        for (axis, expected) in [[-6.0, 6.0], [-6.0, 6.0], [0.0, 12.0]]
            .into_iter()
            .enumerate()
        {
            assert!(definition.bounds.min[axis] <= expected[0]);
            assert!(definition.bounds.max[axis] >= expected[1]);
            assert!(definition.bounds.min[axis] >= expected[0] - tolerance);
            assert!(definition.bounds.max[axis] <= expected[1] + tolerance);
        }
        let mesh = crate::tessellate(
            &definition,
            spiling_contracts::geometry::DisplayProfile::MeshTrimmedMm005V2,
            &cancel,
        )
        .unwrap();
        assert!(mesh.triangle_count > 0 && mesh.vertex_count > 0);
        assert!(!mesh.chunks.is_empty());
        for chunk in &mesh.chunks {
            let receipt = &chunk.descriptor;
            assert_eq!(receipt.definition_id, definition.id);
            assert_eq!(receipt.face_count as usize, definition.faces.len());
            assert!(
                receipt.carrier_deviation_mm
                    + receipt.trim_deviation_mm
                    + receipt.quantization_error_mm
                    <= spiling_contracts::geometry::MESH_SURFACE_TOLERANCE_MM
            );
        }
        let projected = crate::analysis::projected_bounds(
            &definition,
            [0.0, 0.0, 1.0],
            crate::analysis::BoundsPolicy {
                maximum_endpoint_uncertainty_mm: tolerance,
            },
            &mut GeometryWorkBudget::new(20_000_000),
            &cancel,
        )
        .unwrap();
        assert!(projected.minimum[0] <= 0.0 && 0.0 <= projected.minimum[1]);
        assert!(projected.maximum[0] <= 12.0 && 12.0 <= projected.maximum[1]);
        assert!(projected.achieved_uncertainty_mm <= tolerance);
    }

    #[test]
    fn whole_turn_rational_sweep_keeps_its_axial_control_hull() {
        for sign in [1.0, -1.0] {
            let profile = Curve::NurbsCurve(NurbsCurve::new(BsplineCurve::new(
                KnotVector::bezier_knot(2),
                vec![
                    Vector4::new(6.0 * sign, 0.0, 0.0, sign),
                    Vector4::new(2.5 * sign, 0.0, 3.0 * sign, 0.5 * sign),
                    Vector4::new(4.0 * sign, 0.0, 12.0 * sign, sign),
                ],
            )));
            let surface = Surface::RevolutionSurface(monstertruck_modeling::Processor::new(
                monstertruck_modeling::RevolutionSurface::by_revolution(
                    profile,
                    Point3::new(0.0, 0.0, 0.0),
                    Vector3::unit_z(),
                ),
            ));
            let cancel = AtomicBool::new(false);
            let mut work = GeometryWorkBudget::new(1_000_000);
            let mut bounds = None;
            surface_bounds(
                &surface,
                [[0.0, 1.0], [0.0, std::f64::consts::TAU]],
                &mut bounds,
                &mut work,
                &cancel,
            )
            .unwrap();
            let bounds = bounds.unwrap();
            contains(bounds, [6.0, 0.0, 0.0]);
            contains(bounds, [4.0, 0.0, 12.0]);
            assert!(bounds[2].lo >= -0.00000001 && bounds[2].hi <= 12.00000001);
        }
    }

    #[test]
    fn actual_multispan_polynomial_and_rational_images_cross_c0() {
        let knots = KnotVector::from(vec![0.0, 0.0, 0.37, 1.0, 1.0]);
        let controls = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.37, 2.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
        ];
        let curve = Curve::BsplineCurve(BsplineCurve::new(knots.clone(), controls.clone()));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let mut bounds = None;
        curve_bounds(&curve, &mut bounds, &mut work, &cancel).unwrap();
        contains(bounds.unwrap(), [0.37, 2.0, 0.0]);
        let net: Vec<_> = controls
            .iter()
            .map(|p| vec![*p, Point3::new(p.x, p.y, 3.0)])
            .collect();
        let v = KnotVector::from(vec![0.0, 0.0, 1.0, 1.0]);
        let polynomial =
            Surface::BsplineSurface(BsplineSurface::new((knots.clone(), v.clone()), net.clone()));
        let rational = Surface::NurbsSurface(NurbsSurface::new(BsplineSurface::new(
            (knots, v),
            net.iter()
                .map(|row| {
                    row.iter()
                        .map(|p| Vector4::new(2.0 * p.x, 2.0 * p.y, 2.0 * p.z, 2.0))
                        .collect()
                })
                .collect(),
        )));
        for surface in [polynomial, rational] {
            let mut bounds = None;
            surface_bounds(&surface, [[0.0, 1.0]; 2], &mut bounds, &mut work, &cancel).unwrap();
            contains(bounds.unwrap(), [0.37, 2.0, 3.0]);
        }
    }

    #[test]
    fn actual_mixed_denominator_refines_but_a_pole_refuses() {
        let rational = |middle| {
            Curve::NurbsCurve(NurbsCurve::new(BsplineCurve::new(
                KnotVector::bezier_knot(2),
                vec![
                    Vector4::new(0.0, 0.0, 0.0, 1.0),
                    Vector4::new(middle, 0.0, 0.0, middle),
                    Vector4::new(2.0, 0.0, 0.0, 1.0),
                ],
            )))
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let mut bounds = None;
        curve_bounds(&rational(-0.1), &mut bounds, &mut work, &cancel).unwrap();
        contains(bounds.unwrap(), [1.0, 0.0, 0.0]);
        let mut bounds = None;
        curve_bounds(&rational(-1.0), &mut bounds, &mut work, &cancel)
            .expect_err("an unresolved source denominator must never publish finite bounds");
    }

    #[test]
    fn exact_unbounded_extrusion_keeps_trim_outside_convenience_profile_range() {
        let vertices = [
            Vertex::new(Point3::new(4.0, 0.0, 0.0)),
            Vertex::new(Point3::new(8.0, 0.0, 0.0)),
            Vertex::new(Point3::new(8.0, 0.0, 3.0)),
            Vertex::new(Point3::new(4.0, 0.0, 3.0)),
        ];
        let wire = Wire::from(
            (0..4)
                .map(|i| builder::line(&vertices[i], &vertices[(i + 1) % 4]))
                .collect::<Vec<_>>(),
        );
        let surface = Surface::ExtrusionSurface(ExtrusionSurface::by_extrusion(
            Curve::Line(Line(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0))),
            Vector3::new(0.0, 0.0, 1.0),
        ));
        let face = Face::new(vec![wire], surface);
        let cancel = AtomicBool::new(false);
        let mut native = ValidatedGeometryBudget::new(100_000);
        let bounds = validated_face_bounding_box(&face, &mut native, &cancel).unwrap();
        assert!(bounds.min().x <= 4.0 && bounds.max().x >= 8.0);
        assert!(bounds.min().z <= 0.0 && bounds.max().z >= 3.0);
    }

    #[test]
    fn import_refinement_obeys_cumulative_limits_and_cancellation() {
        let curve = Curve::Line(Line(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)));
        let mut bounds = None;
        assert_eq!(
            curve_bounds(
                &curve,
                &mut bounds,
                &mut GeometryWorkBudget::new(0),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(
            curve_bounds(
                &curve,
                &mut bounds,
                &mut GeometryWorkBudget::new(100),
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
    }
}
