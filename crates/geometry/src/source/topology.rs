// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{
    normalize::{cross, dot, normal, sub},
    *,
};
use spiling_model_import::Mesh;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
type EdgeUses = BTreeMap<(u32, u32), Vec<(usize, bool)>>;
pub(super) type Adjacency = Vec<Vec<(usize, bool)>>;

fn edges(triangles: &[[u32; 3]], cancel: &AtomicBool) -> Result<EdgeUses, GeometryError> {
    let mut edges = EdgeUses::new();
    for (i, t) in triangles.iter().enumerate() {
        if i % 1024 == 0 {
            crate::check_cancel(cancel)?;
        }
        for a in 0..3 {
            let (v, w) = (t[a], t[(a + 1) % 3]);
            edges
                .entry((v.min(w), v.max(w)))
                .or_default()
                .push((i, v < w));
        }
    }
    Ok(edges)
}
fn adjacency(
    triangles: &[[u32; 3]],
    partition: bool,
    cancel: &AtomicBool,
) -> Result<Adjacency, GeometryError> {
    let mut result = vec![Vec::new(); triangles.len()];
    for uses in edges(triangles, cancel)?.values() {
        let pairs: Vec<_> = if uses.len() == 2 {
            vec![(uses[0], uses[1])]
        } else if partition && uses.len().is_multiple_of(2) {
            let a: Vec<_> = uses.iter().copied().filter(|u| u.1).collect();
            let b: Vec<_> = uses.iter().copied().filter(|u| !u.1).collect();
            if a.len() != b.len() {
                return Err(invalid(
                    "nonmanifold source has unresolved edge orientation",
                ));
            }
            // Stable candidate pairing preserves exported component order. Each
            // proposed shell is checked independently below before composition.
            a.into_iter().zip(b).collect()
        } else {
            return Err(invalid(
                "mesh has open or nonmanifold edges; material closure is unresolved",
            ));
        };
        for ((a, fa), (b, fb)) in pairs {
            result[a].push((b, fa == fb));
            result[b].push((a, fa == fb));
        }
    }
    for neighbours in &mut result {
        neighbours.sort_unstable();
    }
    Ok(result)
}
fn subset(positions: &[[f64; 3]], triangles: &[[u32; 3]], group: &[usize]) -> Mesh {
    let mut map = BTreeMap::new();
    let mut mesh = Mesh::default();
    for &i in group {
        mesh.triangles.push(triangles[i].map(|id| {
            *map.entry(id).or_insert_with(|| {
                let next = mesh.vertices.len() as u32;
                mesh.vertices.push(positions[id as usize]);
                next
            })
        }));
    }
    mesh
}
/// Exact-coordinate topology first; bounded crack closure only at an open
/// boundary, with mutually unique partner vertices and full closure afterward.
fn weld_boundary(
    mesh: &mut Mesh,
    findings: &mut Vec<ImportFinding>,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let boundary: BTreeSet<_> = edges(&mesh.triangles, cancel)?
        .into_iter()
        .filter(|(_, uses)| uses.len() == 1)
        .flat_map(|((a, b), _)| [a, b])
        .collect();
    if boundary.is_empty() {
        return Ok(());
    }
    let tolerance = TolerantPolicy::default().join_mm;
    let mut bins = BTreeMap::<[i64; 3], Vec<u32>>::new();
    let mut partners = BTreeMap::new();
    let key = |p: [f64; 3]| p.map(|v| (v / tolerance).floor() as i64);
    for &v in &boundary {
        bins.entry(key(mesh.vertices[v as usize]))
            .or_default()
            .push(v);
    }
    for &a in &boundary {
        crate::check_cancel(cancel)?;
        let p = mesh.vertices[a as usize];
        let cell = key(p);
        let mut candidates = Vec::new();
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    let Some(cx) = cell[0].checked_add(x) else {
                        continue;
                    };
                    let Some(cy) = cell[1].checked_add(y) else {
                        continue;
                    };
                    let Some(cz) = cell[2].checked_add(z) else {
                        continue;
                    };
                    for &b in bins.get(&[cx, cy, cz]).into_iter().flatten() {
                        let d = sub(p, mesh.vertices[b as usize]);
                        if a != b && dot(d, d).sqrt() <= tolerance {
                            candidates.push(b);
                        }
                    }
                }
            }
        }
        if candidates.len() == 1 {
            partners.insert(a, candidates[0]);
        }
    }
    let mut changed = Vec::new();
    let mut remap = BTreeMap::new();
    let mut maximum = 0_f64;
    for (&a, &b) in &partners {
        if a < b && partners.get(&b) == Some(&a) {
            let pa = mesh.vertices[a as usize];
            let pb = mesh.vertices[b as usize];
            let d = sub(pa, pb);
            maximum = maximum.max(dot(d, d).sqrt());
            changed.extend([pa, pb]);
            remap.insert(b, a);
        }
    }
    for t in &mut mesh.triangles {
        for v in t {
            if let Some(&a) = remap.get(v) {
                *v = a;
            }
        }
    }
    if !changed.is_empty() {
        findings.push(finding(ImportFindingKind::Changed, ImportFindingCode::BoundaryWeld,
            (changed.len()/2) as u32, "Boundary joins", "Paired open-boundary vertices joined within the source join tolerance; resulting material closure checked.", Some(point_bounds(&changed)?), Some(maximum)));
    }
    Ok(())
}
pub(super) fn prepare(
    mesh: &mut Mesh,
    cancel: &AtomicBool,
) -> Result<(Adjacency, Vec<ImportFinding>), GeometryError> {
    let mut findings = Vec::new();
    let mut removed = Vec::new();
    mesh.triangles.retain(|&t| {
        if normal(&mesh.vertices, t).is_none() {
            removed.extend(t.map(|v| mesh.vertices[v as usize]));
            false
        } else {
            true
        }
    });
    if !removed.is_empty() {
        findings.push(finding(
            ImportFindingKind::Changed,
            ImportFindingCode::DegenerateFacets,
            (removed.len() / 3) as u32,
            "Zero-area facets",
            "Removed facets with no material area; surviving material closure checked.",
            Some(point_bounds(&removed)?),
            Some(0.),
        ));
    }
    if mesh.triangles.is_empty() {
        return Err(invalid("source has no material facets"));
    }
    // An odd multiplicity cannot partition into closed sheets. Deduplicate only
    // this case; even coincident sheets must retain component identity first.
    let incidence = edges(&mesh.triangles, cancel)?;
    if incidence.values().any(|u| !u.len().is_multiple_of(2)) {
        let mut unique = BTreeMap::<[u32; 3], usize>::new();
        let mut removed = Vec::new();
        mesh.triangles.retain(|t| {
            let min = (0..3).min_by_key(|&i| t[i]).unwrap();
            let key = [t[min], t[(min + 1) % 3], t[(min + 2) % 3]];
            let sheets = (0..3)
                .map(|a| {
                    let v = t[a];
                    let w = t[(a + 1) % 3];
                    incidence[&(v.min(w), v.max(w))].len() / 2
                })
                .min()
                .unwrap()
                .max(1);
            let count = unique.entry(key).or_default();
            *count += 1;
            if *count <= sheets {
                true
            } else {
                removed.extend(t.map(|v| mesh.vertices[v as usize]));
                false
            }
        });
        if !removed.is_empty() {
            findings.push(finding(
                ImportFindingKind::Changed,
                ImportFindingCode::DuplicateFacets,
                (removed.len() / 3) as u32,
                "Repeated facets",
                "Removed identically wound repeated facets; component material closure checked.",
                Some(point_bounds(&removed)?),
                Some(0.),
            ));
        }
    }
    drop(incidence);
    weld_boundary(mesh, &mut findings, cancel)?;
    let graph = adjacency(&mesh.triangles, true, cancel)?;
    let mut flips = vec![None; mesh.triangles.len()];
    let mut groups = Vec::new();
    for first in 0..mesh.triangles.len() {
        if flips[first].is_some() {
            continue;
        }
        let mut group = Vec::new();
        let mut queue = VecDeque::from([first]);
        flips[first] = Some(false);
        while let Some(i) = queue.pop_front() {
            crate::check_cancel(cancel)?;
            group.push(i);
            for &(j, opposite) in &graph[i] {
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
        let shell = subset(&mesh.vertices, &mesh.triangles, &group);
        adjacency(&shell.triangles, false, cancel)?; // a pairing proposal alone never admits a shell
        groups.push(group);
    }
    let mut changed = Vec::new();
    for (t, flip) in mesh.triangles.iter_mut().zip(flips) {
        if flip.unwrap() {
            changed.extend(t.map(|v| mesh.vertices[v as usize]));
            t.swap(1, 2);
        }
    }
    // Strict nesting preserves enclosed cavities. Coincident boundary contact
    // belongs to component union, not an invented interior void.
    let vertices: Vec<BTreeSet<u32>> = groups
        .iter()
        .map(|g| g.iter().flat_map(|&i| mesh.triangles[i]).collect())
        .collect();
    let bounds: Vec<_> = vertices
        .iter()
        .map(|ids| {
            point_bounds(
                &ids.iter()
                    .map(|&v| mesh.vertices[v as usize])
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Result<_, _>>()?;
    let mut depths = vec![0; groups.len()];
    for (i, group) in groups.iter().enumerate() {
        let point = mesh.vertices[mesh.triangles[group[0]][0] as usize];
        for (j, other) in groups.iter().enumerate() {
            crate::check_cancel(cancel)?;
            if i == j || !vertices[i].is_disjoint(&vertices[j]) {
                continue;
            }
            if !(0..3)
                .all(|a| bounds[j].min[a] < bounds[i].min[a] && bounds[i].max[a] < bounds[j].max[a])
            {
                continue;
            }
            // Outside vertices refute nesting; positive screens still need
            // complete closed-boundary intersection. Avoid empty operands.
            let mut angle = 0.;
            for &t in other {
                let p = mesh.triangles[t].map(|v| sub(mesh.vertices[v as usize], point));
                let n = p.map(|v| dot(v, v).sqrt());
                angle += 2.
                    * dot(p[0], cross(p[1], p[2])).atan2(
                        n[0] * n[1] * n[2]
                            + dot(p[0], p[1]) * n[2]
                            + dot(p[1], p[2]) * n[0]
                            + dot(p[2], p[0]) * n[1],
                    );
            }
            if angle.abs() <= std::f64::consts::TAU {
                continue;
            }
            let outer = subset(&mesh.vertices, &mesh.triangles, other);
            let inner = subset(&mesh.vertices, &mesh.triangles, group);
            // The Boolean operands need outward orientation, independent of
            // the original cavity winding. Orient temporary copies only.
            let outward = |mut value: Mesh| {
                let o = value.vertices[value.triangles[0][0] as usize];
                let volume: f64 = value
                    .triangles
                    .iter()
                    .map(|t| {
                        let p = t.map(|v| sub(value.vertices[v as usize], o));
                        dot(p[0], cross(p[1], p[2]))
                    })
                    .sum();
                if volume < 0. {
                    for t in &mut value.triangles {
                        t.swap(1, 2);
                    }
                }
                value
            };
            if super::csg::contains(&outward(outer), &outward(inner), cancel)? {
                depths[i] += 1;
            }
        }
        let volume: f64 = group
            .iter()
            .map(|&t| {
                let p = mesh.triangles[t].map(|v| sub(mesh.vertices[v as usize], point));
                dot(p[0], cross(p[1], p[2]))
            })
            .sum();
        if !volume.is_finite() || volume == 0. {
            return Err(invalid("mesh shell has no resolved volume"));
        }
        if (volume > 0.) != (depths[i] % 2 == 0) {
            for &t in group {
                changed.extend(mesh.triangles[t].map(|v| mesh.vertices[v as usize]));
            }
        }
        if volume < 0. {
            for &t in group {
                mesh.triangles[t].swap(1, 2);
            }
        }
    }
    if !changed.is_empty() {
        findings.push(finding(
            ImportFindingKind::Changed,
            ImportFindingCode::CorrectedWinding,
            (changed.len() / 3) as u32,
            "Facet orientation",
            "Oriented closed component facets consistently before material composition.",
            Some(point_bounds(&changed)?),
            Some(0.),
        ));
    }
    if groups.len() > 1 {
        let mut order: Vec<_> = (0..groups.len()).collect();
        order.sort_by_key(|&i| (depths[i], i));
        let first = order.remove(0);
        let mut material = subset(&mesh.vertices, &mesh.triangles, &groups[first]);
        for i in order {
            let next = subset(&mesh.vertices, &mesh.triangles, &groups[i]);
            material = super::csg::compose(material, next, depths[i] % 2 == 1, cancel)?;
        }
        *mesh = material;
        findings.push(finding(
            ImportFindingKind::Information,
            ImportFindingCode::ComponentUnion,
            groups.len() as u32,
            "Material components",
            "Composed closed components; strictly nested shells remain voids.",
            Some(point_bounds(&mesh.vertices)?),
            None,
        ));
    }
    // Remove unused positions (including removed debris) from retained bounds.
    let all: Vec<_> = (0..mesh.triangles.len()).collect();
    *mesh = subset(&mesh.vertices, &mesh.triangles, &all);
    let graph = adjacency(&mesh.triangles, false, cancel)?;
    Ok((graph, findings))
}
pub(super) fn point_bounds(points: &[[f64; 3]]) -> Result<AabbMm, GeometryError> {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for p in points {
        for a in 0..3 {
            min[a] = min[a].min(p[a]);
            max[a] = max[a].max(p[a]);
        }
    }
    AabbMm::new(min, max)
}
