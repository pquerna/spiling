// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A closed product paired with a complete filled-material metric. The radial
//! metric lane uses the SAME physical cylinder coordinates; an independently
//! completed full closed-slab metric can instead be consumed after world
//! placement. Bare boundary nearness about an unrelated axis is insufficient.

use super::radial_region::certify_actual_polygon_radial_region;
use super::*;
use crate::analysis::critical::material_link::{BoundaryCell, BoundaryVisit};
use crate::analysis::critical::ray_index::RayEdgeIndex;
use crate::analysis::critical::{ClosedRadialMaterialProduct, RadialRegionKind};
use crate::analysis::material_source::MaterialSourceView;
use crate::analysis::placed::ActualMaterialCoordinates;
use crate::parametric::{
    SurfaceEvaluator, SurfaceFunctional, SurfaceFunctionalEnclosure, surface_functional_enclosures,
    surface_functional_scratch_bytes,
};

const CORRELATED_SUPPORT_SCRATCH_BYTES: usize = surface_functional_scratch_bytes::<1>()
    + std::mem::size_of::<SurfaceFunctional>()
    + 2 * std::mem::size_of::<SurfaceFunctionalEnclosure>()
    + std::mem::size_of::<[[Interval; 2]; 2]>()
    + std::mem::size_of::<[Interval; 3]>()
    + std::mem::size_of::<BoundaryCell<'_>>();

#[derive(Debug)]
pub(in crate::analysis) struct ClosedRadialSlab {
    pub(super) product: ClosedRadialMaterialProduct,
    pub(super) bound_mm: f64,
    native_xy_sha256: [u8; 32],
    // Only the complete per-role same-azimuth visitor establishes this
    // stronger inner-material authority. A general filled metric does not.
    pub(super) inner_displacement_mm: Option<f64>,
}

fn polygon_hash(
    loops: &[SectionLoopMm],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[u8; 32], GeometryError> {
    let mut hash = Sha256::new();
    hash.update(b"spiling-closed-radial-native-polygon-1\0");
    hash.update((loops.len() as u64).to_le_bytes());
    // Canonicalization may rotate vertices and reorder the unique role rings,
    // but cannot change winding, coordinates or material roles.
    for inward in [false, true] {
        for ring in loops.iter().filter(|ring| ring.is_hole == inward) {
            hash.update(loop_xy_hash(ring, work, cancel)?);
        }
    }
    Ok(hash.finalize().into())
}

impl ClosedRadialSlab {
    pub(in crate::analysis) fn bound_mm(&self) -> f64 {
        self.bound_mm
    }

    pub(in crate::analysis) fn into_product(self) -> ClosedRadialMaterialProduct {
        self.product
    }

    pub(super) fn matches_native(
        &self,
        loops: &[SectionLoopMm],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        Ok(self.native_xy_sha256 == polygon_hash(loops, work, cancel)?)
    }
}

/// Seal an already complete WORLD filled-material metric with fresh radial
/// topology and calibration of the exact committed native polygon. This does
/// not upgrade a directed/core-only metric or discharge endpoint/collar
/// obligations. The existing placed bound already includes source uncertainty,
/// affine amplification, plane drift and transform arithmetic; keep its exact
/// value rather than charging any of those terms a second time.
pub(in crate::analysis) fn seal_closed_radial_slab_from_complete_metric(
    definition: &Definition,
    analysis: &SlabAnalysis,
    product: ClosedRadialMaterialProduct,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ClosedRadialSlab>, GeometryError> {
    work.charge(1, cancel)?;
    if !analysis.matches_native_material(definition)
        || analysis.section.definition_id() != definition.id()
        || !analysis.section.is_native_intersection()
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "complete radial seal lacks the same original material/native sample",
        ));
    }
    let (Some(pose), Some(interval), Some(sample), Some(policy)) = (
        analysis.placed_pose(),
        analysis.world_height_interval_mm(),
        analysis.sample_world_z_mm(),
        analysis.world_shape_error_policy_mm(),
    ) else {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "complete radial seal requires an already committed world slab capture",
        ));
    };
    if !product.matches_world(definition, pose, interval, sample) {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "complete radial seal product differs from the original world pose/scope/sample",
        ));
    }
    let Some(bound_mm) = analysis.closed_shape_error_mm() else {
        // Keep all transition/collar/original endpoint obligations intact.
        return Ok(None);
    };
    if !bound_mm.is_finite() || bound_mm < 0.0 || !policy.is_finite() || policy <= 0.0 {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "complete radial seal has an invalid captured world metric/policy",
        ));
    }
    if bound_mm > policy {
        return Err(error(
            GeometryErrorCode::ShapeErrorExceeded,
            "complete world slab metric exceeds its captured shape policy",
        ));
    }
    let fixed = std::mem::size_of::<ClosedRadialSlab>()
        .checked_add(std::mem::size_of::<
            super::radial_region::ActualPolygonRadialRegion,
        >())
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "complete radial seal fixed storage overflow",
            )
        })?;
    if fixed > remaining_bytes.min(MAX_SCRATCH) {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "complete radial seal exceeds remaining coexistent storage",
        ));
    }
    let Some(_region) = certify_actual_polygon_radial_region(
        &analysis.section.loops,
        product.coordinates(),
        product.kind(),
        work,
        cancel,
    )?
    else {
        return Ok(None);
    };
    // Calibration borrows these exact committed loops; the compact retained
    // hash uses the existing role-sensitive canonical XY binding. Final leaf
    // calibration of the actual used candidate remains independently required.
    let native_xy_sha256 = polygon_hash(&analysis.section.loops, work, cancel)?;
    Ok(Some(ClosedRadialSlab {
        product,
        bound_mm,
        native_xy_sha256,
        inner_displacement_mm: None,
    }))
}

