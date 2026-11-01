// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Transient inspection coordinates, never project geometry or manufacturing authority.
mod contours;
pub use contours::ContourPreview;
use glam::{DQuat, DVec3};
use serde::{Deserialize, Serialize};
use spiling_contracts::geometry::{AabbMm, PlaneMm, RigidPoseMm};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
    #[default]
    Z,
    Free,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Representation {
    #[default]
    Source,
    Actual,
    Compare,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionView {
    pub enabled: bool,
    pub axis: Axis,
    pub offset_mm: f64,
    pub angles_deg: [f64; 2],
    pub local_frame: bool,
    pub flipped: bool,
    pub section_only: bool,
    pub selected_only: bool,
    pub representation: Representation,
}
impl Default for SectionView {
    fn default() -> Self {
        Self {
            enabled: false,
            axis: Axis::Z,
            offset_mm: 1.0,
            angles_deg: [0.0, 90.0],
            local_frame: false,
            flipped: false,
            section_only: false,
            selected_only: false,
            representation: Representation::Source,
        }
    }
}
impl SectionView {
    pub fn valid(&self) -> bool {
        self.offset_mm.is_finite()
            && self
                .angles_deg
                .iter()
                .all(|a| a.is_finite() && a.abs() <= 360.0)
    }
    pub fn normal(&self) -> DVec3 {
        match self.axis {
            Axis::X => DVec3::X,
            Axis::Y => DVec3::Y,
            Axis::Z => DVec3::Z,
            Axis::Free => {
                let [azimuth, elevation] = self.angles_deg.map(f64::to_radians);
                DVec3::new(
                    elevation.cos() * azimuth.cos(),
                    elevation.cos() * azimuth.sin(),
                    elevation.sin(),
                )
            }
        }
    }
    pub fn plane(&self, pose: Option<&RigidPoseMm>) -> PlaneMm {
        let mut normal = self.normal();
        let mut origin = normal * self.offset_mm;
        if let Some(pose) = pose.filter(|_| self.local_frame) {
            let rotation = DQuat::from_array(pose.rotation_xyzw);
            origin = rotation * origin + DVec3::from_array(pose.translation_mm);
            normal = rotation * normal;
        }
        PlaneMm {
            origin_mm: origin.to_array(),
            normal: normal.to_array(),
        }
    }
    pub fn range(&self, bounds: AabbMm, pose: Option<&RigidPoseMm>) -> [f64; 2] {
        let mut range = [f64::INFINITY, f64::NEG_INFINITY];
        for i in 0..8 {
            let point = DVec3::from_array(std::array::from_fn(|j| {
                if i & (1 << j) == 0 {
                    bounds.min[j]
                } else {
                    bounds.max[j]
                }
            }));
            let local = if let Some(pose) = pose.filter(|_| self.local_frame) {
                DQuat::from_array(pose.rotation_xyzw).conjugate()
                    * (point - DVec3::from_array(pose.translation_mm))
            } else {
                point
            };
            let d = self.normal().dot(local);
            range[0] = range[0].min(d);
            range[1] = range[1].max(d);
        }
        range
    }
    pub fn keep(&self, plane: PlaneMm, point: DVec3) -> bool {
        let d = DVec3::from_array(plane.normal).dot(point - DVec3::from_array(plane.origin_mm));
        !self.enabled || if self.flipped { d >= -1e-9 } else { d <= 1e-9 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plane_frame_flip_and_projected_range_follow_the_part_pose() {
        let mut s = SectionView {
            enabled: true,
            axis: Axis::X,
            offset_mm: 2.0,
            local_frame: true,
            ..Default::default()
        };
        let pose = RigidPoseMm {
            rotation_xyzw: DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2).to_array(),
            translation_mm: [10.0, 20.0, 0.0],
        };
        let p = s.plane(Some(&pose));
        assert!((DVec3::from_array(p.normal) - DVec3::Y).length() < 1e-12);
        assert!((DVec3::from_array(p.origin_mm) - DVec3::new(10.0, 22.0, 0.0)).length() < 1e-12);
        assert!(s.keep(p, DVec3::new(10.0, 21.0, 0.0)));
        s.flipped = true;
        assert!(!s.keep(p, DVec3::new(10.0, 21.0, 0.0)));
        let r = s.range(
            AabbMm {
                min: [8.0, 20.0, 0.0],
                max: [10.0, 25.0, 2.0],
            },
            Some(&pose),
        );
        assert!(r[0].abs() < 1e-12 && (r[1] - 5.0).abs() < 1e-12);
        s.axis = Axis::Free;
        s.angles_deg = [35.0, -20.0];
        assert!((s.normal().length() - 1.0).abs() < 1e-12);
    }
}
