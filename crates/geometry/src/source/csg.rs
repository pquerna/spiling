// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Mesh material composition, confined to import. The result replaces the
//! operands as the one retained source; no Boolean expression leaks to slicing.
use super::*;
use boolmesh::prelude::{Manifold, OpType, compute_boolean};
use spiling_model_import::Mesh;

pub(super) fn compose(
    left: Mesh,
    right: Mesh,
    subtract: bool,
    cancel: &AtomicBool,
) -> Result<Mesh, GeometryError> {
    crate::check_cancel(cancel)?;
    let result = std::panic::catch_unwind(|| {
        let (a, b) = (operand(&left)?, operand(&right)?);
        let policy = TolerantPolicy::default();
        if !a.eps.is_finite()
            || !b.eps.is_finite()
            || a.eps < 0.
            || b.eps < 0.
            || a.eps.max(b.eps) > policy.profile_mm
        {
            return Err(invalid(
                "mesh composition precision exceeds the source tolerance",
            ));
        }
        let output = compute_boolean(
            &a,
            &b,
            if subtract {
                OpType::Subtract
            } else {
                OpType::Add
            },
        )
        .map_err(|e| invalid(format!("mesh material composition: {e}")))?;
        if !output.is_manifold() || output.nf == 0 {
            return Err(invalid("mesh composition has empty or unresolved material"));
        }
        let vertices = output.ps.iter().map(|v| [v.x, v.y, v.z]).collect();
        let triangles = output
            .hs
            .as_chunks::<3>()
            .0
            .iter()
            .map(|row| {
                let index = |i: usize| {
                    u32::try_from(row[i].tail)
                        .map_err(|_| invalid("composed source index overflow"))
                };
                Ok([index(0)?, index(1)?, index(2)?])
            })
            .collect::<Result<_, _>>()?;
        let mesh = Mesh {
            vertices,
            triangles,
        };
        mesh.validate().map_err(|e| invalid(e.to_string()))?;
        Ok(mesh)
    })
    .map_err(|_| invalid("mesh material composition could not resolve this source"))?;
    crate::check_cancel(cancel)?;
    result
}

fn operand(mesh: &Mesh) -> Result<Manifold, GeometryError> {
    Manifold::new(
        mesh.vertices.as_flattened(),
        &mesh
            .triangles
            .iter()
            .flatten()
            .map(|&v| v as usize)
            .collect::<Vec<_>>(),
    )
    .map_err(invalid)
}
/// Full boundary composition decides nesting; a centroid or vertex sample does
/// not determine whether a concave shell crosses another component.
pub(super) fn contains(
    outer: &Mesh,
    inner: &Mesh,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    crate::check_cancel(cancel)?;
    let result = std::panic::catch_unwind(|| {
        let a = operand(inner)?;
        let b = operand(outer)?;
        let intersection = compute_boolean(&a, &b, OpType::Intersect).map_err(invalid)?;
        let metrics = |m: &Manifold| {
            let origin = m.ps[0];
            let mut volume = 0.;
            let mut area = 0.;
            for t in m.hs.as_chunks::<3>().0.iter() {
                let p: [_; 3] = std::array::from_fn(|i| m.ps[t[i].tail] - origin);
                volume += p[0].dot(p[1].cross(p[2])) / 6.;
                area += (p[1] - p[0]).cross(p[2] - p[0]).length() * 0.5;
            }
            (volume, area)
        };
        let (v, area) = metrics(&a);
        let (clipped, clipped_area) = metrics(&intersection);
        // Integral roundoff at the composer's own precision, not a widened
        // material tolerance or a sampled containment claim. Strict bounds
        // and a complete closed Boolean intersection precede this comparison.
        let roundoff = 16. * a.eps.max(b.eps) * (area + clipped_area);
        if a.eps.max(b.eps) > TolerantPolicy::default().profile_mm {
            return Err(invalid("component nesting exceeds source tolerance"));
        }
        Ok((v - clipped).abs() <= roundoff)
    })
    .map_err(|_| invalid("mesh nesting could not resolve component containment"))?;
    crate::check_cancel(cancel)?;
    result
}
