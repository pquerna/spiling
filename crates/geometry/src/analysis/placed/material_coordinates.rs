// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{HeightMap, Matrix, add, cross, dot, error, finite, sub};
use crate::{Definition, GeometryError, GeometryWorkBudget, PlaneFrameMm, parametric::Interval};
use sha2::{Digest, Sha256};
use spiling_contracts::geometry::{GeometryErrorCode, RigidPoseMm};
use std::sync::atomic::AtomicBool;

/// Coordinate input only: this never establishes material, topology or a metric.
/// The matrix is borrowed from the validated query owner, not copied per cell.
pub(in crate::analysis) struct ActualMaterialCoordinates<'m> {
    rows: &'m Matrix,
    cofactor: Matrix,
    source_anchor: [f64; 3],
    height_at_anchor: Interval,
    capture: MaterialCoordinatesCapture,
}

/// A compact input binding. Products retain this, never the query's matrix.
#[derive(Debug, Clone, Copy)]
pub(in crate::analysis) struct MaterialCoordinatesCapture {
    axis_sha256: [u8; 32],
    center: [[f64; 2]; 2],
    interval: [f64; 2],
    sample: f64,
}

enum AxisInput<'a> {
    World(&'a RigidPoseMm),
    Source(&'a PlaneFrameMm),
}

fn source_anchor(definition: &Definition) -> Result<[f64; 3], GeometryError> {
    let bounds = definition.bounds_mm();
    let anchor = std::array::from_fn(|i| bounds.min[i] * 0.5 + bounds.max[i] * 0.5);
    if anchor.iter().any(|v| !v.is_finite()) {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "material coordinate proposal requires finite immutable source bounds",
        ));
    }
    // This is a fixed proposal, not a claim that the axis lies in material.
    Ok(anchor)
}

fn axis_hash(definition: &Definition, anchor: [f64; 3], input: AxisInput<'_>) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"spiling-actual-material-axis-1\0");
    hash.update(definition.id().as_str().as_bytes());
    hash.update(crate::KERNEL_REVISION.as_bytes());
    hash.update(crate::kernel::MaterialRealizationIdentity::new(definition).identity_bytes());
    for value in anchor {
        hash.update(value.to_le_bytes());
    }
    match input {
        AxisInput::World(pose) => {
            hash.update([1]);
            for value in pose.translation_mm.into_iter().chain(pose.rotation_xyzw) {
                hash.update(value.to_le_bytes());
            }
        }
        AxisInput::Source(frame) => {
            hash.update([0]);
            for vector in [frame.origin_mm, frame.x_axis, frame.y_axis, frame.z_axis] {
                for value in vector {
                    hash.update(value.to_le_bytes());
                }
            }
        }
    }
    hash.finalize().into()
}

fn same_scope(a: [f64; 2], sample_a: f64, b: [f64; 2], sample_b: f64) -> bool {
    a.map(f64::to_bits) == b.map(f64::to_bits) && sample_a.to_bits() == sample_b.to_bits()
}

impl MaterialCoordinatesCapture {
    pub(in crate::analysis) fn axis_sha256(&self) -> [u8; 32] {
        self.axis_sha256
    }
    pub(in crate::analysis) fn world_center_enclosure(&self) -> [[f64; 2]; 2] {
        self.center
    }
    pub(in crate::analysis) fn world_interval(&self) -> [f64; 2] {
        self.interval
    }
    pub(in crate::analysis) fn sample_world_z(&self) -> f64 {
        self.sample
    }
    pub(in crate::analysis) fn matches_world(
        &self,
        definition: &Definition,
        pose: RigidPoseMm,
        interval: [f64; 2],
        sample: f64,
    ) -> bool {
        if pose.validate().is_err() || !same_scope(self.interval, self.sample, interval, sample) {
            return false;
        }
        source_anchor(definition).is_ok_and(|anchor| {
            self.axis_sha256 == axis_hash(definition, anchor, AxisInput::World(&pose))
        })
    }
    pub(in crate::analysis) fn matches_source(
        &self,
        definition: &Definition,
        frame: &PlaneFrameMm,
        interval: [f64; 2],
        sample: f64,
    ) -> bool {
        if frame.validate().is_err() || !same_scope(self.interval, self.sample, interval, sample) {
            return false;
        }
        source_anchor(definition).is_ok_and(|anchor| {
            self.axis_sha256 == axis_hash(definition, anchor, AxisInput::Source(frame))
        })
    }
}

