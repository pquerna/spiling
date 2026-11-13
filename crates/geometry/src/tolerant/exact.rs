// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Exact section geometry for analytic faces.
//!
//! The tessellation supplies topology: which faces a section crosses and in
//! what order. Where a face is analytic, its implicit equation supplies the
//! geometry: each crossing is moved onto the true surface within the section
//! plane, curved runs are densified on the true curve, and surfaces of
//! revolution about a vertical axis report their exact section circles.

use spiling_contracts::geometry::{AnalyticSurfaceDescription, RigidPoseMm};

/// Newton steps before a projection is abandoned for the mesh point.
const MAX_STEPS: usize = 12;
/// A surface this close to horizontal at a point has no well-conditioned
/// section curve there; the mesh point is kept.
const MIN_HORIZONTAL_GRADIENT: f64 = 0.05;
/// Two section curves meeting at a shallower angle than this are treated as
/// tangent, and a shared crossing is projected onto one surface only.
const MIN_CORNER_SINE: f64 = 0.2;

type V3 = [f64; 3];

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: V3) -> Option<V3> {
    let n = norm(a);
    (n.is_finite() && n > 0.0).then(|| scale(a, 1.0 / n))
}

/// An analytic face as a distance-like implicit function `f(p) = 0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Implicit {
    Plane {
        origin: V3,
        normal: V3,
    },
    Cylinder {
        origin: V3,
        axis: V3,
        radius: f64,
    },
    /// Radius at axial distance `t` is `radius + t * slope`.
    Cone {
        origin: V3,
        axis: V3,
        radius: f64,
        slope: f64,
    },
    Sphere {
        center: V3,
        radius: f64,
    },
    Torus {
        center: V3,
        axis: V3,
        major: f64,
        minor: f64,
    },
}

/// One exact circle of a section, in the section's XY.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SectionCircle {
    pub center: [f64; 2],
    pub radius: f64,
    /// Source face ordinal the circle lies on.
    pub face: u32,
}

impl Implicit {
    pub(super) fn from_analytic(surface: &AnalyticSurfaceDescription) -> Option<Self> {
        let finite = |v: f64| v.is_finite();
        let result = match surface {
            AnalyticSurfaceDescription::Plane { origin_mm, normal } => Self::Plane {
                origin: *origin_mm,
                normal: unit(*normal)?,
            },
            AnalyticSurfaceDescription::Cylinder {
                axis_origin_mm,
                axis_direction,
                radius_mm,
            } => Self::Cylinder {
                origin: *axis_origin_mm,
                axis: unit(*axis_direction)?,
                radius: *radius_mm,
            },
            AnalyticSurfaceDescription::Cone {
                frame,
                reference_radius_mm,
                semi_angle_radians,
            } => Self::Cone {
                origin: frame.origin_mm,
                axis: unit(frame.z_axis)?,
                radius: *reference_radius_mm,
                slope: semi_angle_radians.tan(),
            },
            AnalyticSurfaceDescription::Sphere { frame, radius_mm } => Self::Sphere {
                center: frame.origin_mm,
                radius: *radius_mm,
            },
            AnalyticSurfaceDescription::Torus {
                frame,
                major_radius_mm,
                minor_radius_mm,
                ..
            } => Self::Torus {
                center: frame.origin_mm,
                axis: unit(frame.z_axis)?,
                major: *major_radius_mm,
                minor: *minor_radius_mm,
            },
        };
        let values: &[f64] = match &result {
            Self::Plane { origin, normal } => &[origin[0], origin[1], origin[2], normal[0]],
            Self::Cylinder { origin, radius, .. } => &[origin[0], origin[1], origin[2], *radius],
            Self::Cone {
                origin,
                radius,
                slope,
                ..
            } => &[origin[0], origin[1], origin[2], *radius, *slope],
            Self::Sphere { center, radius } => &[center[0], center[1], center[2], *radius],
            Self::Torus {
                center,
                major,
                minor,
                ..
            } => &[center[0], center[1], center[2], *major, *minor],
        };
        values.iter().copied().all(finite).then_some(result)
    }

