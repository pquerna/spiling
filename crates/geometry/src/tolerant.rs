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

mod conform;
mod exact;
mod support;
pub use conform::Conformed;
pub use exact::SectionCircle;
pub use support::{
    TolerantSupportPiece, TolerantSupportProjection, TolerantVoidProjection, TolerantVoidRoof,
};

use crate::{GeometryError, GeometryErrorCode, kernel::Definition};
use solidcore_tess::MeshTolerance;
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// Height of a tessellation bucket used to find the triangles a plane crosses.
const BUCKET_MM: f64 = 0.5;
/// How many items each phase of a section handles between checks of its cancellation flag.
pub const CANCEL_STRIDE: usize = 4096;
/// Deepest midpoint subdivision of one section chord on its true curve.
const DENSIFY_DEPTH: u8 = 8;
/// Most pieces one section chord is densified into: a full binary subdivision to
/// [`DENSIFY_DEPTH`].
pub const DENSIFY_PIECES: usize = 1 << DENSIFY_DEPTH;

type Key = (u32, u32);
type Segment = (Key, Key, u32);
type Chain = (Vec<[f64; 2]>, Vec<u32>, bool);
type Ring = (Vec<[f64; 2]>, Vec<u32>);

/// What one tolerant section works on and may allocate at most, known before it runs
/// ([`TolerantModel::section_demand`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionDemand {
    /// Triangles the section reads: the work it is charged with.
    pub triangles: usize,
    /// Most heap it holds at once besides its result, at any moment of the section.
    pub scratch_bytes: usize,
    /// Most heap its returned section holds.
    pub output_bytes: usize,
}

impl SectionDemand {
    /// The bound for a section reading `t` triangles, besides its densification. A triangle
    /// gives at most two crossings and one segment; chains hold at most two points a segment (an
    /// open chain's last point); joining adds at most one face entry a segment; each crossed face
    /// gives at most two circles. Densification starts each loop's new buffers at the loop's own
    /// size and admits every growth past it.
    pub fn for_triangles(t: usize) -> Self {
        use std::mem::size_of;
        let segments = t;
        let crossings = t.saturating_mul(2);
        let points = t.saturating_mul(2);
        let faces = t.saturating_mul(3);
        let point = size_of::<[f64; 2]>();
        let face = size_of::<u32>();
        // A vector grown by pushes to `n` items: its buffer at most twice that (or the smallest
        // nonzero capacity), with the previous buffer still live while one grows.
        let grown =
            |n: usize, item: usize| n.saturating_mul(3).saturating_add(8).saturating_mul(item);
        // A hash table sized for `n` entries up front: power-of-two buckets at 7/8 load, one
        // control byte each, plus a group of control bytes and alignment.
        let table = |n: usize, entry: usize| {
            let buckets = (n.saturating_mul(8) / 7)
                .saturating_add(1)
                .next_power_of_two()
                .max(8);
            buckets
                .saturating_mul(entry.saturating_add(1))
                .saturating_add(32)
        };
        let scratch = [
            segments.saturating_mul(size_of::<Segment>()),
            table(crossings, size_of::<(Key, Crossing)>()),
            segments
                .saturating_mul(2)
                .saturating_mul(size_of::<(Key, usize)>()),
            crossings.saturating_mul(size_of::<Key>()),
            segments,
            segments.saturating_mul(size_of::<Chain>()),
            segments.saturating_mul(2).saturating_mul(size_of::<Ring>()),
            // Chains' rings and faces, every one grown by pushes (at most one a segment).
            grown(points, point).saturating_add(segments.saturating_mul(8 * point)),
            grown(faces, face).saturating_add(segments.saturating_mul(8 * face)),
            // A join's extension: the joined ring's old and new buffers.
            grown(points, point).saturating_add(grown(faces, face)),
            // Deduplication copies one ring.
            points
                .saturating_mul(point)
                .saturating_add(faces.saturating_mul(face)),
            // Densification of one ring: its input and new buffers at its size (growth past it
            // is admitted), the chord's subdivision stack, midpoints and their order.
            points.saturating_mul(point + face),
            points.saturating_mul(point + face),
            (2 * DENSIFY_DEPTH as usize + 1).saturating_mul(size_of::<([f64; 2], [f64; 2], u8)>()),
            grown(DENSIFY_PIECES, size_of::<(u8, [f64; 2])>()),
            DENSIFY_PIECES.saturating_mul(size_of::<(f64, [f64; 2])>()),
            // Nesting depths (the crossed faces are admitted at their count).
            segments.saturating_mul(size_of::<usize>()),
        ]
        .into_iter()
        .fold(0usize, usize::saturating_add);
        let output = [
            segments.saturating_mul(size_of::<TolerantLoop>()),
            points.saturating_mul(point + face),
            segments
                .saturating_mul(2)
                .saturating_mul(size_of::<SectionCircle>()),
        ]
        .into_iter()
        .fold(0usize, usize::saturating_add);
        Self {
            triangles: t,
            scratch_bytes: scratch,
            output_bytes: output,
        }
    }
}
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
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TolerantModel {
    positions: Vec<[f64; 3]>,
    triangles: Arc<Vec<[u32; 3]>>,
    faces: Arc<Vec<u32>>,
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
    /// Open mesh edges split to share a neighbouring face's crossing vertex.
    pub conformed: Conformed,
}