impl<'m> ActualMaterialCoordinates<'m> {
    pub(in crate::analysis) const SCRATCH_BYTES: usize =
        std::mem::size_of::<Self>() + std::mem::size_of::<Matrix>();

    pub(super) fn placed(
        definition: &Definition,
        map: &'m HeightMap,
        interval: [f64; 2],
        sample: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        Self::new(
            definition,
            &map.matrix,
            map.pose.translation_mm.map(Interval::point),
            AxisInput::World(&map.pose),
            interval,
            sample,
            work,
            cancel,
        )
    }

    pub(in crate::analysis) fn source_rows(frame: &PlaneFrameMm) -> Result<Matrix, GeometryError> {
        frame.validate()?;
        Ok([frame.x_axis, frame.y_axis, frame.z_axis].map(|row| row.map(Interval::point)))
    }

    pub(in crate::analysis) fn source(
        definition: &Definition,
        frame: &PlaneFrameMm,
        rows: &'m Matrix,
        interval: [f64; 2],
        sample: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        frame.validate()?;
        for (actual, expected) in rows.iter().zip([frame.x_axis, frame.y_axis, frame.z_axis]) {
            for (actual, expected) in actual.iter().zip(expected) {
                if actual.lo.to_bits() != expected.to_bits() || actual.hi.to_bits() != expected.to_bits() {
                    return Err(error(GeometryErrorCode::InvalidGeometry, "source material coordinate rows do not match their captured frame"));
                }
            }
        }
        let origin = frame.origin_mm.map(Interval::point);
        let offset = std::array::from_fn(|i| sub(Interval::point(0.0), dot(rows[i], origin)));
        Self::new(definition, rows, offset, AxisInput::Source(frame), interval, sample, work, cancel)
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        definition: &Definition,
        rows: &'m Matrix,
        offset: [Interval; 3],
        input: AxisInput<'_>,
        interval: [f64; 2],
        sample: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        if interval.iter().any(|v| !v.is_finite()) || !sample.is_finite() || !(interval[0] < sample && sample < interval[1]) {
            return Err(error(GeometryErrorCode::InvalidGeometry, "material coordinates require a finite CLOSED interval and literal interior sample"));
        }
        let cofactor = [cross(rows[1], rows[2]), cross(rows[2], rows[0]), cross(rows[0], rows[1])];
        if finite(dot(rows[0], cofactor[0]))?.lo <= 0.0 {
            return Err(error(GeometryErrorCode::ConditioningFailure, "material coordinates require a proved positive affine orientation"));
        }
        let anchor = source_anchor(definition)?;
        let anchor_intervals = anchor.map(Interval::point);
        let at_anchor = std::array::from_fn::<_, 3, _>(|i| add(dot(rows[i], anchor_intervals), offset[i]));
        let at_anchor = [finite(at_anchor[0])?, finite(at_anchor[1])?, finite(at_anchor[2])?];
        let capture = MaterialCoordinatesCapture {
            axis_sha256: axis_hash(definition, anchor, input),
            center: [[at_anchor[0].lo, at_anchor[0].hi], [at_anchor[1].lo, at_anchor[1].hi]],
            interval,
            sample,
        };
        Ok(Self { rows, cofactor, source_anchor: anchor, height_at_anchor: at_anchor[2], capture })
    }

    pub(in crate::analysis) fn source_anchor(&self) -> [f64; 3] { self.source_anchor }
    pub(in crate::analysis) fn affine_rows(&self) -> &Matrix { self.rows }
    pub(in crate::analysis) fn height_at_anchor(&self) -> Interval { self.height_at_anchor }
    pub(in crate::analysis) fn world_interval(&self) -> [f64; 2] { self.capture.interval }
    pub(in crate::analysis) fn sample_world_z(&self) -> f64 { self.capture.sample }
    pub(in crate::analysis) fn capture(&self) -> MaterialCoordinatesCapture { self.capture }

