// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use std::collections::{BTreeMap, HashMap, VecDeque};
pub(super) fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
pub(super) fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
pub(super) fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn unit(p: [f64; 3]) -> Option<[f64; 3]> {
    let n = dot(p, p).sqrt();
    (n.is_finite() && n > 0.).then(|| p.map(|v| v / n))
}
pub(super) fn normal(p: &[[f64; 3]], t: [u32; 3]) -> Option<[f64; 3]> {
    unit(cross(
        sub(p[t[1] as usize], p[t[0] as usize]),
        sub(p[t[2] as usize], p[t[0] as usize]),
    ))
}
pub(super) fn build(
    mesh: spiling_model_import::Mesh,
    scale: f64,
    transform: spiling_model_import::Affine,
    cancel: &AtomicBool,
) -> Result<(SurfaceComplex, Vec<ImportFinding>), GeometryError> {
    build_report(mesh, scale, transform, cancel).map_err(|rejected| rejected.error)
}
pub(super) fn build_report(
    mesh: spiling_model_import::Mesh,
    scale: f64,
    transform: spiling_model_import::Affine,
    cancel: &AtomicBool,
) -> Result<(SurfaceComplex, Vec<ImportFinding>), RejectedSource> {
    mesh.validate().map_err(|e| invalid(e.to_string()))?;
    if !scale.is_finite() || scale <= 0. {
        return Err(invalid("invalid interpreted mesh units").into());
    }
    transform.validate().map_err(|e| invalid(e.to_string()))?;
    let mut referenced = vec![false; mesh.vertices.len()];
    for &v in mesh.triangles.iter().flatten() {
        referenced[v as usize] = true;
    }
    let mut positions = Vec::new();
    let mut weld = HashMap::new();
    let mut remap = Vec::with_capacity(mesh.vertices.len());
    for (i, p) in mesh.vertices.into_iter().enumerate() {
        if i % 1024 == 0 {
            crate::check_cancel(cancel)?;
        }
        if !referenced[i] {
            remap.push(u32::MAX);
            continue;
        }
        let p = transform.apply(p.map(|v| v * scale));
        if p.iter().any(|v| !v.is_finite()) {
            return Err(invalid("normalized mesh is not finite").into());
        }
        let key = p.map(|v| if v == 0. { 0 } else { v.to_bits() });
        let id = match weld.get(&key) {
            Some(&id) => id,
            None => {
                let id = u32::try_from(positions.len())
                    .map_err(|_| invalid("mesh index representation overflow"))?;
                positions.push(p);
                weld.insert(key, id);
                id
            }
        };
        remap.push(id);
    }
    drop(weld);
    let triangles = mesh
        .triangles
        .into_iter()
        .map(|t| {
            let mut t = t.map(|v| remap[v as usize]);
            if transform.determinant() < 0. {
                t.swap(1, 2);
            }
            t
        })
        .collect();
    let mut mesh = spiling_model_import::Mesh {
        vertices: positions,
        triangles,
    };
    let (adjacency, findings) = match super::topology::prepare(&mut mesh, cancel) {
        Ok(value) => value,
        Err(error) => return Err(RejectedSource::material(mesh, error, cancel)),
    };
    if mesh
        .triangles
        .iter()
        .any(|&t| normal(&mesh.vertices, t).is_none())
    {
        return Err(RejectedSource::material(
            mesh,
            invalid("boundary joins or composition left a zero-area material facet"),
            cancel,
        ));
    }
    let positions = mesh.vertices;
    let triangles = mesh.triangles;
    let normals: Vec<_> = triangles
        .iter()
        .map(|&t| {
            normal(&positions, t).ok_or_else(|| {
                invalid("boundary joins or composition left a zero-area material facet")
            })
        })
        .collect::<Result<_, _>>()?;
    // Recover connected planes before smooth grouping: a fillet must not
    // swallow the flat plate it touches. Broad planar patches dominate their
    // adjacent smooth facets; equal-sized strips remain eligible for a shared
    // cylinder. This ranking proposes regions only, never admits a fit.
    let mut plane_labels = vec![usize::MAX; triangles.len()];
    let mut plane_groups: Vec<Vec<usize>> = Vec::new();
    for first in 0..triangles.len() {
        if plane_labels[first] != usize::MAX {
            continue;
        }
        let label = plane_groups.len();
        let origin = positions[triangles[first][0] as usize];
        let n = normals[first];
        let roundoff =
            128. * f64::EPSILON * origin.iter().copied().map(f64::abs).fold(1., f64::max);
        let mut group = Vec::new();
        let mut queue = VecDeque::from([first]);
        plane_labels[first] = label;
        while let Some(i) = queue.pop_front() {
            crate::check_cancel(cancel)?;
            group.push(i);
            for &(j, _) in &adjacency[i] {
                if plane_labels[j] == usize::MAX
                    && roundoff <= 0.1 * TolerantPolicy::default().profile_mm
                    && dot(n, normals[j]) > 1. - 128. * f64::EPSILON
                    && triangles[j]
                        .iter()
                        .all(|&v| dot(n, sub(positions[v as usize], origin)).abs() <= roundoff)
                {
                    plane_labels[j] = label;
                    queue.push_back(j);
                }
            }
        }
        plane_groups.push(group);
    }
    let areas: Vec<f64> = plane_groups
        .iter()
        .map(|g| {
            g.iter()
                .map(|&i| {
                    let p = triangles[i].map(|v| positions[v as usize]);
                    let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
                    dot(n, n).sqrt() / 2.
                })
                .sum()
        })
        .collect();
    let protected: Vec<bool> = plane_groups
        .iter()
        .enumerate()
        .map(|(label, group)| {
            let plane_labels = &plane_labels;
            let normals = &normals;
            let neighbour = group
                .iter()
                .flat_map(|&i| {
                    adjacency[i].iter().filter_map(move |&(j, _)| {
                        (plane_labels[j] != label
                            && dot(normals[i], normals[j]) > 0.8660254037844386)
                            .then_some(plane_labels[j])
                    })
                })
                .map(|j| areas[j])
                .fold(0., f64::max);
            neighbour == 0. || areas[label] > 2. * neighbour
        })
        .collect();
    let protected_labels: Vec<_> = plane_labels
        .iter()
        .map(|&label| protected[label].then_some(label))
        .collect();
    drop(plane_groups);
    drop(plane_labels);
    let mut regions = vec![u32::MAX; triangles.len()];
    let mut groups = Vec::<Vec<usize>>::new();
    // Connected near-smooth groups bound cylinder proposals; sharp edges
    // separate caps from walls. Failed fits remain one faceted region.
    for first in 0..triangles.len() {
        if regions[first] != u32::MAX {
            continue;
        }
        let region = u32::try_from(groups.len())
            .map_err(|_| invalid("mesh region representation overflow"))?;
        let mut group = Vec::new();
        let mut queue = VecDeque::from([first]);
        regions[first] = region;
        while let Some(i) = queue.pop_front() {
            crate::check_cancel(cancel)?;
            group.push(i);
            for &(j, _) in &adjacency[i] {
                if regions[j] == u32::MAX
                    && protected_labels[i] == protected_labels[j]
                    && dot(normals[i], normals[j]) > 0.8660254037844386
                {
                    regions[j] = region;
                    queue.push_back(j);
                }
            }
        }
        group.sort_unstable();
        groups.push(group);
    }
    let mut descriptions = Vec::new();
    for group in &groups {
        crate::check_cancel(cancel)?;
        let first = group[0];
        let origin = positions[triangles[first][0] as usize];
        let n = normals[first];
        let magnitude = origin.iter().copied().map(f64::abs).fold(1., f64::max);
        let roundoff = 128. * f64::EPSILON * magnitude;
        let planar = roundoff <= 0.1 * TolerantPolicy::default().profile_mm
            && group.iter().all(|&i| {
                dot(n, normals[i]) > 1. - 128. * f64::EPSILON
                    && triangles[i]
                        .iter()
                        .all(|&v| dot(n, sub(positions[v as usize], origin)).abs() <= roundoff)
            });
        if planar {
            descriptions.push(Some(AnalyticSurfaceDescription::Plane {
                origin_mm: origin,
                normal: n,
            }));
        } else if let Some((cylinder, _)) = cylinder(&positions, &triangles, &normals, group) {
            descriptions.push(Some(cylinder));
        } else {
            descriptions.push(None);
        }
    }
    // Curved recovery only adopts a patch whose every trim joins an adopted
    // plane or the same cylinder. Difficult fillet transitions stay faceted.
    for (r, group) in groups.iter().enumerate() {
        if matches!(
            descriptions[r],
            Some(AnalyticSurfaceDescription::Cylinder { .. })
        ) && group.iter().any(|&i| {
            adjacency[i].iter().any(|&(j, _)| {
                regions[j] as usize != r
                    && !matches!(
                        descriptions[regions[j] as usize],
                        Some(AnalyticSurfaceDescription::Plane { .. })
                    )
            })
        }) {
            descriptions[r] = None;
        }
    }
    let mut faces = Vec::new();
    for (r, analytic) in descriptions.into_iter().enumerate() {
        let family = analytic
            .as_ref()
            .map_or(SurfaceFamily::Faceted, AnalyticSurfaceDescription::family);
        faces.push(FaceInfo {
            face_id: FaceId::from_mesh_region((r + 1) as u32)?,
            surface_face_entity: (r + 1).to_string(),
            source_entity_kind: "MESH_REGION".into(),
            source_surface_entity_kind: if analytic.is_some() {
                "RECOVERED_SURFACE"
            } else {
                "FACETED_SURFACE"
            }
            .into(),
            orientation: match &analytic {
                Some(AnalyticSurfaceDescription::Cylinder {
                    axis_origin_mm,
                    axis_direction,
                    ..
                }) => {
                    let first = groups[r][0];
                    let d = sub(positions[triangles[first][0] as usize], *axis_origin_mm);
                    dot(
                        sub(d, axis_direction.map(|v| v * dot(d, *axis_direction))),
                        normals[first],
                    ) > 0.
                }
                _ => true,
            },
            geometry: FaceGeometryInfo::new(
                family,
                analytic,
                FaceDomainSummary::new(0, 0, false, false, false)?,
            )?,
        });
    }
    let mut result = SurfaceComplex {
        positions,
        triangles,
        regions,
        faces,
        error_mm: 0.,
    };
    let repair_displacement = findings
        .iter()
        .filter(|f| f.code == ImportFindingCode::BoundaryWeld)
        .filter_map(|f| f.displacement_mm)
        .fold(0_f64, f64::max);
    result.error_mm = result.measured_deviation()? + repair_displacement;
    result.validate()?;
    Ok((result, findings))
}
fn cylinder(
    p: &[[f64; 3]],
    t: &[[u32; 3]],
    normals: &[[f64; 3]],
    group: &[usize],
) -> Option<(AnalyticSurfaceDescription, f64)> {
    let n = normals[group[0]];
    let axis = group
        .iter()
        .filter_map(|&i| unit(cross(n, normals[i])))
        .find(|a| group.iter().all(|&i| dot(*a, normals[i]).abs() < 1e-6))?;
    if axis[0].abs() > 1e-10 || axis[1].abs() > 1e-10 {
        return None;
    }
    let u = n;
    let v = cross(axis, u);
    let origin = p[t[group[0]][0] as usize];
    let mut unique = BTreeMap::new();
    for &i in group {
        for &id in &t[i] {
            unique.insert(id, p[id as usize]);
        }
    }
    if unique.len() < 6 {
        return None;
    }
    let mut a = [[0.; 4]; 3];
    for point in unique.values() {
        let d = sub(*point, origin);
        let (x, y) = (dot(d, u), dot(d, v));
        let row = [2. * x, 2. * y, 1.];
        let rhs = x * x + y * y;
        for r in 0..3 {
            for c in 0..3 {
                a[r][c] += row[r] * row[c];
            }
            a[r][3] += row[r] * rhs;
        }
    }
    for c in 0..3 {
        let pivot = (c..3).max_by(|&r, &s| a[r][c].abs().total_cmp(&a[s][c].abs()))?;
        a.swap(c, pivot);
        let scale = a[c][c];
        if scale.abs() < 1e-16 {
            return None;
        }
        for value in &mut a[c][c..] {
            *value /= scale;
        }
        for r in 0..3 {
            if r == c {
                continue;
            }
            let m = a[r][c];
            let pivot = a[c];
            for (value, source) in a[r][c..].iter_mut().zip(&pivot[c..]) {
                *value -= m * source;
            }
        }
    }
    let center = std::array::from_fn(|i| origin[i] + u[i] * a[0][3] + v[i] * a[1][3]);
    let radius = (a[2][3] + a[0][3].powi(2) + a[1][3].powi(2)).sqrt();
    if !radius.is_finite() || radius <= 0. {
        return None;
    }
    let surface = AnalyticSurfaceDescription::Cylinder {
        axis_origin_mm: center,
        axis_direction: axis,
        radius_mm: radius,
    };
    let error = group
        .iter()
        .try_fold(0_f64, |maximum, &i| {
            super::deviation::triangle(&surface, t[i].map(|v| p[v as usize]))
                .map(|error| maximum.max(error))
        })
        .ok()?;
    if error > 0.5 * TolerantPolicy::default().profile_mm {
        return None;
    }
    Some((
        AnalyticSurfaceDescription::Cylinder {
            axis_origin_mm: center,
            axis_direction: axis,
            radius_mm: radius,
        },
        error.next_up(),
    ))
}
