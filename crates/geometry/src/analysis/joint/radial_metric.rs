// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A closed product paired with a complete filled-material metric. The radial
//! metric lane uses the SAME physical cylinder coordinates; an independently
//! completed full closed-slab metric can instead be consumed after world
//! placement. Bare boundary nearness about an unrelated axis is insufficient.

use super::radial_region::certify_actual_polygon_radial_region;
use super::*;
use crate::analysis::critical::ray_index::RayEdgeIndex;
use crate::analysis::critical::{ClosedRadialMaterialProduct, RadialRegionKind};
use crate::analysis::placed::ActualMaterialCoordinates;
use crate::section::OriginalTrimSource;

#[derive(Debug)]
pub(in crate::analysis) struct ClosedRadialSlab {
    pub(super) product: ClosedRadialMaterialProduct,
    pub(super) bound_mm: f64,
    native_xy_sha256: [u8; 32],
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
    if !analysis.matches_original_material(definition)
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
            let direction = std::array::from_fn(|i| b[i].sub(a[i]));
            let denominator = cross(point, direction).mul(sign);
            let numerator = cross(a, b).mul(sign);
            if denominator.lo <= 0.0 || numerator.lo <= 0.0 {
                resolved = false;
                return Ok(());
            }
            let scale = numerator.div(denominator)?;
            let distance = radius.mul(Interval::point(1.0).sub(scale)).abs_upper();
            resolved &= distance.is_finite();
            maximum = maximum.max(distance);
            Ok(())
        })?;
        Ok((resolved && count != 0).then_some(maximum))
    }
}

pub(in crate::analysis) fn certify_closed_radial_slab(
    source: OriginalTrimSource<'_, '_>,
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
            std::mem::size_of::<RadialCandidate<'_>>() + std::mem::size_of::<ClosedRadialSlab>(),
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
        &mut |position, inward, can_refine, work, cancel| {
            let point = coordinates.position_relative_to_center_family(position)?;
            let Some(distance) = candidate.distance([point[0], point[1]], inward, work, cancel)?
                else { return Ok(false); };
            if distance > target {
                if !can_refine {
                    return Err(error(GeometryErrorCode::ShapeErrorExceeded,
                        "whole native boundary radial distance exceeds the actual polygon shape policy"));
                }
                return Ok(false);
            }
            maximum = maximum.max(distance);
            Ok(true)
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
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            radial_distance >= 7.0 && radial_distance < 7.000001,
            "the unit-circle source hole must pay for the arm's missing material"
        );
        // A near boundary point exists on each adjacent arm side. It cannot
        // replace the distant endpoint at the SOURCE point's own azimuth.
        assert!(distance_upper([1.0, 0.0], [1.0, 0.1]).unwrap() < 0.101);
    }
}
