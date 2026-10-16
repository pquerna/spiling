// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::super::*;

/// Optional outer-meridian restriction in the actual stored projection frame.
#[derive(Clone, Copy, Debug)]
pub(super) struct FiberCore {
    pub(super) frame: PlaneFrameMm,
    pub(super) radius: f64,
}

/// A coefficient family of the two affine equations
/// `rows[i].(S-center) + offsets[i] = 0`. The intended exact symbolic fiber
/// must belong to this enclosure. Every exclusion, root inclusion and normal
/// sign holds uniformly over ALL family members, not a midpoint fiber.
/// Disk fibers use the positive forward scalar coordinate; outer-meridian
/// fibers instead require projected `px > 0` AND `rho > radius`.
#[derive(Clone, Copy, Debug)]
pub(super) struct OriginalFiber {
    pub(super) center: [f64; 3],
    pub(super) rows: [[Interval; 3]; 2],
    pub(super) offsets: [Interval; 2],
    pub(super) forward: [Interval; 3],
    pub(super) min_forward: f64,
    pub(super) core: Option<FiberCore>,
}

/// Exactly one outward-positive root on the complete original trimmed source.
/// This does NOT certify that correction collars, vertex supports or cut caps
/// miss the fiber. The material-degree consumer must separately prove those
/// gates for this identical fiber and closed request before using the receipt.
pub(super) struct FiberDegreeOne;

#[derive(Clone, Copy)]
struct CensusRestriction<'a> {
    height: HeightFunctional,
    request: &'a SlabRequest,
    faces: Option<&'a [usize]>,
    inward: bool,
}

#[derive(Clone, Copy)]
struct FiberCell {
    uv: [[f64; 2]; 2],
    depth: u8,
}

const FIBER_STACK_CAPACITY: usize = MAX_DEPTH as usize + 1;
const FIBER_STACK_BYTES: usize = std::mem::size_of::<[Option<FiberCell>; FIBER_STACK_CAPACITY]>();

/// Census every original face and every original parameter span. Whole-image
/// exclusions and strictly owned interval roots form a complete cover. No root
/// is selected by a sampled point or merged by physical nearness. A root on a
/// chart, trim, split, knot, half-ray or slab boundary remains unresolved.
pub(super) fn original_fiber_degree_one(
    source: OriginalTrimSource<'_, '_>,
    fiber: &OriginalFiber,
    request: &SlabRequest,
    height: HeightFunctional,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<FiberDegreeOne>, GeometryError> {
    work.charge(1, cancel)?;
    validate_fiber(fiber)?;
    let restriction = CensusRestriction {
        height,
        request,
        faces: None,
        inward: false,
    };
    Ok(
        census_original_fiber(source, fiber, restriction, remaining_bytes, work, cancel)?
            .map(|()| FiberDegreeOne),
    )
}

/// Calibrate the COMPLETE declared wall component, with material normals
/// pointing into its bounded void. The caller must separately prove complete
/// correction-support exclusion for this identical physical fiber.
#[allow(clippy::too_many_arguments)]
pub(super) fn original_wall_fiber_degree_one(
    source: OriginalTrimSource<'_, '_>,
    faces: &[usize],
    fiber: &OriginalFiber,
    request: &SlabRequest,
    height: HeightFunctional,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<FiberDegreeOne>, GeometryError> {
    work.charge(faces.len() as u64, cancel)?;
    if faces.is_empty()
        || faces
            .iter()
            .any(|&face| face >= source.definition().faces.len())
        || faces
            .iter()
            .enumerate()
            .any(|(i, face)| faces[..i].contains(face))
    {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "wall fiber needs a nonempty unique original face inventory",
        ));
    }
    validate_fiber(fiber)?;
    let restriction = CensusRestriction {
        height,
        request,
        faces: Some(faces),
        inward: true,
    };
    Ok(
        census_original_fiber(source, fiber, restriction, remaining_bytes, work, cancel)?
            .map(|()| FiberDegreeOne),
    )
}

