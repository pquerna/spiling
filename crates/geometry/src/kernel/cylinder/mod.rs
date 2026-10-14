// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{BoundaryCurveMm, Definition};
use crate::{GeometryError, VerticalCylinderFace, check_cancel, section::projected_extent};
use monstertruck_modeling::{Curve, Surface, base::*};
use spiling_contracts::geometry::{FaceCarrier, FaceId, GeometryErrorCode};
use std::{collections::HashMap, f64::consts::TAU, sync::atomic::AtomicBool};

const RESIDUAL_MM: f64 = 1e-7;
const ANGULAR: f64 = 1e-10;

fn unsupported(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::UnsupportedGeometry, message)
}
fn malformed(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::KernelFailure, message)
}
fn vector(p: [f64; 3]) -> Vector3 {
    Vector3::new(p[0], p[1], p[2])
}
fn finite(p: Vector3) -> bool {
    p.x.is_finite() && p.y.is_finite() && p.z.is_finite()
}
fn unit(p: Vector3) -> Result<Vector3, GeometryError> {
    let scale = p.x.abs().max(p.y.abs()).max(p.z.abs());
    if !finite(p) || scale == 0.0 {
        return Err(malformed("nonfinite or zero cylinder direction"));
    }
    Ok((p / scale).normalize())
}
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2])
}

fn check_rim_join(a: [f64; 3], b: [f64; 3], center: [f64; 2]) -> Result<[f64; 2], GeometryError> {
    let u = [a[0] - center[0], a[1] - center[1]];
    let v = [b[0] - center[0], b[1] - center[1]];
    let ru = u[0].hypot(u[1]);
    let rv = v[0].hypot(v[1]);
    if !ru.is_finite() || !rv.is_finite() || ru <= 0.0 || rv <= 0.0 {
        return Err(malformed("nonfinite cylinder rim join arithmetic"));
    }
    let u = u.map(|x| x / ru);
    let v = v.map(|x| x / rv);
    let angle = (u[0] * v[1] - u[1] * v[0]).atan2(u[0] * v[0] + u[1] * v[1]);
    if !angle.is_finite() || angle.abs() > ANGULAR || distance(a, b) > RESIDUAL_MM {
        return Err(unsupported(
            "cylinder rim has an analytic angular gap or overlap",
        ));
    }
    Ok([angle.abs(), distance(a, b)])
}

impl Definition {
    /// Certify one exact admitted source face, never a union of co-carrier trims.
    pub fn vertical_cylinder_face(
        &self,
        face_id: &FaceId,
    ) -> Result<VerticalCylinderFace, GeometryError> {
        self.vertical_cylinder_face_with_cancel(face_id, &AtomicBool::new(false))
    }