/// Exact coordinate welding restores shared mesh-edge identity without
/// moving geometry or applying an unrecorded repair tolerance. Reuses the
/// position allocation in place.
fn weld(positions: &mut Vec<[f64; 3]>, triangles: &mut [[u32; 3]]) {
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
    for triangle in triangles.iter_mut() {
        *triangle = triangle.map(|v| remap[v as usize]);
    }
}

struct Cancellable<'a> {
    cancel: &'a AtomicBool,
    bytes: AtomicUsize,
    limit: usize,
}
impl solidcore_tess::Control for Cancellable<'_> {
    fn charge(&self, _work: u64) -> Result<(), solidcore_tess::Stop> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(solidcore_tess::Stop::Cancelled)
        } else {
            Ok(())
        }
    }
    fn admit(&self, bytes: usize) -> Result<(), solidcore_tess::Stop> {
        self.bytes
            .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(bytes)
                    .filter(|total| *total <= self.limit)
            })
            .map(|_| ())
            .map_err(|_| solidcore_tess::Stop::Bytes)
    }
}

#[derive(Default)]
pub(crate) struct PreparationCache(Mutex<Option<Arc<TolerantModel>>>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparationOutcome {
    Hit,
    Published(usize),
    Unpublished(usize),
}

impl PreparationOutcome {
    pub fn allocated_bytes(self) -> usize {
        match self {
            Self::Hit => 0,
            Self::Published(bytes) | Self::Unpublished(bytes) => bytes,
        }
    }
}

impl PreparationCache {
    pub(crate) fn retained_bytes(&self) -> usize {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map_or(0, |model| model.retained_bytes())
    }

    pub(crate) fn prepare(
        &self,
        policy: TolerantPolicy,
        cancel: &AtomicBool,
        build: impl FnOnce() -> Result<TolerantModel, GeometryError>,
    ) -> Result<(Arc<TolerantModel>, PreparationOutcome), GeometryError> {
        crate::check_cancel(cancel)?;
        {
            let slot = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(model) = slot.as_ref().filter(|model| model.policy == policy) {
                return Ok((Arc::clone(model), PreparationOutcome::Hit));
            }
        }
        let model = Arc::new(build()?);
        crate::check_cancel(cancel)?;
        let mut slot = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = slot.as_ref() {
            if existing.policy == policy {
                return Ok((Arc::clone(existing), PreparationOutcome::Hit));
            }
            return Ok((
                Arc::clone(&model),
                PreparationOutcome::Unpublished(model.retained_bytes()),
            ));
        }
        *slot = Some(Arc::clone(&model));
        Ok((
            Arc::clone(&model),
            PreparationOutcome::Published(model.retained_bytes()),
        ))
    }
}

fn unavailable(message: impl Into<String>) -> GeometryError {
    GeometryError::new(GeometryErrorCode::UnsupportedGeometry, message.into())
}

fn preparation_resource() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "source preparation exceeds admitted additional memory",
    )
}