    /// The same surface under a rigid placement.
    pub(super) fn placed(&self, pose: &RigidPoseMm) -> Self {
        let rotate = |v: V3| super::rotate(pose, v);
        let point = |p: V3| {
            let r = rotate(p);
            [
                r[0] + pose.translation_mm[0],
                r[1] + pose.translation_mm[1],
                r[2] + pose.translation_mm[2],
            ]
        };
        match *self {
            Self::Plane { origin, normal } => Self::Plane {
                origin: point(origin),
                normal: rotate(normal),
            },
            Self::Cylinder {
                origin,
                axis,
                radius,
            } => Self::Cylinder {
                origin: point(origin),
                axis: rotate(axis),
                radius,
            },
            Self::Cone {
                origin,
                axis,
                radius,
                slope,
            } => Self::Cone {
                origin: point(origin),
                axis: rotate(axis),
                radius,
                slope,
            },
            Self::Sphere { center, radius } => Self::Sphere {
                center: point(center),
                radius,
            },
            Self::Torus {
                center,
                axis,
                major,
                minor,
            } => Self::Torus {
                center: point(center),
                axis: rotate(axis),
                major,
                minor,
            },
        }
    }

    /// Whether every horizontal section is a set of concentric circles.
    pub(super) fn has_vertical_axis(&self) -> bool {
        match self {
            Self::Plane { .. } => false,
            Self::Sphere { .. } => true,
            Self::Cylinder { axis, .. } | Self::Cone { axis, .. } | Self::Torus { axis, .. } => {
                axis[2].abs() >= 1.0 - 1e-12
            }
        }
    }

    pub(super) fn is_plane(&self) -> bool {
        matches!(self, Self::Plane { .. })
    }

    /// Value and gradient at `p`, or `None` on the axis or center where the
    /// function is not differentiable.
    fn value_gradient(&self, p: V3) -> Option<(f64, V3)> {
        let radial = |origin: V3, axis: V3| {
            let d = sub(p, origin);
            let t = dot(d, axis);
            let q = sub(d, scale(axis, t));
            let rho = norm(q);
            (rho > 0.0).then(|| (t, scale(q, 1.0 / rho), rho))
        };
        match *self {
            Self::Plane { origin, normal } => Some((dot(sub(p, origin), normal), normal)),
            Self::Cylinder {
                origin,
                axis,
                radius,
            } => {
                let (_, e, rho) = radial(origin, axis)?;
                Some((rho - radius, e))
            }
            Self::Cone {
                origin,
                axis,
                radius,
                slope,
            } => {
                let (t, e, rho) = radial(origin, axis)?;
                Some((rho - (radius + t * slope), sub(e, scale(axis, slope))))
            }
            Self::Sphere { center, radius } => {
                let d = sub(p, center);
                let r = norm(d);
                (r > 0.0).then(|| (r - radius, scale(d, 1.0 / r)))
            }
            Self::Torus {
                center,
                axis,
                major,
                minor,
            } => {
                let (t, e, rho) = radial(center, axis)?;
                let s = (rho - major).hypot(t);
                (s > 0.0).then(|| {
                    let g = [
                        ((rho - major) * e[0] + t * axis[0]) / s,
                        ((rho - major) * e[1] + t * axis[1]) / s,
                        ((rho - major) * e[2] + t * axis[2]) / s,
                    ];
                    (s - minor, g)
                })
            }
        }
    }

    /// Move `p` within the horizontal plane through it onto the surface.
    /// `None` if the section curve is ill-conditioned there or the move would
    /// exceed `max_move`.
    pub(super) fn project(&self, p: [f64; 2], z: f64, max_move: f64) -> Option<[f64; 2]> {
        let mut q = p;
        for _ in 0..MAX_STEPS {
            let (f, g) = self.value_gradient([q[0], q[1], z])?;
            let horizontal = g[0] * g[0] + g[1] * g[1];
            if horizontal < MIN_HORIZONTAL_GRADIENT * MIN_HORIZONTAL_GRADIENT * dot(g, g) {
                return None;
            }
            let step = [f * g[0] / horizontal, f * g[1] / horizontal];
            q = [q[0] - step[0], q[1] - step[1]];
            if step[0].hypot(step[1]) <= 1e-12 * (1.0 + q[0].abs().max(q[1].abs())) {
                break;
            }
        }
        let (f, _) = self.value_gradient([q[0], q[1], z])?;
        ((q[0] - p[0]).hypot(q[1] - p[1]) <= max_move && f.abs() <= 1e-9).then_some(q)
    }

