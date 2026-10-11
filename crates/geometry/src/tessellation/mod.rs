// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{MeshArtifact, check_cancel, kernel::Definition};
use monstertruck_meshing::prelude::{
    PolygonMesh, TessellationOptions, TessellationPrimitiveMode, TessellationPrimitiveOptions,
    cshell_triangulation_with,
};
use monstertruck_modeling::{ParametricSurface3D, Shell, Surface};
use spiling_contracts::display::{MeshBudget, MeshChunkBuilder};
use spiling_contracts::geometry::{
    DisplayProfile, FaceCarrier, FaceInfo, GeometryError, GeometryErrorCode, MAX_NATIVE_FACES,
    MESH_CARRIER_TOLERANCE_MM, NORMAL_NORM_TOLERANCE,
};
use std::{collections::HashMap, sync::atomic::AtomicBool};

pub(crate) fn tessellate(
    definition: &Definition,
    profile: DisplayProfile,
    cancel: &AtomicBool,
    budget: MeshBudget,
) -> Result<MeshArtifact, GeometryError> {
    check_cancel(cancel)?;
    budget.validate()?;
    let face_count = definition.faces.len();
    if face_count == 0 || face_count > MAX_NATIVE_FACES as usize {
        return Err(resource("native face count exceeds display support limits"));
    }
    if definition.solid.face_iter().count() != face_count {
        return Err(invalid("native face slots disagree with source-face table"));
    }
    let mut builder =
        MeshChunkBuilder::with_budget(definition.id.clone(), face_count as u32, profile, budget)?;
    let options = TessellationOptions {
        tolerance: MESH_CARRIER_TOLERANCE_MM,
        search_trials: 2,
        primitive: TessellationPrimitiveOptions {
            mode: TessellationPrimitiveMode::Triangles,
            ..Default::default()
        },
    };
    // A one-face shell preserves the public mesher's boundary/trim path without
    // materializing native polygon data for the rest of the solid.
    for (ordinal, (face, info)) in definition
        .solid
        .face_iter()
        .zip(&definition.faces)
        .enumerate()
    {
        check_cancel(cancel)?;
        info.validate()?;
        let shell: Shell = std::iter::once(face.clone()).collect();
        let compressed = shell.compress();
        if compressed.faces.len() != 1 || compressed.faces[0].orientation != face.orientation() {
            return Err(invalid(
                "native face compression changed face slots or orientation",
            ));
        }
        let mut meshed = cshell_triangulation_with(&compressed, options);
        check_cancel(cancel)?;
        if meshed.faces.len() != 1 || meshed.faces[0].orientation != face.orientation() {
            return Err(invalid(
                "strict meshing changed native face slots or orientation",
            ));
        }
        let polygon = meshed.faces[0]
            .surface
            .take()
            .ok_or_else(|| invalid("strict meshing dropped a native face"))?;
        let data = audit_face(
            &polygon,
            &compressed.faces[0].surface,
            face.orientation(),
            info,
            cancel,
            builder.remaining_budget(),
        )?;
        builder.push_face(
            ordinal as u32,
            &data.positions,
            &data.normals,
            &data.triangles,
            data.carrier_deviation_mm,
        )?;
        // polygon and indexed adapter data are dropped at this face boundary.
    }
    check_cancel(cancel)?;
    builder.finish()
}

struct FaceMesh {
    positions: Vec<[f64; 3]>,
    normals: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
    carrier_deviation_mm: f64,
}

