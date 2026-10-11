// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{in_frame, kernel_error, resource_error};
use crate::{GeometryError, PlaneFrameMm, SectionLoopMm, check_cancel};
use monstertruck_modeling::{BoundedCurve, Curve, ParameterDivision1D, Wire};
use spiling_contracts::geometry::{
    MAX_GEOMETRY_CHUNK_BYTES, MAX_LOOP_POINTS, MAX_SECTION_BYTES, SECTION_PLANE_TOLERANCE_MM,
    SECTION_SAMPLING_TOLERANCE_MM,
};
use std::{cmp::Ordering, sync::atomic::AtomicBool};

/// Mirrors complete-loop SPLS packing without allocating an encoded copy.
/// Header and aligned offsets count against the same aggregate transfer cap.
pub(super) struct SectionBudget {
    committed_bytes: usize,
    chunk_loops: usize,
    chunk_points: usize,
    byte_limit: usize,
}

impl SectionBudget {
    pub(super) fn with_limit(bytes: u32) -> Result<Self, GeometryError> {
        if bytes > MAX_SECTION_BYTES {
            return Err(resource_error("section capacity exceeds frozen cap"));
        }
        Ok(Self {
            committed_bytes: 0,
            chunk_loops: 0,
            chunk_points: 0,
            byte_limit: bytes as usize,
        })
    }
    fn chunk_size(loops: usize, points: usize) -> usize {
        if loops == 0 {
            return 0;
        }
        let offsets = ((loops + 1) * 4 + 7) & !7;
        64 + offsets + points * 24
    }

    pub(super) fn check_loop(&self, points: usize) -> Result<(), GeometryError> {
        if points > MAX_LOOP_POINTS as usize {
            return Err(resource_error(
                "native section loop exceeds 40000-point cap",
            ));
        }
        let pending = Self::chunk_size(self.chunk_loops + 1, self.chunk_points + points);
        let bytes = if pending <= MAX_GEOMETRY_CHUNK_BYTES as usize {
            self.committed_bytes + pending
        } else {
            self.committed_bytes
                + Self::chunk_size(self.chunk_loops, self.chunk_points)
                + Self::chunk_size(1, points)
        };
        if bytes > self.byte_limit {
            return Err(resource_error("native section exceeds aggregate byte cap"));
        }
        Ok(())
    }

    pub(super) fn push_loop(&mut self, points: usize) -> Result<(), GeometryError> {
        self.check_loop(points)?;
        if Self::chunk_size(self.chunk_loops + 1, self.chunk_points + points)
            > MAX_GEOMETRY_CHUNK_BYTES as usize
        {
            self.committed_bytes += Self::chunk_size(self.chunk_loops, self.chunk_points);
            self.chunk_loops = 0;
            self.chunk_points = 0;
        }
        self.chunk_loops += 1;
        self.chunk_points += points;
        Ok(())
    }
}

impl Default for SectionBudget {
    fn default() -> Self {
        Self {
            committed_bytes: 0,
            chunk_loops: 0,
            chunk_points: 0,
            byte_limit: MAX_SECTION_BYTES as usize,
        }
    }
}

pub(super) struct CanonicalLoop {
    native: SectionLoopMm,
    bounds: [f64; 4],
}

impl CanonicalLoop {
    pub(super) fn is_hole(&self) -> bool {
        self.native.is_hole
    }
    pub(super) fn point_count(&self) -> usize {
        self.native.points_mm.len()
    }
    pub(super) fn into_native(self) -> SectionLoopMm {
        self.native
    }
}

pub(super) fn sample_wire(
    wire: &Wire,
    frame: &PlaneFrameMm,
    face_sign: f64,
    budget: &SectionBudget,
    cancel: &AtomicBool,
) -> Result<CanonicalLoop, GeometryError> {
    check_cancel(cancel)?;
    if !wire.is_closed() || !face_sign.is_finite() || face_sign.abs() < 0.5 {
        return Err(kernel_error(
            "native section cap boundary is not a closed oriented wire",
        ));
    }
    let mut points = Vec::new();
    for edge in wire {
        check_cancel(cancel)?;
        let curve = edge.oriented_curve();
        let range = curve.range_tuple();
        if !range.0.is_finite() || !range.1.is_finite() {
            return Err(kernel_error(
                "native cap curve has an unbounded parameter domain",
            ));
        }
        // The enum's optimized IntersectionCurve divider samples its leader.
        // Caps require points on the native surface intersection itself.
        let samples = match &curve {
            Curve::IntersectionCurve(native) => {
                native.try_parameter_division(range, SECTION_SAMPLING_TOLERANCE_MM)
            }
            _ => curve.try_parameter_division(range, SECTION_SAMPLING_TOLERANCE_MM),
        };
        check_cancel(cancel)?;
        let (_, samples) =
            samples.ok_or_else(|| kernel_error("native section curve sampling failed"))?;
        if samples.len() < 2 {
            return Err(kernel_error(
                "native section curve sampling omitted its endpoints",
            ));
        }
        let first: [f64; 3] = samples[0].into();
        let last: [f64; 3] = samples[samples.len() - 1].into();
        if in_frame(first, frame)?[2].abs() > SECTION_PLANE_TOLERANCE_MM {
            return Err(kernel_error(
                "native section boundary exceeds plane residual tolerance",
            ));
        }
        if distance(first, edge.front().point().into()) > SECTION_PLANE_TOLERANCE_MM
            || distance(last, edge.back().point().into()) > SECTION_PLANE_TOLERANCE_MM
        {
            return Err(kernel_error(
                "native section samples disagree with oriented edge endpoints",
            ));
        }
        let skip = usize::from(!points.is_empty());
        if let Some(previous) = points.last()
            && distance(*previous, first) > SECTION_PLANE_TOLERANCE_MM
        {
            return Err(kernel_error("native section sampled wire has a gap"));
        }
        budget.check_loop(points.len() + samples.len() - skip)?;
        for point in samples.into_iter().skip(skip) {
            let point: [f64; 3] = point.into();
            if in_frame(point, frame)?[2].abs() > SECTION_PLANE_TOLERANCE_MM {
                return Err(kernel_error(
                    "native section boundary exceeds plane residual tolerance",
                ));
            }
            points.push(point);
        }
        check_cancel(cancel)?;
    }
    canonicalize(points, frame, face_sign)
}