    /// Move `p` onto the section curves of both surfaces at once: the
    /// section corner where two faces meet. `None` if they meet too
    /// tangentially to define a corner, or the solve does not converge.
    pub(super) fn project_pair(
        &self,
        other: &Self,
        p: [f64; 2],
        z: f64,
        max_move: f64,
    ) -> Option<[f64; 2]> {
        let mut q = p;
        for _ in 0..MAX_STEPS {
            let (f1, g1) = self.value_gradient([q[0], q[1], z])?;
            let (f2, g2) = other.value_gradient([q[0], q[1], z])?;
            let det = g1[0] * g2[1] - g1[1] * g2[0];
            let n1 = g1[0].hypot(g1[1]);
            let n2 = g2[0].hypot(g2[1]);
            if det.abs() < MIN_CORNER_SINE * n1 * n2 {
                return None;
            }
            let step = [
                (f1 * g2[1] - f2 * g1[1]) / det,
                (g1[0] * f2 - g2[0] * f1) / det,
            ];
            q = [q[0] - step[0], q[1] - step[1]];
            if step[0].hypot(step[1]) <= 1e-12 * (1.0 + q[0].abs().max(q[1].abs())) {
                break;
            }
        }
        let (f1, _) = self.value_gradient([q[0], q[1], z])?;
        let (f2, _) = other.value_gradient([q[0], q[1], z])?;
        ((q[0] - p[0]).hypot(q[1] - p[1]) <= max_move && f1.abs().max(f2.abs()) <= 1e-9)
            .then_some(q)
    }