fn index_bytes(positions: &[[f64; 3]], triangles: &[[u32; 3]]) -> Result<usize, GeometryError> {
    let minimum = positions
        .iter()
        .map(|point| point[2])
        .fold(f64::INFINITY, f64::min);
    let maximum = positions
        .iter()
        .map(|point| point[2])
        .fold(f64::NEG_INFINITY, f64::max);
    let buckets = (((maximum - minimum) / BUCKET_MM).floor() as usize)
        .checked_add(1)
        .ok_or_else(preparation_resource)?;
    let mut entries = 0usize;
    for triangle in triangles {
        let heights = triangle.map(|index| positions[index as usize][2]);
        let lower = heights.into_iter().fold(f64::INFINITY, f64::min);
        let upper = heights.into_iter().fold(f64::NEG_INFINITY, f64::max);
        let first = ((lower - minimum) / BUCKET_MM).floor() as usize;
        let last = (((upper - minimum) / BUCKET_MM).floor() as usize).min(buckets - 1);
        entries = entries
            .checked_add(last - first + 1)
            .ok_or_else(preparation_resource)?;
    }
    buckets
        .checked_mul(size_of::<Vec<u32>>() + 4 * size_of::<u32>())
        .and_then(|bytes| {
            entries
                .checked_mul(2 * size_of::<u32>())
                .and_then(|indices| bytes.checked_add(indices))
        })
        .ok_or_else(preparation_resource)
}

impl TolerantModel {
    /// Tessellate `definition` once at half the profile tolerance.
    pub fn build(
        definition: &Definition,
        policy: TolerantPolicy,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        Self::build_with_budget(definition, policy, isize::MAX as usize, cancel)
    }

