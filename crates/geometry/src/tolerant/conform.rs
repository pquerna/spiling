// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Shared-edge conformity of the tolerant tessellation.
//!
//! The mesher divides every BREP edge once and shares that polyline with both
//! faces, as OCCT's BRepMesh does. One construct still breaks the sharing: two
//! boundary edges of a face that meet almost tangentially at a vertex (a
//! thread band running into a hole circle). Their chords cross, and the
//! face's constrained triangulation splits both at the crossing. The
//! neighbouring faces keep the unsplit chord, so the mesh has T-junctions
//! and the shared edge is open on both sides.
//!
//! OCCT refines the crossing edges until the discrete wire no longer
//! intersects itself. Here the mesh is conformed instead: a mesh edge used
//! by one triangle is split at every open-edge vertex of another face that
//! only that face uses (a crossing, not a shared polyline vertex) and that
//! lies on the edge within the join tolerance, so both sides use the same
//! pieces. No vertex
//! moves and no area is added; the largest distance of an inserted vertex
//! from the edge it splits is measured and reported. Open edges with no such
//! vertex stay open for the consumers that require a closed mesh to refuse.

use super::unavailable;
use crate::{GeometryError, GeometryErrorCode};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// What conforming changed.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Conformed {
    /// Vertices inserted into an open mesh edge of a neighbouring face.
    pub vertices: usize,
    /// Largest distance from an inserted vertex to the edge it split.
    pub max_offset_mm: f64,
}

fn key(a: u32, b: u32) -> (u32, u32) {
    if a < b { (a, b) } else { (b, a) }
}

/// Each mesh edge used by exactly one triangle, with that triangle and side.
fn open_edges(triangles: &[[u32; 3]]) -> HashMap<(u32, u32), (usize, u8)> {
    let mut edges = HashMap::<(u32, u32), (u32, usize, u8)>::new();
    for (index, triangle) in triangles.iter().enumerate() {
        for side in 0..3u8 {
            let a = triangle[usize::from(side)];
            let b = triangle[(usize::from(side) + 1) % 3];
            edges.entry(key(a, b)).or_insert((0, index, side)).0 += 1;
        }
    }
    edges
        .into_iter()
        .filter(|(_, (uses, _, _))| *uses == 1)
        .map(|(k, (_, index, side))| (k, (index, side)))
        .collect()
}