    /// Exact circles of this surface at height `z`, when its axis is vertical.
    pub(super) fn circles(&self, z: f64, face: u32, out: &mut Vec<SectionCircle>) {
        let vertical = |axis: V3| axis[2].abs() >= 1.0 - 1e-12;
        let at = |origin: V3, axis: V3| -> (f64, [f64; 2]) {
            let t = (z - origin[2]) / axis[2];
            (t, [origin[0] + t * axis[0], origin[1] + t * axis[1]])
        };
        let mut push = |center: [f64; 2], radius: f64| {
            if radius.is_finite() && radius > 0.0 {
                out.push(SectionCircle {
                    center,
                    radius,
                    face,
                });
            }
        };
        match *self {
            Self::Plane { .. } => {}
            Self::Cylinder {
                origin,
                axis,
                radius,
            } if vertical(axis) => push(at(origin, axis).1, radius),
            Self::Cone {
                origin,
                axis,
                radius,
                slope,
            } if vertical(axis) => {
                let (t, center) = at(origin, axis);
                push(center, radius + t * slope);
            }
            Self::Sphere { center, radius } => {
                let t = z - center[2];
                push([center[0], center[1]], (radius * radius - t * t).sqrt());
            }
            Self::Torus {
                center,
                axis,
                major,
                minor,
            } if vertical(axis) => {
                let (t, c) = at(center, [0.0, 0.0, axis[2].signum()]);
                let half = (minor * minor - t * t).sqrt();
                push(c, major + half);
                // A horn or spindle torus's outer sheet has no inner circle
                // past the axis; admitted faces never use the inner sheet.
                push(c, major - half);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projections_land_on_the_true_section_curves() {
        let cylinder = Implicit::Cylinder {
            origin: [1.0, 2.0, 0.0],
            axis: [0.0, 0.0, 1.0],
            radius: 3.0,
        };
        // A chord midpoint lies inside the circle; projection is radial.
        let q = cylinder.project([1.0 + 2.9, 2.0], 5.0, 0.2).unwrap();
        assert!((q[0] - 4.0).abs() < 1e-12 && (q[1] - 2.0).abs() < 1e-12);
        // A horizontal-axis cylinder is sectioned in two lines.
        let lying = Implicit::Cylinder {
            origin: [0.0, 0.0, 0.0],
            axis: [1.0, 0.0, 0.0],
            radius: 2.0,
        };
        let q = lying.project([5.0, 1.0], 1.0, 1.0).unwrap();
        assert!((q[1] - 3.0_f64.sqrt()).abs() < 1e-12 && q[0] == 5.0);
        // A tilted cone, a sphere and a torus converge from a nearby point.
        let cone = Implicit::Cone {
            origin: [0.0, 0.0, 0.0],
            axis: unit([0.3, 0.0, 1.0]).unwrap(),
            radius: 4.0,
            slope: 0.2,
        };
        let q = cone.project([4.3, 0.1], 1.0, 1.0).unwrap();
        assert!(cone.value_gradient([q[0], q[1], 1.0]).unwrap().0.abs() < 1e-12);
        let sphere = Implicit::Sphere {
            center: [0.0, 0.0, 0.0],
            radius: 5.0,
        };
        let q = sphere.project([3.9, 0.0], 3.0, 0.5).unwrap();
        assert!((q[0] - 4.0).abs() < 1e-12);
        // Near the pole the section is ill-conditioned; the mesh point stays.
        assert!(sphere.project([0.1, 0.0], 4.999, 0.5).is_none());
        let torus = Implicit::Torus {
            center: [0.0, 0.0, 0.0],
            axis: [0.0, 0.0, 1.0],
            major: 10.0,
            minor: 2.0,
        };
        let q = torus.project([11.9, 0.0], 0.0, 0.5).unwrap();
        assert!((q[0] - 12.0).abs() < 1e-12);
        let mut circles = Vec::new();
        torus.circles(1.0, 7, &mut circles);
        let half = 3.0_f64.sqrt();
        assert_eq!(circles.len(), 2);
        assert!((circles[0].radius - (10.0 + half)).abs() < 1e-12);
        assert!((circles[1].radius - (10.0 - half)).abs() < 1e-12);
        // A spindle torus's outer sheet: one circle where its inner one
        // would cross the axis, two once it no longer does.
        let spindle = Implicit::Torus {
            center: [0.0, 0.0, 0.0],
            axis: [0.0, 0.0, 1.0],
            major: 1.0,
            minor: 2.0,
        };
        let q = spindle.project([3.1, 0.0], 0.0, 0.5).unwrap();
        assert!((q[0] - 3.0).abs() < 1e-12, "{q:?}");
        circles.clear();
        spindle.circles(1.0, 7, &mut circles);
        assert_eq!(circles.len(), 1);
        assert!((circles[0].radius - (1.0 + 3.0_f64.sqrt())).abs() < 1e-12);
        circles.clear();
        spindle.circles(1.9, 7, &mut circles);
        let half = (4.0_f64 - 1.9 * 1.9).sqrt();
        assert_eq!(circles.len(), 2);
        assert!((circles[1].radius - (1.0 - half)).abs() < 1e-12);
        // A horn torus closes at its apex: one circle above and below, none at it.
        let horn = Implicit::Torus {
            center: [0.0, 0.0, 0.0],
            axis: [0.0, 0.0, 1.0],
            major: 1.0,
            minor: 1.0,
        };
        let q = horn.project([0.45, 0.0], 0.6, 0.5).unwrap();
        assert!((q[0] - 0.2).abs() < 1e-12, "{q:?}");
        circles.clear();
        horn.circles(0.0, 7, &mut circles);
        assert_eq!(circles.len(), 1);
        assert!((circles[0].radius - 2.0).abs() < 1e-12);
        // A far move is refused rather than snapping to another sheet.
        assert!(cylinder.project([1.0 + 2.0, 2.0], 5.0, 0.2).is_none());
        // A crossing on an unconverged mesh comes back with a matching reach.
        assert!(cylinder.project([1.0 + 2.9825, 2.0], 5.0, 0.01).is_none());
        let q = cylinder.project([1.0 + 2.9825, 2.0], 5.0, 0.035).unwrap();
        assert!((q[0] - 4.0).abs() < 1e-12);
    }

    #[test]
    fn a_corner_lands_on_both_faces() {
        let wall = Implicit::Plane {
            origin: [0.0, 0.0, 0.0],
            normal: [1.0, 0.0, 0.0],
        };
        let hole = Implicit::Cylinder {
            origin: [3.0, 0.0, 0.0],
            axis: [0.0, 0.0, 1.0],
            radius: 5.0,
        };
        let q = wall.project_pair(&hole, [0.01, 3.99], 2.0, 0.1).unwrap();
        assert!(q[0].abs() < 1e-12 && (q[1] - 4.0).abs() < 1e-12);
        // Tangent faces define no corner.
        let tangent = Implicit::Plane {
            origin: [-2.0, 0.0, 0.0],
            normal: [1.0, 0.0, 0.0],
        };
        assert!(
            tangent
                .project_pair(&hole, [-2.0, 0.01], 2.0, 0.1)
                .is_none()
        );
    }
}