    /// Immutable source-local full-cylinder proof. No native boolean, curve fit,
    /// section sampling or display geometry participates in this certificate.
    pub fn vertical_cylinder_face_with_cancel(
        &self,
        face_id: &FaceId,
        cancel: &AtomicBool,
    ) -> Result<VerticalCylinderFace, GeometryError> {
        check_cancel(cancel)?;
        let mut selected = None;
        for (index, info) in self.faces.iter().enumerate() {
            check_cancel(cancel)?;
            if &info.face_id == face_id {
                selected = Some((index, info));
                break;
            }
        }
        let (index, info) = selected.ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::UnknownHandle,
                "source face is not in native definition",
            )
        })?;
        let FaceCarrier::Cylinder {
            axis_origin_mm,
            axis_direction,
            radius_mm,
        } = &info.carrier
        else {
            return Err(unsupported(
                "cylindrical finish requires a native cylindrical source face",
            ));
        };
        let source_axis = unit(vector(*axis_direction))?;
        if !axis_origin_mm.iter().all(|v| v.is_finite())
            || !radius_mm.is_finite()
            || *radius_mm <= RESIDUAL_MM
        {
            return Err(malformed("invalid admitted cylinder carrier"));
        }
        if source_axis.x.abs() > ANGULAR || source_axis.y.abs() > ANGULAR {
            return Err(unsupported(
                "cylindrical finish requires a source-local vertical axis",
            ));
        }
        if !info.orientation {
            return Err(unsupported(
                "cylindrical finish requires an outward exterior face, not a bore",
            ));
        }
        let shells = self.solid.boundaries();
        if shells.len() != 1
            || shells[0].len() != self.faces.len()
            || self.face_boundaries.len() != self.faces.len()
        {
            return Err(malformed(
                "native cylinder source face-use mapping is inconsistent",
            ));
        }
        let face = &shells[0][index];
        let refs = &self.face_boundaries[index];
        if refs.face != face.id() {
            return Err(malformed(
                "native cylinder face identity changed after admission",
            ));
        }
        // Prove the actual transformed revolution is a circular cylinder. The
        // radial basis and axial profile are checked analytically, not fitted.
        face.with_surface(|surface| -> Result<(), GeometryError> {
            let Surface::RevolutionSurface(processor) = surface else {
                return Err(malformed(
                    "source cylinder mapped to a different native carrier",
                ));
            };
            let revolution = processor.entity();
            let Curve::Line(profile) = revolution.entity_curve() else {
                return Err(malformed("native cylinder profile is not an exact line"));
            };
            let matrix = processor.transform();
            let origin = matrix.transform_point(revolution.origin()).to_vec();
            let axis = unit(matrix.transform_vector(revolution.axis()))?;
            let p = matrix.transform_point(profile.0).to_vec();
            let q = matrix.transform_point(profile.1).to_vec();
            let radial = p - origin - (p - origin).dot(axis) * axis;
            let radial_q = q - origin - (q - origin).dot(axis) * axis;
            let local_delta = profile.0 - revolution.origin();
            let local_radial = local_delta - local_delta.dot(revolution.axis()) * revolution.axis();
            let transverse = matrix.transform_vector(revolution.axis().cross(local_radial));
            let radius = radial.magnitude();
            if !finite(origin)
                || !finite(p)
                || !finite(q)
                || !radius.is_finite()
                || radius <= RESIDUAL_MM
                || (radial_q - radial).magnitude() > RESIDUAL_MM
                || (q - p).dot(axis).abs() <= RESIDUAL_MM
                || (transverse.magnitude() - radius).abs() > RESIDUAL_MM
                || unit(transverse)?.dot(unit(radial)?).abs() > ANGULAR
                || unit(transverse)?.dot(axis).abs() > ANGULAR
                || axis.cross(source_axis).magnitude() > ANGULAR
                || ((origin - vector(*axis_origin_mm)).cross(source_axis)).magnitude() > RESIDUAL_MM
                || (radius - *radius_mm).abs() > RESIDUAL_MM
            {
                return Err(malformed(
                    "native circular cylinder coefficients disagree with source carrier",
                ));
            }
            let point = surface.subs(0.37, 0.73).to_vec();
            let normal =
                unit(surface.normal(0.37, 0.73))? * if face.orientation() { 1.0 } else { -1.0 };
            let outward = unit(point - origin - (point - origin).dot(axis) * axis)?;
            if !finite(point) || normal.dot(outward) < 1.0 - ANGULAR {
                return Err(malformed(
                    "native cylinder orientation disagrees with outward source face",
                ));
            }
            Ok(())
        })?;
        let center = [axis_origin_mm[0], axis_origin_mm[1]];
        let z = projected_extent(self, [0.0, 0.0, 1.0], cancel)?;
        if !z.iter().all(|v| v.is_finite()) || z[1] - z[0] <= 2.0 * RESIDUAL_MM {
            return Err(unsupported("cylinder has no finite full source height"));
        }
        let wires = face.absolute_boundaries();
        if wires.len() != refs.wires.len() {
            return Err(malformed("native cylinder positional wire mapping changed"));
        }
        // These maps retain only identity/incidence and bounded endpoint data,
        // not curves or a second BREP. Their combined size is bounded above by
        // the admitted coedge reference cap; arbitrary subdivisions and chart
        // wire/seam placement do not change the physical two-cycle proof.
        let mut turns = [0.0f64; 2];
        let mut turn_roundoff = [0.0f64; 2];
        let mut rims = [HashMap::new(), HashMap::new()];
        let mut seams = HashMap::new();
        for (wire, refs) in wires.iter().zip(&refs.wires) {
            check_cancel(cancel)?;
            if wire.len() != refs.len() {
                return Err(malformed(
                    "native cylinder positional coedge mapping changed",
                ));
            }
            if wire.is_empty() || !wire.is_closed() {
                return Err(unsupported("cylinder trim wire is empty or open"));
            }
            for (i, (edge, reference)) in wire.iter().zip(refs).enumerate() {
                check_cancel(cancel)?;
                if edge.id() != reference.edge || edge.orientation() != reference.orientation {
                    return Err(malformed(
                        "native cylinder coedge identity or orientation changed",
                    ));
                }
                if edge.back().id() != wire[(i + 1) % wire.len()].front().id() {
                    return Err(malformed(
                        "native cylinder coedge incidence is inconsistent",
                    ));
                }
                let witness = self.boundary_curves.get(reference.witness).ok_or_else(|| {
                    malformed("native cylinder analytic coedge reference is missing")
                })?;
                let (mut a, mut b) = witness.endpoints();
                if !reference.orientation {
                    std::mem::swap(&mut a, &mut b);
                }
                let front: [f64; 3] = edge.front().point().into();
                let back: [f64; 3] = edge.back().point().into();
                if !a
                    .iter()
                    .chain(&b)
                    .chain(&front)
                    .chain(&back)
                    .all(|v| v.is_finite())
                    || distance(a, front) > RESIDUAL_MM
                    || distance(b, back) > RESIDUAL_MM
                {
                    return Err(malformed(
                        "native cylinder endpoints disagree with analytic coedge",
                    ));
                }
                match witness {
                    BoundaryCurveMm::CircleArc {
                        center_mm,
                        u_mm,
                        v_mm,
                        start_angle,
                        end_angle,
                    } => {
                        let u = vector(*u_mm);
                        let v = vector(*v_mm);
                        let r = u.magnitude();
                        if !center_mm.iter().all(|v| v.is_finite())
                            || !finite(u)
                            || !finite(v)
                            || (center_mm[0] - center[0]).hypot(center_mm[1] - center[1])
                                > RESIDUAL_MM
                            || (r - *radius_mm).abs() > RESIDUAL_MM
                            || (v.magnitude() - *radius_mm).abs() > RESIDUAL_MM
                            || u.z.abs() > RESIDUAL_MM
                            || v.z.abs() > RESIDUAL_MM
                            || unit(u)?.dot(unit(v)?).abs() > ANGULAR
                            || unit(u)?.z.abs() > ANGULAR
                            || unit(v)?.z.abs() > ANGULAR
                        {
                            return Err(unsupported(
                                "cylinder rim is not an exact coaxial horizontal circle",
                            ));
                        }
                        let rim = if (center_mm[2] - z[0]).abs() <= RESIDUAL_MM {
                            0
                        } else if (center_mm[2] - z[1]).abs() <= RESIDUAL_MM {
                            1
                        } else {
                            return Err(unsupported(
                                "cylinder rim does not span analytic whole-source height",
                            ));
                        };
                        let direction = if reference.orientation { 1.0 } else { -1.0 }
                            * if face.orientation() { 1.0 } else { -1.0 };
                        let delta = (end_angle - start_angle)
                            * unit(u)?.cross(unit(v)?).z.signum()
                            * direction;
                        let directed = if rim == 0 { delta } else { -delta };
                        if !directed.is_finite() || directed <= 0.0 || directed > TAU + ANGULAR {
                            return Err(unsupported(
                                "cylinder rim contains degenerate or backtracking angular trims",
                            ));
                        }
                        let increment = directed - turn_roundoff[rim];
                        let sum = turns[rim] + increment;
                        turn_roundoff[rim] = (sum - turns[rim]) - increment;
                        turns[rim] = sum;
                        if !turns[rim].is_finite() || turns[rim] > TAU + ANGULAR {
                            return Err(unsupported(
                                "cylinder rim overlaps or covers more than one turn",
                            ));
                        }
                        let (front_id, back_id, a, b) = if face.orientation() {
                            (edge.front().id(), edge.back().id(), a, b)
                        } else {
                            (edge.back().id(), edge.front().id(), b, a)
                        };
                        if rims[rim].insert(front_id, (back_id, a, b)).is_some() {
                            return Err(unsupported(
                                "cylinder rim has branching or repeated native incidence",
                            ));
                        }
                    }
                    BoundaryCurveMm::Line { .. } => {
                        let axial = (a[0] - b[0]).hypot(a[1] - b[1]);
                        let axial_direction = unit(vector(b) - vector(a))?;
                        let radius = (a[0] - center[0]).hypot(a[1] - center[1]);
                        let full_height = ((a[2] - z[0]).abs() <= RESIDUAL_MM
                            && (b[2] - z[1]).abs() <= RESIDUAL_MM)
                            || ((b[2] - z[0]).abs() <= RESIDUAL_MM
                                && (a[2] - z[1]).abs() <= RESIDUAL_MM);
                        if axial > RESIDUAL_MM
                            || axial_direction.x.abs() > ANGULAR
                            || axial_direction.y.abs() > ANGULAR
                            || (radius - *radius_mm).abs() > RESIDUAL_MM
                            || !full_height
                        {
                            return Err(unsupported(
                                "cylinder chart seam is not an exact full-height axial generator",
                            ));
                        }
                        let pair = seams.entry(edge.id()).or_insert((
                            edge.orientation(),
                            reference.witness,
                            0usize,
                        ));
                        if pair.2 >= 2
                            || (pair.2 == 1
                                && (pair.0 == edge.orientation() || pair.1 != reference.witness))
                        {
                            return Err(unsupported(
                                "cylinder seams are not opposite uses of the same native edge",
                            ));
                        }
                        pair.2 += 1;
                    }
                }
            }
        }
        for (_, _, count) in seams.values() {
            check_cancel(cancel)?;
            if *count != 2 {
                return Err(unsupported(
                    "cylinder has an unpaired axial trim, not a full periodic face",
                ));
            }
        }
        for (rim, cycle) in rims.iter_mut().enumerate() {
            if cycle.is_empty()
                || (turns[rim] - TAU).abs() > ANGULAR
                || (turns[rim] - TAU).abs() * radius_mm > RESIDUAL_MM
            {
                return Err(unsupported(
                    "selected source face does not own two complete one-turn cylinder rims",
                ));
            }
            let (&first, &(_, first_point, _)) = cycle.iter().next().unwrap();
            let count = cycle.len();
            let mut next = first;
            let mut previous = first_point;
            let mut join_error = [0.0; 2];
            for step in 0..count {
                check_cancel(cancel)?;
                let (back, a, b) = cycle.remove(&next).ok_or_else(|| {
                    unsupported("cylinder rim is disconnected or has repeated incidence")
                })?;
                if step != 0 {
                    let error = check_rim_join(previous, a, center)?;
                    for i in 0..2 {
                        join_error[i] += error[i];
                    }
                }
                previous = b;
                next = back;
            }
            if next != first {
                return Err(unsupported("cylinder rim is not one closed native cycle"));
            }
            let error = check_rim_join(previous, first_point, center)?;
            for i in 0..2 {
                join_error[i] += error[i];
            }
            if join_error[0] > ANGULAR
                || join_error[0] * radius_mm > RESIDUAL_MM
                || join_error[1] > RESIDUAL_MM
            {
                return Err(unsupported(
                    "cylinder rim accumulates analytic angular gaps or overlap",
                ));
            }
        }
        // A second source face on this infinite carrier is ambiguous ownership,
        // even if its STEP placement/axis sign/reference angle is different.
        for (other_index, other) in self.faces.iter().enumerate() {
            check_cancel(cancel)?;
            if other_index == index {
                continue;
            }
            if let FaceCarrier::Cylinder {
                axis_origin_mm: origin,
                axis_direction: axis,
                radius_mm: radius,
            } = &other.carrier
            {
                let axis = unit(vector(*axis))?;
                let separation = (vector(*origin) - vector(*axis_origin_mm))
                    .cross(source_axis)
                    .magnitude();
                if !separation.is_finite() || !radius.is_finite() {
                    return Err(malformed("nonfinite native cylinder carrier comparison"));
                }
                if axis.cross(source_axis).magnitude() <= ANGULAR
                    && separation <= RESIDUAL_MM
                    && (radius - radius_mm).abs() <= RESIDUAL_MM
                {
                    return Err(unsupported(
                        "selected cylinder has ambiguous additional co-carrier source trims",
                    ));
                }
            }
        }
        check_cancel(cancel)?;
        Ok(VerticalCylinderFace {
            center_mm: center,
            radius_mm: *radius_mm,
            min_z_mm: z[0],
            max_z_mm: z[1],
        })
    }
}
