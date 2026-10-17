// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Calibration of the exact used XY region, independently of native genealogy,
//! the native sampling-plane tube and all actual-candidate metric obligations.

use super::super::critical::RadialRegionKind;
use super::super::placed::MaterialCoordinatesCapture;
use super::{cross, delta};
use crate::parametric::Interval;
use crate::{GeometryError, GeometryWorkBudget, SectionLoopMm};
use std::sync::atomic::AtomicBool;

/// Ephemeral calibration bounds for the exact input filled region.
/// The caller binds its candidate separately; this is not a retained receipt.
#[derive(Debug)]
pub(super) struct ActualPolygonRadialRegion {
    kind: RadialRegionKind,
    outer_kernel_radius_mm: f64,
    inner_cover_radius_mm: f64,
}

impl ActualPolygonRadialRegion {
    pub(super) fn kind(&self) -> RadialRegionKind {
        self.kind
    }

    pub(super) fn outer_kernel_radius_mm(&self) -> f64 {
        self.outer_kernel_radius_mm
    }

    pub(super) fn inner_cover_radius_mm(&self) -> f64 {
        self.inner_cover_radius_mm
    }
}

pub(super) fn certify_actual_polygon_radial_region(
    loops: &[SectionLoopMm],
    coordinates: &MaterialCoordinatesCapture,
    kind: RadialRegionKind,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ActualPolygonRadialRegion>, GeometryError> {
    work.charge(1, cancel)?;
    let Some([outer_kernel_radius_mm, inner_cover_radius_mm]) = radial_region_bounds(
        loops,
        coordinates.world_center_enclosure(),
        kind,
        work,
        cancel,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(ActualPolygonRadialRegion {
        kind,
        outer_kernel_radius_mm,
        inner_cover_radius_mm,
    }))
}

/// The complete encoding, not a selected subset, must match the source kind.
/// A common positive radial interval is a conservative, uniform nesting proof.
pub(super) fn radial_region_bounds(
    loops: &[SectionLoopMm],
    center: [[f64; 2]; 2],
    kind: RadialRegionKind,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[f64; 2]>, GeometryError> {
    work.charge(1, cancel)?;
    let count = match kind {
        RadialRegionKind::Disk => 1,
        RadialRegionKind::Annulus => 2,
    };
    if loops.len() != count
        || center
            .iter()
            .any(|axis| axis.iter().any(|value| !value.is_finite()) || axis[0] > axis[1])
    {
        return Ok(None);
    }
    let center = center.map(|axis| Interval {
        lo: axis[0],
        hi: axis[1],
    });
    let mut outer = None;
    let mut inner = None;
    for ring in loops {
        let Some(bound) = radial_ring_bound(ring, center, work, cancel)? else {
            return Ok(None);
        };
        let role = if ring.is_hole { &mut inner } else { &mut outer };
        if role.replace(bound).is_some() {
            return Ok(None);
        }
    }
    let Some(outer) = outer else {
        return Ok(None);
    };
    let inner = match (kind, inner) {
        (RadialRegionKind::Disk, None) => 0.0,
        (RadialRegionKind::Annulus, Some(inner)) => inner,
        _ => return Ok(None),
    };
    Ok((inner < outer).then_some([outer, inner]))
}

/// Strict angular derivative on EVERY whole edge, uniformly in the captured
/// center box, plus incidence-counted degree one. The former alone would admit
/// a star-kernel pentagram. Together they make the angular projection a
/// one-sheeted covering, hence a simple radial graph with no hidden spur.
///
/// This uses the same supporting-halfplane determinant and outward norm bounds
/// as `source_loop_shape`, but about the fixed actual material axis. That
/// helper's bbox midpoint/kernel and retained XY hash cannot certify this axis.
/// Returns an outer kernel lower bound or an inner cover upper bound by role.
fn radial_ring_bound(
    ring: &SectionLoopMm,
    center: [Interval; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<f64>, GeometryError> {
    work.charge(1, cancel)?;
    if ring.points_mm.len() < 4 || ring.points_mm.first() != ring.points_mm.last() {
        return Ok(None);
    }
    let mut bound = if ring.is_hole { 0.0_f64 } else { f64::INFINITY };
    let mut positive_ray_incidence = 0usize;
    // One literal corner is enough to calibrate degree. Uniform nonzero edge
    // determinants exclude the axis throughout the connected center family, so
    // the degree cannot change elsewhere in that family.
    let ray_y = center[1].lo;
    for edge in ring.points_mm.windows(2) {
        work.charge(1, cancel)?;
        if edge[0].iter().any(|value| !value.is_finite()) {
            return Ok(None);
        }
        let a = [edge[0][0], edge[0][1]];
        let b = [edge[1][0], edge[1][1]];
        if a == b {
            // Do not silently discard a zero projected appendage, even when
            // its native 3D endpoints differ.
            return Ok(None);
        }
        let direction = delta(b, a);
        let to_center = std::array::from_fn(|i| center[i].sub(Interval::point(a[i])));
        let determinant = cross(direction, to_center);
        let signed = if ring.is_hole {
            -determinant.hi
        } else {
            determinant.lo
        };
        if !signed.is_finite() || signed <= 0.0 {
            return Ok(None);
        }

        // Half-open vertex ownership counts a ray-through-vertex exactly once,
        // including the closing endpoint. With the strict angular sign above,
        // an upward (outer) / downward (inner) crossing lies on the POSITIVE
        // ray: x_intersection = cross(a-c,b-c)/(b_y-a_y). Thus no rounded ray
        // intersection, atan2 sum, sampled angle or epsilon is needed.
        let crosses_positive_ray = if ring.is_hole {
            b[1] <= ray_y && ray_y < a[1]
        } else {
            a[1] <= ray_y && ray_y < b[1]
        };
        if crosses_positive_ray {
            positive_ray_incidence += 1;
            if positive_ray_incidence > 1 {
                return Ok(None);
            }
        }

        let squared = if ring.is_hole {
            // Convexity of distance along an edge bounds its whole extent by
            // the endpoints. Every endpoint occurs once as edge[0].
            to_center[0].square().add(to_center[1].square())
        } else {
            direction[0].square().add(direction[1].square())
        };
        if !squared.hi.is_finite() || squared.lo < 0.0 {
            return Ok(None);
        }
        let length = squared.sqrt()?;
        if ring.is_hole {
            bound = bound.max(length.hi);
        } else {
            if length.lo <= 0.0 || !length.hi.is_finite() {
                return Ok(None);
            }
            // Distance to the whole supporting line is a lower bound for
            // distance to its segment, uniformly over actual center choices.
            let kernel = Interval::point(signed).div(Interval::point(length.hi))?.lo;
            if !kernel.is_finite() || kernel <= 0.0 {
                return Ok(None);
            }
            bound = bound.min(kernel);
        }
    }
    Ok((positive_ray_incidence == 1 && bound.is_finite() && bound > 0.0).then_some(bound))
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::geometry::GeometryErrorCode;

    fn ring(points: &[[f64; 2]], hole: bool) -> SectionLoopMm {
        let mut points_mm: Vec<_> = points
            .iter()
            .map(|p| [p[0], p[1], 7.0 + p[0] * 0.125])
            .collect();
        points_mm.push(points_mm[0]);
        SectionLoopMm {
            is_hole: hole,
            points_mm,
        }
    }

    fn graph(
        count: usize,
        phase: f64,
        scale: f64,
        center: [f64; 2],
        hole: bool,
        concave: bool,
    ) -> SectionLoopMm {
        let points: Vec<_> = (0..count)
            .map(|i| {
                let theta = phase + std::f64::consts::TAU * i as f64 / count as f64;
                let radius = scale * if concave && i % 2 == 1 { 0.45 } else { 1.0 };
                [
                    center[0] + radius * theta.cos(),
                    center[1] + radius * theta.sin(),
                ]
            })
            .collect();
        let mut result = ring(&points, hole);
        if hole {
            result.points_mm.reverse();
        }
        result
    }

    fn bounds(
        loops: &[SectionLoopMm],
        center: [[f64; 2]; 2],
        kind: RadialRegionKind,
    ) -> Option<[f64; 2]> {
        radial_region_bounds(
            loops,
            center,
            kind,
            &mut GeometryWorkBudget::new(1_000_000),
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn convex_and_concave_radial_graphs_have_uniform_filled_disk_and_annulus_bounds() {
        for count in [4, 8, 16, 30] {
            for phase in [0.0, 0.137, 1.25] {
                for scale in [0.125, 1.0, 128.0] {
                    for concave in [false, true] {
                        let center = [17.0, -31.0];
                        let family = center.map(|v| [v - scale * 0.001, v + scale * 0.001]);
                        let outer = graph(count, phase, scale, center, false, concave);
                        let disk = bounds(&[outer], family, RadialRegionKind::Disk).unwrap();
                        assert!(disk[0] > scale * 0.1);
                        assert_eq!(disk[1], 0.0);
                        let outer = graph(count, phase, scale, center, false, concave);
                        let inner = graph(count, phase + 0.05, scale * 0.05, center, true, concave);
                        let annulus =
                            bounds(&[inner, outer], family, RadialRegionKind::Annulus).unwrap();
                        assert!(annulus[1] >= scale * 0.05);
                        assert!(annulus[1] < annulus[0]);
                    }
                }
            }
        }
    }

    #[test]
    fn star_kernel_pentagram_is_not_a_one_sheeted_angular_cover() {
        let points = [
            [0.0, 5.0],
            [-3.0, -4.0],
            [5.0, 2.0],
            [-5.0, 2.0],
            [3.0, -4.0],
        ];
        for i in 0..points.len() {
            let a = points[i];
            let b = points[(i + 1) % points.len()];
            assert!(a[0] * b[1] - a[1] * b[0] > 0.0);
        }
        assert!(
            bounds(
                &[ring(&points, false)],
                [[0.0; 2]; 2],
                RadialRegionKind::Disk
            )
            .is_none()
        );
        let mut hole = ring(&points, true);
        hole.points_mm.reverse();
        assert!(
            radial_ring_bound(
                &hole,
                [Interval::point(0.0); 2],
                &mut GeometryWorkBudget::new(100),
                &AtomicBool::new(false)
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn material_left_orientation_and_complete_region_inventory_are_required() {
        let center = [[0.0; 2]; 2];
        for outer_hole in [false, true] {
            for inner_hole in [false, true] {
                let outer = graph(8, 0.0, 8.0, [0.0; 2], outer_hole, false);
                let inner = graph(8, 0.0, 1.0, [0.0; 2], inner_hole, false);
                assert_eq!(
                    bounds(&[outer, inner], center, RadialRegionKind::Annulus).is_some(),
                    !outer_hole && inner_hole
                );
            }
        }
        let mut clockwise_outer = graph(8, 0.0, 8.0, [0.0; 2], true, false);
        clockwise_outer.is_hole = false;
        assert!(bounds(&[clockwise_outer], center, RadialRegionKind::Disk).is_none());
        let outer = graph(8, 0.0, 8.0, [0.0; 2], false, false);
        let mut counterclockwise_hole = graph(8, 0.0, 1.0, [0.0; 2], false, false);
        counterclockwise_hole.is_hole = true;
        assert!(
            bounds(
                &[outer, counterclockwise_hole],
                center,
                RadialRegionKind::Annulus
            )
            .is_none()
        );
        for extra_hole in [false, true] {
            let loops = [
                graph(8, 0.0, 8.0, [0.0; 2], false, false),
                graph(8, 0.0, 1.0, [0.0; 2], true, false),
                graph(8, 0.0, 0.25, [3.0, 0.0], extra_hole, false),
            ];
            assert!(bounds(&loops, center, RadialRegionKind::Annulus).is_none());
            assert!(bounds(&loops, center, RadialRegionKind::Disk).is_none());
        }
        assert!(bounds(&[], center, RadialRegionKind::Disk).is_none());
        assert!(
            bounds(
                &[graph(8, 0.0, 8.0, [0.0; 2], false, false)],
                center,
                RadialRegionKind::Annulus
            )
            .is_none()
        );
        assert!(
            bounds(
                &[graph(8, 0.0, 1.0, [0.0; 2], true, false)],
                center,
                RadialRegionKind::Disk
            )
            .is_none()
        );
    }

    #[test]
    fn closed_ray_endpoints_are_owned_once_without_coordinate_cancellation() {
        let diamond = [[4.0, 0.0], [0.0, 4.0], [-4.0, 0.0], [0.0, -4.0]];
        for offset in [0.0, 35_184_372_088_832.0] {
            for first in 0..4 {
                let points: Vec<_> = (0..4)
                    .map(|i| {
                        let p = diamond[(first + i) % 4];
                        [p[0] + offset, p[1] - offset]
                    })
                    .collect();
                let center = [[offset, offset], [-offset, -offset]];
                let disk = bounds(&[ring(&points, false)], center, RadialRegionKind::Disk).unwrap();
                assert!(disk[0] > 2.8 && disk[0] < 2.9);
                let mut hole = ring(&points, true);
                hole.points_mm.reverse();
                assert!(
                    radial_ring_bound(
                        &hole,
                        [Interval::point(offset), Interval::point(-offset)],
                        &mut GeometryWorkBudget::new(100),
                        &AtomicBool::new(false)
                    )
                    .unwrap()
                    .is_some()
                );
            }
        }
    }

    #[test]
    fn axis_contacts_spurs_open_and_nonfinite_rings_are_not_filled_regions() {
        let center = [[0.0; 2]; 2];
        for points in [
            vec![[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]],
            vec![[-4.0, 0.0], [4.0, 0.0], [0.0, 4.0]],
            vec![[4.0, 0.0], [0.0, 4.0], [0.0, 4.0], [-4.0, 0.0], [0.0, -4.0]],
            vec![
                [4.0, 0.0],
                [0.0, 4.0],
                [0.0, 5.0],
                [0.0, 4.0],
                [-4.0, 0.0],
                [0.0, -4.0],
            ],
        ] {
            assert!(bounds(&[ring(&points, false)], center, RadialRegionKind::Disk).is_none());
        }
        let mut open = graph(8, 0.0, 8.0, [0.0; 2], false, false);
        open.points_mm.pop();
        assert!(bounds(&[open], center, RadialRegionKind::Disk).is_none());
        for axis in 0..3 {
            let mut nonfinite = graph(8, 0.0, 8.0, [0.0; 2], false, false);
            nonfinite.points_mm[2][axis] = f64::NAN;
            assert!(bounds(&[nonfinite], center, RadialRegionKind::Disk).is_none());
        }
        let outer = graph(8, 0.0, 8.0, [0.0; 2], false, false);
        assert!(bounds(&[outer], [[-9.0, 9.0], [0.0; 2]], RadialRegionKind::Disk).is_none());
    }

    #[test]
    fn annular_material_requires_strict_uniform_nesting_not_just_two_degree_one_rings() {
        for inner_radius in [4.0, 8.0, 9.0] {
            let outer = ring(&[[-4.0, -4.0], [4.0, -4.0], [4.0, 4.0], [-4.0, 4.0]], false);
            let mut inner = ring(
                &[
                    [inner_radius, 0.0],
                    [0.0, inner_radius],
                    [-inner_radius, 0.0],
                    [0.0, -inner_radius],
                ],
                true,
            );
            inner.points_mm.reverse();
            assert!(bounds(&[outer, inner], [[0.0; 2]; 2], RadialRegionKind::Annulus).is_none());
        }
        let outer = graph(8, 0.0, 8.0, [0.0; 2], false, false);
        let inner = graph(8, 0.0, 1.0, [10.0, 0.0], true, false);
        assert!(bounds(&[outer, inner], [[0.0; 2]; 2], RadialRegionKind::Annulus).is_none());
    }

    #[test]
    fn cancellation_and_exhaustion_propagate_instead_of_minting_authority() {
        let loops = [graph(8, 0.0, 8.0, [0.0; 2], false, false)];
        let cancelled = radial_region_bounds(
            &loops,
            [[0.0; 2]; 2],
            RadialRegionKind::Disk,
            &mut GeometryWorkBudget::new(100),
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert_eq!(cancelled.code, GeometryErrorCode::Cancelled);
        let exhausted = radial_region_bounds(
            &loops,
            [[0.0; 2]; 2],
            RadialRegionKind::Disk,
            &mut GeometryWorkBudget::new(2),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(exhausted.code, GeometryErrorCode::ResourceLimit);
    }
}