    pub fn build_with_budget(
        definition: &Definition,
        policy: TolerantPolicy,
        additional_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        crate::check_cancel(cancel)?;
        if !(policy.join_mm.is_finite()
            && policy.profile_mm.is_finite()
            && policy.join_mm > 0.0
            && policy.profile_mm > 0.0)
        {
            return Err(unavailable("tolerant section tolerances must be positive"));
        }
        // The chord alone (no facet angle): the profile, not the look, decides the mesh.
        let size = {
            let b = &definition.bounds;
            let d: [f64; 3] = std::array::from_fn(|i| b.max[i] - b.min[i]);
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        };
        let cancelled = || {
            GeometryError::new(
                GeometryErrorCode::Cancelled,
                "tolerant tessellation cancelled",
            )
        };
        let control = Cancellable {
            cancel,
            bytes: AtomicUsize::new(0),
            limit: additional_bytes,
        };
        let (plan, stats) = solidcore_tess::plan_with(
            definition.record(),
            &MeshTolerance::chord(policy.profile_mm * 0.5),
            size,
            &control,
        )
        .map_err(|e| match e {
            solidcore_tess::TessError::Stopped(solidcore_tess::Stop::Cancelled) => cancelled(),
            solidcore_tess::TessError::Stopped(_) => preparation_resource(),
            e => unavailable(format!("tolerant tessellation failed: {e}")),
        })?;
        let (mesh, refused) = plan.evaluate_with(&control).map_err(|stop| match stop {
            solidcore_tess::TessError::Stopped(solidcore_tess::Stop::Cancelled) => cancelled(),
            solidcore_tess::TessError::Stopped(_) => preparation_resource(),
            cause => unavailable(format!("tolerant tessellation failed: {cause}")),
        })?;
        use solidcore_tess::Control;
        control
            .admit(
                mesh.positions
                    .len()
                    .saturating_mul(
                        size_of::<[f64; 3]>() + size_of::<([u64; 3], u32)>() + size_of::<u32>(),
                    )
                    .saturating_add(
                        mesh.triangles
                            .len()
                            .saturating_mul(size_of::<[u32; 3]>() + size_of::<u32>()),
                    )
                    .saturating_add(
                        definition
                            .faces()
                            .len()
                            .saturating_mul(size_of::<Option<exact::Implicit>>()),
                    ),
            )
            .map_err(|_| preparation_resource())?;
        // Faces left out of the mesh (unplanned, or refused where their carrier would not
        // evaluate): their area is missing, reported as failed faces.
        let analytic = |record_face: usize| {
            definition
                .record_order()
                .faces
                .get(record_face)
                .and_then(|face| definition.face_ordinal(*face))
                .and_then(|o| definition.faces()[o].geometry.analytic.as_ref())
                .and_then(exact::Implicit::from_analytic)
                .is_some()
        };
        let mut reach = policy.profile_mm;
        let unplanned = stats.unmeshed.len() + refused.len();
        let mut failed_faces = unplanned + stats.over_tolerance.len();
        // A memory bound of the planner can leave a face's sag above its tolerance; the
        // planner measures it. On analytic faces only the mesh's topology is used: admit it if
        // projection can bring every crossing back. A face measured within the profile
        // tolerance needs no projection.
        if unplanned == 0 && !stats.over_tolerance.is_empty() {
            let all_analytic = stats.over_tolerance.iter().all(|over| analytic(over.face));
            let measured = stats.over_tolerance.iter().all(|over| over.unmeasured == 0);
            let deviation = if measured {
                stats
                    .over_tolerance
                    .iter()
                    .map(|over| over.sag)
                    .fold(0.0_f64, f64::max)
            } else {
                f64::INFINITY
            };
            if (all_analytic || deviation <= policy.profile_mm)
                && let Some(admitted) = unconverged_reach(deviation, policy)
            {
                reach = admitted;
                failed_faces = 0;
            }
        }
        let mut positions: Vec<[f64; 3]> = mesh.positions.iter().map(|p| [p.x, p.y, p.z]).collect();
        let mut faces = Vec::with_capacity(mesh.tri_face.len());
        for &record_face in &mesh.tri_face {
            let ordinal = definition
                .record_order()
                .faces
                .get(record_face as usize)
                .and_then(|face| definition.face_ordinal(*face))
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
        let mut triangles = mesh.triangles;
        weld(&mut positions, &mut triangles);
        let conformed = conform::conform(
            &positions,
            &mut triangles,
            &mut faces,
            policy.join_mm,
            cancel,
        )?;
        control
            .admit(index_bytes(&positions, &triangles)?)
            .map_err(|_| preparation_resource())?;
        let mut model = Self::indexed(
            positions,
            triangles,
            faces,
            surfaces,
            policy,
            reach,
            failed_faces,
        );
        model.conformed = conformed;
        if model.retained_bytes() > additional_bytes {
            return Err(preparation_resource());
        }
        crate::check_cancel(cancel)?;
        Ok(model)
    }

    /// Shared preparation from an admitted analytic/faceted source complex.
    pub fn from_complex(
        source: &crate::source::SurfaceComplex,
        policy: TolerantPolicy,
    ) -> Result<Self, GeometryError> {
        Self::from_complex_with_budget(source, policy, isize::MAX as usize)
    }

    pub fn from_complex_with_budget(
        source: &crate::source::SurfaceComplex,
        policy: TolerantPolicy,
        additional_bytes: usize,
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
        let required = source
            .retained_bytes()
            .saturating_add(index_bytes(&source.positions, &source.triangles)?)
            .saturating_add(
                source
                    .positions
                    .len()
                    .saturating_mul(size_of::<([u64; 3], u32)>() + size_of::<u32>()),
            )
            .saturating_add(
                source
                    .faces
                    .len()
                    .saturating_mul(size_of::<Option<exact::Implicit>>()),
            );
        if required > additional_bytes {
            return Err(preparation_resource());
        }
        let mut positions = source.positions.clone();
        let mut triangles = source.triangles.clone();
        weld(&mut positions, &mut triangles);
        Ok(Self::indexed(
            positions,
            triangles,
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
            Arc::clone(&self.triangles),
            Arc::clone(&self.faces),
            self.surfaces
                .iter()
                .map(|surface| surface.map(|surface| surface.placed(pose)))
                .collect(),
            self.policy,
            self.reach,
            self.failed_faces,
        )
        .with_conformed(self.conformed)
    }

    fn with_conformed(mut self, conformed: Conformed) -> Self {
        self.conformed = conformed;
        self
    }

    fn indexed(
        positions: Vec<[f64; 3]>,
        triangles: impl Into<Arc<Vec<[u32; 3]>>>,
        faces: impl Into<Arc<Vec<u32>>>,
        surfaces: Vec<Option<exact::Implicit>>,
        policy: TolerantPolicy,
        reach: f64,
        failed_faces: usize,
    ) -> Self {
        let triangles = triangles.into();
        let faces = faces.into();
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
            conformed: Conformed::default(),
        }
    }

    /// Mesh edges not used by exactly two triangles after exact welding: the
    /// edge's two positions and the source face ordinals of its triangles.
    pub fn open_edges(&self) -> Vec<([[f64; 3]; 2], Vec<u32>)> {
        let mut edges = HashMap::<(u32, u32), Vec<u32>>::new();
        for (triangle, face) in self.triangles.iter().zip(self.faces.iter()) {
            for [a, b] in [
                [triangle[0], triangle[1]],
                [triangle[1], triangle[2]],
                [triangle[2], triangle[0]],
            ] {
                edges
                    .entry(if a < b { (a, b) } else { (b, a) })
                    .or_default()
                    .push(*face);
            }
        }
        let mut open: Vec<_> = edges
            .into_iter()
            .filter(|(_, faces)| faces.len() != 2)
            .map(|((a, b), faces)| {
                (
                    [self.positions[a as usize], self.positions[b as usize]],
                    faces,
                )
            })
            .collect();
        open.sort_by(|a, b| {
            a.0[0]
                .partial_cmp(&b.0[0])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        open
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
        for (triangle, face) in self.triangles.iter().zip(self.faces.iter()) {
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
        for (triangle, face) in self.triangles.iter().zip(self.faces.iter()) {
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
    /// Check the indexing invariants of a prepared model received from a
    /// developer stage snapshot before invoking section or occupancy queries.
    pub fn validate_stage_snapshot(&self) -> bool {
        self.z_min.is_finite()
            && self.reach.is_finite()
            && self.reach >= 0.0
            && self.support_enclosure_mm.is_finite()
            && self.support_enclosure_mm >= 0.0
            && self.policy.join_mm.is_finite()
            && self.policy.join_mm > 0.0
            && self.policy.profile_mm.is_finite()
            && self.policy.profile_mm > 0.0
            && self.positions.iter().flatten().all(|x| x.is_finite())
            && self.faces.len() == self.triangles.len()
            && self
                .triangles
                .iter()
                .flatten()
                .all(|&i| (i as usize) < self.positions.len())
            && self
                .faces
                .iter()
                .all(|&i| (i as usize) < self.surfaces.len())
            && self
                .buckets
                .iter()
                .flatten()
                .all(|&i| (i as usize) < self.triangles.len())
    }

    pub fn retained_bytes(&self) -> usize {
        self.positions.capacity() * 24
            + self.triangles.capacity() * 12
            + self.faces.capacity() * 4
            + self.surfaces.capacity() * std::mem::size_of::<Option<exact::Implicit>>()
            + self.buckets.capacity() * std::mem::size_of::<Vec<u32>>()
            + self.buckets.iter().map(|b| b.capacity() * 4).sum::<usize>()
    }

    pub fn placement_bytes(&self) -> usize {
        self.retained_bytes()
            - self.triangles.capacity() * size_of::<[u32; 3]>()
            - self.faces.capacity() * size_of::<u32>()
    }

    pub fn placed_with_budget(
        &self,
        pose: &spiling_contracts::geometry::RigidPoseMm,
        additional_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        crate::check_cancel(cancel)?;
        pose.validate()?;
        let positions_bytes = self
            .positions
            .len()
            .checked_mul(size_of::<[f64; 3]>())
            .ok_or_else(preparation_resource)?;
        let surface_bytes = self
            .surfaces
            .len()
            .checked_mul(size_of::<Option<exact::Implicit>>())
            .ok_or_else(preparation_resource)?;
        if positions_bytes.saturating_add(surface_bytes) > additional_bytes {
            return Err(preparation_resource());
        }
        let mut positions = Vec::with_capacity(self.positions.len());
        for point in &self.positions {
            crate::check_cancel(cancel)?;
            let rotated = rotate(pose, *point);
            positions.push(std::array::from_fn(|axis| {
                rotated[axis] + pose.translation_mm[axis]
            }));
            if positions
                .last()
                .is_some_and(|point| point.iter().any(|value| !value.is_finite()))
            {
                return Err(unavailable("placed source coordinates are not finite"));
            }
        }
        if positions_bytes
            .saturating_add(surface_bytes)
            .saturating_add(index_bytes(&positions, &self.triangles)?)
            > additional_bytes
        {
            return Err(preparation_resource());
        }
        let model = Self::indexed(
            positions,
            Arc::clone(&self.triangles),
            Arc::clone(&self.faces),
            self.surfaces
                .iter()
                .map(|surface| surface.map(|surface| surface.placed(pose)))
                .collect(),
            self.policy,
            self.reach,
            self.failed_faces,
        )
        .with_conformed(self.conformed);
        crate::check_cancel(cancel)?;
        if model.placement_bytes() > additional_bytes {
            return Err(preparation_resource());
        }
        Ok(model)
    }

    /// The section at definition-local height `z`.
    pub fn section(&self, z: f64) -> Result<TolerantSection, GeometryError> {
        self.section_with(z, true, None, None)
    }

    /// [`Self::section`] under the caller's control. It stops with
    /// [`GeometryErrorCode::Cancelled`] when `cancel` is set (checked on entry and every
    /// [`CANCEL_STRIDE`] items of each of its phases). Densification on the true curve, which
    /// [`Self::section_demand`] does not bound, asks `admit(bytes)` before each growth of its
    /// buffers, and before the buffers sized by its result: `bytes` is heap it is about to hold
    /// in addition to everything admitted before, and an `Err` from `admit` stops the section
    /// and is returned unchanged. With `demand` the section's [`Self::section_demand`] and
    /// `admitted` the sum of every `bytes` it asked for:
    /// - the heap it holds at any moment of the call is at most
    ///   `demand.scratch_bytes + demand.output_bytes + admitted`;
    /// - the heap the returned section holds (its buffers' capacities) is at most
    ///   `demand.output_bytes + admitted`.
    ///
    /// All of it is released when the call returns, but for the returned section.
    pub fn section_admitted(
        &self,
        z: f64,
        cancel: &AtomicBool,
        admit: &mut dyn FnMut(usize) -> Result<(), GeometryError>,
    ) -> Result<TolerantSection, GeometryError> {
        self.section_with(z, true, Some(cancel), Some(admit))
    }

    /// What [`Self::section_admitted`] at `z` works on and holds besides its densification,
    /// known before it runs: from the one bucket of triangles the plane can cross and the sizes
    /// of the collections built from them, including hash-table control bytes and growth by
    /// doubling with the old and new buffers both live. A caller holds
    /// `scratch_bytes + output_bytes` for the call and admits densification as it asks.
    pub fn section_demand(&self, z: f64) -> Result<SectionDemand, GeometryError> {
        if !z.is_finite() {
            return Err(unavailable("tolerant section height is not finite"));
        }
        Ok(SectionDemand::for_triangles(
            self.bucket_at(z).map_or(0, |b| self.buckets[b].len()),
        ))
    }

    fn bucket_at(&self, z: f64) -> Option<usize> {
        let offset = (z - self.z_min) / BUCKET_MM;
        (offset >= 0.0 && (offset as usize) < self.buckets.len()).then_some(offset as usize)
    }

    // Raw support traces label outgoing segments by triangle identity. Their
    // connectivity is constant strictly between consecutive mesh vertex heights.
    // Every collection is sized from the bucket's triangles up front (see
    // `SectionDemand::for_triangles`), so its growth is the bound's.
    fn section_with(
        &self,
        z: f64,
        analytic: bool,
        cancel: Option<&AtomicBool>,
        mut admit: Option<&mut dyn FnMut(usize) -> Result<(), GeometryError>>,
    ) -> Result<TolerantSection, GeometryError> {
        let check = |i: usize| -> Result<(), GeometryError> {
            match cancel {
                Some(cancel) if i.is_multiple_of(CANCEL_STRIDE) => crate::check_cancel(cancel),
                _ => Ok(()),
            }
        };
        check(0)?;
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
        let Some(bucket) = self.bucket_at(z) else {
            return Ok(empty);
        };
        let bucket = &self.buckets[bucket];
        let triangles = bucket.len();
        // A vertex exactly on the plane counts as above it, so each triangle
        // gives zero or two crossings and no vertex is shared by more than two.
        let above = |v: u32| self.positions[v as usize][2] >= z;
        let crossing = |a: u32, b: u32| -> [f64; 2] {
            let (pa, pb) = (self.positions[a as usize], self.positions[b as usize]);
            let t = (z - pa[2]) / (pb[2] - pa[2]);
            [pa[0] + t * (pb[0] - pa[0]), pa[1] + t * (pb[1] - pa[1])]
        };
        let key = |a: u32, b: u32| if a < b { (a, b) } else { (b, a) };
        let mut segments: Vec<Segment> = Vec::with_capacity(triangles);
        // Each crossing with the face of each triangle that uses it.
        let mut points: HashMap<Key, Crossing> = HashMap::with_capacity(2 * triangles);
        for (i, &index) in bucket.iter().enumerate() {
            check(i)?;
            let [a, b, c] = self.triangles[index as usize];
            let face = if analytic {
                self.faces[index as usize]
            } else {
                index
            };
            let (mut ends, mut count) = ([(0, 0); 2], 0);
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
                    if count < 2 {
                        ends[count] = k;
                    }
                    count += 1;
                }
            }
            if count == 2 {
                segments.push((ends[0], ends[1], face));
            }
        }
        if segments.is_empty() {
            return Ok(empty);
        }
        if analytic {
            for (i, (p, first, second)) in points.values_mut().enumerate() {
                check(i)?;
                *p = self.exact_crossing(*p, z, *first, *second);
            }
        }
        // Chain by crossing identity: open chains from degree-one crossings
        // first, then the remaining closed cycles. Incidence is a sorted list
        // of (crossing, segment), each crossing's segments one run of it.
        let mut incident: Vec<(Key, usize)> = Vec::with_capacity(2 * segments.len());
        for (i, (p, q, _)) in segments.iter().enumerate() {
            incident.push((*p, i));
            incident.push((*q, i));
        }
        incident.sort_unstable();
        let run = |node: Key| -> &[(Key, usize)] {
            let start = incident.partition_point(|(k, _)| *k < node);
            let end = incident.partition_point(|(k, _)| *k <= node);
            &incident[start..end]
        };
        let mut used = vec![false; segments.len()];
        let mut chains: Vec<Chain> = Vec::with_capacity(segments.len());
        let walk = |start_node: Key, first: usize, used: &mut Vec<bool>| {
            let (mut ring, mut faces) = (Vec::new(), Vec::new());
            let (mut node, mut segment) = (start_node, first);
            loop {
                used[segment] = true;
                let (p, q, face) = segments[segment];
                ring.push(points[&node].0);
                faces.push(face);
                node = if p == node { q } else { p };
                if node == start_node {
                    return (ring, faces, true);
                }
                match run(node).iter().map(|(_, s)| *s).find(|&s| !used[s]) {
                    Some(next) => segment = next,
                    None => {
                        ring.push(points[&node].0);
                        return (ring, faces, false);
                    }
                }
            }
        };
        let mut ends: Vec<Key> = Vec::with_capacity(points.len());
        for group in incident.chunk_by(|a, b| a.0 == b.0) {
            if group.len() % 2 == 1 {
                ends.push(group[0].0);
            }
        }
        for (i, end) in ends.iter().enumerate() {
            check(i)?;
            if let Some(first) = run(*end).iter().map(|(_, s)| *s).find(|&s| !used[s]) {
                chains.push(walk(*end, first, &mut used));
            }
        }
        drop(ends);
        for first in 0..segments.len() {
            check(first)?;
            if !used[first] {
                let start_node = segments[first].0;
                chains.push(walk(start_node, first, &mut used));
            }
        }
        // Join open chains end to start within the join tolerance.
        let mut joined_gaps = 0;
        let mut max_gap: f64 = 0.0;
        let mut open: Vec<Ring> = Vec::with_capacity(chains.len());
        let mut rings: Vec<Ring> = Vec::with_capacity(chains.len());
        for (ring, faces, closed) in chains {
            if closed {
                rings.push((ring, faces));
            } else {
                open.push((ring, faces));
            }
        }
        let mut steps = 0usize;
        while let Some((mut ring, mut faces)) = open.pop() {
            loop {
                steps += 1;
                check(steps)?;
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
        drop(open);
        // Validate on the result: drop slivers, then orient by nesting depth.
        let sliver = if analytic {
            self.policy.profile_mm * self.policy.profile_mm
        } else {
            0.0
        };
        let mut dropped_slivers = 0;
        let mut loops = Vec::with_capacity(rings.len());
        for (i, (mut points, mut faces)) in rings.into_iter().enumerate() {
            check(i)?;
            dedupe(&mut points, &mut faces);
            if analytic {
                self.densify(&mut points, &mut faces, z, &mut admit)?;
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
        let mut depths: Vec<usize> = Vec::with_capacity(loops.len());
        for i in 0..loops.len() {
            check(i)?;
            let probe = loops[i].points[0];
            depths.push(
                (0..loops.len())
                    .filter(|&j| j != i && loops[j].area.abs() > loops[i].area.abs())
                    .filter(|&j| contains(&loops[j].points, probe))
                    .count(),
            );
        }
        for (l, depth) in loops.iter_mut().zip(depths) {
            let outer = depth % 2 == 0;
            if (l.area > 0.0) != outer {
                l.points.reverse();
                l.faces.reverse();
                l.faces.rotate_left(1);
                l.area = -l.area;
            }
        }
        let total: usize = loops.iter().map(|l| l.faces.len()).sum();
        if let Some(admit) = admit.as_mut() {
            admit(total.saturating_mul(std::mem::size_of::<u32>()))?;
        }
        let mut crossed: Vec<u32> = Vec::with_capacity(total);
        crossed.extend(loops.iter().flat_map(|l| l.faces.iter().copied()));
        crossed.sort_unstable();
        crossed.dedup();
        let mut circles = Vec::with_capacity(if analytic { 2 * crossed.len() } else { 0 });
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
    fn densify(
        &self,
        points: &mut Vec<[f64; 2]>,
        faces: &mut Vec<u32>,
        z: f64,
        admit: &mut Option<&mut dyn FnMut(usize) -> Result<(), GeometryError>>,
    ) -> Result<(), GeometryError> {
        let sag = self.policy.profile_mm * 0.25;
        let reach = self.reach;
        if points.len() < 2 {
            return Ok(());
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
            // Depth-first midpoint subdivision, at most DENSIFY_PIECES pieces per chord.
            let mut stack = vec![(a, b, 0u8)];
            let mut inserted = Vec::new();
            while let Some((a, b, depth)) = stack.pop() {
                let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
                let Some(on) = surface.project(mid, z, reach) else {
                    continue;
                };
                if (on[0] - mid[0]).hypot(on[1] - mid[1]) <= sag || depth >= DENSIFY_DEPTH {
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
            // Grow by doubling, each new pair of buffers admitted before it is made (the old
            // pair, held until the move, was within the demand or admitted before).
            let needed = dense_points.len() + along.len();
            if needed > dense_points.capacity() {
                let capacity = needed.max(dense_points.capacity().saturating_mul(2));
                if let Some(admit) = admit.as_mut() {
                    admit(capacity.saturating_mul(
                        std::mem::size_of::<[f64; 2]>() + std::mem::size_of::<u32>(),
                    ))?;
                }
                dense_points.reserve_exact(capacity - dense_points.len());
                dense_faces.reserve_exact(capacity - dense_faces.len());
            }
            for (_, p) in along {
                dense_points.push(p);
                dense_faces.push(face);
            }
        }
        dense_points.shrink_to_fit();
        dense_faces.shrink_to_fit();
        *points = dense_points;
        *faces = dense_faces;
        Ok(())
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

    fn preparation_model() -> TolerantModel {
        TolerantModel::indexed(
            vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [2, 0, 3]],
            vec![0; 4],
            vec![None],
            TolerantPolicy::default(),
            0.01,
            1,
        )
    }

    #[test]
    fn preparation_build_does_not_block_readers_and_concurrent_publish_is_first_wins() {
        let cache = PreparationCache::default();
        let cancel = AtomicBool::new(false);
        let policy = TolerantPolicy::default();
        std::thread::scope(|scope| {
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let cache_ref = &cache;
            let cancel_ref = &cancel;
            let slow = scope.spawn(move || {
                cache_ref.prepare(policy, cancel_ref, || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(preparation_model())
                })
            });
            started_rx.recv().unwrap();
            let (read_tx, read_rx) = std::sync::mpsc::channel();
            scope.spawn(move || read_tx.send(cache_ref.retained_bytes()).unwrap());
            let read = read_rx.recv_timeout(std::time::Duration::from_secs(5));
            if read.is_err() {
                release_tx.send(()).unwrap();
                panic!("resident accounting blocked behind preparation");
            }
            assert_eq!(read.unwrap(), 0);
            let (winner, outcome) = cache
                .prepare(policy, &cancel, || Ok(preparation_model()))
                .unwrap();
            assert_eq!(
                outcome,
                PreparationOutcome::Published(winner.retained_bytes())
            );
            release_tx.send(()).unwrap();
            let (other, outcome) = slow.join().unwrap().unwrap();
            assert!(Arc::ptr_eq(&winner, &other));
            assert_eq!(outcome, PreparationOutcome::Hit);
            let (failed, outcome) = cache
                .prepare(policy, &cancel, || panic!("failed-face result rebuilt"))
                .unwrap();
            assert_eq!(failed.failed_faces, 1);
            assert_eq!(outcome, PreparationOutcome::Hit);
        });
    }

    #[test]
    fn cancelled_preparation_is_not_published() {
        let cache = PreparationCache::default();
        let cancel = AtomicBool::new(false);
        let policy = TolerantPolicy::default();
        let failure = cache
            .prepare(policy, &cancel, || {
                cancel.store(true, Ordering::Release);
                Ok(preparation_model())
            })
            .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::Cancelled);
        assert_eq!(cache.retained_bytes(), 0);
        cancel.store(false, Ordering::Release);
        let (_, outcome) = cache
            .prepare(policy, &cancel, || Ok(preparation_model()))
            .unwrap();
        assert!(matches!(outcome, PreparationOutcome::Published(_)));
    }

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
