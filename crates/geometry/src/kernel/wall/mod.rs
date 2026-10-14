// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::Definition;
use crate::{GeometryError, VerticalWallFace, check_cancel, section::projected_extent};
use monstertruck_modeling::{Curve, Surface};
use spiling_contracts::geometry::{FaceCarrier, FaceId, GeometryErrorCode};
use std::sync::atomic::AtomicBool;

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
        let FaceCarrier::Plane {
            origin_mm: source_origin,
            normal: source_normal,
        } = &info.carrier
        else {
            return Err(unsupported(
                "surface finish requires a vertical planar native face, not a cylinder",
            ));
        };
        let shells = self.solid.boundaries();
        if shells.len() != 1 || shells[0].len() != self.faces.len() {
            return Err(malformed("native source face-use mapping is inconsistent"));
        }
        let face = &shells[0][index];
        let (native_origin, native_normal) = face
            .with_surface(|surface| match surface {
                Surface::Plane(plane) => {
                    let origin = plane.origin();
                    let normal = plane.normal();
                    Some((
                        [origin.x, origin.y, origin.z],
                        [normal.x, normal.y, normal.z],
                    ))
                }
                _ => None,
            })
            .ok_or_else(|| malformed("source planar face mapped to a nonplanar native carrier"))?;
        let raw_normal = unit(native_normal)?;
        let outward = raw_normal.map(|v| if face.orientation() { v } else { -v });
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
        let wires = face.absolute_boundaries();
        if wires.len() != 1 || wires[0].len() != 4 || !wires[0].is_closed() {
            return Err(unsupported(
                "surface finish requires one closed four-line rectangular trim without holes",
            ));
        }
        let wire = &wires[0];
        let mut points = [[0.0; 3]; 4];
        for i in 0..4 {
            check_cancel(cancel)?;
            let edge = &wire[i];
            if edge.back().id() != wire[(i + 1) % 4].front().id()
                || (0..i)
                    .any(|j| edge.id() == wire[j].id() || edge.front().id() == wire[j].front().id())
            {
                return Err(unsupported(
                    "native wall trim does not have four distinct connected corners and edges",
                ));
            }
            points[i] = edge.front().point().into();
            let back: [f64; 3] = edge.back().point().into();
            let line_matches = edge.with_curve(|curve| {
                let Curve::Line(line) = curve else {
                    return false;
                };
                let (a, b): ([f64; 3], [f64; 3]) = if edge.orientation() {
                    (line.0.into(), line.1.into())
                } else {
                    (line.1.into(), line.0.into())
                };
                a.iter().chain(&b).all(|v| v.is_finite())
                    && length(sub(a, points[i])) <= RESIDUAL_MM
                    && length(sub(b, back)) <= RESIDUAL_MM
            });
            if !line_matches {
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
        let winding = dot(cross, outward) * if face.orientation() { 1.0 } else { -1.0 };
        if !winding.is_finite() || winding < 1.0 - ANGULAR {
            return Err(unsupported(
                "native rectangular trim is not the oriented outer material boundary",
            ));
        }
        let z_span = projected_extent(self, [0.0, 0.0, 1.0], cancel)?;
        if (min_z - z_span[0]).abs() > RESIDUAL_MM || (max_z - z_span[1]).abs() > RESIDUAL_MM {
            return Err(unsupported(
                "surface finish requires a native wall spanning the complete local source height",
            ));
        }
        let support_span = projected_extent(self, outward, cancel)?;
        let support = dot(points[origin_index], outward);
        if !support.is_finite() || (support_span[1] - support).abs() > RESIDUAL_MM {
            return Err(unsupported(
                "selected native wall is recessed or is not an outward supporting material plane",
            ));
        }
        for (other_index, other) in self.faces.iter().enumerate() {
            check_cancel(cancel)?;
            if other_index == index {
                continue;
            }
            if let FaceCarrier::Plane { origin_mm, normal } = &other.carrier {
                let other_normal = unit(*normal)?;
                let residual = dot(sub(*origin_mm, points[origin_index]), outward);
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
