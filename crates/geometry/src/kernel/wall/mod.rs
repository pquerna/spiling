// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::Definition;
use crate::{GeometryError, GeometryWorkBudget, VerticalWallFace, check_cancel};
use solidcore::curve::curve::{Curve, p2::CurveKind};
use solidcore::nurbs::b_spline_surface_n_lib::p2::OrientType;
use solidcore::surface::surface::p2::SurfaceKind;
use solidcore::topology::{EdgeuseOwner, LoopuseStart, shell::ShellMembers};
use spiling_contracts::geometry::{
    AnalyticSurfaceDescription, FaceId, GeometryErrorCode, SurfaceFamily,
};
use std::sync::atomic::AtomicBool;
// Native B-Rep coefficients and original STEP operands are already millimetres.
// A certificate uses the actual plane law and bounded coedge census; labels and
// convenience Loop.edges never supply carrier, trim, or outward authority.

// Admission/carrier residual tolerance, not a manufacturing approximation budget.
const RESIDUAL_MM: f64 = 1e-7;
const ANGULAR: f64 = 1e-10;

fn unsupported(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::UnsupportedGeometry, message)
}
fn malformed(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::KernelFailure, message)
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn length(a: [f64; 3]) -> f64 {
    a[0].hypot(a[1]).hypot(a[2])
}
fn unit(a: [f64; 3]) -> Result<[f64; 3], GeometryError> {
    let scale = a.iter().map(|v| v.abs()).fold(0.0, f64::max);
    if !a.iter().all(|v| v.is_finite()) || scale == 0.0 {
        return Err(malformed("nonfinite or zero native wall direction"));
    }
    let scaled = a.map(|v| v / scale);
    let norm = length(scaled);
    Ok(scaled.map(|v| v / norm))
}

fn native_line_matches(
    curve: &Curve,
    orientation: bool,
    front: [f64; 3],
    back: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    if !matches!(curve.kind(), CurveKind::Line(line) if line.is_bounded()) {
        return Ok(false);
    }
    let domain = curve.natural_interval();
    let (a, b) = (domain.min(), domain.max());
    if !a.is_finite() || !b.is_finite() || a >= b {
        return Ok(false);
    }
    let prepared = crate::parametric::PreparedValidatedCurve::new(
        curve,
        [a, b],
        1.0,
        usize::MAX,
        work,
        cancel,
    )?;
    let parameters = if orientation { [a, b] } else { [b, a] };
    for (parameter, vertex) in parameters.into_iter().zip([front, back]) {
        let position = prepared.position_bounds([parameter, parameter], work, cancel)?;
        let squared = (0..3).fold(crate::parametric::Interval::point(0.0), |sum, axis| {
            sum.add(
                position[axis]
                    .sub(crate::parametric::Interval::point(vertex[axis]))
                    .square(),
            )
        });
        let error = crate::parametric::Interval {
            lo: squared.lo.max(0.0),
            hi: squared.hi,
        }
        .sqrt()?
        .hi;
        if error > RESIDUAL_MM {
            return Ok(false);
        }
    }
    Ok(true)
}

impl Definition {
    /// Certify an exact source-local, full-height rectangular exterior wall.
    /// Face identity is the admitted positional native face use, never a mesh
    /// face, section line, infinite-carrier match or parsed `step:N` guess.
    pub fn vertical_wall_face(&self, face_id: &FaceId) -> Result<VerticalWallFace, GeometryError> {
        self.vertical_wall_face_with_cancel(face_id, &AtomicBool::new(false))
    }

    /// The same immutable certificate with cooperative cancellation at each
    /// native edge/face and analytic projection witness boundary.
    pub fn vertical_wall_face_with_cancel(
        &self,
        face_id: &FaceId,
        cancel: &AtomicBool,
    ) -> Result<VerticalWallFace, GeometryError> {
        self.vertical_wall_face_with_work(face_id, &mut GeometryWorkBudget::new(50_000_000), cancel)
    }