struct RadialCandidate<'a> {
    outer: &'a SectionLoopMm,
    inner: Option<&'a SectionLoopMm>,
    center: [Interval; 2],
    index: RayEdgeIndex,
}

impl RadialCandidate<'_> {
    fn edge(&self, edge: usize) -> ([Interval; 2], [Interval; 2], bool) {
        let outer_count = self.outer.points_mm.len() - 1;
        let (ring, edge, inward) = if edge < outer_count {
            (self.outer, edge, false)
        } else {
            (
                self.inner.expect("admitted annulus has an inner loop"),
                edge - outer_count,
                true,
            )
        };
        let relative = |point: [f64; 3]| {
            std::array::from_fn(|i| Interval::point(point[i]).sub(self.center[i]))
        };
        (
            relative(ring.points_mm[edge]),
            relative(ring.points_mm[edge + 1]),
            inward,
        )
    }

    /// Every compatible edge is included, including both incident edges when
    /// a ray meets a vertex. Taking their MAXIMUM prevents a long hidden hole
    /// arm from borrowing an unrelated nearby candidate boundary witness.
    fn distance(
        &self,
        point: [Interval; 2],
        inward: bool,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<f64>, GeometryError> {
        self.distance_with_support(
            point,
            inward,
            f64::INFINITY,
            work,
            cancel,
            &mut |_, enclosure, _, _| Ok(enclosure),
        )
    }

    fn distance_with_support(
        &self,
        point: [Interval; 2],
        inward: bool,
        sufficient_mm: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        support: &mut impl FnMut(
            [Interval; 2],
            Interval,
            &mut GeometryWorkBudget,
            &AtomicBool,
        ) -> Result<Interval, GeometryError>,
    ) -> Result<Option<f64>, GeometryError> {
        let radius = point[0].square().add(point[1].square()).sqrt()?;
        if radius.lo <= 0.0 || !radius.hi.is_finite() {
            return Ok(None);
        }
        let mut count = 0;
        let mut maximum = 0.0_f64;
        let mut resolved = true;
        self.index.visit(point, work, cancel, |edge, work| {
            work.charge(1, cancel)?;
            let (a, b, role) = self.edge(edge);
            if role != inward {
                return Ok(());
            }
            let sign = if inward {
                Interval::point(-1.0)
            } else {
                Interval::point(1.0)
            };
            if cross(a, point).mul(sign).hi < 0.0 || cross(point, b).mul(sign).hi < 0.0 {
                return Ok(());
            }
            count += 1;
            let direction = std::array::from_fn(|i| b[i].sub(a[i]).mul(sign));
            let denominator = cross(point, direction);
            let numerator = cross(a, b).mul(sign);
            if numerator.lo <= 0.0 {
                resolved = false;
                return Ok(());
            }
            let coarse_distance = if denominator.lo > 0.0 {
                radius
                    .mul(Interval::point(1.0).sub(numerator.div(denominator)?))
                    .abs_upper()
            } else {
                f64::INFINITY
            };
            let distance = if !coarse_distance.is_finite() || coarse_distance > sufficient_mm {
                let denominator = support(direction, denominator, work, cancel)?;
                if denominator.lo <= 0.0 {
                    resolved = false;
                    return Ok(());
                }
                let scale = numerator.div(denominator)?;
                radius.mul(Interval::point(1.0).sub(scale)).abs_upper()
            } else {
                coarse_distance
            };
            resolved &= distance.is_finite();
            maximum = maximum.max(distance);
            Ok(())
        })?;
        Ok((resolved && count != 0).then_some(maximum))
    }
}