fn census_original_fiber(
    source: OriginalTrimSource<'_, '_>,
    fiber: &OriginalFiber,
    restriction: CensusRestriction<'_>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<()>, GeometryError> {
    let domain_bytes = remaining_bytes
        .checked_sub(FIBER_STACK_BYTES)
        .ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "original fiber census stack exceeds remaining scratch allowance",
            )
        })?;
    let definition = source.definition();
    let mut found = None;
    let mut stack = [None; FIBER_STACK_CAPACITY];
    for (face_index, _) in definition.solid.face_iter().enumerate() {
        work.charge(1, cancel)?;
        if restriction
            .faces
            .is_some_and(|faces| !faces.contains(&face_index))
        {
            continue;
        }
        let domain = source.face_domain(face_index, domain_bytes, work, cancel)?;
        let face = domain.native().face();
        let resolved = face.with_surface(|surface| -> Result<bool, GeometryError> {
            let spans = surface_parameter_boxes(surface, domain.uv_bounds(), work, cancel)?;
            for span in spans {
                work.charge(1, cancel)?;
                // Only whole-position exclusions are valid on an unqualified
                // original C0 span. Subdivision cannot manufacture C1 authority.
                let smooth = surface_parameter_span_smooth(surface, span, work, cancel)?;
                let mut pending = 1_usize;
                stack[0] = Some(FiberCell { uv: span, depth: 0 });
                while pending != 0 {
                    work.charge(1, cancel)?;
                    pending -= 1;
                    let cell = stack[pending].take().ok_or_else(|| {
                        GeometryError::new(
                            GeometryErrorCode::KernelFailure,
                            "original fiber census lost its bounded cover stack",
                        )
                    })?;
                    if domain.classify(cell.uv, work, cancel)? == TrimClassification::Outside {
                        continue;
                    }
                    let jet = if smooth {
                        surface_enclosure(surface, cell.uv, work, cancel)?
                    } else {
                        crate::parametric::surface_piecewise_enclosure(
                            surface, cell.uv, work, cancel,
                        )?
                    };
                    if fiber_image_excluded(
                        fiber,
                        jet.position,
                        restriction.height,
                        restriction.request,
                    ) {
                        continue;
                    }
                    if !smooth {
                        return Ok(false);
                    }
                    match fiber_krawczyk(surface, cell.uv, &jet, fiber, work, cancel)? {
                        FiberNewton::Excluded => continue,
                        FiberNewton::Enclosure { uv, unique: true } => {
                            match qualify_fiber_root(
                                surface,
                                &domain,
                                uv,
                                fiber,
                                restriction,
                                face.orientation(),
                                work,
                                cancel,
                            )? {
                                QualifiedRoot::Excluded => continue,
                                QualifiedRoot::Unresolved => return Ok(false),
                                QualifiedRoot::Positive => {
                                    if found.is_some() {
                                        // Distinct original owners are not
                                        // deduplicated, even at equal positions.
                                        return Ok(false);
                                    }
                                    found = Some(());
                                    continue;
                                }
                            }
                        }
                        FiberNewton::Enclosure { unique: false, .. } | FiberNewton::Unresolved => {}
                    }
                    if cell.depth >= MAX_DEPTH {
                        return Ok(false);
                    }
                    let children = match split(cell.uv, cell.depth, face_index) {
                        Ok(children) => children,
                        Err(cause) if cause.code == GeometryErrorCode::AnalysisIndeterminate => {
                            return Ok(false);
                        }
                        Err(cause) => return Err(cause),
                    };
                    if pending + children.len() > stack.len() {
                        return Err(GeometryError::new(
                            GeometryErrorCode::ResourceLimit,
                            "original fiber census exhausted its bounded cover stack",
                        ));
                    }
                    for uv in children {
                        work.charge(1, cancel)?;
                        stack[pending] = Some(FiberCell {
                            uv,
                            depth: cell.depth + 1,
                        });
                        pending += 1;
                    }
                }
            }
            Ok(true)
        })?;
        if !resolved {
            return Ok(None);
        }
    }
    Ok(found)
}

fn validate_fiber(fiber: &OriginalFiber) -> Result<(), GeometryError> {
    let exact_forward = cross(fiber.rows[0], fiber.rows[1]);
    if fiber.center.into_iter().any(|v| !v.is_finite())
        || fiber
            .rows
            .into_iter()
            .flatten()
            .chain(fiber.offsets)
            .any(|v| !v.lo.is_finite() || !v.hi.is_finite() || v.lo > v.hi)
        || !fiber.min_forward.is_finite()
        || fiber.min_forward < 0.0
        || fiber
            .forward
            .iter()
            .zip(exact_forward)
            .any(|(supplied, exact)| {
                !supplied.lo.is_finite()
                    || !supplied.hi.is_finite()
                    || supplied.lo > exact.lo
                    || supplied.hi < exact.hi
            })
        || length(exact_forward)?.lo <= 0.0
    {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "original fiber lacks finite independent row families and a covering forward vector",
        ));
    }
    if let Some(core) = fiber.core {
        if !core.radius.is_finite()
            || core.radius <= 0.0
            || core
                .frame
                .origin_mm
                .into_iter()
                .chain(core.frame.x_axis)
                .chain(core.frame.y_axis)
                .chain(core.frame.z_axis)
                .any(|v| !v.is_finite())
        {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "original fiber outer-core gate has invalid stored coefficients",
            ));
        }
    }
    Ok(())
}

