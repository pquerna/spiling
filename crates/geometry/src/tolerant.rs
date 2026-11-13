// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! ADR012 tolerant sections: mesh topology, BREP geometry.
//!
//! One f64 tessellation of the imported BREP is built once per definition at a
//! chord tolerance below the profile tolerance. A section intersects its
//! triangles with a horizontal plane. Crossings are keyed by the two vertex
//! indices of the crossed mesh edge; neighbouring faces share edge vertices, so
//! pieces chain by mesh topology. Only genuine source gaps are joined by
//! proximity, within the join tolerance. Every segment keeps its source face.
//!
//! The mesh decides only which faces a section crosses and in what order.
//! Where a face is analytic (plane, cylinder, cone, sphere, torus), each
//! crossing is moved onto its true section curve, corners onto both faces,
//! and curved runs are densified on the true curve; such sections lie on the
//! BREP to rounding, and vertical-axis surfaces of revolution report their
//! exact circles. Other faces keep the mesh's chord accuracy. Loops are
//! validated, oriented and nested on the result; nothing here is a proof, and
//! the measured gaps and dropped slivers are reported.

mod exact;
mod support;
pub use exact::SectionCircle;
pub use support::{TolerantSupportPiece, TolerantSupportProjection, TolerantVoidProjection};

use crate::{GeometryError, GeometryErrorCode, kernel::Definition};
use solidcore_mesh::{EdgeOptions, OutputOptions, TessellateOptions, Tolerance};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// Height of a tessellation bucket used to find the triangles a plane crosses.
const BUCKET_MM: f64 = 0.5;
/// Largest measured mesh deviation, in profile tolerances, admitted where the
/// mesher stopped refining analytic faces: their sections are projected onto
/// the true surface, so only the mesh's topology is used there.
const MAX_UNCONVERGED_DEVIATION: f64 = 4.0;

/// One planar section loop in definition-local XY.
#[derive(Debug, Clone, PartialEq)]
pub struct TolerantLoop {
    /// Closed ring; the first point is not repeated. Counter-clockwise for
    /// material boundaries, clockwise for holes.
    pub points: Vec<[f64; 2]>,
    /// Source face ordinal of the segment leaving each point.
    pub faces: Vec<u32>,
    /// Signed area: positive for outer boundaries, negative for holes.
    pub area: f64,
}

/// A tolerant section with its measured joins and drops.
#[derive(Debug, Clone, PartialEq)]
pub struct TolerantSection {
    pub z_mm: f64,
    pub loops: Vec<TolerantLoop>,
    /// Open chain ends joined by proximity, and the largest such gap.
    pub joined_gaps: usize,
    pub max_gap_mm: f64,
    /// Loops dropped for having less than the profile tolerance squared of area.
    pub dropped_slivers: usize,
    /// Exact circles of the crossed vertical-axis surfaces of revolution.
    pub circles: Vec<SectionCircle>,
}

/// The tolerances a tolerant section is built and admitted with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TolerantPolicy {
    /// Largest source gap treated as one point.
    pub join_mm: f64,
    /// Largest deviation of a section from the true surface.
    pub profile_mm: f64,
}
impl Default for TolerantPolicy {
    fn default() -> Self {
        Self {
            join_mm: 0.01,
            profile_mm: 0.01,
        }
    }
}

/// The definition's watertight tessellation, prepared once for many sections.
#[derive(Debug, Clone)]
pub struct TolerantModel {
    positions: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
    faces: Vec<u32>,
    /// Analytic surface of each source face, by ordinal, in the model frame.
    surfaces: Vec<Option<exact::Implicit>>,
    buckets: Vec<Vec<u32>>,
    z_min: f64,
    policy: TolerantPolicy,
    /// How far a mesh crossing may move onto its analytic surface: the
    /// profile tolerance, or twice the measured deviation where the mesher
    /// left analytic faces unconverged.
    reach: f64,
    support_enclosure_mm: f64,
    /// Source faces the mesher could not tessellate; their area is missing.
    pub failed_faces: usize,
}

