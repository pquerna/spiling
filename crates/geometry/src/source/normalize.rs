// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use std::collections::{BTreeMap, HashMap, VecDeque};
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
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
fn normal(p: &[[f64; 3]], t: [u32; 3]) -> Option<[f64; 3]> {
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
) -> Result<(SurfaceComplex, usize), GeometryError> {
    mesh.validate().map_err(|e| invalid(e.to_string()))?;
    if !scale.is_finite() || scale <= 0. {
        return Err(invalid("invalid interpreted mesh units"));
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
            return Err(invalid("normalized mesh is not finite"));
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
    let mut triangles = Vec::with_capacity(mesh.triangles.len());
    let mut unique = HashMap::<[u32; 3], [u32; 3]>::new();
    let mut duplicates = 0usize;
    for (i, t) in mesh.triangles.into_iter().enumerate() {
        if i % 1024 == 0 {
            crate::check_cancel(cancel)?;
        }
        let mut t = t.map(|v| remap[v as usize]);
        if transform.determinant() < 0. {
            t.swap(1, 2);
        }
        if normal(&positions, t).is_none() {
            return Err(invalid(
                "mesh contains a degenerate triangle; explicit repair is required",
            ));
        }
        let mut key = t;
        key.sort_unstable();
        if let Some(previous) = unique.get(&key) {
            if (0..3).any(|offset| (0..3).all(|a| t[a] == previous[(a + offset) % 3])) {
                duplicates += 1;
                continue;
            }
            return Err(invalid(
                "oppositely wound duplicate facets have ambiguous material; explicit repair is required",
            ));
        }
        unique.insert(key, t);
        triangles.push(t);
    }
    drop(remap);
    drop(unique);
    // Adjacency is shared by orientation, segmentation and boundary checks.
    let mut edges = HashMap::<(u32, u32), Vec<(usize, bool)>>::new();
    for (i, t) in triangles.iter().enumerate() {
        for a in 0..3 {
            let (v, w) = (t[a], t[(a + 1) % 3]);
            edges
                .entry((v.min(w), v.max(w)))
                .or_default()
                .push((i, v < w));
        }
    }
    if edges.values().any(|uses| uses.len() != 2) {
        return Err(invalid(
            "mesh has open or nonmanifold edges; material closure is unresolved",
        ));
    }
    let mut adjacency = vec![Vec::new(); triangles.len()];
    for uses in edges.values() {
        let [(a, forward_a), (b, forward_b)] = uses[..] else {
            unreachable!()
        };
        adjacency[a].push((b, forward_a == forward_b));
        adjacency[b].push((a, forward_a == forward_b));
    }
    drop(edges);
    for neighbours in &mut adjacency {
        neighbours.sort_unstable();
    }
    let mut flips = vec![None; triangles.len()];
    let mut shells = Vec::<Vec<usize>>::new();
    for first in 0..triangles.len() {
        if flips[first].is_some() {
            continue;
        }
        let mut shell = Vec::new();
        let mut queue = VecDeque::from([first]);
        flips[first] = Some(false);
        while let Some(i) = queue.pop_front() {
            crate::check_cancel(cancel)?;
            shell.push(i);
            for &(j, opposite) in &adjacency[i] {
                let expected = flips[i].unwrap() ^ opposite;
                match flips[j] {
                    None => {
                        flips[j] = Some(expected);
                        queue.push_back(j);
                    }
                    Some(old) if old != expected => return Err(invalid("mesh is non-orientable")),
                    _ => {}
                }
            }
        }
        shells.push(shell);
    }
    for (t, flip) in triangles.iter_mut().zip(flips) {
        if flip.unwrap() {
            t.swap(1, 2);
        }
    }
    // Winding correction preserves nested void shells. The representative is
    // an existing vertex, not a centroid that might lie outside a concave shell.
    for shell in &shells {
        let point = positions[triangles[shell[0]][0] as usize];
        let mut depth = 0;
        for other in &shells {
            if std::ptr::eq(shell, other) {
                continue;
            }
            let mut angle = 0.;
            for &i in other {
                let p = triangles[i].map(|v| sub(positions[v as usize], point));
                let n = p.map(|v| dot(v, v).sqrt());
                if n.contains(&0.) {
                    return Err(invalid(
                        "mesh shells share a vertex; material interpretation is ambiguous",
                    ));
                }
                angle += 2.
                    * dot(p[0], cross(p[1], p[2])).atan2(
                        n[0] * n[1] * n[2]
                            + dot(p[0], p[1]) * n[2]
                            + dot(p[1], p[2]) * n[0]
                            + dot(p[2], p[0]) * n[1],
                    );
            }
            if angle.abs() > std::f64::consts::TAU {
                depth += 1;
            }
        }
        let origin = point;
        let volume: f64 = shell
            .iter()
            .map(|&i| {
                let p = triangles[i].map(|v| sub(positions[v as usize], origin));
                dot(p[0], cross(p[1], p[2]))
            })
            .sum();
        if !volume.is_finite() || volume == 0. {
            return Err(invalid("mesh shell has no resolved volume"));
        }
        if (volume > 0.) != (depth % 2 == 0) {
            for &i in shell {
                triangles[i].swap(1, 2);
            }
        }
    }
    let normals: Vec<_> = triangles
        .iter()
        .map(|&t| normal(&positions, t).unwrap())
        .collect();
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
    result.error_mm = result.measured_deviation()?;
    result.validate()?;
    Ok((result, duplicates))
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