fn fiber_value(fiber: &OriginalFiber, position: [Interval; 3]) -> [Interval; 2] {
    let relative = std::array::from_fn(|i| position[i].sub(Interval::point(fiber.center[i])));
    std::array::from_fn(|i| dot(fiber.rows[i], relative).add(fiber.offsets[i]))
}

fn fiber_forward(fiber: &OriginalFiber, position: [Interval; 3]) -> Interval {
    dot(
        fiber.forward,
        std::array::from_fn(|i| position[i].sub(Interval::point(fiber.center[i]))),
    )
}

fn fiber_core_radius(core: FiberCore, position: [Interval; 3]) -> Interval {
    let xy = project(position, core.frame);
    xy[0].square().add(xy[1].square())
}

fn fiber_image_excluded(
    fiber: &OriginalFiber,
    position: [Interval; 3],
    height: HeightFunctional,
    request: &SlabRequest,
) -> bool {
    fiber_value(fiber, position)
        .into_iter()
        .any(|v| !v.contains_zero())
        || fiber_restriction_excluded(fiber, position, height, request)
}

fn fiber_restriction_excluded(
    fiber: &OriginalFiber,
    position: [Interval; 3],
    height: HeightFunctional,
    request: &SlabRequest,
) -> bool {
    if let Some(core) = fiber.core {
        if project(position, core.frame)[0].hi <= 0.0
            || fiber_core_radius(core, position).hi <= Interval::point(core.radius).square().lo
        {
            return true;
        }
    } else if fiber_forward(fiber, position).hi < fiber.min_forward {
        return true;
    }
    let h = height.height(position);
    h.hi < request.height_interval_mm[0] || h.lo > request.height_interval_mm[1]
}

fn census_root_owned(
    fiber: &OriginalFiber,
    position: [Interval; 3],
    normal: [Interval; 3],
    restriction: CensusRestriction<'_>,
) -> bool {
    let CensusRestriction {
        height,
        request,
        inward,
        ..
    } = restriction;
    let h = height.height(position);
    let positive_half = match fiber.core {
        Some(core) => {
            project(position, core.frame)[0].lo > 0.0
                && fiber_core_radius(core, position).lo > Interval::point(core.radius).square().hi
        }
        None => fiber_forward(fiber, position).lo > fiber.min_forward,
    };
    positive_half
        && h.lo > request.height_interval_mm[0]
        && h.hi < request.height_interval_mm[1]
        && if inward {
            dot(normal, fiber.forward).hi < 0.0
        } else {
            dot(normal, fiber.forward).lo > 0.0
        }
}

#[derive(Clone, Copy)]
pub(super) enum FiberNewton {
    Excluded,
    Enclosure { uv: [[f64; 2]; 2], unique: bool },
    Unresolved,
}