    /// The same intended center is subtracted algebraically. Do not subtract
    /// an independently rounded world-center midpoint from transformed points.
    pub(in crate::analysis) fn position(&self, source: [Interval; 3]) -> Result<[Interval; 3], GeometryError> {
        let relative = std::array::from_fn(|i| sub(source[i], Interval::point(self.source_anchor[i])));
        Ok([
            finite(dot(self.rows[0], relative))?,
            finite(dot(self.rows[1], relative))?,
            finite(add(dot(self.rows[2], relative), self.height_at_anchor))?,
        ])
    }

    /// Uniform in every independent candidate center in the captured box.
    /// `position` cancels the one intended center algebraically; that does not
    /// by itself quantify over independently rounded centers.
    pub(in crate::analysis) fn position_relative_to_center_family(
        &self,
        source: [Interval; 3],
    ) -> Result<[Interval; 3], GeometryError> {
        let mut position = self.position(source)?;
        for (axis, value) in position[..2].iter_mut().enumerate() {
            let center = Interval::new(self.capture.center[axis][0], self.capture.center[axis][1])?;
            *value = finite(add(*value, sub(center, center)))?;
        }
        Ok(position)
    }

    /// Oriented area normal, not a normalized direction. Positive determinant
    /// is proved by construction; no exact-orthogonal transform is assumed.
    pub(in crate::analysis) fn normal(&self, normal: [Interval; 3]) -> Result<[Interval; 3], GeometryError> {
        Ok([finite(dot(self.cofactor[0], normal))?, finite(dot(self.cofactor[1], normal))?, finite(dot(self.cofactor[2], normal))?])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coordinates(rows: &Matrix) -> ActualMaterialCoordinates<'_> {
        ActualMaterialCoordinates {
            rows,
            cofactor: [cross(rows[1], rows[2]), cross(rows[2], rows[0]), cross(rows[0], rows[1])],
            source_anchor: [1.0, 2.0, 3.0],
            height_at_anchor: Interval::point(-6.0),
            capture: MaterialCoordinatesCapture {
                axis_sha256: [0; 32],
                center: [[1e16, 1e16 + 2.0], [-1e16, -1e16 + 2.0]],
                interval: [-3.0, -1.0],
                sample: -2.0,
            },
        }
    }

    #[test]
    fn affine_area_normal_matches_transformed_tangents_not_rotated_normal() {
        let rows = [[2.0, 1.0, 0.0], [0.0, 3.0, 1.0], [0.0, 0.0, 4.0]]
            .map(|row| row.map(Interval::point));
        let coordinates = coordinates(&rows);
        let u = [0.0, 0.0, 1.0].map(Interval::point);
        let v = [1.0, 0.0, 0.0].map(Interval::point);
        let transformed_u = std::array::from_fn(|i| dot(rows[i], u));
        let transformed_v = std::array::from_fn(|i| dot(rows[i], v));
        let geometric = cross(transformed_u, transformed_v);
        let normal = coordinates.normal(cross(u, v)).unwrap();
        for ((actual, geometric), expected) in normal.into_iter().zip(geometric).zip([0.0, 8.0, -2.0]) {
            assert!(actual.lo <= expected && expected <= actual.hi);
            assert!(geometric.lo <= expected && expected <= geometric.hi);
        }
        // Multiplication by A would instead give [1,3,0], a different plane.
        assert!(normal[0].hi < 1.0 && normal[1].lo > 3.0 && normal[2].hi < 0.0);
    }

    #[test]
    fn large_translation_cancels_only_the_intended_center_not_the_center_family() {
        let rows = [[2.0, 1.0, 0.0], [0.0, 3.0, 1.0], [0.0, 0.0, 4.0]]
            .map(|row| row.map(Interval::point));
        let coordinates = coordinates(&rows);
        let point = [2.0, 3.0, 4.0].map(Interval::point);
        let intended = coordinates.position(point).unwrap();
        for (actual, expected) in intended.into_iter().zip([3.0, 4.0, -2.0]) {
            assert_eq!([actual.lo, actual.hi], [expected; 2]);
        }
        let family = coordinates.position_relative_to_center_family(point).unwrap();
        for (axis, range) in [[1.0, 5.0], [2.0, 6.0], [-2.0, -2.0]].into_iter().enumerate() {
            assert!(family[axis].lo <= range[0] && range[1] <= family[axis].hi);
        }
        assert!(family[0].lo < intended[0].lo && family[0].hi > intended[0].hi);
    }
}