/// The same-azimuth denominator is a scalar facet support, not an independent
/// XY box. Its original source first jet preserves coefficient correlations.
/// A centered mean-value enclosure is valid across admitted continuous C0
/// spans; it grants neither smoothness nor a substitute topology certificate.
fn native_face_support(
    surface: &(impl SurfaceEvaluator + ?Sized),
    uv: [[f64; 2]; 2],
    coordinates: &ActualMaterialCoordinates<'_>,
    direction: [Interval; 2],
    coarse: Interval,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Interval, GeometryError> {
    let rows = coordinates.affine_rows();
    let functional = SurfaceFunctional {
        direction: std::array::from_fn(|i| {
            rows[0][i]
                .mul(direction[1])
                .sub(rows[1][i].mul(direction[0]))
        }),
        origin: coordinates.source_anchor().map(Interval::point),
        offset: cross(coordinates.center_offset_family(), direction),
    };
    let [whole] = surface_functional_enclosures(surface, uv, &[functional], work, cancel)?;
    let center = uv.map(|range| range[0] * 0.5 + range[1] * 0.5);
    let [at_center] = surface_functional_enclosures(
        surface,
        center.map(|value| [value; 2]),
        &[functional],
        work,
        cancel,
    )?;
    let delta: [Interval; 2] = std::array::from_fn(|axis| {
        Interval {
            lo: uv[axis][0],
            hi: uv[axis][1],
        }
        .sub(Interval::point(center[axis]))
    });
    let mean_value = at_center
        .position
        .add(whole.du.mul(delta[0]))
        .add(whole.dv.mul(delta[1]));
    let lo = coarse.lo.max(whole.position.lo).max(mean_value.lo);
    let hi = coarse.hi.min(whole.position.hi).min(mean_value.hi);
    if lo > hi {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "original facet support enclosures are inconsistent",
        ));
    }
    Ok(Interval { lo, hi })
}