    pub(crate) fn vertical_wall_face_with_work(
        &self,
        face_id: &FaceId,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<VerticalWallFace, GeometryError> {
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
        if info.geometry.family != SurfaceFamily::Plane {
            return Err(unsupported(
                "surface finish requires an analytic planar source face",
            ));
        }
        let Some(AnalyticSurfaceDescription::Plane { .. }) = &info.geometry.analytic else {
            return Err(unsupported(
                "surface finish requires a vertical planar native face",
            ));
        };
        let brep = self.brep();
        let native_id = *self
            .face_ids()
            .get(index)
            .ok_or_else(|| malformed("native source face-use mapping is inconsistent"))?;
        let original = self.original_source().face(native_id)?;
        let solidcraft_step::geom::Surface::Plane {
            position: source_position,
        } = original.carrier.as_ref()
        else {
            return Err(unsupported(
                "surface finish requires an original analytic planar carrier",
            ));
        };
        let source_origin = &source_position.origin;
        let source_normal = &source_position.axis;
        if (original.same_sense == original.face_use_forward) != info.orientation {
            return Err(malformed(
                "native wall orientation disagrees with original source face use",
            ));
        }
        let face = brep
            .face(native_id)
            .map_err(|_| malformed("native source face is absent"))?;
        let surface = self
            .face_surface(native_id)
            .ok_or_else(|| malformed("native wall surface is absent"))?;
        let SurfaceKind::Plane(plane) = surface.kind() else {
            return Err(malformed(
                "source planar face mapped to a nonplanar native carrier",
            ));
        };
        let position = plane.position();
        let p = position.origin();
        let x = position.x_axis();
        let y = position.y_axis();
        let uv_scale = plane.uv_scale();
        if ![uv_scale.x, uv_scale.y]
            .iter()
            .all(|v| v.is_finite() && *v != 0.0)
        {
            return Err(malformed("native wall plane has invalid parameter scales"));
        }
        let native_origin = [p.x, p.y, p.z];
        let raw_normal = unit([
            x.y * y.z - x.z * y.y,
            x.z * y.x - x.x * y.z,
            x.x * y.y - x.y * y.x,
        ])?
        .map(|v| {
            v * if uv_scale.x.signum() == uv_scale.y.signum() {
                1.0
            } else {
                -1.0
            }
        });
        let shell = brep
            .shell(face.shell)
            .map_err(|_| malformed("native wall shell is absent"))?;
        let ShellMembers::Faceuses(uses) = &shell.members else {
            return Err(malformed("native wall shell has no oriented face uses"));
        };
        let mut orientation = None;
        for &id in uses {
            work.charge(1, cancel)?;
            let use_ = brep
                .faceuse(id)
                .map_err(|_| malformed("native wall faceuse is absent"))?;
            if use_.face == Some(native_id) {
                if orientation.is_some() {
                    return Err(malformed("duplicate native wall face use"));
                }
                orientation = Some(match use_.orientation {
                    OrientType::Same => true,
                    OrientType::Opposite => false,
                    _ => return Err(malformed("native wall face use lacks orientation")),
                });
            }
        }
        let orientation = orientation.ok_or_else(|| malformed("native wall face use is absent"))?;
        let outward = raw_normal.map(|v| if orientation { v } else { -v });
        let source_outward = unit(*source_normal)?.map(|v| if info.orientation { v } else { -v });
        if !native_origin
            .iter()
            .chain(source_origin)
            .all(|v| v.is_finite())
            || dot(outward, source_outward) < 1.0 - ANGULAR
            || !dot(sub(native_origin, *source_origin), outward).is_finite()
            || dot(sub(native_origin, *source_origin), outward).abs() > RESIDUAL_MM
        {
            return Err(malformed(
                "native wall carrier disagrees with its oriented source face use",
            ));
        }
        if outward[2].abs() > ANGULAR {
            return Err(unsupported(
                "surface finish requires a vertical planar face; horizontal or sloped carrier",
            ));
        }
        let mut horizontal = unit([-outward[1], outward[0], 0.0])?;
        // Fixed lexical source-local orientation, independent of face winding,
        // source carrier parameterization and occurrence pose.
        if horizontal[0] < 0.0 || (horizontal[0] == 0.0 && horizontal[1] < 0.0) {
            horizontal = horizontal.map(|v| -v);
        }
        if face.loops.len() != 1 {
            return Err(unsupported(
                "surface finish requires one closed four-line rectangular trim without holes",
            ));
        }
        let loop_ = brep
            .loop_(face.loops[0])
            .map_err(|_| malformed("native wall loop is absent"))?;
        let lu_id = loop_
            .loopuse
            .ok_or_else(|| malformed("native wall loopuse is absent"))?;
        let lu = brep
            .loopuse(lu_id)
            .map_err(|_| malformed("native wall loopuse is absent"))?;
        let LoopuseStart::Edgeuse(Some(first)) = lu.start else {
            return Err(unsupported("native wall requires an explicit coedge cycle"));
        };
        let mut current = first;
        let mut points = [[0.0; 3]; 4];
        let mut edges = [None; 4];
        let mut vertices = [None; 4];
        let mut previous_back = None;
        for i in 0..4 {
            work.charge(1, cancel)?;
            if i != 0 && current == first {
                return Err(unsupported("native wall has fewer than four trim edges"));
            }
            let eu = brep
                .edgeuse(current)
                .map_err(|_| malformed("native wall coedge is absent"))?;
            if eu.owner != EdgeuseOwner::Loopuse(Some(lu_id)) {
                return Err(malformed("native wall coedge enters a foreign loopuse"));
            }
            let edge_id = eu
                .edge
                .ok_or_else(|| malformed("native wall coedge lacks an edge"))?;
            let edge = brep
                .edge(edge_id)
                .map_err(|_| malformed("native wall edge is absent"))?;
            let forward = match eu.orientation {
                OrientType::Same => true,
                OrientType::Opposite => false,
                _ => return Err(malformed("native wall coedge lacks orientation")),
            };
            let [front_id, back_id] = if forward {
                edge.vertices
            } else {
                [edge.vertices[1], edge.vertices[0]]
            };
            if previous_back.is_some_and(|id| id != front_id)
                || edges[..i].contains(&Some(edge_id))
                || vertices[..i].contains(&Some(front_id))
            {
                return Err(unsupported(
                    "native wall trim does not have four distinct connected corners and edges",
                ));
            }
            edges[i] = Some(edge_id);
            vertices[i] = Some(front_id);
            previous_back = Some(back_id);
            let p = brep
                .vertex(front_id)
                .map_err(|_| malformed("native wall corner is absent"))?
                .point;
            let b = brep
                .vertex(back_id)
                .map_err(|_| malformed("native wall corner is absent"))?
                .point;
            points[i] = [p.x, p.y, p.z];
            let curve = self
                .edge_curve(edge_id)
                .ok_or_else(|| malformed("native wall edge curve is absent"))?;
            if !native_line_matches(curve, forward, points[i], [b.x, b.y, b.z], work, cancel)? {
                return Err(unsupported(
                    "native wall trim must consist of four exact bounded line edges",
                ));
            }
            let residual = dot(sub(points[i], native_origin), outward);
            if !points[i].iter().all(|v| v.is_finite())
                || !residual.is_finite()
                || residual.abs() > RESIDUAL_MM
            {
                return Err(malformed(
                    "native wall corner is nonfinite or off its supporting carrier",
                ));
            }
            let next = eu
                .ccw
                .ok_or_else(|| malformed("native wall coedge cycle is open"))?;
            if brep
                .edgeuse(next)
                .map_err(|_| malformed("native wall successor is absent"))?
                .cw
                != Some(current)
            {
                return Err(malformed("native wall coedge links are inconsistent"));
            }
            current = next;
        }
        if current != first || previous_back != vertices[0] {
            return Err(unsupported(
                "surface finish requires one closed four-line rectangular trim without holes",
            ));
        }
        let coordinate = points.map(|p| [dot(sub(p, points[0]), horizontal), p[2]]);
        if !coordinate.iter().flatten().all(|v| v.is_finite()) {
            return Err(malformed("native wall span is nonfinite"));
        }
        let min_h = coordinate
            .iter()
            .map(|p| p[0])
            .fold(f64::INFINITY, f64::min);
        let max_h = coordinate
            .iter()
            .map(|p| p[0])
            .fold(f64::NEG_INFINITY, f64::max);
        let min_z = coordinate
            .iter()
            .map(|p| p[1])
            .fold(f64::INFINITY, f64::min);
        let max_z = coordinate
            .iter()
            .map(|p| p[1])
            .fold(f64::NEG_INFINITY, f64::max);
        let width = max_h - min_h;
        let height = max_z - min_z;
        if !width.is_finite()
            || !height.is_finite()
            || width <= 2.0 * RESIDUAL_MM
            || height <= 2.0 * RESIDUAL_MM
        {
            return Err(unsupported(
                "native rectangular wall has no finite nondegenerate span",
            ));
        }
        let mut corners = [0u8; 4];
        let mut seen = 0u8;
        let mut origin_index = 0;
        for (i, p) in coordinate.iter().enumerate() {
            let h = if (p[0] - min_h).abs() <= RESIDUAL_MM {
                0
            } else if (p[0] - max_h).abs() <= RESIDUAL_MM {
                1
            } else {
                return Err(unsupported(
                    "native wall trim is not a rectangle in local horizontal/Z coordinates",
                ));
            };
            let z = if (p[1] - min_z).abs() <= RESIDUAL_MM {
                0
            } else if (p[1] - max_z).abs() <= RESIDUAL_MM {
                2
            } else {
                return Err(unsupported(
                    "native wall trim is not a rectangle in local horizontal/Z coordinates",
                ));
            };
            let corner = h | z;
            if seen & (1 << corner) != 0 {
                return Err(unsupported(
                    "native rectangular wall repeats a geometric corner",
                ));
            }
            seen |= 1 << corner;
            corners[i] = corner;
            if corner == 0 {
                origin_index = i;
            }
        }
        for i in 0..4 {
            if !matches!(corners[i] ^ corners[(i + 1) % 4], 1 | 2) {
                return Err(unsupported(
                    "native rectangular wall contains diagonal or crossing trim edges",
                ));
            }
        }
        let e0 = unit(sub(points[1], points[0]))?;
        let e1 = unit(sub(points[2], points[1]))?;
        let cross = [
            e0[1] * e1[2] - e0[2] * e1[1],
            e0[2] * e1[0] - e0[0] * e1[2],
            e0[0] * e1[1] - e0[1] * e1[0],
        ];
        let winding = dot(cross, outward) * if orientation { 1.0 } else { -1.0 };
        if !winding.is_finite() || winding < 1.0 - ANGULAR {
            return Err(unsupported(
                "native rectangular trim is not the oriented outer material boundary",
            ));
        }
        let z_span = super::finish_support_extent(self, [0.0, 0.0, 1.0], work, cancel)?;
        if (min_z - z_span[0]).abs() > RESIDUAL_MM || (max_z - z_span[1]).abs() > RESIDUAL_MM {
            return Err(unsupported(
                "surface finish requires a native wall spanning the complete local source height",
            ));
        }
        let support_span = super::finish_support_extent(self, outward, work, cancel)?;
        let support = dot(points[origin_index], outward);
        if !support.is_finite() || (support_span[1] - support).abs() > RESIDUAL_MM {
            return Err(unsupported(
                "selected native wall is recessed or is not an outward supporting material plane",
            ));
        }
        for (other_index, &other_id) in self.face_ids().iter().enumerate() {
            work.charge(1, cancel)?;
            if other_index == index {
                continue;
            }
            let other_surface = self
                .face_surface(other_id)
                .ok_or_else(|| malformed("native supporting face has no carrier"))?;
            if let SurfaceKind::Plane(other_plane) = other_surface.kind() {
                let position = other_plane.position();
                let p = position.origin();
                let x = position.x_axis();
                let y = position.y_axis();
                let origin_mm = [p.x, p.y, p.z];
                let other_normal = unit([
                    x.y * y.z - x.z * y.y,
                    x.z * y.x - x.x * y.z,
                    x.x * y.y - x.y * y.x,
                ])?;
                let residual = dot(sub(origin_mm, points[origin_index]), outward);
                if !origin_mm.iter().all(|v| v.is_finite()) || !residual.is_finite() {
                    return Err(malformed("native supporting-plane comparison is nonfinite"));
                }
                if dot(other_normal, outward).abs() >= 1.0 - ANGULAR
                    && residual.abs() <= RESIDUAL_MM
                {
                    return Err(unsupported(
                        "selected supporting plane has ambiguous additional native face trims",
                    ));
                }
            }
        }
        let mut origin = points[origin_index];
        origin[2] = z_span[0];
        check_cancel(cancel)?;
        Ok(VerticalWallFace {
            origin_mm: origin,
            horizontal_direction: horizontal,
            outward_normal: outward,
            width_mm: width,
            min_z_mm: z_span[0],
            max_z_mm: z_span[1],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solidcore::curve::line::Line;
    use solidcore::geom::vector3d::Point3;

    #[test]
    fn native_line_uses_actual_bounded_range_endpoints_and_coedge_sense() {
        let curve: Curve =
            Line::from_points(Point3::new(6.0, 0.0, 0.0), Point3::new(8.0, 0.0, 0.0), 3).into();
        let mut work = GeometryWorkBudget::new(100_000);
        let cancel = AtomicBool::new(false);
        assert!(
            native_line_matches(
                &curve,
                true,
                [6.0, 0.0, 0.0],
                [8.0, 0.0, 0.0],
                &mut work,
                &cancel
            )
            .unwrap()
        );
        assert!(
            native_line_matches(
                &curve,
                false,
                [8.0, 0.0, 0.0],
                [6.0, 0.0, 0.0],
                &mut work,
                &cancel
            )
            .unwrap()
        );
        assert!(
            !native_line_matches(
                &curve,
                true,
                [8.0, 0.0, 0.0],
                [6.0, 0.0, 0.0],
                &mut work,
                &cancel
            )
            .unwrap()
        );
        assert!(
            !native_line_matches(
                &curve,
                true,
                [2.0, 0.0, 0.0],
                [3.0, 0.0, 0.0],
                &mut work,
                &cancel
            )
            .unwrap()
        );
    }
}
