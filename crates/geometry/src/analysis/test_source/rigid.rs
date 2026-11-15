// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A test source record moved by a rigid motion: every point, axis and control point moves;
//! every parameter, pcurve, tolerance and orientation stays.
//!
//! Origin: SolidCraft's `crates/geom/src/brep/placement.rs` (MIT OR Apache-2.0), ported onto
//! `solidcore_record::brep`.

#![allow(clippy::many_single_char_names)]

use solidcore_record::brep::{Brep, Curve, Frame, Surface};

/// A rotation (rows of an orthonormal matrix with determinant +1) and a translation:
/// `p ↦ rotation · p + translation`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rigid {
    pub(crate) rotation: [[f64; 3]; 3],
    pub(crate) translation: [f64; 3],
}

impl Rigid {
    fn direction(&self, d: [f64; 3]) -> [f64; 3] {
        self.rotation
            .map(|row| row[0] * d[0] + row[1] * d[1] + row[2] * d[2])
    }

    fn point(&self, p: [f64; 3]) -> [f64; 3] {
        let r = self.direction(p);
        [
            r[0] + self.translation[0],
            r[1] + self.translation[1],
            r[2] + self.translation[2],
        ]
    }

    /// A homogeneous control point `[x w, y w, z w, w]`: the point moves, its weight stays.
    fn homogeneous(&self, h: [f64; 4]) -> [f64; 4] {
        let r = self.direction([h[0], h[1], h[2]]);
        let w = h[3];
        [
            r[0] + w * self.translation[0],
            r[1] + w * self.translation[1],
            r[2] + w * self.translation[2],
            w,
        ]
    }

    /// The column-major affine matrix of this motion.
    fn matrix(&self) -> [[f64; 4]; 4] {
        let (r, t) = (self.rotation, self.translation);
        [
            [r[0][0], r[1][0], r[2][0], 0.0],
            [r[0][1], r[1][1], r[2][1], 0.0],
            [r[0][2], r[1][2], r[2][2], 0.0],
            [t[0], t[1], t[2], 1.0],
        ]
    }

    fn frame(&self, f: &Frame) -> Frame {
        Frame {
            origin: self.point(f.origin),
            x_axis: self.direction(f.x_axis),
            y_axis: self.direction(f.y_axis),
        }
    }

    fn curve(&self, c: &Curve) -> Curve {
        match c {
            Curve::Line { start, end } => Curve::Line {
                start: self.point(*start),
                end: self.point(*end),
            },
            Curve::Circle { frame, radius } => Curve::Circle {
                frame: self.frame(frame),
                radius: *radius,
            },
            Curve::Ellipse { frame, radii } => Curve::Ellipse {
                frame: self.frame(frame),
                radii: *radii,
            },
            Curve::Polyline { points } => Curve::Polyline {
                points: points.iter().map(|p| self.point(*p)).collect(),
            },
            Curve::Spline { knots, points } => Curve::Spline {
                knots: knots.clone(),
                points: points.iter().map(|p| self.point(*p)).collect(),
            },
            Curve::RationalSpline { knots, homogeneous } => Curve::RationalSpline {
                knots: knots.clone(),
                homogeneous: homogeneous.iter().map(|h| self.homogeneous(*h)).collect(),
            },
            Curve::SurfaceIntersection {
                surfaces,
                leader,
                tolerance,
            } => Curve::SurfaceIntersection {
                surfaces: [
                    Box::new(self.surface(&surfaces[0])),
                    Box::new(self.surface(&surfaces[1])),
                ],
                leader: Box::new(self.curve(leader)),
                tolerance: *tolerance,
            },
        }
    }

    fn surface(&self, s: &Surface) -> Surface {
        match s {
            Surface::Plane {
                origin,
                u_axis,
                v_axis,
            } => Surface::Plane {
                origin: self.point(*origin),
                u_axis: self.direction(*u_axis),
                v_axis: self.direction(*v_axis),
            },
            Surface::PlaneThrough {
                origin,
                u_point,
                v_point,
            } => Surface::PlaneThrough {
                origin: self.point(*origin),
                u_point: self.point(*u_point),
                v_point: self.point(*v_point),
            },
            Surface::Cylinder { frame, radius } => Surface::Cylinder {
                frame: self.frame(frame),
                radius: *radius,
            },
            Surface::Cone {
                frame,
                radius_at_origin,
                half_angle,
            } => Surface::Cone {
                frame: self.frame(frame),
                radius_at_origin: *radius_at_origin,
                half_angle: *half_angle,
            },
            Surface::Sphere { frame, radius } => Surface::Sphere {
                frame: self.frame(frame),
                radius: *radius,
            },
            Surface::Torus {
                frame,
                major_radius,
                minor_radius,
                tube_start,
            } => Surface::Torus {
                frame: self.frame(frame),
                major_radius: *major_radius,
                minor_radius: *minor_radius,
                tube_start: *tube_start,
            },
            Surface::BallPatch { center, radius } => Surface::BallPatch {
                center: self.point(*center),
                radius: *radius,
            },
            Surface::Spline {
                u_knots,
                v_knots,
                points,
            } => Surface::Spline {
                u_knots: u_knots.clone(),
                v_knots: v_knots.clone(),
                points: points
                    .iter()
                    .map(|row| row.iter().map(|p| self.point(*p)).collect())
                    .collect(),
            },
            Surface::RationalSpline {
                u_knots,
                v_knots,
                homogeneous,
            } => Surface::RationalSpline {
                u_knots: u_knots.clone(),
                v_knots: v_knots.clone(),
                homogeneous: homogeneous
                    .iter()
                    .map(|row| row.iter().map(|h| self.homogeneous(*h)).collect())
                    .collect(),
            },
            // The revolution's own placement is composed with the motion; profile and axis stay
            // where the producer put them.
            Surface::PlacedRevolution {
                profile,
                origin,
                axis,
                matrix,
                forward,
            } => Surface::PlacedRevolution {
                profile: profile.clone(),
                origin: *origin,
                axis: *axis,
                matrix: compose(&self.matrix(), matrix),
                forward: *forward,
            },
            Surface::Blend { .. } => panic!("a test source with a blend face is not moved"),
        }
    }
}

/// `a · b` of column-major 4×4 matrices.
fn compose(a: &[[f64; 4]; 4], b: &[[f64; 4]; 4]) -> [[f64; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for (col, out_col) in out.iter_mut().enumerate() {
        for (row, cell) in out_col.iter_mut().enumerate() {
            *cell = (0..4).map(|k| a[k][row] * b[col][k]).sum();
        }
    }
    out
}

/// `source` moved by `motion`: topology, parameters and pcurves unchanged.
pub(crate) fn moved(source: &Brep, motion: &Rigid) -> Brep {
    {
        let this = source;
        let mut out = this.clone();
        for f in &mut out.faces {
            f.surface = motion.surface(&f.surface);
            f.nurbs = f.nurbs.as_ref().map(|n| motion.surface(n));
        }
        for e in &mut out.edges {
            e.curve = motion.curve(&e.curve);
            e.nurbs = e.nurbs.as_ref().map(|n| motion.curve(n));
        }
        for v in &mut out.vertices {
            v.point = motion.point(v.point);
        }
        out
    }
}
