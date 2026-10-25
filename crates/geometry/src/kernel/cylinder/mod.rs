// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::Definition;
use crate::{GeometryError, GeometryWorkBudget, VerticalCylinderFace, check_cancel};
use solidcore::nurbs::b_spline_surface_n_lib::p2::OrientType;
use solidcore::topology::{EdgeuseOwner, LoopuseStart};
use solidcore::{curve::curve::p2::CurveKind, surface::surface::p2::SurfaceKind};
use spiling_contracts::geometry::{
    AnalyticSurfaceDescription, FaceId, GeometryErrorCode, SurfaceFamily,
};
use std::{collections::HashMap, f64::consts::TAU, sync::atomic::AtomicBool};

const RESIDUAL_MM: f64 = 1e-7;
const ANGULAR: f64 = 1e-10;

fn unsupported(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::UnsupportedGeometry, message)
}
fn malformed(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::KernelFailure, message)
}
#[derive(Clone, Copy)]
struct Vector3 {
    x: f64,
    y: f64,
    z: f64,
}
impl Vector3 {
    fn dot(self, b: Self) -> f64 {
        self.x * b.x + self.y * b.y + self.z * b.z
    }
    fn cross(self, b: Self) -> Self {
        vector([
            self.y * b.z - self.z * b.y,
            self.z * b.x - self.x * b.z,
            self.x * b.y - self.y * b.x,
        ])
    }
    fn magnitude(self) -> f64 {
        self.x.hypot(self.y).hypot(self.z)
    }
    fn normalize(self) -> Self {
        self / self.magnitude()
    }
}
impl std::ops::Sub for Vector3 {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        vector([self.x - b.x, self.y - b.y, self.z - b.z])
    }
}
impl std::ops::Div<f64> for Vector3 {
    type Output = Self;
    fn div(self, b: f64) -> Self {
        vector([self.x / b, self.y / b, self.z / b])
    }
}
fn vector(p: [f64; 3]) -> Vector3 {
    Vector3 {
        x: p[0],
        y: p[1],
        z: p[2],
    }
}
fn native<T>(r: solidcore::error::Result<T>) -> Result<T, GeometryError> {
    r.map_err(|_| malformed("invalid native cylinder topology"))
}
fn orientation(o: OrientType) -> Result<bool, GeometryError> {
    match o {
        OrientType::Same => Ok(true),
        OrientType::Opposite => Ok(false),
        _ => Err(malformed("unoriented native cylinder use")),
    }
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
        self.vertical_cylinder_face_with_work(
            face_id,
            &mut GeometryWorkBudget::new(50_000_000),
            cancel,
        )
    }

    pub(crate) fn vertical_cylinder_face_with_work(
        &self,
        face_id: &FaceId,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<VerticalCylinderFace, GeometryError> {
        check_cancel(cancel)?;
        let mut selected = None;
        for (index, info) in self.faces.iter().enumerate() {
            work.charge(1, cancel)?;
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
        if info.geometry.family != SurfaceFamily::Cylinder {
            return Err(unsupported(
                "cylindrical finish requires an analytic cylindrical source face",
            ));
        }
        let Some(AnalyticSurfaceDescription::Cylinder {
            axis_origin_mm,
            axis_direction,
            radius_mm,
        }) = &info.geometry.analytic
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
        let brep = self.brep();
        let native_id = *self
            .face_ids()
            .get(index)
            .ok_or_else(|| malformed("missing native cylinder face"))?;
        let source = self.original_source().face(native_id)?;
        if info.surface_face_entity.parse::<u64>().ok() != Some(source.source_face.0)
            || source.same_sense != source.face_use_forward
        {
            return Err(unsupported(
                "cylinder requires its original outward source carrier",
            ));
        }
        let solidcraft_step::geom::Surface::Cylinder { position, radius } = source.carrier.as_ref()
        else {
            return Err(unsupported("original source carrier is not a cylinder"));
        };
        if !radius.is_finite()
            || (*radius - *radius_mm).abs() > RESIDUAL_MM
            || unit(vector(position.axis))?.cross(source_axis).magnitude() > ANGULAR
            || (vector(position.origin) - vector(*axis_origin_mm))
                .cross(source_axis)
                .magnitude()
                > RESIDUAL_MM
        {
            return Err(malformed(
                "original and admitted cylinder coefficients disagree",
            ));
        }
        let source_x = unit(vector(position.ref_direction))?;
        let source_z = unit(vector(position.axis))?;
        if source_x.dot(source_z).abs() > ANGULAR {
            return Err(malformed("original cylinder placement is not orthogonal"));
        }
        let face = native(brep.face(native_id))?;
        let faceuse_id = face
            .faceuse
            .ok_or_else(|| malformed("missing cylinder faceuse"))?;
        let faceuse = native(brep.faceuse(faceuse_id))?;
        let face_forward = orientation(faceuse.orientation)?;
        let mut carrier = self
            .face_surface(native_id)
            .ok_or_else(|| malformed("missing cylinder surface"))?;
        let mut step = false;
        for depth in 0..=32 {
            work.charge(1, cancel)?;
            match carrier.kind() {
                SurfaceKind::Step(s) => {
                    carrier = s
                        .surface()
                        .ok_or_else(|| malformed("empty STEP cylinder"))?;
                    step = true;
                }
                SurfaceKind::CurveBounded(s) => carrier = s.basis(),
                _ => break,
            }
            if depth == 32 {
                return Err(malformed("nested cylinder carrier depth exceeded"));
            }
        }
        let SurfaceKind::Cone(c) = carrier.kind() else {
            return Err(unsupported("native carrier is not an exact cylinder"));
        };
        let c = c.canonical();
        let o = c.origin.origin();
        let x = c.origin.x_axis();
        let y = c.origin.y_axis();
        let a = c.origin.z_axis();
        let origin = vector([o.x, o.y, o.z]);
        let x = vector([x.x, x.y, x.z]);
        let y = vector([y.x, y.y, y.z]);
        let axis = vector([a.x, a.y, a.z]);
        if !step
            || c.semi_angle_deg != 0.0
            || !c.radius.is_finite()
            || (c.radius - *radius_mm).abs() > RESIDUAL_MM
            || !finite(origin)
            || !finite(x)
            || !finite(y)
            || !finite(axis)
            || (x.magnitude() - 1.0).abs() > ANGULAR
            || (y.magnitude() - 1.0).abs() > ANGULAR
            || (axis.magnitude() - 1.0).abs() > ANGULAR
            || x.dot(y).abs() > ANGULAR
            || x.dot(axis).abs() > ANGULAR
            || y.dot(axis).abs() > ANGULAR
            || axis.cross(source_axis).magnitude() > ANGULAR
            || (origin - vector(*axis_origin_mm))
                .cross(source_axis)
                .magnitude()
                > RESIDUAL_MM
            || x.cross(y).dot(axis) * if face_forward { 1.0 } else { -1.0 } < 1.0 - ANGULAR
        {
            return Err(malformed(
                "native cylinder coefficients or sense disagree with source carrier",
            ));
        }
        let center = [axis_origin_mm[0], axis_origin_mm[1]];
        let z = super::finish_support_extent(self, [0.0, 0.0, 1.0], work, cancel)?;
        if !z.iter().all(|v| v.is_finite()) || z[1] - z[0] <= 2.0 * RESIDUAL_MM {
            return Err(unsupported("cylinder has no finite full source height"));
        }
        if face.loops.is_empty() || face.loops.len() != faceuse.loopuses.len() {
            return Err(malformed("native cylinder loop ownership is inconsistent"));
        }
        // These maps retain only bounded identity/incidence and endpoint data,
        // not curves or another BREP. Native coefficients are already mm;
        // native UV seams do not define the source +X/+Z angular convention.
        // wire/seam placement do not change the physical two-cycle proof.
        let mut turns = [0.0f64; 2];
        let mut turn_roundoff = [0.0f64; 2];
        let mut rims = [HashMap::new(), HashMap::new()];
        let mut seams = HashMap::new();
        let census = brep.edgeuses().len();
        if census == 0 {
            return Err(unsupported("native cylinder has no admitted coedges"));
        }
        for (&loop_id, &lu_id) in face.loops.iter().zip(&faceuse.loopuses) {
            work.charge(1, cancel)?;
            let lu = native(brep.loopuse(lu_id))?;
            if lu.loop_ != Some(loop_id) || lu.faceuse != Some(faceuse_id) {
                return Err(malformed("cylinder loopuse ownership changed"));
            }
            let LoopuseStart::Edgeuse(Some(first)) = lu.start else {
                return Err(unsupported("empty cylinder wire"));
            };
            let mut current = first;
            for count in 0..census {
                work.charge(1, cancel)?;
                let edge = native(brep.edgeuse(current))?;
                if edge.owner != EdgeuseOwner::Loopuse(Some(lu_id)) {
                    return Err(malformed("foreign cylinder coedge"));
                }
                let edge_id = edge
                    .edge
                    .ok_or_else(|| malformed("missing cylinder edge"))?;
                let forward = orientation(edge.orientation)?;
                let next = edge
                    .ccw
                    .ok_or_else(|| malformed("open cylinder coedge cycle"))?;
                let (v0, v1) =
                    native(solidcore::topology::edge::start_end_vertices(brep, edge_id))?;
                let v0 = v0.ok_or_else(|| malformed("missing cylinder start vertex"))?;
                let v1 = v1.ok_or_else(|| malformed("missing cylinder end vertex"))?;
                let (front_id, back_id) = if forward { (v0, v1) } else { (v1, v0) };
                let p0 = native(brep.vertex(front_id))?.point;
                let next_use = native(brep.edgeuse(next))?;
                let next_edge = next_use
                    .edge
                    .ok_or_else(|| malformed("missing next cylinder edge"))?;
                let (next_a, next_b) = native(solidcore::topology::edge::start_end_vertices(
                    brep, next_edge,
                ))?;
                let next_front = if orientation(next_use.orientation)? {
                    next_a
                } else {
                    next_b
                };
                if next_front != Some(back_id) {
                    return Err(malformed(
                        "native cylinder coedge incidence is inconsistent",
                    ));
                }
                let p1 = native(brep.vertex(back_id))?.point;
                let a = [p0.x, p0.y, p0.z];
                let b = [p1.x, p1.y, p1.z];
                let curve = self
                    .edge_curve(edge_id)
                    .ok_or_else(|| malformed("missing cylinder edge curve"))?;
                match curve.kind() {
                    CurveKind::Ellipse(ellipse) => {
                        let c = ellipse.canonical();
                        let o = c.position.origin();
                        let x = c.position.x_axis();
                        let y = c.position.y_axis();
                        let center_mm = [o.x, o.y, o.z];
                        let u_mm = [x.x * c.radius_x, x.y * c.radius_x, x.z * c.radius_x];
                        let v_mm = [y.x * c.radius_y, y.y * c.radius_y, y.z * c.radius_y];
                        let start_angle = c.domain.min().to_radians();
                        let end_angle = c.domain.max().to_radians();
                        let point = |t: f64| {
                            std::array::from_fn(|i| {
                                center_mm[i] + u_mm[i] * t.cos() + v_mm[i] * t.sin()
                            })
                        };
                        let (mut start, mut end) = (point(start_angle), point(end_angle));
                        if c.inside_out {
                            std::mem::swap(&mut start, &mut end);
                        }
                        if !forward {
                            std::mem::swap(&mut start, &mut end);
                        }
                        if distance(start, a) > RESIDUAL_MM || distance(end, b) > RESIDUAL_MM {
                            return Err(malformed(
                                "native cylinder rim endpoints disagree with exact coefficients",
                            ));
                        }
                        let u = vector(u_mm);
                        let v = vector(v_mm);
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
                        let direction = if forward { 1.0 } else { -1.0 }
                            * if face_forward { 1.0 } else { -1.0 }
                            * if c.inside_out { -1.0 } else { 1.0 };
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
                        let (front_id, back_id, a, b) = if face_forward {
                            (front_id, back_id, a, b)
                        } else {
                            (back_id, front_id, b, a)
                        };
                        if rims[rim].insert(front_id, (back_id, a, b)).is_some() {
                            return Err(unsupported(
                                "cylinder rim has branching or repeated native incidence",
                            ));
                        }
                    }
                    CurveKind::Line(line) => {
                        let c = line.canonical_unit();
                        let direction = vector([c.vector.x, c.vector.y, c.vector.z]);
                        let origin = vector([c.point.x, c.point.y, c.point.z]);
                        if direction.cross(vector(b) - vector(a)).magnitude() > RESIDUAL_MM
                            || (vector(a) - origin).cross(direction).magnitude() > RESIDUAL_MM
                        {
                            return Err(malformed(
                                "native seam coefficients disagree with its vertices",
                            ));
                        }
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
                        let pair = seams.entry(edge_id).or_insert((forward, edge_id, 0usize));
                        if pair.2 >= 2 || (pair.2 == 1 && (pair.0 == forward || pair.1 != edge_id))
                        {
                            return Err(unsupported(
                                "cylinder seams are not opposite uses of the same native edge",
                            ));
                        }
                        pair.2 += 1;
                    }
                    _ => {
                        return Err(unsupported(
                            "cylindrical finish requires exact line/circle trims",
                        ));
                    }
                }
                current = next;
                if current == first {
                    break;
                }
                if count + 1 == census {
                    return Err(malformed("cylinder coedge cycle exceeds native census"));
                }
            }
        }
        for (_, _, count) in seams.values() {
            work.charge(1, cancel)?;
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
                work.charge(1, cancel)?;
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
            if let Some(AnalyticSurfaceDescription::Cylinder {
                axis_origin_mm: origin,
                axis_direction: axis,
                radius_mm: radius,
            }) = &other.geometry.analytic
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