fn audit_face(
    polygon: &PolygonMesh,
    surface: &Surface,
    native_orientation: bool,
    info: &FaceInfo,
    cancel: &AtomicBool,
    budget: MeshBudget,
) -> Result<FaceMesh, GeometryError> {
    if polygon.tri_faces().is_empty()
        || polygon.positions().is_empty()
        || !polygon.quad_faces().is_empty()
        || !polygon.other_faces().is_empty()
    {
        return Err(invalid(
            "strict meshing returned an empty or non-triangle face",
        ));
    }
    if polygon.positions().len() > budget.vertices as usize
        || polygon.normals().len() > budget.vertices as usize
        || polygon.uv_coords().len() > budget.vertices as usize
        || polygon.tri_faces().len() > budget.triangles as usize
    {
        return Err(resource(
            "per-face native tessellation exceeds count limits",
        ));
    }
    if polygon.positions().iter().any(|p| !finite([p.x, p.y, p.z]))
        || polygon.normals().iter().any(|n| {
            !finite([n.x, n.y, n.z])
                || (length([n.x, n.y, n.z]) - 1.0).abs() > NORMAL_NORM_TOLERANCE
        })
        || polygon
            .uv_coords()
            .iter()
            .any(|uv| !uv.x.is_finite() || !uv.y.is_finite())
    {
        return Err(invalid(
            "native tessellation contains nonfinite attributes or nonunit normals",
        ));
    }
    let mut result = FaceMesh {
        positions: Vec::new(),
        normals: Vec::new(),
        triangles: Vec::with_capacity(polygon.tri_faces().len()),
        carrier_deviation_mm: 0.0,
    };
    // Exact equal position/normal values share a vertex, including identical UV
    // seam attributes. A UV-only boundary must not duplicate display vertices.
    let mut vertices: HashMap<[u64; 6], u32> = HashMap::new();
    let mut corners: HashMap<(usize, Option<usize>, Option<usize>), u32> = HashMap::new();
    for (triangle_index, triangle) in polygon.tri_faces().iter().enumerate() {
        if triangle_index % 1024 == 0 {
            check_cancel(cancel)?;
        }
        let mut points = [[0.0; 3]; 3];
        let mut normals = [[0.0; 3]; 3];
        let mut indices = [0; 3];
        for (slot, vertex) in triangle.iter().enumerate() {
            let corner = (vertex.pos, vertex.nor, vertex.uv);
            if let Some(&index) = corners.get(&corner) {
                indices[slot] = index;
                points[slot] = result.positions[index as usize];
                normals[slot] = result.normals[index as usize];
                continue;
            }
            if corners.len() == budget.vertices as usize {
                return Err(resource(
                    "per-face native attribute combination budget exceeded",
                ));
            }
            let point = polygon
                .positions()
                .get(vertex.pos)
                .ok_or_else(|| invalid("native mesh position index out of range"))?;
            let normal = vertex
                .nor
                .and_then(|i| polygon.normals().get(i))
                .ok_or_else(|| invalid("native mesh normal index absent or out of range"))?;
            let uv = vertex
                .uv
                .and_then(|i| polygon.uv_coords().get(i))
                .ok_or_else(|| invalid("native mesh parameter index absent or out of range"))?;
            let point = [point.x, point.y, point.z];
            let normal = [normal.x, normal.y, normal.z];
            let analytic = surface.normal(uv.x, uv.y);
            let analytic = unit([analytic.x, analytic.y, analytic.z])?;
            if length(sub(normal, analytic)) > NORMAL_NORM_TOLERANCE {
                return Err(invalid(
                    "native mesh normal disagrees with analytic surface orientation",
                ));
            }
            let carrier_normal = carrier_normal(&info.carrier, point)?;
            let normal = if info.orientation {
                carrier_normal
            } else {
                scale(carrier_normal, -1.0)
            };
            let native_normal = if native_orientation {
                analytic
            } else {
                scale(analytic, -1.0)
            };
            if length(sub(native_normal, normal)) > NORMAL_NORM_TOLERANCE {
                return Err(invalid(
                    "native oriented normal disagrees with cached carrier orientation",
                ));
            }
            // Cached orientation is relative to the canonical carrier; topology
            // orientation is separately applied to native triangle winding.
            points[slot] = point;
            normals[slot] = normal;
            let key = std::array::from_fn(|axis| {
                canonical_bits(if axis < 3 {
                    point[axis]
                } else {
                    normal[axis - 3]
                })
            });
            indices[slot] = if let Some(&index) = vertices.get(&key) {
                index
            } else {
                if result.positions.len() == budget.vertices as usize {
                    return Err(resource("per-face remapped vertex budget exceeded"));
                }
                let index = result.positions.len() as u32;
                vertices.insert(key, index);
                result.positions.push(point);
                result.normals.push(normal);
                index
            };
            corners.insert(corner, indices[slot]);
        }
        if !native_orientation {
            indices.swap(1, 2);
            points.swap(1, 2);
            normals.swap(1, 2);
        }
        let geometric_normal = cross(sub(points[1], points[0]), sub(points[2], points[0]));
        if !finite(geometric_normal)
            || length(geometric_normal) == 0.0
            || normals
                .iter()
                .any(|&normal| dot(geometric_normal, normal) <= 0.0)
        {
            return Err(invalid(
                "native mesh triangle is degenerate or incorrectly oriented",
            ));
        }
        let deviation = triangle_carrier_deviation(&info.carrier, points)?;
        if deviation > MESH_CARRIER_TOLERANCE_MM {
            return Err(invalid(
                "native triangle carrier deviation exceeds display profile",
            ));
        }
        result.carrier_deviation_mm = result.carrier_deviation_mm.max(deviation);
        result.triangles.push(indices);
    }
    Ok(result)
}

