// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{check_memory, invalid, resource, shape, trim};
use crate::{
    GeometryWorkBudget,
    kernel::Definition,
    parametric::{self, DisplayMeshBox, DisplayTriangleProof, Interval},
};
use solidcore::nurbs::b_spline_surface_n_lib::p2::OrientType;
use solidcore::{Brep, FaceId};
use solidcore_mesh::{EdgeSet, FaceMesh};
use spiling_contracts::{
    display::MeshBudget,
    geometry::{
        GeometryError, GeometryErrorCode, MESH_CARRIER_TOLERANCE_MM, NORMAL_NORM_TOLERANCE,
    },
};
use std::{mem::size_of, sync::atomic::AtomicBool};

pub(super) struct AuditedFace {
    pub positions: Vec<[f64; 3]>,
    pub normals: Vec<[f64; 3]>,
    pub carrier_error: f64,
    pub trim_error: f64,
}

pub(super) fn face(
    definition: &Definition,
    ordinal: usize,
    mesh: &FaceMesh,
    edges: &EdgeSet,
    budget: MeshBudget,
    outside: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<AuditedFace, GeometryError> {
    let count = mesh.positions.len();
    if count == 0
        || mesh.triangles.is_empty()
        || mesh.uvs.len() != count
        || mesh.normals.len() != count
        || mesh.shared.len() != count
        || mesh.gaps.len() != count
    {
        return Err(invalid(
            "native candidate has empty or inconsistent face attributes",
        ));
    }
    if count > budget.vertices as usize || mesh.triangles.len() > budget.triangles as usize {
        return Err(resource(
            "native face candidate exceeds remaining display counts",
        ));
    }
    if mesh.unconverged {
        return Err(shape("native candidate stopped before convergence"));
    }
    if mesh.split_boundary || mesh.ear_clipped {
        return Err(invalid(
            "native candidate has no independently auditable regular UV triangle chain",
        ));
    }
    let native_bytes = mesh.retained_bytes();
    let base = outside.saturating_add(native_bytes);
    let surface = definition
        .face_surface(mesh.face)
        .ok_or_else(|| invalid("native display face has no carrier"))?;
    let sense = outward_sense(definition.brep(), mesh.face)?;
    let (trim_error, singular) = trim::certify(definition, mesh, edges, sense, base, work, cancel)?;
    // The complete native carrier is the union of retained triangle patches
    // and the proved missing chart fans. Direct transport on disjoint patches
    // combines by max, not by adding one patch's error to another's. Original
    // trim transport is a separate serial bound and remains additive for the
    // entire published face, conservatively including its singular fans.
    if parametric::up(singular.carrier_error + trim_error) > MESH_CARRIER_TOLERANCE_MM {
        return Err(shape(format!(
            "source face {ordinal} singular carrier={} trim={trim_error} exceeds display tolerance",
            singular.carrier_error,
        )));
    }
    // Narrow proof-path stopping to the actual remaining allowance. A bound
    // below the full carrier cap may still need the existing SAME-triangle
    // proof subdivision before adding this independently certified trim.
    let carrier_allowance = Interval::point(MESH_CARRIER_TOLERANCE_MM)
        .sub(Interval::point(trim_error))
        .lo;
    let base = base.saturating_add(singular.retained_bytes());
    let proof_bytes = DisplayTriangleProof::base_retained_bytes_for_vertices(count)?;
    let (box_ids, boxes) = box_inventory(mesh, base.saturating_add(proof_bytes), work, cancel)?;
    check_memory(
        base.saturating_add(proof_bytes)
            .saturating_add(box_ids.capacity().saturating_mul(size_of::<u32>()))
            .saturating_add(boxes.capacity().saturating_mul(size_of::<DisplayMeshBox>()))
            .saturating_add(count.saturating_mul(49)),
    )?;
    let allowance = super::PROOF_BYTES
        .saturating_sub(base)
        .saturating_sub(count.saturating_mul(49));
    let mut proof = DisplayTriangleProof::with_mesh_inventory(
        surface, &mesh.uvs, box_ids, boxes, allowance, work, cancel,
    )?;
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut used = Vec::new();
    positions
        .try_reserve_exact(count)
        .map_err(|_| resource("native display position allocation failed"))?;
    normals
        .try_reserve_exact(count)
        .map_err(|_| resource("native display normal allocation failed"))?;
    used.try_reserve_exact(count)
        .map_err(|_| resource("native display vertex audit allocation failed"))?;
    check_memory(
        base.saturating_add(proof.retained_bytes())
            .saturating_add(positions.capacity().saturating_mul(24))
            .saturating_add(normals.capacity().saturating_mul(24))
            .saturating_add(used.capacity()),
    )?;
    for point in &mesh.positions {
        work.charge(1, cancel)?;
        let p = [point.x, point.y, point.z];
        if p.iter().any(|v| !v.is_finite()) {
            return Err(invalid("native candidate has nonfinite positions"));
        }
        positions.push(p);
        normals.push([0.0; 3]);
        used.push(false);
    }
    let mut carrier_error = singular.carrier_error;
    for (triangle_index, &triangle) in mesh.triangles.iter().enumerate() {
        work.charge(1, cancel)?;
        let uv = triangle_uv(mesh, triangle)?;
        let points = triangle.map(|i| positions[i as usize]);
        let vectors = [points[1], points[2]].map(|point| {
            std::array::from_fn::<_, 3, _>(|i| {
                Interval::point(point[i]).sub(Interval::point(points[0][i]))
            })
        });
        let [a, b] = vectors;
        let area = [
            a[1].mul(b[2]).sub(a[2].mul(b[1])),
            a[2].mul(b[0]).sub(a[0].mul(b[2])),
            a[0].mul(b[1]).sub(a[1].mul(b[0])),
        ];
        let error = proof.indexed_triangle_error_bound(
            &uv,
            &points,
            triangle_index,
            &triangle.map(|i| i as usize),
            carrier_allowance,
            work,
            cancel,
        )?;
        if !error.is_finite()
            || error < 0.0
            || parametric::up(error + trim_error) > MESH_CARRIER_TOLERANCE_MM
        {
            return Err(shape(format!(
                "source face {ordinal} whole triangle {triangle_index} carrier={error} trim={trim_error} exceeds display tolerance"
            )));
        }
        carrier_error = carrier_error.max(error);
        for &index in &triangle {
            let i = index as usize;
            if !used[i] {
                if let Some(junction) = singular.at(index) {
                    // The candidate's singular fallback is not authority. The
                    // source-owned, one-sided same-law field supplies this normal.
                    normals[i] = junction.normal.map(|n| n * sense);
                } else {
                    let q = mesh.uvs[i];
                    let enclosure = parametric::surface_geometric_normal_enclosure(
                        surface,
                        [[q.x, q.x], [q.y, q.y]],
                        work,
                        cancel,
                    )?;
                    let expected = enclosure.map(|v| v.mul(Interval::point(sense)));
                    let n = mesh.normals[i];
                    let n = [n.x, n.y, n.z];
                    if n.iter().any(|v| !v.is_finite())
                        || (length(n) - 1.0).abs() > NORMAL_NORM_TOLERANCE
                    {
                        return Err(GeometryError::new(
                            GeometryErrorCode::SingularGeometry,
                            "native display vertex has no finite regular unit normal",
                        ));
                    }
                    let difference =
                        std::array::from_fn(|a| expected[a].sub(Interval::point(n[a])));
                    if norm_upper(difference) > NORMAL_NORM_TOLERANCE {
                        return Err(invalid(
                            "native candidate normal disagrees with stored carrier coefficients and outward topology",
                        ));
                    }
                    normals[i] = n;
                }
                used[i] = true;
            }
            let winding = (0..3).fold(Interval::point(0.0), |sum, k| {
                sum.add(area[k].mul(Interval::point(normals[i][k])))
            });
            if winding.lo <= 0.0 {
                return Err(invalid(
                    "native display triangle is degenerate, reversed or uncertain against its audited outward normals",
                ));
            }
        }
    }
    if used.iter().any(|used| !used) {
        return Err(invalid(
            "native face snapshot retained vertices without an audited triangle",
        ));
    }
    Ok(AuditedFace {
        positions,
        normals,
        carrier_error,
        trim_error,
    })
}

pub(super) fn triangle_uv(
    mesh: &FaceMesh,
    triangle: [u32; 3],
) -> Result<[[f64; 2]; 3], GeometryError> {
    let mut uv = [[0.0; 2]; 3];
    for (slot, index) in triangle.into_iter().enumerate() {
        let q = mesh
            .uvs
            .get(index as usize)
            .ok_or_else(|| invalid("native triangle index is outside its face UV table"))?;
        if !q.x.is_finite() || !q.y.is_finite() {
            return Err(invalid("native display has nonfinite UV"));
        }
        uv[slot] = [q.x, q.y];
    }
    Ok(uv)
}
pub(super) fn uv_box(uv: [[f64; 2]; 3]) -> [[f64; 2]; 2] {
    std::array::from_fn(|a| {
        [
            uv.iter().map(|p| p[a]).fold(f64::INFINITY, f64::min),
            uv.iter().map(|p| p[a]).fold(f64::NEG_INFINITY, f64::max),
        ]
    })
}
pub(super) fn norm_upper(v: [Interval; 3]) -> f64 {
    v.into_iter()
        .fold(Interval::point(0.0), |sum, v| sum.add(v.square()))
        .sqrt()
        .map_or(f64::INFINITY, |n| n.hi)
}
fn outward_sense(brep: &Brep, face: FaceId) -> Result<f64, GeometryError> {
    let primary = brep
        .face(face)
        .map_err(super::native_error)?
        .faceuse
        .ok_or_else(|| invalid("native display face has no faceuse"))?;
    let use_ = brep.faceuse(primary).map_err(super::native_error)?;
    let sign = |orientation| match orientation {
        OrientType::Same => Ok(1.0),
        OrientType::Opposite => Ok(-1.0),
        _ => Err(invalid("native display faceuse orientation is unresolved")),
    };
    let void = |fu: &solidcore::topology::Faceuse| -> Result<Option<i32>, GeometryError> {
        let Some(shell) = fu.shell else {
            return Ok(None);
        };
        let Some(region) = brep.shell(shell).map_err(super::native_error)?.region else {
            return Ok(None);
        };
        let value = brep.region(region).map_err(super::native_error)?.is_void;
        if value != 0 && value != 1 {
            return Err(invalid("native display region side is unresolved"));
        }
        Ok(Some(value))
    };
    if let Some(mate) = use_.mate {
        let mate = brep.faceuse(mate).map_err(super::native_error)?;
        match (void(use_)?, void(mate)?) {
            (Some(0), Some(1)) => return sign(mate.orientation),
            (Some(1), Some(0)) => return sign(use_.orientation),
            _ => {}
        }
    }
    sign(use_.orientation)
}
#[cfg(test)]
pub(super) fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
fn length(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

fn box_inventory(
    mesh: &FaceMesh,
    outside: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(Vec<u32>, Vec<DisplayMeshBox>), GeometryError> {
    let count = mesh.triangles.len();
    check_memory(
        outside
            .saturating_add(count.saturating_mul(size_of::<([u64; 4], u32)>() + size_of::<u32>())),
    )?;
    let mut keys = Vec::new();
    let mut ids = Vec::new();
    keys.try_reserve_exact(count)
        .map_err(|_| resource("native UV-box key allocation failed"))?;
    ids.try_reserve_exact(count)
        .map_err(|_| resource("native UV-box index allocation failed"))?;
    for (index, &triangle) in mesh.triangles.iter().enumerate() {
        work.charge(1, cancel)?;
        let d = uv_box(triangle_uv(mesh, triangle)?);
        keys.push((
            DisplayMeshBox::key_for(d),
            u32::try_from(index).map_err(|_| resource("native UV-box ordinal overflow"))?,
        ));
        ids.push(0);
    }
    work.charge(
        (count as u64).saturating_mul((count.max(1).ilog2() + 1) as u64),
        cancel,
    )?;
    keys.sort_unstable_by_key(|entry| entry.0);
    let unique = keys
        .iter()
        .enumerate()
        .filter(|(index, entry)| *index == 0 || keys[*index - 1].0 != entry.0)
        .count();
    let mut boxes = Vec::new();
    check_memory(
        outside
            .saturating_add(keys.capacity().saturating_mul(size_of::<([u64; 4], u32)>()))
            .saturating_add(ids.capacity().saturating_mul(size_of::<u32>()))
            .saturating_add(unique.saturating_mul(size_of::<DisplayMeshBox>())),
    )?;
    boxes
        .try_reserve_exact(unique)
        .map_err(|_| resource("native exact UV-box proof allocation failed"))?;
    let mut previous = None;
    for (key, index) in keys {
        work.charge(1, cancel)?;
        if previous != Some(key) {
            boxes.push(DisplayMeshBox::new(key));
            previous = Some(key);
        }
        ids[index as usize] =
            u32::try_from(boxes.len() - 1).map_err(|_| resource("native UV-box slot overflow"))?;
    }
    Ok((ids, boxes))
}
