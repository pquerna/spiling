// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::CameraState;
use crate::scene::FocusTarget;
use glam::{DQuat, DVec3};

pub(super) const DURATION_SECONDS: f64 = 0.18;

pub(super) struct Request {
    pub summary: spiling_contracts::geometry::SceneSummary,
    pub target: FocusTarget,
    pub participants: Option<super::reveal::ParticipantMask>,
    pub reveal: bool,
    pub started_reveal: bool,
    pub area_applied: bool,
}

pub(super) fn plan(
    current: &CameraState,
    target: FocusTarget,
    origin: DVec3,
    aspect: f64,
) -> Option<CameraState> {
    if !current.valid()
        || !target.valid()
        || !origin.is_finite()
        || !aspect.is_finite()
        || aspect <= 0.0
    {
        return None;
    }
    let current_direction = DQuat::from_array(current.rotation) * DVec3::Z;
    let mut view_direction = current_direction;
    if let Some(normal) = target.normal {
        let alignment = current_direction.dot(normal).clamp(-1.0, 1.0);
        let angle = 25.0_f64.to_radians();
        if alignment < angle.cos() {
            let tangent = (current_direction - normal * alignment)
                .try_normalize()
                .unwrap_or_else(|| normal.any_orthonormal_vector());
            view_direction = normal * angle.cos() + tangent * angle.sin();
        }
    }
    let radius = (DVec3::from_array(target.bounds_mm.max)
        - DVec3::from_array(target.bounds_mm.min))
    .length()
    .max(0.02)
        * 0.5;
    let half_angle = 22.5_f64.to_radians();
    let limiting_angle = half_angle.min((half_angle.tan() * aspect).atan());
    let distance = radius * 1.2
        / if current.orthographic {
            limiting_angle.tan()
        } else {
            limiting_angle.sin()
        };
    let camera = CameraState {
        rotation: if target.normal.is_none() {
            current.rotation
        } else {
            super::navigation::orientation(view_direction).to_array()
        },
        distance,
        pan: (target.center() - origin).to_array(),
        orthographic: current.orthographic,
    };
    camera.valid().then_some(camera)
}