fn carrier_normal(carrier: &FaceCarrier, point: [f64; 3]) -> Result<[f64; 3], GeometryError> {
    match carrier {
        FaceCarrier::Plane { normal, .. } => Ok(*normal),
        FaceCarrier::Cylinder {
            axis_origin_mm,
            axis_direction,
            ..
        } => unit(project_radial(sub(point, *axis_origin_mm), *axis_direction)),
    }
}

/// The radial norm is convex: its maximum over a triangle is at a vertex.
/// Its minimum is zero when the projected triangle contains the axis, otherwise
/// it is the minimum distance on all three projected edges (including endpoints).
fn triangle_carrier_deviation(
    carrier: &FaceCarrier,
    triangle: [[f64; 3]; 3],
) -> Result<f64, GeometryError> {
    let deviation = match carrier {
        FaceCarrier::Plane { origin_mm, normal } => {
            let mut maximum = 0.0_f64;
            for point in triangle {
                let relative = sub(point, *origin_mm);
                let distance = dot(relative, *normal).abs();
                if !distance.is_finite() {
                    return Err(invalid("nonfinite plane carrier distance"));
                }
                let rounding = 16.0 * f64::EPSILON * length(relative);
                maximum = maximum.max(distance + rounding);
            }
            maximum
        }
        FaceCarrier::Cylinder {
            axis_origin_mm,
            axis_direction,
            radius_mm,
        } => {
            let projected =
                triangle.map(|point| project_radial(sub(point, *axis_origin_mm), *axis_direction));
            if projected.iter().any(|&point| !finite(point)) {
                return Err(invalid("nonfinite cylinder triangle projection"));
            }
            let maximum = projected.into_iter().map(length).fold(0.0_f64, f64::max);
            let minimum = projected_triangle_distance(projected);
            if !maximum.is_finite() || !minimum.is_finite() {
                return Err(invalid("nonfinite projected triangle distance"));
            }
            // Outward rounding allowance protects the conservative bound from
            // scalar projection/distance arithmetic, separately from f32 packing.
            let rounding = 64.0 * f64::EPSILON * (maximum + radius_mm);
            (maximum + rounding - radius_mm)
                .max(radius_mm - (minimum - rounding).max(0.0))
                .max(0.0)
        }
    };
    if !deviation.is_finite() {
        return Err(invalid("nonfinite analytic carrier deviation"));
    }
    Ok(deviation)
}

fn projected_triangle_distance(points: [[f64; 3]; 3]) -> f64 {
    let normal = cross(sub(points[1], points[0]), sub(points[2], points[0]));
    let edges = [
        (points[0], points[1]),
        (points[1], points[2]),
        (points[2], points[0]),
    ];
    if dot(normal, normal) > 0.0
        && edges
            .iter()
            .all(|&(a, b)| dot(cross(sub(b, a), scale(a, -1.0)), normal) >= 0.0)
    {
        0.0
    } else {
        edges
            .into_iter()
            .map(|(a, b)| segment_distance(a, b))
            .fold(f64::INFINITY, f64::min)
    }
}

fn segment_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let direction = sub(b, a);
    let denominator = dot(direction, direction);
    let t = if denominator == 0.0 {
        0.0
    } else {
        (-dot(a, direction) / denominator).clamp(0.0, 1.0)
    };
    length(add(a, scale(direction, t)))
}
fn project_radial(point: [f64; 3], axis: [f64; 3]) -> [f64; 3] {
    sub(point, scale(axis, dot(point, axis)))
}
fn unit(vector: [f64; 3]) -> Result<[f64; 3], GeometryError> {
    let norm = length(vector);
    if !finite(vector) || !norm.is_finite() || norm == 0.0 {
        return Err(invalid("invalid analytic normal"));
    }
    Ok(scale(vector, 1.0 / norm))
}
fn canonical_bits(value: f64) -> u64 {
    if value == 0.0 { 0 } else { value.to_bits() }
}
fn finite(vector: [f64; 3]) -> bool {
    vector.into_iter().all(f64::is_finite)
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    a.map(|v| v * s)
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(a: [f64; 3]) -> f64 {
    a[0].hypot(a[1]).hypot(a[2])
}
fn invalid(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::InvalidGeometry, message)
}
fn resource(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, message)
}

#[cfg(test)]
mod tests;