/// Complete filled-solid AND boundary distance in both directions. The closed
/// source atlas supplies coverage, role and degree-one authority; the visitor
/// independently pays for every role's same-azimuth displacement using native
/// coefficient images. Neither topology nor polygon bounds supply this metric.
#[expect(
    clippy::too_many_arguments,
    reason = "Bind the source and slab to actual material coordinates and native loops while keeping geometric policy and live resources explicit"
)]
pub(in crate::analysis) fn certify_closed_radial_slab(
    source: MaterialSourceView<'_, '_>,
    request: &super::super::SlabRequest,
    coordinates: &ActualMaterialCoordinates<'_>,
    actual_native_loops: &[SectionLoopMm],
    policy_mm: f64,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ClosedRadialSlab>, GeometryError> {
    work.charge(1, cancel)?;
    let kind = match actual_native_loops.len() {
        1 => RadialRegionKind::Disk,
        2 => RadialRegionKind::Annulus,
        _ => return Ok(None),
    };
    let capture = coordinates.capture();
    let Some(region) =
        certify_actual_polygon_radial_region(actual_native_loops, &capture, kind, work, cancel)?
    else {
        return Ok(None);
    };
    let outer = actual_native_loops
        .iter()
        .find(|ring| !ring.is_hole)
        .expect("calibrated radial region has an outer ring");
    let inner = actual_native_loops.iter().find(|ring| ring.is_hole);
    let center = capture.world_center_enclosure().map(|range| Interval {
        lo: range[0],
        hi: range[1],
    });
    let count = actual_native_loops
        .iter()
        .try_fold(0usize, |count, ring| {
            count.checked_add(ring.points_mm.len() - 1)
        })
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "radial metric edge count overflow",
            )
        })?;
    let index_allowance = remaining_bytes
        .checked_sub(
            std::mem::size_of::<RadialCandidate<'_>>()
                + std::mem::size_of::<ClosedRadialSlab>()
                + CORRELATED_SUPPORT_SCRATCH_BYTES,
        )
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "radial metric fixed storage exceeds remaining scratch",
            )
        })?;
    let index_bytes = RayEdgeIndex::required_heap_bytes(count)
        .filter(|bytes| *bytes <= index_allowance)
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "radial metric index exceeds remaining scratch",
            )
        })?;
    let outer_count = outer.points_mm.len() - 1;
    let index = RayEdgeIndex::new(
        count,
        index_bytes,
        |edge| {
            let (ring, edge) = if edge < outer_count {
                (outer, edge)
            } else {
                (
                    inner.expect("calibrated annulus has inner ring"),
                    edge - outer_count,
                )
            };
            std::array::from_fn(|i| {
                Interval::point(ring.points_mm[edge][i])
                    .hull(Interval::point(ring.points_mm[edge + 1][i]))
                    .sub(center[i])
            })
        },
        work,
        cancel,
    )?;
    let available = index_allowance
        .checked_sub(index.heap_bytes())
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "radial metric coexisting storage exceeds scratch",
            )
        })?;
    let candidate = RadialCandidate {
        outer,
        inner,
        center,
        index,
    };
    // The admitted source uncertainty is not erased by choosing canonical G.
    // Frobenius norm conservatively bounds its actual affine image; unlike a
    // rotation shortcut it remains valid for accepted near-unit quaternions.
    let sigma = coordinates
        .affine_rows()
        .iter()
        .flatten()
        .fold(Interval::point(0.0), |sum, coefficient| {
            sum.add(coefficient.square())
        })
        .sqrt()?
        .hi;
    let uncertainty = source
        .definition()
        .provenance
        .uncertainty_mm
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "closed radial metric requires admitted source uncertainty",
            )
        })?;
    let source_error = Interval::point(uncertainty).mul(Interval::point(sigma)).hi;
    let target = Interval::point(policy_mm)
        .sub(Interval::point(source_error))
        .lo;
    if !target.is_finite() || target <= 0.0 {
        return Ok(None);
    }
    let mut maximum = 0.0_f64;
    let product = super::super::critical::material_link::certify_closed_radial_material_product_with_boundary_visitor(
        source, request, coordinates, available, work, cancel,
        &mut |cell: BoundaryCell<'_>, inward, can_refine, work, cancel| {
            let point = coordinates.position_relative_to_center_family(cell.position)?;
            let distance = if let Some((surface, uv)) = cell.native_face {
                candidate.distance_with_support(
                    [point[0], point[1]], inward, target, work, cancel,
                    &mut |direction, coarse, work, cancel| {
                        native_face_support(
                            &surface, uv, coordinates, direction, coarse, work, cancel,
                        )
                    },
                )?
            } else {
                // Native edge and vertex images use their direct interval bounds.
                candidate.distance([point[0], point[1]], inward, work, cancel)?
            };
            let Some(distance) = distance
                else { return Ok(BoundaryVisit::RefineActualXy); };
            if distance > target {
                if !can_refine {
                    return Err(error(GeometryErrorCode::ShapeErrorExceeded,
                        "whole native boundary radial distance exceeds the actual polygon shape policy"));
                }
                return Ok(BoundaryVisit::RefineActualXy);
            }
            maximum = maximum.max(distance);
            Ok(BoundaryVisit::Accepted)
        },
    )?;
    let Some(product) = product else {
        return Ok(None);
    };
    if product.kind() != region.kind() {
        return Ok(None);
    }
    // At every (theta,z), both source and polygon fill [0,R], or [r,R].
    // Linear interpolation of corresponding interval endpoints bounds BOTH
    // filled-material directions by the largest per-role same-azimuth error.
    // Proper closed cuts and degree-one surjectivity are product premises.
    Ok(Some(ClosedRadialSlab {
        product,
        bound_mm: Interval::point(maximum)
            .add(Interval::point(source_error))
            .hi,
        native_xy_sha256: polygon_hash(actual_native_loops, work, cancel)?,
        inner_displacement_mm: Some(
            Interval::point(maximum)
                .add(Interval::point(source_error))
                .hi,
        ),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_facet_support_preserves_correlated_native_images_between_sample_points() {
        use solidcore::nurbs::knots::Side;
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(include_bytes!(
            "../../../../../fixtures/geometry/independent-exporters/occt-tapered-fitting-mm.step"
        ),
        &cancel,).unwrap();
        let frame = crate::PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
        };
        let rows = ActualMaterialCoordinates::source_rows(&frame).unwrap();
        let mut work = GeometryWorkBudget::new(20_000_000);
        let coordinates = ActualMaterialCoordinates::source(
            &definition,
            &frame,
            &rows,
            [0.6, 1.2],
            0.9,
            &mut work,
            &cancel,
        )
        .unwrap();
        let kernel = solidcore::Kernel::default();
        assert!(!definition.face_ids().is_empty());
        for &face_id in definition.face_ids() {
            let face = definition.brep().face(face_id).unwrap();
            let surface = definition.face_surface(face_id).unwrap();
            // Query the imported face's actual chart, not the legacy exporter's
            // first-face ordering or an unrelated hand-picked parameter box.
            let min = face.uv_domain.min();
            let max = face.uv_domain.max();
            let uv = [[min.x, max.x], [min.y, max.y]].map(|range| {
                let width = range[1] - range[0];
                [range[0] + 0.2 * width, range[0] + 0.8 * width]
            });
            assert!(
                uv.iter().all(|range| range[0].is_finite()
                    && range[1].is_finite()
                    && range[0] < range[1])
            );
            // Sample the native scalar coefficient evaluator independently of
            // the interval functional. A STEP analytic chart has no NURBS
            // substitution; a stored NURBS chart uses its literal coefficients.
            let nurbs = crate::parametric::native_surface_nurbs(surface).unwrap();
            let sample = |u, v| {
                if let Some(nurbs) = nurbs {
                    nurbs.eval(u, v, Side::Left, Side::Left).unwrap()
                } else {
                    surface
                        .evaluate_point(&kernel, solidcore::Vec2::new(u, v))
                        .unwrap()
                }
            };
            let jet =
                crate::parametric::surface_piecewise_enclosure(surface, uv, &mut work, &cancel)
                    .unwrap();
            let point = coordinates
                .position_relative_to_center_family(jet.position)
                .unwrap();
            let center_uv = uv.map(|range| range[0] * 0.5 + range[1] * 0.5);
            let center = sample(center_uv[0], center_uv[1]);
            let center = coordinates
                .position_relative_to_center_family(
                    [center.x, center.y, center.z].map(Interval::point),
                )
                .unwrap();
            let direction = [center[1].neg(), center[0]];
            let coarse = cross([point[0], point[1]], direction);
            let support = native_face_support(
                surface,
                uv,
                &coordinates,
                direction,
                coarse,
                &mut work,
                &cancel,
            )
            .unwrap();
            for i in 0..=8 {
                for j in 0..=6 {
                    let u = uv[0][0] + (uv[0][1] - uv[0][0]) * f64::from(i) / 8.0;
                    let v = uv[1][0] + (uv[1][1] - uv[1][0]) * f64::from(j) / 6.0;
                    let native = sample(u, v);
                    let native = coordinates
                        .position_relative_to_center_family(
                            [native.x, native.y, native.z].map(Interval::point),
                        )
                        .unwrap();
                    let value = cross([native[0], native[1]], direction);
                    assert!(
                        support.lo <= value.hi && value.lo <= support.hi,
                        "original native facet image escaped its whole-cell enclosure"
                    );
                }
            }
        }
    }

    #[test]
    fn inner_same_ray_maximum_detects_a_long_narrow_hole_arm() {
        let outer = SectionLoopMm {
            is_hole: false,
            points_mm: vec![
                [-12.0, -12.0, 0.0],
                [12.0, -12.0, 0.0],
                [12.0, 12.0, 0.0],
                [-12.0, 12.0, 0.0],
                [-12.0, -12.0, 0.0],
            ],
        };
        let inner = SectionLoopMm {
            is_hole: true,
            points_mm: vec![
                [8.0, 0.0, 0.0],
                [1.0, -0.1, 0.0],
                [0.0, -1.0, 0.0],
                [-1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.1, 0.0],
                [8.0, 0.0, 0.0],
            ],
        };
        let loops = [outer, inner];
        let mut work = GeometryWorkBudget::new(10_000);
        let cancel = AtomicBool::new(false);
        let region = super::super::radial_region::radial_region_bounds(
            &loops,
            [[0.0; 2]; 2],
            RadialRegionKind::Annulus,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            region.is_some(),
            "this adversary really is a nested one-sheet radial annulus"
        );
        let outer = &loops[0];
        let inner = &loops[1];
        let count = outer.points_mm.len() + inner.points_mm.len() - 2;
        let index = RayEdgeIndex::new(
            count,
            1_000_000,
            |edge| {
                let (ring, edge) = if edge < 4 {
                    (outer, edge)
                } else {
                    (inner, edge - 4)
                };
                std::array::from_fn(|i| {
                    Interval::point(ring.points_mm[edge][i])
                        .hull(Interval::point(ring.points_mm[edge + 1][i]))
                })
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let candidate = RadialCandidate {
            outer,
            inner: Some(inner),
            center: [Interval::point(0.0); 2],
            index,
        };
        let radial_distance = candidate
            .distance(
                [Interval::point(1.0), Interval::point(0.0)],
                true,
                &mut work,
                &cancel,
            )
            .unwrap()
            .unwrap();
        assert!(
            (7.0..7.000001).contains(&radial_distance),
            "the unit-circle source hole must pay for the arm's missing material"
        );
        // A near boundary point exists on each adjacent arm side. It cannot
        // replace the distant endpoint at the SOURCE point's own azimuth.
        assert!(distance_upper([1.0, 0.0], [1.0, 0.1]).unwrap() < 0.101);
    }
}