/// A numerical midpoint inverse only proposes the preconditioner. Existence
/// requires strict interval inclusion; uniqueness additionally requires an
/// interval infinity-norm contraction on the ENTIRE owning original cell.
pub(super) fn fiber_krawczyk(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    jet: &SurfaceEnclosure,
    fiber: &OriginalFiber,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<FiberNewton, GeometryError> {
    work.charge(1, cancel)?;
    let jacobian = fiber.rows.map(|row| [dot(row, jet.du), dot(row, jet.dv)]);
    let middle = jacobian.map(|row| row.map(Interval::mid));
    let determinant = middle[0][0] * middle[1][1] - middle[0][1] * middle[1][0];
    if !determinant.is_finite() || determinant == 0.0 {
        return Ok(FiberNewton::Unresolved);
    }
    let inverse = [
        [middle[1][1] / determinant, -middle[0][1] / determinant],
        [-middle[1][0] / determinant, middle[0][0] / determinant],
    ];
    if inverse.iter().flatten().any(|v| !v.is_finite()) {
        return Ok(FiberNewton::Unresolved);
    }
    let inverse = inverse.map(|row| row.map(Interval::point));
    if inverse[0][0]
        .mul(inverse[1][1])
        .sub(inverse[0][1].mul(inverse[1][0]))
        .contains_zero()
    {
        return Ok(FiberNewton::Unresolved);
    }
    let center = uv.map(|v| v[0] * 0.5 + v[1] * 0.5);
    let at_center = surface_enclosure(surface, center.map(|v| [v, v]), work, cancel)?;
    let value = fiber_value(fiber, at_center.position);
    let remainder: [[Interval; 2]; 2] = std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            (0..2).fold(Interval::point(if i == j { 1.0 } else { 0.0 }), |s, k| {
                s.sub(inverse[i][k].mul(jacobian[k][j]))
            })
        })
    });
    let boxes = uv.map(|v| Interval { lo: v[0], hi: v[1] });
    let krawczyk: [Interval; 2] = std::array::from_fn(|i| {
        let residual = (0..2).fold(Interval::point(center[i]), |s, k| {
            s.sub(inverse[i][k].mul(value[k]))
        });
        (0..2).fold(residual, |s, j| {
            s.add(remainder[i][j].mul(boxes[j].sub(Interval::point(center[j]))))
        })
    });
    let mut contracted = [[0.0; 2]; 2];
    for axis in 0..2 {
        let Some(overlap) = intersection(boxes[axis], krawczyk[axis]) else {
            return Ok(FiberNewton::Excluded);
        };
        contracted[axis] = [overlap.lo, overlap.hi];
    }
    let contraction = remainder.map(|row| up(row[0].abs_upper() + row[1].abs_upper()));
    let unique = contraction.into_iter().all(|v| v < 1.0)
        && (0..2).all(|i| strictly_contains(boxes[i], krawczyk[i]));
    Ok(FiberNewton::Enclosure {
        uv: contracted,
        unique,
    })
}

enum QualifiedRoot {
    Excluded,
    Positive,
    Unresolved,
}