pub(super) struct Motion {
    from: CameraState,
    to: CameraState,
    start: f64,
}
impl Motion {
    pub(super) fn new(from: CameraState, to: CameraState, start: f64) -> Self {
        Self { from, to, start }
    }
    pub(super) fn sample(&self, now: f64) -> (CameraState, bool) {
        let progress = ((now - self.start) / DURATION_SECONDS).clamp(0.0, 1.0);
        let eased = progress.powi(3) * (progress * (progress * 6.0 - 15.0) + 10.0);
        let blend = |from: f64, to: f64| from * (1.0 - eased) + to * eased;
        (
            CameraState {
                rotation: DQuat::from_array(self.from.rotation)
                    .slerp(DQuat::from_array(self.to.rotation), eased)
                    .normalize()
                    .to_array(),
                distance: blend(self.from.distance.ln(), self.to.distance.ln()).exp(),
                pan: std::array::from_fn(|axis| blend(self.from.pan[axis], self.to.pan[axis])),
                orthographic: self.to.orthographic,
            },
            progress >= 1.0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn camera() -> CameraState {
        crate::viewport::Viewport::default().camera()
    }
    fn target(normal: Option<DVec3>) -> FocusTarget {
        FocusTarget {
            bounds_mm: spiling_contracts::geometry::AabbMm {
                min: [1.0, 2.0, 3.0],
                max: [11.0, 6.0, 3.0],
            },
            normal,
        }
    }
    #[test]
    fn turns_toward_face_but_keeps_a_readable_oblique_angle() {
        let current = camera();
        let focused = plan(&current, target(Some(-DVec3::X)), DVec3::ZERO, 1.5).unwrap();
        assert!(
            ((DQuat::from_array(focused.rotation) * DVec3::Z).dot(-DVec3::X)
                - 25_f64.to_radians().cos())
            .abs()
                < 1e-12
        );
        assert_eq!(focused.pan, [6.0, 4.0, 3.0]);
        assert!(focused.valid());
    }
    #[test]
    fn no_stable_normal_preserves_the_existing_orientation() {
        let current = camera();
        let focused = plan(&current, target(None), DVec3::new(6.0, 0.0, 0.0), 1.0).unwrap();
        assert_eq!(focused.rotation, current.rotation);
        assert_eq!(focused.pan, [0.0, 4.0, 3.0]);
    }
    #[test]
    fn portrait_and_orthographic_views_frame_the_complete_bounding_sphere() {
        for orthographic in [false, true] {
            let current = CameraState {
                orthographic,
                ..camera()
            };
            for aspect in [0.2, 1.0, 3.0] {
                let focused = plan(&current, target(None), DVec3::ZERO, aspect).unwrap();
                let viewport = crate::viewport::Viewport {
                    rotation: DQuat::from_array(focused.rotation),
                    distance: focused.distance,
                    pan: DVec3::from_array(focused.pan),
                    orthographic,
                    ..Default::default()
                };
                let matrix = viewport.matrix(aspect, 20.0);
                for corner in 0..8 {
                    let bounds = target(None).bounds_mm;
                    let point = DVec3::from_array(std::array::from_fn(|axis| {
                        if corner & (1 << axis) == 0 {
                            bounds.min[axis]
                        } else {
                            bounds.max[axis]
                        }
                    }));
                    let projected = matrix.project_point3(point);
                    assert!(projected.x.abs() < 1.0 && projected.y.abs() < 1.0);
                }
            }
        }
    }
    #[test]
    fn animation_is_bounded_logarithmic_and_takes_the_short_yaw_arc() {
        let from = CameraState {
            rotation: super::super::navigation::orientation(DVec3::new(
                179_f64.to_radians().cos(),
                179_f64.to_radians().sin(),
                0.0,
            ))
            .to_array(),
            distance: 10.0,
            ..camera()
        };
        let to = CameraState {
            rotation: super::super::navigation::orientation(DVec3::new(
                (-179_f64).to_radians().cos(),
                (-179_f64).to_radians().sin(),
                0.0,
            ))
            .to_array(),
            distance: 1000.0,
            ..from.clone()
        };
        let motion = Motion::new(from.clone(), to, 3.0);
        assert_eq!(motion.sample(2.0).0.pan, from.pan);
        let (half, done) = motion.sample(3.0 + DURATION_SECONDS * 0.5);
        assert!(!done);
        assert!(((DQuat::from_array(half.rotation) * DVec3::Z) + DVec3::X).length() < 1e-12);
        assert!((half.distance - 100.0).abs() < 1e-10);
        assert!(motion.sample(4.0).1);
        for step in 0..100 {
            assert!(motion.sample(3.0 + f64::from(step) * 0.01).0.valid());
        }
    }
    #[test]
    fn reduced_motion_and_escape_stop_share_the_navigation_controller() {
        let mut viewport = crate::viewport::Viewport::default();
        let initial = viewport.camera();
        let focused = plan(&initial, target(Some(-DVec3::X)), DVec3::ZERO, 1.0).unwrap();
        viewport.animate_to(focused, 1.0, true);
        assert!(viewport.stop_focus());
        assert!(!viewport.stop_focus());
        assert!(!viewport.advance_focus(2.0));
        assert_eq!(viewport.camera().pan, initial.pan);
        let focused = plan(&initial, target(None), DVec3::ZERO, 1.0).unwrap();
        viewport.animate_to(focused, 3.0, false);
        assert!(viewport.motion.is_none());
        viewport.animate_to(initial.clone(), 4.0, false);
        assert!(viewport.motion.is_none());
        assert_eq!(viewport.camera().pan, initial.pan);
        assert_eq!(viewport.camera().distance, initial.distance);
    }
    #[test]
    fn refuses_invalid_targets_and_handles_poles_and_points() {
        let current = camera();
        assert!(plan(&current, target(Some(DVec3::ZERO)), DVec3::ZERO, 1.0).is_none());
        assert!(plan(&current, target(None), DVec3::ZERO, 0.0).is_none());
        for normal in [DVec3::Z, -DVec3::Z] {
            assert!(
                plan(&current, target(Some(normal)), DVec3::ZERO, 1.0)
                    .unwrap()
                    .valid()
            );
        }
        let mut point = target(None);
        point.bounds_mm.max = point.bounds_mm.min;
        assert!(plan(&current, point, DVec3::ZERO, 1.0).unwrap().valid());
    }
}