fn unavailable(message: impl Into<String>) -> GeometryError {
    GeometryError::new(GeometryErrorCode::UnsupportedGeometry, message.into())
}

impl TolerantModel {
    /// Tessellate `definition` once at half the profile tolerance.
    pub fn build(
        definition: &Definition,
        policy: TolerantPolicy,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        if !(policy.join_mm > 0.0 && policy.profile_mm > 0.0) {
            return Err(unavailable("tolerant section tolerances must be positive"));
        }
        let kernel = crate::make_kernel_context();
        let options = TessellateOptions {
            chord_tolerance: Tolerance::Absolute(policy.profile_mm * 0.5),
            edge: EdgeOptions::default(),
            output: OutputOptions {
                normals: false,
                uvs: false,
                face_ids: true,
                edge_polylines: false,
            },
            measure_deviation: false,
            ..TessellateOptions::default()
        };
        let mut mesh = solidcore_mesh::tessellate(&kernel, definition.brep(), &options)
            .map_err(|e| unavailable(format!("tolerant tessellation failed: {e}")))?;
        let analytic = |face: &solidcore::FaceId| {
            definition
                .face_ordinal(*face)
                .and_then(|o| definition.faces()[o].geometry.analytic.as_ref())
                .and_then(exact::Implicit::from_analytic)
                .is_some()
        };
        let mut reach = policy.profile_mm;
        let mut failed_faces = mesh.failures.len() + mesh.unconverged_faces.len();
        // The mesher can stop refining a face with work left. Measure how far
        // it strays. On analytic faces only the mesh's topology is used: admit
        // it if projection can bring every crossing back. Any face whose mesh
        // is measured within the profile tolerance needs no projection.
        if mesh.failures.is_empty() && !mesh.unconverged_faces.is_empty() {
            let all_analytic = mesh.unconverged_faces.iter().all(analytic);
            let measured = solidcore_mesh::tessellate(
                &kernel,
                definition.brep(),
                &TessellateOptions {
                    measure_deviation: true,
                    ..options
                },
            )
            .map_err(|e| unavailable(format!("tolerant tessellation failed: {e}")))?;
            let deviation = measured.stats.face_deviation.unwrap_or(f64::INFINITY);
            if (all_analytic || deviation <= policy.profile_mm)
                && let Some(admitted) = unconverged_reach(deviation, policy)
            {
                reach = admitted;
                failed_faces = 0;
                mesh = measured;
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(GeometryError::new(
                GeometryErrorCode::Cancelled,
                "tolerant tessellation cancelled",
            ));
        }
        let positions: Vec<[f64; 3]> = mesh.positions.iter().map(|p| [p.x, p.y, p.z]).collect();
        let mut faces = Vec::with_capacity(mesh.triangle_faces.len());
        for face in &mesh.triangle_faces {
            let ordinal = definition
                .face_ordinal(*face)
                .ok_or_else(|| unavailable("tessellated triangle has no source face"))?;
            faces.push(ordinal as u32);
        }
        if positions.is_empty() || positions.iter().flatten().any(|v| !v.is_finite()) {
            return Err(unavailable("tolerant tessellation is empty or not finite"));
        }
        let surfaces = definition
            .faces()
            .iter()
            .map(|face| {
                face.geometry
                    .analytic
                    .as_ref()
                    .and_then(exact::Implicit::from_analytic)
            })
            .collect();
        Ok(Self::indexed(
            positions,
            mesh.triangles,
            faces,
            surfaces,
            policy,
            reach,
            failed_faces,
        ))
    }

    /// Shared preparation from an admitted analytic/faceted source complex.
    pub fn from_complex(
        source: &crate::source::SurfaceComplex,
        policy: TolerantPolicy,
    ) -> Result<Self, GeometryError> {
        source.validate()?;
        if !policy.join_mm.is_finite()
            || !policy.profile_mm.is_finite()
            || policy.join_mm <= 0.
            || policy.profile_mm < source.error_mm
        {
            return Err(unavailable(
                "source interpretation exceeds section profile tolerance",
            ));
        }
        Ok(Self::indexed(
            source.positions.clone(),
            source.triangles.clone(),
            source.regions.clone(),
            source
                .faces
                .iter()
                .map(|f| {
                    f.geometry
                        .analytic
                        .as_ref()
                        .and_then(exact::Implicit::from_analytic)
                })
                .collect(),
            policy,
            policy.profile_mm,
            0,
        ))
    }
    pub fn policy(&self) -> TolerantPolicy {
        self.policy
    }

    /// The same tessellation placed by a rigid pose, so sections at world Z
    /// are horizontal planes in the placed frame.
    pub fn placed(&self, pose: &spiling_contracts::geometry::RigidPoseMm) -> Self {
        let positions = self
            .positions
            .iter()
            .map(|p| {
                let r = rotate(pose, *p);
                [
                    r[0] + pose.translation_mm[0],
                    r[1] + pose.translation_mm[1],
                    r[2] + pose.translation_mm[2],
                ]
            })
            .collect();
        Self::indexed(
            positions,
            self.triangles.clone(),
            self.faces.clone(),
            self.surfaces
                .iter()
                .map(|surface| surface.map(|surface| surface.placed(pose)))
                .collect(),
            self.policy,
            self.reach,
            self.failed_faces,
        )
    }

    fn indexed(
        mut positions: Vec<[f64; 3]>,
        mut triangles: Vec<[u32; 3]>,
        faces: Vec<u32>,
        surfaces: Vec<Option<exact::Implicit>>,
        policy: TolerantPolicy,
        reach: f64,
        failed_faces: usize,
    ) -> Self {
        // Exact coordinate welding restores shared mesh-edge identity without
        // moving geometry or applying an unrecorded repair tolerance. Reuse the
        // position allocation in place; support topology is checked on demand.
        let mut vertices = HashMap::new();
        let mut remap = Vec::with_capacity(positions.len());
        let mut unique = 0;
        for index in 0..positions.len() {
            let point = positions[index];
            let key = point.map(|v| {
                if v == 0.0 {
                    0.0_f64.to_bits()
                } else {
                    v.to_bits()
                }
            });
            let id = *vertices.entry(key).or_insert_with(|| {
                positions[unique] = point;
                unique += 1;
                (unique - 1) as u32
            });
            remap.push(id);
        }
        positions.truncate(unique);
        for triangle in &mut triangles {
            *triangle = triangle.map(|v| remap[v as usize]);
        }
        drop(vertices);
        drop(remap);
        let z_min = positions.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
        let z_max = positions
            .iter()
            .map(|p| p[2])
            .fold(f64::NEG_INFINITY, f64::max);
        let count = (((z_max - z_min) / BUCKET_MM).floor() as usize) + 1;
        let mut buckets = vec![Vec::new(); count];
        for (index, triangle) in triangles.iter().enumerate() {
            let zs = triangle.map(|v| positions[v as usize][2]);
            let lo = zs.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = zs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let first = ((lo - z_min) / BUCKET_MM).floor() as usize;
            let last = (((hi - z_min) / BUCKET_MM).floor() as usize).min(count - 1);
            for bucket in &mut buckets[first..=last] {
                bucket.push(index as u32);
            }
        }
        let scale = positions
            .iter()
            .flatten()
            .copied()
            .map(f64::abs)
            .fold(1.0, f64::max);
        let support_enclosure_mm = (reach + policy.join_mm + 32.0 * f64::EPSILON * scale).next_up();
        Self {
            positions,
            triangles,
            faces,
            surfaces,
            buckets,
            z_min,
            policy,
            reach,
            support_enclosure_mm,
            failed_faces,
        }
    }

    /// Axis-aligned bounds of the tessellation.
    pub fn bounds(&self) -> ([f64; 3], [f64; 3]) {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for p in &self.positions {
            for axis in 0..3 {
                min[axis] = min[axis].min(p[axis]);
                max[axis] = max[axis].max(p[axis]);
            }
        }
        (min, max)
    }

    /// Heights of horizontal faces (every triangle of the face flat within
    /// `tolerance_mm`), merged within `tolerance_mm`, plus the bounds.
    pub fn feature_heights(&self, tolerance_mm: f64) -> Vec<f64> {
        let mut flat: HashMap<u32, (f64, f64, bool)> = HashMap::new();
        for (triangle, face) in self.triangles.iter().zip(&self.faces) {
            let zs = triangle.map(|v| self.positions[v as usize][2]);
            let lo = zs.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = zs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let entry = flat.entry(*face).or_insert((lo, hi, true));
            entry.0 = entry.0.min(lo);
            entry.1 = entry.1.max(hi);
            entry.2 &= hi - lo <= tolerance_mm;
        }
        let (min, max) = self.bounds();
        let mut heights: Vec<f64> = flat
            .values()
            .filter(|(lo, hi, all_flat)| *all_flat && hi - lo <= tolerance_mm)
            .map(|(lo, hi, _)| 0.5 * (lo + hi))
            .chain([min[2], max[2]])
            .collect();
        heights.sort_by(f64::total_cmp);
        let mut merged: Vec<f64> = Vec::with_capacity(heights.len());
        for z in heights {
            if merged.last().is_none_or(|last| z - last > tolerance_mm) {
                merged.push(z);
            }
        }
        merged
    }

    /// Heights where material ends facing up and where it begins facing
    /// down: flat faces (every triangle within `tolerance_mm`) classified by
    /// their outward normal, plus the model's top (up) and bottom (down).
    /// Only a downward height can carry material that needs support below.
    pub fn facing_heights(&self, tolerance_mm: f64) -> (Vec<f64>, Vec<f64>) {
        // Per face: z extent, flatness and signed projected area (+ up).
        let mut faces: HashMap<u32, (f64, f64, bool, f64)> = HashMap::new();
        for (triangle, face) in self.triangles.iter().zip(&self.faces) {
            let [a, b, c] = triangle.map(|v| self.positions[v as usize]);
            let lo = a[2].min(b[2]).min(c[2]);
            let hi = a[2].max(b[2]).max(c[2]);
            let normal_z = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            let entry = faces.entry(*face).or_insert((lo, hi, true, 0.0));
            entry.0 = entry.0.min(lo);
            entry.1 = entry.1.max(hi);
            entry.2 &= hi - lo <= tolerance_mm;
            entry.3 += normal_z;
        }
        let (min, max) = self.bounds();
        let (mut up, mut down) = (vec![max[2]], vec![min[2]]);
        for (lo, hi, flat, area) in faces.values() {
            if !*flat || hi - lo > tolerance_mm {
                continue;
            }
            let z = 0.5 * (lo + hi);
            if *area > 0.0 {
                up.push(z);
            } else if *area < 0.0 {
                down.push(z);
            }
        }
        up.sort_by(f64::total_cmp);
        down.sort_by(f64::total_cmp);
        (up, down)
    }

    /// Heap held by the prepared tessellation.
    pub fn retained_bytes(&self) -> usize {
        self.positions.capacity() * 24
            + self.triangles.capacity() * 12
            + self.faces.capacity() * 4
            + self.surfaces.capacity() * std::mem::size_of::<Option<exact::Implicit>>()
            + self.buckets.capacity() * std::mem::size_of::<Vec<u32>>()
            + self.buckets.iter().map(|b| b.capacity() * 4).sum::<usize>()
    }

    /// The section at definition-local height `z`.
    pub fn section(&self, z: f64) -> Result<TolerantSection, GeometryError> {
        self.section_with(z, true)
    }

    // Raw support traces label outgoing segments by triangle identity. Their
    // connectivity is constant strictly between consecutive mesh vertex heights.
    fn section_with(&self, z: f64, analytic: bool) -> Result<TolerantSection, GeometryError> {
        let empty = TolerantSection {
            z_mm: z,
            loops: Vec::new(),
            joined_gaps: 0,
            max_gap_mm: 0.0,
            dropped_slivers: 0,
            circles: Vec::new(),
        };
        if !z.is_finite() {
            return Err(unavailable("tolerant section height is not finite"));
        }
        let offset = (z - self.z_min) / BUCKET_MM;
        if offset < 0.0 || offset as usize >= self.buckets.len() {
            return Ok(empty);
        }
        // A vertex exactly on the plane counts as above it, so each triangle
        // gives zero or two crossings and no vertex is shared by more than two.
        let above = |v: u32| self.positions[v as usize][2] >= z;
        let crossing = |a: u32, b: u32| -> [f64; 2] {
            let (pa, pb) = (self.positions[a as usize], self.positions[b as usize]);
            let t = (z - pa[2]) / (pb[2] - pa[2]);
            [pa[0] + t * (pb[0] - pa[0]), pa[1] + t * (pb[1] - pa[1])]
        };
        let key = |a: u32, b: u32| if a < b { (a, b) } else { (b, a) };
        let mut segments = Vec::new();
        // Each crossing with the face of each triangle that uses it.
        let mut points: HashMap<(u32, u32), Crossing> = HashMap::new();
        for &index in &self.buckets[offset as usize] {
            let [a, b, c] = self.triangles[index as usize];
            let face = if analytic {
                self.faces[index as usize]
            } else {
                index
            };
            let mut ends = Vec::with_capacity(2);
            for (p, q) in [(a, b), (b, c), (c, a)] {
                if above(p) != above(q) {
                    let k = key(p, q);
                    points
                        .entry(k)
                        .and_modify(|entry| {
                            if entry.1 != face {
                                entry.2 = Some(face);
                            }
                        })
                        .or_insert_with(|| (crossing(p, q), face, None));
                    ends.push(k);
                }
            }
            if ends.len() == 2 {
                segments.push((ends[0], ends[1], face));
            }
        }
        if segments.is_empty() {
            return Ok(empty);
        }
        let points: HashMap<(u32, u32), [f64; 2]> = points
            .into_iter()
            .map(|(k, (p, first, second))| {
                (
                    k,
                    if analytic {
                        self.exact_crossing(p, z, first, second)
                    } else {
                        p
                    },
                )
            })
            .collect();
        // Chain by crossing identity: open chains from degree-one crossings
        // first, then the remaining closed cycles.
        let mut incident: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (i, (p, q, _)) in segments.iter().enumerate() {
            incident.entry(*p).or_default().push(i);
            incident.entry(*q).or_default().push(i);
        }
        let mut used = vec![false; segments.len()];
        let mut chains: Vec<(Vec<[f64; 2]>, Vec<u32>, bool)> = Vec::new();
        let walk = |start_node: (u32, u32), first: usize, used: &mut Vec<bool>| {
            let (mut ring, mut faces) = (Vec::new(), Vec::new());
            let (mut node, mut segment) = (start_node, first);
            loop {
                used[segment] = true;
                let (p, q, face) = segments[segment];
                ring.push(points[&node]);
                faces.push(face);
                node = if p == node { q } else { p };
                if node == start_node {
                    return (ring, faces, true);
                }
                match incident[&node].iter().copied().find(|&s| !used[s]) {
                    Some(next) => segment = next,
                    None => {
                        ring.push(points[&node]);
                        return (ring, faces, false);
                    }
                }
            }
        };
        let mut ends: Vec<(u32, u32)> = incident
            .iter()
            .filter(|(_, s)| s.len() % 2 == 1)
            .map(|(k, _)| *k)
            .collect();
        ends.sort_unstable();
        for end in ends {
            if let Some(first) = incident[&end].iter().copied().find(|&s| !used[s]) {
                chains.push(walk(end, first, &mut used));
            }
        }
        for first in 0..segments.len() {
            if !used[first] {
                let start_node = segments[first].0;
                chains.push(walk(start_node, first, &mut used));
            }
        }
        // Join open chains end to start within the join tolerance.
        let mut joined_gaps = 0;
        let mut max_gap: f64 = 0.0;
        let mut open: Vec<(Vec<[f64; 2]>, Vec<u32>)> = Vec::new();
        let mut rings: Vec<(Vec<[f64; 2]>, Vec<u32>)> = Vec::new();
        for (ring, faces, closed) in chains {
            if closed {
                rings.push((ring, faces));
            } else {
                open.push((ring, faces));
            }
        }
        while let Some((mut ring, mut faces)) = open.pop() {
            loop {
                let tail = *ring.last().unwrap();
                let distance = |p: [f64; 2]| (p[0] - tail[0]).hypot(p[1] - tail[1]);
                let head_gap = distance(ring[0]);
                let best = open
                    .iter()
                    .enumerate()
                    .map(|(i, (other, _))| (i, distance(other[0])))
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                match best {
                    Some((i, gap)) if gap < head_gap && gap <= self.policy.join_mm => {
                        let (other, other_faces) = open.swap_remove(i);
                        joined_gaps += 1;
                        max_gap = max_gap.max(gap);
                        faces.push(*faces.last().unwrap());
                        ring.extend(other);
                        faces.extend(other_faces);
                    }
                    _ if head_gap <= self.policy.join_mm => {
                        joined_gaps += 1;
                        max_gap = max_gap.max(head_gap);
                        ring.pop();
                        rings.push((ring, faces));
                        break;
                    }
                    _ => {
                        return Err(unavailable(format!(
                            "section at Z {z} mm has an open boundary near ({:.4}, {:.4}) wider than the {} mm join tolerance",
                            tail[0], tail[1], self.policy.join_mm
                        )));
                    }
                }
            }
        }
        // Validate on the result: drop slivers, then orient by nesting depth.
        let sliver = if analytic {
            self.policy.profile_mm * self.policy.profile_mm
        } else {
            0.0
        };
        let mut dropped_slivers = 0;
        let mut loops = Vec::new();
        for (mut points, mut faces) in rings {
            dedupe(&mut points, &mut faces);
            if analytic {
                self.densify(&mut points, &mut faces, z);
            }
            if points.len() < 3 {
                dropped_slivers += 1;
                continue;
            }
            let area = signed_area(&points);
            if area.abs() < sliver {
                dropped_slivers += 1;
                continue;
            }
            loops.push(TolerantLoop {
                points,
                faces,
                area,
            });
        }
        let depths: Vec<usize> = (0..loops.len())
            .map(|i| {
                let probe = loops[i].points[0];
                (0..loops.len())
                    .filter(|&j| j != i && loops[j].area.abs() > loops[i].area.abs())
                    .filter(|&j| contains(&loops[j].points, probe))
                    .count()
            })
            .collect();
        for (l, depth) in loops.iter_mut().zip(depths) {
            let outer = depth % 2 == 0;
            if (l.area > 0.0) != outer {
                l.points.reverse();
                l.faces.reverse();
                l.faces.rotate_left(1);
                l.area = -l.area;
            }
        }
        let mut crossed: Vec<u32> = loops.iter().flat_map(|l| l.faces.iter().copied()).collect();
        crossed.sort_unstable();
        crossed.dedup();
        let mut circles = Vec::new();
        for face in crossed {
            if analytic && let Some(Some(surface)) = self.surfaces.get(face as usize) {
                surface.circles(z, face, &mut circles);
            }
        }
        Ok(TolerantSection {
            z_mm: z,
            loops,
            joined_gaps,
            max_gap_mm: max_gap,
            dropped_slivers,
            circles,
        })
    }

    fn surface(&self, face: u32) -> Option<&exact::Implicit> {
        self.surfaces.get(face as usize).and_then(Option::as_ref)
    }

    /// A mesh crossing moved onto its analytic face, or onto both faces where
    /// it is a corner between two. The mesh point stands where no analytic
    /// surface applies or the true curve is ill-conditioned.
    fn exact_crossing(&self, p: [f64; 2], z: f64, first: u32, second: Option<u32>) -> [f64; 2] {
        let reach = self.reach;
        let (a, b) = (self.surface(first), second.and_then(|f| self.surface(f)));
        let single = |surface: Option<&exact::Implicit>| {
            surface.and_then(|surface| surface.project(p, z, reach))
        };
        match (a, b) {
            (Some(a), Some(b)) if a != b => a
                .project_pair(b, p, z, reach)
                // Tangent faces share their section curve to first order;
                // the curved one carries it.
                .or_else(|| single(Some(if a.is_plane() { b } else { a })))
                .unwrap_or(p),
            (Some(a), _) => single(Some(a)).unwrap_or(p),
            (None, Some(b)) => single(Some(b)).unwrap_or(p),
            (None, None) => p,
        }
    }

    /// Insert points on the true curve wherever a chord on a curved analytic
    /// face sags more than a quarter of the profile tolerance.
    fn densify(&self, points: &mut Vec<[f64; 2]>, faces: &mut Vec<u32>, z: f64) {
        let sag = self.policy.profile_mm * 0.25;
        let reach = self.reach;
        if points.len() < 2 {
            return;
        }
        let mut dense_points = Vec::with_capacity(points.len());
        let mut dense_faces = Vec::with_capacity(faces.len());
        let n = points.len();
        for i in 0..n {
            let (a, b, face) = (points[i], points[(i + 1) % n], faces[i]);
            dense_points.push(a);
            dense_faces.push(face);
            let Some(surface) = self.surface(face).filter(|s| !s.is_plane()) else {
                continue;
            };
            // Depth-first midpoint subdivision, at most 2^8 pieces per chord.
            let mut stack = vec![(a, b, 0u8)];
            let mut inserted = Vec::new();
            while let Some((a, b, depth)) = stack.pop() {
                let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
                let Some(on) = surface.project(mid, z, reach) else {
                    continue;
                };
                if (on[0] - mid[0]).hypot(on[1] - mid[1]) <= sag || depth >= 8 {
                    continue;
                }
                // Visit the later half first so points come out in order.
                stack.push((on, b, depth + 1));
                stack.push((a, on, depth + 1));
                inserted.push((depth, on));
            }
            // Midpoints were found in pre-order; sort them along the chord.
            let direction = [b[0] - a[0], b[1] - a[1]];
            let mut along: Vec<(f64, [f64; 2])> = inserted
                .into_iter()
                .map(|(_, p)| {
                    (
                        (p[0] - a[0]) * direction[0] + (p[1] - a[1]) * direction[1],
                        p,
                    )
                })
                .collect();
            along.sort_by(|x, y| x.0.total_cmp(&y.0));
            for (_, p) in along {
                dense_points.push(p);
                dense_faces.push(face);
            }
        }
        *points = dense_points;
        *faces = dense_faces;
    }
}

/// The analytic surfaces of a placed definition whose horizontal sections are
/// circles: the exact centers and radii arcs in a layer can lie on. Built once
/// per occurrence; independent of which lane sliced the part.
#[derive(Debug, Clone)]
pub struct CircleSource {
    surfaces: Vec<(u32, exact::Implicit)>,
}
impl CircleSource {
    pub fn new(definition: &Definition, pose: &spiling_contracts::geometry::RigidPoseMm) -> Self {
        Self::from_faces(definition.faces(), pose)
    }
    pub fn from_faces(
        faces: &[spiling_contracts::geometry::FaceInfo],
        pose: &spiling_contracts::geometry::RigidPoseMm,
    ) -> Self {
        let surfaces = faces
            .iter()
            .enumerate()
            .filter_map(|(ordinal, face)| {
                let surface = face
                    .geometry
                    .analytic
                    .as_ref()
                    .and_then(exact::Implicit::from_analytic)?
                    .placed(pose);
                surface
                    .has_vertical_axis()
                    .then_some((ordinal as u32, surface))
            })
            .collect();
        Self { surfaces }
    }