#[allow(clippy::too_many_arguments)]
fn qualify_fiber_root(
    surface: &Surface,
    domain: &PreparedFaceTrimDomain<'_>,
    mut uv: [[f64; 2]; 2],
    fiber: &OriginalFiber,
    restriction: CensusRestriction<'_>,
    orientation: bool,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<QualifiedRoot, GeometryError> {
    for _ in 0..=MAX_DEPTH {
        work.charge(1, cancel)?;
        let membership = domain.classify(uv, work, cancel)?;
        if membership == TrimClassification::Outside {
            return Ok(QualifiedRoot::Excluded);
        }
        let jet = surface_enclosure(surface, uv, work, cancel)?;
        // Uniqueness on the original owner was already proved. Its contracted
        // enclosure covers ALL roots of that owner, so whole-image exclusions
        // of this enclosure do not discard unexamined roots elsewhere in it.
        if fiber_value(fiber, jet.position)
            .into_iter()
            .any(|v| !v.contains_zero())
        {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "original fiber unique-root image became inconsistent",
            ));
        }
        if fiber_restriction_excluded(fiber, jet.position, restriction.height, restriction.request)
        {
            return Ok(QualifiedRoot::Excluded);
        }
        if membership == TrimClassification::Inside
            && census_root_owned(fiber, jet.position, normal(&jet, orientation), restriction)
        {
            return Ok(QualifiedRoot::Positive);
        }
        match fiber_krawczyk(surface, uv, &jet, fiber, work, cancel)? {
            FiberNewton::Excluded => {
                return Err(GeometryError::new(
                    GeometryErrorCode::ConditioningFailure,
                    "original fiber unique-root enclosure became inconsistent",
                ));
            }
            FiberNewton::Enclosure { uv: next, .. } if next != uv => uv = next,
            FiberNewton::Enclosure { .. } | FiberNewton::Unresolved => {
                return Ok(QualifiedRoot::Unresolved);
            }
        }
    }
    Ok(QualifiedRoot::Unresolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use monstertruck_modeling::{Plane, Point3};

    fn fiber(center: [f64; 3]) -> OriginalFiber {
        OriginalFiber {
            center,
            rows: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]].map(|v| v.map(Interval::point)),
            offsets: [Interval::point(0.0); 2],
            forward: [
                Interval::point(0.0),
                Interval::point(0.0),
                Interval::point(1.0),
            ],
            min_forward: 0.0,
            core: None,
        }
    }

    fn request() -> SlabRequest {
        SlabRequest {
            frame: PlaneFrameMm {
                origin_mm: [0.0; 3],
                x_axis: [1.0, 0.0, 0.0],
                y_axis: [0.0, 1.0, 0.0],
                z_axis: [0.0, 0.0, 1.0],
            },
            height_interval_mm: [-1.0, 20.0],
            sample_height_mm: 2.0,
            native_shape_error_mm: 0.1,
        }
    }

    #[test]
    fn affine_fiber_root_requires_strict_whole_cell_interval_inclusion() {
        let surface = Surface::Plane(Plane::new(
            Point3::new(0.0, 0.0, 4.0),
            Point3::new(4.0, 0.0, 4.0),
            Point3::new(0.0, 3.0, 4.0),
        ));
        let uv = [[0.0, 1.0], [0.0, 1.0]];
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
        assert!(matches!(
            fiber_krawczyk(
                &surface,
                uv,
                &jet,
                &fiber([1.25, 1.75, 2.0]),
                &mut work,
                &cancel,
            )
            .unwrap(),
            FiberNewton::Enclosure { unique: true, .. }
        ));
        assert!(!matches!(
            fiber_krawczyk(
                &surface,
                uv,
                &jet,
                &fiber([0.0, 1.75, 2.0]),
                &mut work,
                &cancel,
            )
            .unwrap(),
            FiberNewton::Enclosure { unique: true, .. }
        ));
        assert!(matches!(
            fiber_krawczyk(
                &surface,
                uv,
                &jet,
                &fiber([8.0, 1.75, 2.0]),
                &mut work,
                &cancel,
            )
            .unwrap(),
            FiberNewton::Excluded
        ));
    }

    #[test]
    fn affine_offset_row_family_has_uniform_not_midpoint_only_root_inclusion() {
        let surface = Surface::Plane(Plane::new(
            Point3::new(0.0, 0.0, 4.0),
            Point3::new(4.0, 0.0, 4.0),
            Point3::new(0.0, 3.0, 4.0),
        ));
        let zero = Interval::point(0.0);
        let one = Interval::point(1.0);
        let uncertainty = Interval::new(-1.0 / 128.0, 1.0 / 128.0).unwrap();
        let rows = [[one, zero, uncertainty], [uncertainty, one, zero]];
        let family = OriginalFiber {
            center: [0.0, 0.0, 2.0],
            rows,
            offsets: [
                Interval::new(-1.3, -1.2).unwrap(),
                Interval::new(-1.8, -1.7).unwrap(),
            ],
            forward: cross(rows[0], rows[1]),
            min_forward: 0.0,
            core: None,
        };
        validate_fiber(&family).unwrap();
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let uv = [[0.0, 1.0], [0.0, 1.0]];
        let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
        let FiberNewton::Enclosure {
            uv: root,
            unique: true,
        } = fiber_krawczyk(&surface, uv, &jet, &family, &mut work, &cancel).unwrap()
        else {
            panic!("entire affine coefficient family must have one strictly owned root");
        };
        assert!(root.into_iter().all(|v| v[0] < v[1]));
        for c in [uncertainty.lo, uncertainty.hi] {
            for d in [uncertainty.lo, uncertainty.hi] {
                for a in [-1.3, -1.2] {
                    for b in [-1.8, -1.7] {
                        let u = (-a - 2.0 * c) / 4.0;
                        let v = (-b - 4.0 * d * u) / 3.0;
                        assert!(root[0][0] <= u && u <= root[0][1]);
                        assert!(root[1][0] <= v && v <= root[1][1]);
                    }
                }
            }
        }
    }

    #[test]
    fn outer_meridian_gate_excludes_inner_roots_even_when_forward_is_positive() {
        let request = request();
        let height = HeightFunctional::from_frame(request.frame);
        let mut fiber = fiber([0.0; 3]);
        fiber.core = Some(FiberCore {
            frame: request.frame,
            radius: 3.0,
        });
        let inner = [1.0, 1.0, 2.0].map(Interval::point);
        assert!(fiber_forward(&fiber, inner).lo > 0.0);
        assert!(fiber_image_excluded(&fiber, inner, height, &request));
        let outer = [4.0, 1.0, 2.0].map(Interval::point);
        assert!(fiber_core_radius(fiber.core.unwrap(), outer).lo > 9.0);
        let backward_chart = [-4.0, 1.0, 2.0].map(Interval::point);
        assert!(fiber_restriction_excluded(
            &fiber,
            backward_chart,
            height,
            &request
        ));
        fiber.center[2] = 10.0;
        assert!(fiber_forward(&fiber, outer).hi < 0.0);
        assert!(!fiber_restriction_excluded(&fiber, outer, height, &request));
    }

    #[test]
    fn source_census_accepts_original_polynomial_and_rational_affine_carriers() {
        for name in [
            "native-parity-bspline-box.step",
            "native-parity-nurbs-box.step",
        ] {
            let (_, solid, _) = crate::parity_fixtures::recipes()
                .into_iter()
                .find(|(candidate, _, _)| *candidate == name)
                .unwrap();
            let text = crate::parity_fixtures::original(name, solid);
            let cancel = AtomicBool::new(false);
            let definition = crate::import_step(text.as_bytes(), &cancel).unwrap();
            let mut work = GeometryWorkBudget::new(100_000_000);
            let source = OriginalTrimSource::standalone(&definition);
            let fiber = fiber([1.23456789, 2.34567891, 2.0]);
            assert!(
                original_fiber_degree_one(
                    source,
                    &fiber,
                    &request(),
                    HeightFunctional::from_frame(request().frame),
                    MAX_SCRATCH_BYTES,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .is_some(),
                "complete original carrier fiber has one outward root"
            );
        }
    }

    #[test]
    fn multiple_intersections_and_void_entry_do_not_grant_degree_one() {
        let name = "native-parity-nurbs-torus.step";
        let (_, solid, _) = crate::parity_fixtures::recipes()
            .into_iter()
            .find(|(candidate, _, _)| *candidate == name)
            .unwrap();
        let text = crate::parity_fixtures::original(name, solid);
        let cancel = AtomicBool::new(false);
        let definition = crate::import_step(text.as_bytes(), &cancel).unwrap();
        let fiber = OriginalFiber {
            center: [0.0; 3],
            rows: [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|v| v.map(Interval::point)),
            offsets: [Interval::point(0.0); 2],
            forward: [
                Interval::point(1.0),
                Interval::point(0.0),
                Interval::point(0.0),
            ],
            min_forward: 0.0,
            core: None,
        };
        let mut request = request();
        request.height_interval_mm = [-100.0, 100.0];
        assert!(
            original_fiber_degree_one(
                OriginalTrimSource::standalone(&definition),
                &fiber,
                &request,
                HeightFunctional::from_frame(request.frame),
                MAX_SCRATCH_BYTES,
                &mut GeometryWorkBudget::new(100_000_000),
                &cancel,
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn census_scratch_work_and_cancellation_failures_remain_typed() {
        let name = "native-parity-bspline-box.step";
        let (_, solid, _) = crate::parity_fixtures::recipes()
            .into_iter()
            .find(|(candidate, _, _)| *candidate == name)
            .unwrap();
        let text = crate::parity_fixtures::original(name, solid);
        let cancel = AtomicBool::new(false);
        let definition = crate::import_step(text.as_bytes(), &cancel).unwrap();
        let source = OriginalTrimSource::standalone(&definition);
        let fiber = fiber([1.25, 2.25, 2.0]);
        let mut work = GeometryWorkBudget::new(100);
        let error = original_fiber_degree_one(
            source,
            &fiber,
            &request(),
            HeightFunctional::from_frame(request().frame),
            FIBER_STACK_BYTES - 1,
            &mut work,
            &cancel,
        )
        .err()
        .unwrap();
        assert_eq!(error.code, GeometryErrorCode::ResourceLimit);
        assert_eq!(work.consumed(), 1);
        let mut work = GeometryWorkBudget::new(0);
        let error = original_fiber_degree_one(
            source,
            &fiber,
            &request(),
            HeightFunctional::from_frame(request().frame),
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .err()
        .unwrap();
        assert_eq!(error.code, GeometryErrorCode::ResourceLimit);
        let mut work = GeometryWorkBudget::new(100);
        let error = original_fiber_degree_one(
            source,
            &fiber,
            &request(),
            HeightFunctional::from_frame(request().frame),
            MAX_SCRATCH_BYTES,
            &mut work,
            &AtomicBool::new(true),
        )
        .err()
        .unwrap();
        assert_eq!(error.code, GeometryErrorCode::Cancelled);
        assert_eq!(work.consumed(), 1);
    }
}