fn canonicalize(
    mut points: Vec<[f64; 3]>,
    frame: &PlaneFrameMm,
    face_sign: f64,
) -> Result<CanonicalLoop, GeometryError> {
    if points.len() < 4
        || distance(points[0], points[points.len() - 1]) > SECTION_PLANE_TOLERANCE_MM
    {
        return Err(kernel_error(
            "native section loop is not closed within tolerance",
        ));
    }
    points.pop();
    let reference = in_frame(points[0], frame)?;
    let mut previous = [0.0, 0.0];
    let mut twice_area = 0.0;
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for point in &points {
        let local = in_frame(*point, frame)?;
        let current = [local[0] - reference[0], local[1] - reference[1]];
        twice_area += previous[0] * current[1] - previous[1] * current[0];
        previous = current;
        bounds[0] = bounds[0].min(local[0]);
        bounds[1] = bounds[1].min(local[1]);
        bounds[2] = bounds[2].max(local[0]);
        bounds[3] = bounds[3].max(local[1]);
    }
    // The final edge returns to the translated origin and contributes zero.
    if !twice_area.is_finite() || twice_area == 0.0 {
        return Err(kernel_error(
            "native section cap boundary has zero or nonfinite area",
        ));
    }
    let is_hole = twice_area * face_sign < 0.0;
    if (twice_area > 0.0) == is_hole {
        points.reverse();
    }
    let mut least = 0;
    let mut least_coordinate = in_frame(points[0], frame)?;
    for (index, point) in points.iter().enumerate().skip(1) {
        let coordinate = in_frame(*point, frame)?;
        if compare_coordinates(&coordinate, &least_coordinate) == Ordering::Less {
            least = index;
            least_coordinate = coordinate;
        }
    }
    points.rotate_left(least);
    // Closure has already been proven. Represent the repeated endpoint exactly
    // once, with the same f64 coordinate as the first canonical native point.
    points.push(points[0]);
    Ok(CanonicalLoop {
        native: SectionLoopMm {
            is_hole,
            points_mm: points,
        },
        bounds,
    })
}

pub(super) fn compare_loops(
    a: &CanonicalLoop,
    b: &CanonicalLoop,
    frame: &PlaneFrameMm,
) -> Ordering {
    a.native
        .is_hole
        .cmp(&b.native.is_hole)
        .then_with(|| compare_coordinates(&a.bounds, &b.bounds))
        .then_with(|| {
            for (a, b) in a.native.points_mm.iter().zip(&b.native.points_mm) {
                // Coordinates were validated during canonicalization; only
                // equal-bounds ties need this frame-sequence comparison.
                let a = in_frame(*a, frame).expect("validated native section point");
                let b = in_frame(*b, frame).expect("validated native section point");
                let order = compare_coordinates(&a, &b);
                if order != Ordering::Equal {
                    return order;
                }
            }
            a.native.points_mm.len().cmp(&b.native.points_mm.len())
        })
}

fn compare_coordinates<const N: usize>(a: &[f64; N], b: &[f64; N]) -> Ordering {
    for axis in 0..N {
        let order = a[axis].total_cmp(&b[axis]);
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlaneMm;
    use spiling_contracts::geometry::GeometryErrorCode;

    #[test]
    fn canonical_loops_preserve_roles_and_reverse_to_declared_winding() {
        let frame = PlaneFrameMm::from_plane(PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        })
        .unwrap();
        let points = vec![
            [1.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ];
        let outer = canonicalize(points.clone(), &frame, -1.0).unwrap();
        let hole = canonicalize(points, &frame, 1.0).unwrap();
        assert!(!outer.is_hole());
        assert!(hole.is_hole());
        assert_eq!(outer.native.points_mm[0], [0.0, 1.0, 0.0]);
        assert_eq!(
            outer.native.points_mm.first(),
            outer.native.points_mm.last()
        );
        assert_eq!(hole.native.points_mm.first(), hole.native.points_mm.last());
        assert_eq!(compare_loops(&outer, &hole, &frame), Ordering::Less);
    }

    #[test]
    fn section_caps_are_enforced_without_truncating_loops() {
        let mut budget = SectionBudget::default();
        assert_eq!(
            budget
                .push_loop(MAX_LOOP_POINTS as usize + 1)
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        for _ in 0..17 {
            budget.push_loop(MAX_LOOP_POINTS as usize).unwrap();
        }
        assert_eq!(
            budget.push_loop(MAX_LOOP_POINTS as usize).unwrap_err().code,
            GeometryErrorCode::ResourceLimit
        );
    }

    #[test]
    fn native_open_or_zero_area_boundary_is_not_canonicalized_as_success() {
        let frame = PlaneFrameMm::from_plane(PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        })
        .unwrap();
        for points in [
            vec![[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0; 3]],
        ] {
            assert!(canonicalize(points, &frame, 1.0).is_err());
        }
    }
}