    /// Circles of every such surface at height `z`. A trimmed face need not
    /// reach `z`; consumers match real geometry against these candidates.
    pub fn circles_at(&self, z: f64, out: &mut Vec<SectionCircle>) {
        for (face, surface) in &self.surfaces {
            surface.circles(z, *face, out);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.surfaces.is_empty()
    }
}

/// Rotate `v` by the pose's unit quaternion: v' = v + 2w(q x v) + 2 q x (q x v).
fn rotate(pose: &spiling_contracts::geometry::RigidPoseMm, v: [f64; 3]) -> [f64; 3] {
    let [qx, qy, qz, qw] = pose.rotation_xyzw;
    let q = [qx, qy, qz];
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t = cross(q, v).map(|c| 2.0 * c);
    let u = cross(q, t);
    [
        v[0] + qw * t[0] + u[0],
        v[1] + qw * t[1] + u[1],
        v[2] + qw * t[2] + u[2],
    ]
}

/// The projection reach for a mesh whose analytic faces stopped short of the
/// chord tolerance, if its measured deviation admits it: projection must be
/// able to bring every crossing back onto the true surface.
fn unconverged_reach(deviation: f64, policy: TolerantPolicy) -> Option<f64> {
    (deviation <= MAX_UNCONVERGED_DEVIATION * policy.profile_mm)
        .then(|| policy.profile_mm.max(2.0 * deviation))
}

/// A mesh-edge crossing and the faces of the one or two triangles using it.
type Crossing = ([f64; 2], u32, Option<u32>);

fn dedupe(points: &mut Vec<[f64; 2]>, faces: &mut Vec<u32>) {
    let mut kept_points = Vec::with_capacity(points.len());
    let mut kept_faces = Vec::with_capacity(faces.len());
    for (p, f) in points.iter().zip(faces.iter()) {
        if kept_points
            .last()
            .is_some_and(|q: &[f64; 2]| (p[0] - q[0]).hypot(p[1] - q[1]) <= 1e-9)
        {
            continue;
        }
        kept_points.push(*p);
        kept_faces.push(*f);
    }
    while kept_points.len() > 1 {
        let (a, b) = (kept_points[0], *kept_points.last().unwrap());
        if (a[0] - b[0]).hypot(a[1] - b[1]) <= 1e-9 {
            kept_points.pop();
            kept_faces.pop();
        } else {
            break;
        }
    }
    *points = kept_points;
    *faces = kept_faces;
}

fn signed_area(points: &[[f64; 2]]) -> f64 {
    let mut sum = 0.0;
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        let origin = points[0];
        let (ax, ay) = (a[0] - origin[0], a[1] - origin[1]);
        let (bx, by) = (b[0] - origin[0], b[1] - origin[1]);
        sum += ax * by - bx * ay;
    }
    sum * 0.5
}

fn contains(ring: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconverged_analytic_faces_are_admitted_only_within_measured_deviation() {
        let policy = TolerantPolicy::default();
        // Observed: one cylinder 0.0175 mm off (admitted, reach covers it);
        // six cylinders 6 mm off (refused: the mesh is wrong, not coarse).
        assert_eq!(unconverged_reach(0.0175, policy), Some(0.035));
        assert_eq!(unconverged_reach(0.001, policy), Some(policy.profile_mm));
        assert_eq!(unconverged_reach(6.08, policy), None);
        assert_eq!(unconverged_reach(f64::INFINITY, policy), None);
    }
}