/// Split each open mesh edge at the other faces' face-local open-edge
/// vertices lying on it within `tolerance_mm`. Repeats until no edge changes.
pub(super) fn conform(
    positions: &[[f64; 3]],
    triangles: &mut Vec<[u32; 3]>,
    faces: &mut Vec<u32>,
    tolerance_mm: f64,
    cancel: &AtomicBool,
) -> Result<Conformed, GeometryError> {
    let mut result = Conformed::default();
    // The face owning each vertex the mesher placed for one face only. Every
    // vertex of a shared edge polyline belongs to both its faces, so only
    // these face-local vertices can be crossings the neighbour never saw.
    // Fixed before any split: inserting a vertex does not make it shared.
    let mut local = HashMap::<u32, Option<u32>>::new();
    {
        let open = open_edges(triangles);
        if open.is_empty() {
            return Ok(result);
        }
        for &(a, b) in open.keys() {
            local.insert(a, None);
            local.insert(b, None);
        }
        let mut seen = HashMap::<u32, (u32, bool)>::new();
        for (triangle, face) in triangles.iter().zip(faces.iter()) {
            for v in triangle {
                if local.contains_key(v) {
                    let entry = seen.entry(*v).or_insert((*face, true));
                    entry.1 &= entry.0 == *face;
                }
            }
        }
        for (v, (face, single)) in seen {
            if single {
                local.insert(v, Some(face));
            }
        }
        local.retain(|_, face| face.is_some());
    }
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(GeometryError::new(
                GeometryErrorCode::Cancelled,
                "tolerant tessellation cancelled",
            ));
        }
        let open = open_edges(triangles);
        if open.is_empty() {
            return Ok(result);
        }
        // Open-edge vertices sorted by x, for an x-range query per edge.
        let mut vertices: Vec<u32> = open
            .keys()
            .flat_map(|&(a, b)| [a, b])
            .filter(|v| local.contains_key(v))
            .collect();
        vertices.sort_unstable();
        vertices.dedup();
        vertices.sort_by(|a, b| positions[*a as usize][0].total_cmp(&positions[*b as usize][0]));
        let xs: Vec<f64> = vertices.iter().map(|v| positions[*v as usize][0]).collect();
        let mut edges: Vec<_> = open.into_iter().collect();
        edges.sort_unstable_by_key(|(k, _)| *k);
        let mut split = vec![false; triangles.len()];
        let mut changed = false;
        for ((a, b), (triangle, side)) in edges {
            if split[triangle] {
                continue; // its other open edges wait for the next pass
            }
            let (pa, pb) = (positions[a as usize], positions[b as usize]);
            let lo = pa[0].min(pb[0]) - tolerance_mm;
            let hi = pa[0].max(pb[0]) + tolerance_mm;
            let first = xs.partition_point(|x| *x < lo);
            let d = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
            let length2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            if length2 <= 0.0 {
                continue;
            }
            let mut inside = Vec::new();
            for &v in vertices[first..]
                .iter()
                .take_while(|v| positions[**v as usize][0] <= hi)
            {
                if v == a
                    || v == b
                    || triangles[triangle].contains(&v)
                    || local[&v] == Some(faces[triangle])
                {
                    continue;
                }
                let p = positions[v as usize];
                let r = [p[0] - pa[0], p[1] - pa[1], p[2] - pa[2]];
                let t = (r[0] * d[0] + r[1] * d[1] + r[2] * d[2]) / length2;
                if !(t > 0.0 && t < 1.0) {
                    continue;
                }
                let offset = (0..3)
                    .map(|i| (r[i] - t * d[i]).powi(2))
                    .sum::<f64>()
                    .sqrt();
                if offset <= tolerance_mm {
                    inside.push((t, v, offset));
                }
            }
            if inside.is_empty() {
                continue;
            }
            inside.sort_by(|x, y| x.0.total_cmp(&y.0));
            let corners = triangles[triangle];
            let s = usize::from(side);
            // The triangle's own order runs from `start` to `end` on this side.
            let (start, end, apex) = (corners[s], corners[(s + 1) % 3], corners[(s + 2) % 3]);
            let mut chain = vec![start];
            if start == a {
                chain.extend(inside.iter().map(|x| x.1));
            } else {
                chain.extend(inside.iter().rev().map(|x| x.1));
            }
            chain.push(end);
            let face = faces[triangle];
            triangles[triangle] = [chain[0], chain[1], apex];
            for pair in chain[1..].windows(2) {
                triangles.push([pair[0], pair[1], apex]);
                faces.push(face);
            }
            split[triangle] = true;
            changed = true;
            for (_, _, offset) in &inside {
                result.vertices += 1;
                result.max_offset_mm = result.max_offset_mm.max(*offset);
            }
        }
        if !changed {
            return Ok(result);
        }
        if result.vertices > positions.len().saturating_mul(3) {
            return Err(unavailable(
                "tolerant tessellation shared-edge conforming did not converge",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two faces share a straight boundary; one face's triangulation split it
    /// at a crossing point the other face never saw.
    #[test]
    fn crossing_split_on_one_side_is_conformed_on_the_other() {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, -1.0, 0.0],
            [1.0, 1e-9, 0.0], // crossing point on the shared chord
        ];
        // Face 0 uses the split chord 0-4-1; face 1 the whole chord 0-1.
        let mut triangles = vec![[0, 4, 2], [4, 1, 2], [1, 0, 3]];
        let mut faces = vec![0, 0, 1];
        let cancel = AtomicBool::new(false);
        let conformed = conform(&positions, &mut triangles, &mut faces, 0.01, &cancel).unwrap();
        assert_eq!(conformed.vertices, 1);
        assert!(conformed.max_offset_mm <= 1e-9 + 1e-15);
        assert_eq!(faces, vec![0, 0, 1, 1]);
        let open = open_edges(&triangles);
        let boundary: Vec<_> = open.keys().copied().collect();
        // Only the outer boundary of the two faces remains open.
        assert_eq!(boundary.len(), 4, "{boundary:?}");
        assert!(!open.contains_key(&(0, 1)));
        // Orientation is preserved: each new triangle keeps face 1's winding.
        for t in &triangles[2..] {
            let [a, b, c] = t.map(|v| positions[v as usize]);
            let z = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            assert!(z > 0.0, "{t:?}");
        }
    }

    #[test]
    fn a_vertex_beyond_the_join_tolerance_is_not_inserted() {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, -1.0, 0.0],
            [1.0, 0.05, 0.0],
        ];
        let mut triangles = vec![[0, 4, 2], [4, 1, 2], [1, 0, 3]];
        let mut faces = vec![0, 0, 1];
        let cancel = AtomicBool::new(false);
        let conformed = conform(&positions, &mut triangles, &mut faces, 0.01, &cancel).unwrap();
        assert_eq!(conformed, Conformed::default());
        assert_eq!(triangles.len(), 3);
    }

    /// The part-26 construct: a sliver face between two nearly tangent
    /// boundary chords that cross. Each neighbour receives the crossing,
    /// never the other neighbour's own polyline vertex, although that vertex
    /// also lies within the join tolerance of its chord.
    #[test]
    fn crossing_chords_conform_without_borrowing_a_shared_polyline_vertex() {
        let positions = vec![
            [0.5, 0.0005, 0.0],  // 0 band chord start, shared band/sliver
            [1.5, -0.0005, 0.0], // 1 band chord end, shared band/sliver
            [0.0, -0.001, 0.0],  // 2 cone chord start, shared cone/sliver
            [2.0, 0.001, 0.0],   // 3 cone chord end, shared cone/sliver
            [1.0, 0.0, 0.0],     // 4 the sliver's crossing vertex
            [1.0, 1.0, 0.0],     // 5 band apex
            [1.0, -1.0, 0.0],    // 6 cone apex
        ];
        let mut triangles = vec![
            [1, 0, 5], // band (face 0)
            [2, 3, 6], // cone (face 1)
            [0, 4, 2], // sliver (face 2), split at the crossing
            [4, 1, 3],
        ];
        let mut faces = vec![0, 1, 2, 2];
        let cancel = AtomicBool::new(false);
        let conformed = conform(&positions, &mut triangles, &mut faces, 0.01, &cancel).unwrap();
        assert_eq!(conformed.vertices, 2);
        let mut uses = HashMap::<(u32, u32), u32>::new();
        for t in &triangles {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *uses.entry(key(a, b)).or_default() += 1;
            }
        }
        for edge in [(0, 4), (1, 4), (2, 4), (3, 4)] {
            assert_eq!(uses[&edge], 2, "{edge:?} {triangles:?}");
        }
        assert!(uses.values().all(|n| *n <= 2), "{uses:?}");
        assert!(!uses.contains_key(&(0, 2)) || uses[&(0, 2)] == 1);
    }
}
