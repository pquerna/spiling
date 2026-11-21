// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{LayerElevation, preflight};
use crate::clearance_screen::{BLOCK_PATHS, ObjectClearanceIndex};
use crate::{check_cancel, error};
use geo::MultiPolygon;
use rayon::prelude::*;
use spiling_contracts::geometry::OccurrenceId;
use spiling_contracts::manufacturing::program_coordinates::{
    project_xyz6_bead_height_mm, project_xyz6_coordinate_mm, project_xyz6_segment_bead_height_mm,
    xyz6_coordinate_error_mm,
};
use spiling_contracts::manufacturing::*;
use std::sync::{Arc, atomic::AtomicBool};

mod clearance_index;
#[cfg(test)]
mod clearance_tests;
mod protected_proof_memo;
mod withdrawal;
pub(super) use withdrawal::Sweep as SupportWithdrawal;

pub(super) const MATERIAL_ARC_BYTES: usize =
    std::mem::size_of::<MultiPolygon<f64>>() + 2 * std::mem::size_of::<usize>();

pub(super) struct SupportClearanceGeometry<'a> {
    pub sections: &'a [super::SectionSpan],
    pub schedule: &'a [LayerElevation],
    pub protected: &'a MultiPolygon<f64>,
    pub scopes: &'a [super::support::VoidScope],
    pub index_bytes: usize,
}

struct ForbiddenPolygonScope<'a> {
    polygon: &'a geo::Polygon<f64>,
    height: spiling_geometry::ProtectedVoidHeightScope,
    label: &'a str,
    memo_bytes: usize,
}

struct ActualDepositionScope<'a> {
    layers: &'a [PrintLayer],
    schedule: &'a [LayerElevation],
    vertical: [f64; 2],
    screen: Option<&'a ObjectClearanceIndex>,
}

struct DepositionEnvelope {
    vertical: [f64; 2],
    inventory: usize,
}

impl DepositionEnvelope {
    /// Each preparation and scan charge is bounded by four ordered traversals
    /// of the Cartesian inventory of layers, path slots, and point slots.
    /// Leave the exhaustive diagnostic order in authority near counter overflow.
    fn screen_work_fits(&self, work: usize) -> bool {
        self.inventory
            .checked_mul(self.inventory)
            .and_then(|pairs| pairs.checked_mul(4))
            .is_some_and(|bound| bound <= usize::MAX - work)
    }
}

struct PolygonDeposition<'a> {
    layers: &'a [PrintLayer],
    range: std::ops::Range<usize>,
    index: Option<&'a clearance_index::ClearanceIndex>,
}

#[derive(Default)]
struct ClearanceTrace {
    enabled: bool,
    memo_enabled: bool,
    polygons: std::cell::Cell<usize>,
    layers: std::cell::Cell<usize>,
    paths: std::cell::Cell<usize>,
    support_paths: std::cell::Cell<usize>,
    bounds_points: std::cell::Cell<usize>,
    path_rejects: std::cell::Cell<usize>,
    edges: std::cell::Cell<usize>,
    height_rejects: std::cell::Cell<usize>,
    polygon_edge_block_queries: std::cell::Cell<usize>,
    polygon_edge_block_clears: std::cell::Cell<usize>,
    polygon_tests: std::cell::Cell<usize>,
    protected_memo_hits: std::cell::Cell<usize>,
    deposited_support_paths: std::cell::Cell<usize>,
    deposited_object_paths: std::cell::Cell<usize>,
    deposited_path_rejects: std::cell::Cell<usize>,
    deposited_block_queries: std::cell::Cell<usize>,
    deposited_block_skipped_slots: std::cell::Cell<usize>,
    deposited_index_bytes: std::cell::Cell<usize>,
    edge_pairs: std::cell::Cell<usize>,
    edge_pair_rejects: std::cell::Cell<usize>,
    height_pair_rejects: std::cell::Cell<usize>,
    pair_tests: std::cell::Cell<usize>,
    index_bytes: std::cell::Cell<usize>,
    skipped_paths: std::cell::Cell<usize>,
    raw_wave_pieces: std::cell::Cell<usize>,
    raw_inline_pieces: std::cell::Cell<usize>,
    raw_waves: std::cell::Cell<usize>,
    raw_declined: std::cell::Cell<usize>,
}

impl ClearanceTrace {
    fn add(&self, counter: &std::cell::Cell<usize>, amount: usize) {
        if self.enabled {
            counter.set(counter.get().saturating_add(amount));
        }
    }

    fn merge_polygon(&self, local: &Self) {
        for (total, part) in [
            (&self.polygons, &local.polygons),
            (&self.layers, &local.layers),
            (&self.paths, &local.paths),
            (&self.support_paths, &local.support_paths),
            (&self.bounds_points, &local.bounds_points),
            (&self.path_rejects, &local.path_rejects),
            (&self.edges, &local.edges),
            (&self.height_rejects, &local.height_rejects),
            (
                &self.polygon_edge_block_queries,
                &local.polygon_edge_block_queries,
            ),
            (
                &self.polygon_edge_block_clears,
                &local.polygon_edge_block_clears,
            ),
            (&self.polygon_tests, &local.polygon_tests),
            (&self.protected_memo_hits, &local.protected_memo_hits),
            (&self.skipped_paths, &local.skipped_paths),
            (&self.raw_wave_pieces, &local.raw_wave_pieces),
            (&self.raw_inline_pieces, &local.raw_inline_pieces),
            (&self.raw_waves, &local.raw_waves),
        ] {
            self.add(total, part.get());
        }
    }

    fn merge_deposited_layer(&self, layer: &Self) {
        for (total, local) in [
            (
                &self.deposited_support_paths,
                &layer.deposited_support_paths,
            ),
            (&self.deposited_object_paths, &layer.deposited_object_paths),
            (&self.deposited_path_rejects, &layer.deposited_path_rejects),
            (
                &self.deposited_block_queries,
                &layer.deposited_block_queries,
            ),
            (
                &self.deposited_block_skipped_slots,
                &layer.deposited_block_skipped_slots,
            ),
            (&self.edge_pairs, &layer.edge_pairs),
            (&self.edge_pair_rejects, &layer.edge_pair_rejects),
            (&self.height_pair_rejects, &layer.height_pair_rejects),
            (&self.pair_tests, &layer.pair_tests),
        ] {
            self.add(total, local.get());
        }
    }

    fn report(&self, phase: &str, started: std::time::Instant) {
        if self.enabled {
            eprintln!(
                "{}",
                serde_json::json!({
                    "event": "support_clearance_trace", "phase": phase,
                    "phase_seconds": started.elapsed().as_secs_f64(),
                    "index_bytes": self.index_bytes.get(), "skipped_paths": self.skipped_paths.get(),
                    "polygons": self.polygons.get(), "layers": self.layers.get(),
                    "paths": self.paths.get(), "support_paths": self.support_paths.get(),
                    "bounds_points": self.bounds_points.get(), "path_rejects": self.path_rejects.get(),
                    "edges": self.edges.get(), "height_rejects": self.height_rejects.get(),
                    "polygon_edge_block_queries": self.polygon_edge_block_queries.get(),
                    "polygon_edge_block_clears": self.polygon_edge_block_clears.get(),
                    "protected_memo_hits": self.protected_memo_hits.get(),
                    "polygon_tests": self.polygon_tests.get(),
                    "deposited_support_paths": self.deposited_support_paths.get(),
                    "deposited_object_paths": self.deposited_object_paths.get(),
                    "deposited_path_rejects": self.deposited_path_rejects.get(),
                    "deposited_block_queries": self.deposited_block_queries.get(),
                    "deposited_block_skipped_slots": self.deposited_block_skipped_slots.get(),
                    "deposited_index_bytes": self.deposited_index_bytes.get(),
                    "edge_pairs": self.edge_pairs.get(), "edge_pair_rejects": self.edge_pair_rejects.get(),
                    "height_pair_rejects": self.height_pair_rejects.get(), "pair_tests": self.pair_tests.get(),
                    "raw_waves": self.raw_waves.get(), "raw_wave_pieces": self.raw_wave_pieces.get(),
                    "raw_inline_pieces": self.raw_inline_pieces.get(), "raw_declined": self.raw_declined.get(),
                })
            );
        }
    }
}

pub(super) struct NativeSpan {
    pub end: u32,
    /// All original source slabs carry exact closed-height invariance, not
    /// merely a bounded moving radial material product.
    pub material: Arc<MultiPolygon<f64>>,
    pub finished: Vec<(OccurrenceId, Arc<MultiPolygon<f64>>)>,
    pub contributions: Vec<(OccurrenceId, Arc<MultiPolygon<f64>>, usize)>,
    pub occupied: Arc<super::ContinuousOccupied>,
    pub source_area_mm2: f64,
    pub point_count: usize,
    pub material_bytes: usize,
    pub finished_bytes: usize,
}

impl NativeSpan {
    /// A one-layer ADR012 tolerant span: no invariance or inner prism.
    #[expect(clippy::too_many_arguments)]
    pub(super) fn tolerant(
        end: u32,
        material: Arc<MultiPolygon<f64>>,
        contributions: Vec<(OccurrenceId, Arc<MultiPolygon<f64>>, usize)>,
        finished: Vec<(OccurrenceId, Arc<MultiPolygon<f64>>)>,
        finished_bytes: usize,
        occupied: Arc<super::ContinuousOccupied>,
        source_area_mm2: f64,
        point_count: usize,
        material_bytes: usize,
    ) -> Self {
        Self {
            end,
            material,
            finished,
            contributions,
            occupied,
            source_area_mm2,
            point_count,
            material_bytes,
            finished_bytes,
        }
    }
}

pub(super) fn polygon_bytes(polygon: &mut geo::Polygon<f64>) -> usize {
    // Temporarily move the owned buffers out to observe the otherwise hidden
    // interior-record capacity. Reconstruction reuses every allocation.
    let (exterior, interiors) = std::mem::replace(
        polygon,
        geo::Polygon::new(geo::LineString(Vec::new()), Vec::new()),
    )
    .into_inner();
    let bytes = exterior
        .0
        .capacity()
        .saturating_mul(std::mem::size_of::<geo::Coord<f64>>())
        .saturating_add(
            interiors
                .capacity()
                .saturating_mul(std::mem::size_of::<geo::LineString<f64>>()),
        )
        .saturating_add(interiors.iter().fold(0usize, |bytes, ring| {
            bytes.saturating_add(
                ring.0
                    .capacity()
                    .saturating_mul(std::mem::size_of::<geo::Coord<f64>>()),
            )
        }));
    *polygon = geo::Polygon::new(exterior, interiors);
    bytes
}

pub(super) fn material_bytes(material: &mut MultiPolygon<f64>) -> usize {
    let bytes = material
        .0
        .capacity()
        .saturating_mul(std::mem::size_of::<geo::Polygon<f64>>());
    material.0.iter_mut().fold(bytes, |bytes, polygon| {
        bytes.saturating_add(polygon_bytes(polygon))
    })
}

fn occupied_bytes(occupied: &super::ContinuousOccupied) -> usize {
    occupied
        .pieces
        .capacity()
        .saturating_mul(std::mem::size_of::<super::OccupiedPiece>())
        .saturating_add(occupied.payload_bytes)
        .saturating_add(
            occupied.tolerant.capacity() * std::mem::size_of::<super::TolerantOccupied>(),
        )
}

/// Caller-owned D/C/M buffers coexist with the next native query. Shared
/// continuous occupancy and image-only mandatory M are charged only once.
pub(super) fn output_bytes(
    sections: &Vec<super::SectionSpan>,
    surfaces: &Vec<super::NativeSurfaceSpan>,
    section_material_bytes: usize,
) -> usize {
    let mut bytes = section_material_bytes
        .saturating_add(
            sections
                .capacity()
                .saturating_mul(std::mem::size_of::<super::SectionSpan>()),
        )
        .saturating_add(
            surfaces
                .capacity()
                .saturating_mul(std::mem::size_of::<super::NativeSurfaceSpan>()),
        );
    for (index, section) in sections.iter().enumerate() {
        if !sections[..index]
            .iter()
            .any(|prior| Arc::ptr_eq(&prior.material, &section.material))
        {
            bytes = bytes.saturating_add(MATERIAL_ARC_BYTES);
        }
        bytes = bytes.saturating_add(
            section.process_domains.capacity() * std::mem::size_of::<super::ProcessDomain>(),
        );
        for (domain_index, domain) in section.process_domains.iter().enumerate() {
            bytes = bytes
                .saturating_add(
                    domain.coverage.contributions.capacity()
                        * std::mem::size_of::<ProcessRegionContribution>(),
                )
                .saturating_add(
                    domain
                        .coverage
                        .contributions
                        .iter()
                        .map(|source| source.definition_id.allocated_bytes())
                        .sum::<usize>(),
                );
            let shared = Arc::ptr_eq(&domain.material, &section.material)
                || section.process_domains[..domain_index]
                    .iter()
                    .any(|other| Arc::ptr_eq(&domain.material, &other.material))
                || sections[..index].iter().any(|prior| {
                    Arc::ptr_eq(&domain.material, &prior.material)
                        || prior
                            .process_domains
                            .iter()
                            .any(|other| Arc::ptr_eq(&domain.material, &other.material))
                });
            if !shared {
                bytes = bytes
                    .saturating_add(domain.bytes)
                    .saturating_add(MATERIAL_ARC_BYTES);
            }
        }
        if let Some(occupied) = &section.occupied
            && !sections[..index].iter().any(|prior| {
                prior
                    .occupied
                    .as_ref()
                    .is_some_and(|prior| Arc::ptr_eq(prior, occupied))
            })
        {
            bytes = bytes.saturating_add(occupied_bytes(occupied));
        }
    }
    for surface in surfaces {
        bytes = bytes
            .saturating_add(
                surface
                    .occurrences
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(OccurrenceId, Arc<MultiPolygon<f64>>)>()),
            )
            .saturating_add(
                surface
                    .image_islands
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(OccurrenceId, usize)>()),
            );
        bytes = bytes.saturating_add(surface.native_bytes);
        // Conservatively cover the retained BTree nodes, not only their values.
        // A sparse std BTree node still owns all 11 key/value slots and,
        // for internal nodes, 12 child slots plus its header. Reserving 16
        // (entry + pointer) units per live entry also covers a one-entry root;
        // every allocated node contains at least one entry.
        bytes = bytes.saturating_add(surface.required.len().saturating_mul(
            16 * (std::mem::size_of::<(OccurrenceId, Arc<MultiPolygon<f64>>)>()
                + std::mem::size_of::<usize>()),
        ));
    }
    bytes
}

// Pinned geo0.32 uses robust1.2 orient2d for filled-polygon intersection.
// That implementation explicitly does not guarantee underflow cases. Within
// this fixed dyadic arithmetic envelope, every input is a multiple of 2^-180,
// every exact orientation term a multiple of 2^-360; splitter intermediates,
// nonzero products/error bounds stay normal/finite. This is predicate conditioning,
// not a geometric tolerance. Outside it the pair stays connected, not disjoint.
pub(super) fn merge_predicate_coordinate(value: f64) -> bool {
    const LOWER: f64 = f64::from_bits((1023 - 128) << 52);
    const UPPER: f64 = f64::from_bits((1023 + 128) << 52);
    value == 0.0 || (LOWER..=UPPER).contains(&value.abs())
}

pub(super) fn encode_polygons<'a>(
    polygons: impl Iterator<Item = &'a geo::Polygon<f64>> + Clone,
    sample: f64,
    cancel: &AtomicBool,
) -> Result<Vec<spiling_geometry::SectionLoopMm>, ManufacturingError> {
    let mut ring_count = 0usize;
    let mut point_count = 0usize;
    for polygon in polygons.clone() {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            check_cancel(cancel)?;
            ring_count = ring_count.checked_add(1).ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native polygon ring count overflow",
                )
            })?;
            point_count = point_count
                .checked_add(ring.0.len())
                .filter(|n| *n <= crate::working_set::units())
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native polygon point budget exceeded",
                    )
                })?;
        }
    }
    ring_count
        .checked_mul(std::mem::size_of::<spiling_geometry::SectionLoopMm>())
        .and_then(|n| {
            point_count
                .checked_mul(std::mem::size_of::<[f64; 3]>())
                .and_then(|points| n.checked_add(points))
        })
        .filter(|n| *n <= spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "native polygon encoding byte budget exceeded",
            )
        })?;
    let mut loops = Vec::with_capacity(ring_count);
    for polygon in polygons {
        for (is_hole, ring) in std::iter::once((false, polygon.exterior()))
            .chain(polygon.interiors().iter().map(|ring| (true, ring)))
        {
            let mut vertices = Vec::with_capacity(ring.0.len());
            for point in &ring.0 {
                check_cancel(cancel)?;
                vertices.push([point.x, point.y, sample]);
            }
            loops.push(spiling_geometry::SectionLoopMm {
                is_hole,
                points_mm: vertices,
            });
        }
    }
    Ok(loops)
}

pub(super) fn split_layer(
    schedule: &mut Vec<LayerElevation>,
    index: u32,
    context: &str,
) -> Result<(), ManufacturingError> {
    let layer = &schedule[index as usize];
    let midpoint = layer.lower_z + (layer.z - layer.lower_z) * 0.5;
    let lower_height = midpoint - layer.lower_z;
    let upper_height = layer.z - midpoint;
    if lower_height.min(upper_height)
        <= 2.0 * spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM
    {
        return Err(error(
            ManufacturingErrorCode::NativeShapeErrorExceeded,
            format!(
                "{context} cannot certify [{:.9}, {:.9}] mm at process-resolvable thickness",
                layer.lower_z, layer.z
            ),
        ));
    }
    split_resolved_layer(schedule, index, 0.0, context)
}

/// The first mapped field whose vertical error over this scheduled layer
/// exceeds its declared bound, as the refinement cause. Both lanes refine the
/// layer until none does.
pub(super) fn surface_vertical_error(
    admission: &preflight::Admission,
    schedule: &[LayerElevation],
    index: u32,
) -> Result<Option<ManufacturingError>, ManufacturingError> {
    let layer = &schedule[index as usize];
    for (occurrence_id, fields) in &admission.surface_fields {
        for field in fields {
            if let Some(allowed) = field.allowed_vertical_error() {
                let bound = field.vertical_error_bound(layer.lower_z, layer.z, layer.sample)?;
                if bound > allowed {
                    return Ok(Some(error(
                        ManufacturingErrorCode::NativeShapeErrorExceeded,
                        format!(
                            "surface field at layer {index}, interval [{:.17e}, {:.17e}] mm, sample {:.17e} mm, occurrence {}: vertical error bound {bound:.17e} mm exceeds policy {allowed:.17e} mm",
                            layer.lower_z,
                            layer.z,
                            layer.sample,
                            occurrence_id.get()
                        ),
                    )));
                }
            }
        }
    }
    Ok(None)
}

/// Split one tolerant layer for a mapped field's vertical error, at the
/// captured process minimum, keeping the cause in any refusal.
pub(super) fn refine_surface_layer(
    schedule: &mut Vec<LayerElevation>,
    index: u32,
    minimum_height_mm: f64,
    cause: &ManufacturingError,
) -> Result<(), ManufacturingError> {
    split_resolved_layer(
        schedule,
        index,
        minimum_height_mm,
        "surface field vertical error",
    )
    .map_err(|mut failure| {
        failure.message.push_str("; ");
        failure.message.push_str(&cause.to_string());
        failure
    })
}

pub(super) fn split_resolved_layer(
    schedule: &mut Vec<LayerElevation>,
    index: u32,
    minimum_height_mm: f64,
    context: &str,
) -> Result<(), ManufacturingError> {
    split_resolved_layer_admitted(schedule, index, minimum_height_mm, context, |_| Ok(()))
}

/// Admit caller-owned growth after child heights qualify and before mutation.
pub(super) fn split_resolved_layer_admitted(
    schedule: &mut Vec<LayerElevation>,
    index: u32,
    minimum_height_mm: f64,
    context: &str,
    admit_growth: impl FnOnce(&mut Vec<LayerElevation>) -> Result<(), ManufacturingError>,
) -> Result<(), ManufacturingError> {
    let layer = &schedule[index as usize];
    let midpoint = layer.lower_z + (layer.z - layer.lower_z) * 0.5;
    let lower_height = midpoint - layer.lower_z;
    let upper_height = layer.z - midpoint;
    let lower_sample = layer.lower_z + lower_height * 0.5;
    let upper_sample = midpoint + upper_height * 0.5;
    let unresolved = || {
        error(
            ManufacturingErrorCode::NativeShapeErrorExceeded,
            format!(
                "{context} cannot certify [{:.17e}, {:.17e}] mm at process-resolvable thickness",
                layer.lower_z, layer.z
            ),
        )
    };
    if !minimum_height_mm.is_finite()
        || minimum_height_mm < 0.0
        || !(layer.lower_z < lower_sample
            && lower_sample < midpoint
            && midpoint < upper_sample
            && upper_sample < layer.z)
    {
        return Err(unresolved());
    }
    let endpoints = [layer.lower_z, midpoint, layer.z];
    let commanded = [
        project_xyz6_bead_height_mm(endpoints[0], 0.0)?,
        project_xyz6_bead_height_mm(endpoints[1], 0.0)?,
        project_xyz6_bead_height_mm(endpoints[2], 0.0)?,
    ];
    for child in 0..2 {
        let physical =
            program_coordinates::enclosed_vertical_gap_mm(endpoints[child + 1], endpoints[child])?;
        let command = program_coordinates::enclosed_vertical_gap_mm(
            commanded[child + 1][0],
            commanded[child][1],
        )?;
        if physical <= 0.0
            || physical < minimum_height_mm
            || command <= 0.0
            || command < minimum_height_mm
        {
            return Err(unresolved());
        }
    }
    if schedule.len() >= crate::working_set::records::<LayerElevation>() {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "native/field refinement exceeds the manufacturing working set",
        ));
    }
    let upper = LayerElevation {
        lower_z: midpoint,
        z: layer.z,
        sample: upper_sample,
        height: upper_height,
        course: layer.course,
    };
    let lower = LayerElevation {
        lower_z: layer.lower_z,
        z: midpoint,
        sample: lower_sample,
        height: lower_height,
        course: layer.course,
    };
    admit_growth(schedule)?;
    schedule[index as usize] = lower;
    schedule.insert(index as usize + 1, upper);
    Ok(())
}

/// Final publication gate over actual emitted support capsules. Boolean masks
/// are candidate construction only; the shared geometry predicate sees every
/// raw continuous occupied piece and every relevant positive protected void.
pub(super) fn certify_support_clearance(
    layers: &[PrintLayer],
    geometry: &SupportClearanceGeometry<'_>,
    policy: &SupportPolicy,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    support_work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<SupportWithdrawal>, ManufacturingError> {
    certify_support_clearance_traced(
        layers,
        geometry,
        policy,
        native_work,
        support_work,
        &ClearanceTrace {
            enabled: std::env::var_os("SPILING_TRACE_SUPPORT_CLEARANCE").is_some(),
            memo_enabled: true,
            ..ClearanceTrace::default()
        },
        cancel,
    )
}

fn certify_support_clearance_traced(
    layers: &[PrintLayer],
    geometry: &SupportClearanceGeometry<'_>,
    policy: &SupportPolicy,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    support_work: &mut usize,
    trace: &ClearanceTrace,
    cancel: &AtomicBool,
) -> Result<Option<SupportWithdrawal>, ManufacturingError> {
    let SupportClearanceGeometry {
        sections,
        schedule,
        protected,
        scopes,
        index_bytes,
    } = *geometry;
    if protected.0.len() != scopes.len() {
        return Err(error(
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "actual support clearance requires one certified height scope for every protected native void mask",
        ));
    }
    if layers.len() != schedule.len() {
        return Err(error(
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "actual support clearance lost the complete actual-height schedule",
        ));
    }
    let SupportPolicy::Breakaway {
        xy_clearance_mm,
        z_gap_mm,
        ..
    } = *policy
    else {
        return Ok(None);
    };
    let Some(deposition) = deposition_vertical_extensions(layers, schedule, support_work, cancel)?
    else {
        // Exhausted the actual paths: there are no emitted support segments,
        // rather than trusting a generated-support flag or skipping a proof.
        return Ok(None);
    };
    let vertical = deposition.vertical;
    let index_started = std::time::Instant::now();
    let index = clearance_index::ClearanceIndex::build(
        layers.iter().map(|layer| layer.paths.as_slice()),
        index_bytes,
        support_work,
        cancel,
    )?;
    trace
        .index_bytes
        .set(index.as_ref().map_or(0, |index| index.retained_bytes()));
    trace.report("path_index", index_started);
    let raw_started = std::time::Instant::now();
    let raw_bytes =
        index_bytes.saturating_sub(index.as_ref().map_or(0, |index| index.retained_bytes()));
    let raw = RawOccupancy {
        sections,
        schedule,
        layers,
        vertical,
        index: index.as_ref(),
        clearance: xy_clearance_mm,
    };
    if !certify_raw_occupancy_waves(&raw, raw_bytes, native_work, support_work, trace, cancel)? {
        let task = raw.task();
        visit_raw_pieces(
            &raw,
            &task,
            support_work,
            cancel,
            |polygon, scope, range, work| {
                certify_polygon_capsules(
                    PolygonDeposition {
                        layers,
                        range,
                        index: index.as_ref(),
                    },
                    &ForbiddenPolygonScope {
                        polygon,
                        height: scope,
                        label: "continuous native/finish outer occupancy",
                        memo_bytes: 0,
                    },
                    xy_clearance_mm,
                    native_work,
                    work,
                    trace,
                    None,
                    cancel,
                )?;
                Ok(true)
            },
        )?;
    }
    trace.report("raw_occupancy", raw_started);
    let void_started = std::time::Instant::now();
    let sweep_bytes =
        index_bytes.saturating_sub(index.as_ref().map_or(0, |index| index.retained_bytes()));
    let mut sweep = withdrawal::Sweep::new(sweep_bytes);
    for (polygon, scope) in protected.0.iter().zip(scopes) {
        let leased = scope.has_access();
        let range =
            scoped_support_layer_range(schedule, scope.height, vertical, support_work, cancel)?;
        certify_polygon_capsules(
            PolygonDeposition {
                layers,
                range,
                index: index.as_ref(),
            },
            &ForbiddenPolygonScope {
                polygon,
                height: scope.height,
                label: "projected protected native void",
                memo_bytes: if trace.memo_enabled && !leased {
                    sweep_bytes.saturating_sub(sweep.retained_bytes())
                } else {
                    0
                },
            },
            xy_clearance_mm,
            native_work,
            support_work,
            trace,
            leased.then_some((&mut sweep, scope)),
            cancel,
        )?;
    }
    trace.report("protected_voids", void_started);
    drop(index);
    let withdrawal_started = std::time::Instant::now();
    let sweep = sweep.certify_source(
        schedule,
        sections,
        xy_clearance_mm,
        native_work,
        support_work,
        cancel,
    )?;
    trace.report("withdrawal", withdrawal_started);
    let deposited_started = std::time::Instant::now();
    let deposited_screen = prepare_deposited_screen(
        layers,
        &deposition,
        index_bytes,
        sweep.as_ref().map_or(0, withdrawal::Sweep::retained_bytes),
        support_work,
        cancel,
    )?;
    trace.deposited_index_bytes.set(
        deposited_screen
            .as_ref()
            .map_or(0, ObjectClearanceIndex::retained_bytes),
    );
    // A bounded-fidelity M/D prism or emitted bead can extend outside true T.
    // Native target exclusion therefore does not replace this distinct actual
    // nominal-program footprint proof.
    certify_deposited_layers(
        &ActualDepositionScope {
            layers,
            schedule,
            vertical,
            screen: deposited_screen.as_ref(),
        },
        &deposition,
        index_bytes
            .saturating_sub(sweep.as_ref().map_or(0, withdrawal::Sweep::retained_bytes))
            .saturating_sub(trace.deposited_index_bytes.get()),
        xy_clearance_mm,
        z_gap_mm,
        native_work,
        support_work,
        trace,
        cancel,
    )?;
    trace.report("deposited_pairs", deposited_started);
    Ok(sweep)
}

/// Outward enclosure of an emitted XYZ6 centerline point's displacement from
/// its binary64 source plus its remaining physical-decimal endpoint error
/// (each at most about one half micrometre per axis). Bounds screens only.
pub(super) const XYZ6_SCREEN_SLACK_MM: f64 = program_coordinates::XYZ6_PROJECTION_SCREEN_MM;

/// Raw XY bounds of a validated path and its largest bead width.
fn path_xy_bounds(path: &DepositionPath) -> Option<([[f64; 2]; 2], f64)> {
    let width = match &path.width {
        WidthProfile::Constant { width_mm } => *width_mm,
        WidthProfile::PerEdge { widths_mm } => widths_mm.iter().copied().fold(0.0, f64::max),
    };
    let mut bounds = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
    for p in &path.points_mm {
        for axis in 0..2 {
            bounds[0][axis] = bounds[0][axis].min(p[axis]);
            bounds[1][axis] = bounds[1][axis].max(p[axis]);
        }
    }
    (width.is_finite() && bounds.iter().flatten().all(|v| v.is_finite())).then_some((bounds, width))
}

fn edge_xy_bounds(edge: &[[f64; 3]]) -> [[f64; 2]; 2] {
    [
        [edge[0][0].min(edge[1][0]), edge[0][1].min(edge[1][1])],
        [edge[0][0].max(edge[1][0]), edge[0][1].max(edge[1][1])],
    ]
}

const CLEARANCE_EDGE_BLOCK: usize = 32;

/// A bounded, allocation-free envelope of consecutive checked edges. No
/// block verdict can depend on a malformed endpoint or a narrower width.
fn edge_block_clear<'a>(
    edges: impl Iterator<Item = DepositionEdge<'a>>,
    forbidden: [[f64; 2]; 2],
    clearance: f64,
) -> bool {
    let mut bounds = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
    let mut width: f64 = 0.0;
    for edge in edges {
        width = width.max(edge.width_mm);
        for point in edge.points_mm {
            for (axis, &value) in point[..2].iter().enumerate() {
                if !value.is_finite() {
                    return false;
                }
                bounds[0][axis] = bounds[0][axis].min(value);
                bounds[1][axis] = bounds[1][axis].max(value);
            }
        }
    }
    spiling_geometry::planar_bounds_clear(
        bounds,
        forbidden,
        (width * 0.5 + XYZ6_SCREEN_SLACK_MM).next_up(),
        clearance,
    )
}

struct RawOccupancy<'a> {
    sections: &'a [super::SectionSpan],
    schedule: &'a [LayerElevation],
    layers: &'a [PrintLayer],
    vertical: [f64; 2],
    index: Option<&'a clearance_index::ClearanceIndex>,
    clearance: f64,
}

/// The immutable operands one raw piece's proof borrows on a worker.
struct RawTask<'a> {
    layers: &'a [PrintLayer],
    index: Option<&'a clearance_index::ClearanceIndex>,
    clearance: f64,
}

impl<'a> RawOccupancy<'a> {
    fn task(&self) -> RawTask<'a> {
        RawTask {
            layers: self.layers,
            index: self.index,
            clearance: self.clearance,
        }
    }
}

/// Visits each raw occupied piece with its certified height scope and layer
/// range, in source order. A piece that no actual support path approaches in
/// that range would perform no capsule proof, so it is declined from its
/// conservative bounds before the piece is constructed. Serial and parallel
/// certification share this visitor, so they see the same pieces and work.
fn visit_raw_pieces(
    raw: &RawOccupancy<'_>,
    task: &RawTask<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
    mut visit: impl FnMut(
        &geo::Polygon<f64>,
        spiling_geometry::ProtectedVoidHeightScope,
        std::ops::Range<usize>,
        &mut usize,
    ) -> Result<bool, ManufacturingError>,
) -> Result<(), ManufacturingError> {
    let counter = std::cell::Cell::new(*work);
    let result = (|| {
        for span in raw.sections {
            let interval = [
                raw.schedule[span.start as usize].lower_z,
                raw.schedule[(span.end - 1) as usize].z,
            ];
            let scoped = |piece_scope: Option<[f64; 2]>| {
                let height = piece_scope.map_or(interval, |height| {
                    [interval[0].max(height[0]), interval[1].min(height[1])]
                });
                (height[0] <= height[1]).then_some(
                    spiling_geometry::ProtectedVoidHeightScope::Bounded {
                        lower_mm: height[0],
                        upper_mm: height[1],
                    },
                )
            };
            let charged = |f: &mut dyn FnMut(&mut usize) -> Result<bool, ManufacturingError>| {
                let mut work = counter.get();
                let result = f(&mut work);
                counter.set(work);
                result
            };
            span.visit_occupied_screened(
                cancel,
                |bounds, piece_scope| {
                    let Some(scope) = scoped(piece_scope) else {
                        return Ok(false);
                    };
                    charged(&mut |work| {
                        let range = scoped_support_layer_range(
                            raw.schedule,
                            scope,
                            raw.vertical,
                            work,
                            cancel,
                        )?;
                        raw_piece_meets_support(task, bounds, range, work, cancel)
                    })
                },
                |polygon, piece_scope| {
                    let Some(scope) = scoped(piece_scope) else {
                        return Ok(true);
                    };
                    charged(&mut |work| {
                        let range = scoped_support_layer_range(
                            raw.schedule,
                            scope,
                            raw.vertical,
                            work,
                            cancel,
                        )?;
                        visit(polygon, scope, range, work)
                    })
                },
            )?;
        }
        Ok(())
    })();
    *work = counter.get();
    result
}

/// Whether certifying a piece within these conservative bounds would test any
/// actual support capsule: the same index query and per-path bounds rejection
/// the proof applies, without the proof.
fn raw_piece_meets_support(
    task: &RawTask<'_>,
    bounds: [[f64; 2]; 2],
    range: std::ops::Range<usize>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    if !bounds.iter().flatten().all(|value| value.is_finite()) {
        return Ok(true);
    }
    for ordinal in range {
        let layer = &task.layers[ordinal];
        charge_support_clearance(work, cancel)?;
        let mut meets = false;
        let mut check = |candidate| {
            let range = match candidate {
                clearance_index::Candidates::Charge(amount) => {
                    return charge_support_clearance_range(work, amount, cancel);
                }
                clearance_index::Candidates::Skip { paths, .. } => {
                    return charge_support_clearance_range(work, paths, cancel);
                }
                clearance_index::Candidates::Visit(range) => range.paths,
            };
            for path in &layer.paths[range] {
                charge_support_clearance(work, cancel)?;
                if path.kind.is_model() {
                    continue;
                }
                if !path_xy_bounds(path).is_some_and(|(path_bounds, width)| {
                    spiling_geometry::planar_bounds_clear(
                        path_bounds,
                        bounds,
                        width * 0.5 + XYZ6_SCREEN_SLACK_MM,
                        task.clearance,
                    )
                }) {
                    meets = true;
                    break;
                }
            }
            Ok(())
        };
        if let Some(index) = task.index.and_then(|index| index.layer(ordinal)) {
            index.visit(
                bounds,
                task.clearance,
                XYZ6_SCREEN_SLACK_MM,
                cancel,
                &mut check,
            )?;
        } else {
            check(clearance_index::Candidates::Visit(
                clearance_index::PathRange {
                    paths: 0..layer.paths.len(),
                    nonmodel_paths: 0,
                },
            ))?;
        }
        if meets {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Pieces buffered per worker in one raw-occupancy wave.
const RAW_PIECES_PER_WORKER: usize = 64;

struct RawPieceResult {
    result: Result<(), ManufacturingError>,
    geometry_work: u64,
    scan_work: usize,
    trace: ClearanceTrace,
}

/// Certifies every raw occupied piece against actual support in byte-admitted
/// parallel waves. Pieces are independent: no memo, lease or withdrawal state
/// is shared. Work and trace counters are speculative until every piece has
/// passed; on any failure, or when the waves cannot be admitted, this returns
/// `false` with the caller's counters untouched, and the caller runs the
/// original serial scan, which reproduces the first refusal and its work.
fn certify_raw_occupancy_waves(
    raw: &RawOccupancy<'_>,
    available: usize,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    support_work: &mut usize,
    trace: &ClearanceTrace,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let threads = rayon::current_num_threads();
    if threads < 2 || !native_work.is_monitored() {
        trace.add(&trace.raw_declined, 1);
        return Ok(false);
    }
    let wave = threads.saturating_mul(RAW_PIECES_PER_WORKER);
    let slot =
        std::mem::size_of::<RawPieceResult>().saturating_add(std::mem::size_of::<RawPiece>());
    if wave.saturating_mul(slot) > available {
        trace.add(&trace.raw_declined, 1);
        return Ok(false);
    }
    let mut pending: Vec<RawPiece> = Vec::new();
    let mut results: Vec<RawPieceResult> = Vec::new();
    if pending.try_reserve_exact(wave).is_err() || results.try_reserve_exact(wave).is_err() {
        return Ok(false);
    }
    let piece_bytes_available = available - wave.saturating_mul(slot);
    let mut speculative = Speculative {
        geometry_work: 0,
        trace: ClearanceTrace {
            enabled: trace.enabled,
            ..ClearanceTrace::default()
        },
        failed: false,
    };
    let task = raw.task();
    let mut pending_bytes = 0usize;
    let mut work = *support_work;
    let visited = visit_raw_pieces(
        raw,
        &task,
        &mut work,
        cancel,
        |polygon, scope, range, work| {
            let bytes = cloned_polygon_bytes(polygon);
            if pending.len() == wave || pending_bytes.saturating_add(bytes) > piece_bytes_available
            {
                flush_raw_wave(
                    &task,
                    &mut pending,
                    &mut results,
                    &mut speculative,
                    work,
                    cancel,
                )?;
                pending_bytes = 0;
                if speculative.failed {
                    return Ok(false);
                }
            }
            if bytes > piece_bytes_available {
                // Too large to buffer: certify it in place, still speculatively.
                speculative
                    .trace
                    .add(&speculative.trace.raw_inline_pieces, 1);
                let local = certify_raw_piece(&task, polygon, scope, range, trace.enabled, cancel);
                speculative.absorb(local, work);
                return Ok(!speculative.failed);
            }
            pending.push(RawPiece {
                polygon: polygon.clone(),
                scope,
                range,
            });
            pending_bytes = pending_bytes.saturating_add(bytes);
            Ok(true)
        },
    );
    if visited.is_err() || speculative.failed {
        return Ok(false);
    }
    if flush_raw_wave(
        &task,
        &mut pending,
        &mut results,
        &mut speculative,
        &mut work,
        cancel,
    )
    .is_err()
        || speculative.failed
    {
        return Ok(false);
    }
    native_work
        .charge(speculative.geometry_work, cancel)
        .map_err(|failure| {
            super::native_error(failure, "account raw occupancy support clearance")
        })?;
    *support_work = work;
    trace.merge_polygon(&speculative.trace);
    Ok(true)
}

struct RawPiece {
    polygon: geo::Polygon<f64>,
    scope: spiling_geometry::ProtectedVoidHeightScope,
    range: std::ops::Range<usize>,
}

struct Speculative {
    geometry_work: u64,
    trace: ClearanceTrace,
    failed: bool,
}

impl Speculative {
    fn absorb(&mut self, local: RawPieceResult, work: &mut usize) {
        if self.failed {
            return;
        }
        match (local.result, work.checked_add(local.scan_work)) {
            (Ok(()), Some(total)) => {
                *work = total;
                self.geometry_work = self.geometry_work.saturating_add(local.geometry_work);
                self.trace.merge_polygon(&local.trace);
            }
            _ => self.failed = true,
        }
    }
}

fn flush_raw_wave(
    raw: &RawTask<'_>,
    pending: &mut Vec<RawPiece>,
    results: &mut Vec<RawPieceResult>,
    speculative: &mut Speculative,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    if pending.is_empty() {
        return Ok(());
    }
    let working_set = crate::working_set::current();
    let trace_enabled = speculative.trace.enabled;
    speculative.trace.add(&speculative.trace.raw_waves, 1);
    speculative
        .trace
        .add(&speculative.trace.raw_wave_pieces, pending.len());
    pending
        .par_drain(..)
        .map(|piece| {
            crate::working_set::scope(working_set, || {
                certify_raw_piece(
                    raw,
                    &piece.polygon,
                    piece.scope,
                    piece.range,
                    trace_enabled,
                    cancel,
                )
            })
        })
        .collect_into_vec(results);
    for local in results.drain(..) {
        speculative.absorb(local, work);
    }
    Ok(())
}

fn certify_raw_piece(
    raw: &RawTask<'_>,
    polygon: &geo::Polygon<f64>,
    scope: spiling_geometry::ProtectedVoidHeightScope,
    range: std::ops::Range<usize>,
    trace_enabled: bool,
    cancel: &AtomicBool,
) -> RawPieceResult {
    let mut native = spiling_geometry::GeometryWorkBudget::monitored();
    let mut scan_work = 0;
    let trace = ClearanceTrace {
        enabled: trace_enabled,
        ..ClearanceTrace::default()
    };
    let result = certify_polygon_capsules(
        PolygonDeposition {
            layers: raw.layers,
            range,
            index: raw.index,
        },
        &ForbiddenPolygonScope {
            polygon,
            height: scope,
            label: "continuous native/finish outer occupancy",
            memo_bytes: 0,
        },
        raw.clearance,
        &mut native,
        &mut scan_work,
        &trace,
        None,
        cancel,
    );
    RawPieceResult {
        result,
        geometry_work: native.consumed(),
        scan_work,
        trace,
    }
}

/// Bytes a clone of `polygon` allocates: clones size to length.
fn cloned_polygon_bytes(polygon: &geo::Polygon<f64>) -> usize {
    let coords = polygon.exterior().0.len()
        + polygon
            .interiors()
            .iter()
            .map(|ring| ring.0.len())
            .sum::<usize>();
    coords
        .saturating_mul(std::mem::size_of::<geo::Coord<f64>>())
        .saturating_add(
            polygon
                .interiors()
                .len()
                .saturating_add(1)
                .saturating_mul(std::mem::size_of::<geo::LineString<f64>>()),
        )
}

#[expect(clippy::too_many_arguments)]
fn certify_polygon_capsules(
    deposition: PolygonDeposition<'_>,
    forbidden: &ForbiddenPolygonScope<'_>,
    clearance: f64,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    support_work: &mut usize,
    trace: &ClearanceTrace,
    mut withdrawal: Option<(&mut withdrawal::Sweep, &super::support::VoidScope)>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let ForbiddenPolygonScope {
        polygon,
        height: scope,
        label,
        memo_bytes,
    } = *forbidden;
    trace.add(&trace.polygons, 1);
    let forbidden_bounds = geo::BoundingRect::bounding_rect(polygon)
        .map(|b| [[b.min().x, b.min().y], [b.max().x, b.max().y]]);
    // Encode and validate the borrowed forbidden polygon lazily, once per scan.
    let encoded = std::cell::OnceCell::<Vec<spiling_geometry::SectionLoopMm>>::new();
    let mut prepared = None;
    for ordinal in deposition.range {
        let layer = &deposition.layers[ordinal];
        trace.add(&trace.layers, 1);
        charge_support_clearance(support_work, cancel)?;
        let mut visit = |candidate| {
            let range = match candidate {
                clearance_index::Candidates::Charge(amount) => {
                    return charge_support_clearance_range(support_work, amount, cancel);
                }
                clearance_index::Candidates::Visit(range) => range.paths,
                clearance_index::Candidates::Skip { paths, .. } => {
                    charge_support_clearance_range(support_work, paths, cancel)?;
                    trace.add(&trace.skipped_paths, paths);
                    return Ok(());
                }
            };
            for path in &layer.paths[range] {
                trace.add(&trace.paths, 1);
                charge_support_clearance(support_work, cancel)?;
                if path.kind.is_model() {
                    continue;
                }
                trace.add(&trace.support_paths, 1);
                trace.add(&trace.bounds_points, path.points_mm.len());
                let edges = path.checked_edges()?;
                if forbidden_bounds.is_some_and(|forbidden| {
                    path_xy_bounds(path).is_some_and(|(bounds, width)| {
                        spiling_geometry::planar_bounds_clear(
                            bounds,
                            forbidden,
                            width * 0.5 + XYZ6_SCREEN_SLACK_MM,
                            clearance,
                        )
                    })
                }) {
                    trace.add(&trace.path_rejects, 1);
                    continue;
                }
                let mut clear_until = 0;
                for (edge_ordinal, local) in edges.enumerate() {
                    // Establish polygon validity via the original ordered proof
                    // before screening. Limited native budgets and protected
                    // memo/withdrawal keep their original proof order.
                    if edge_ordinal > 0
                        && (edge_ordinal - 1).is_multiple_of(CLEARANCE_EDGE_BLOCK)
                        && edges.len() - edge_ordinal >= CLEARANCE_EDGE_BLOCK
                        && prepared.is_some()
                        && native_work.is_monitored()
                        && memo_bytes == 0
                        && withdrawal.is_none()
                        && let Some(bounds) = forbidden_bounds
                    {
                        trace.add(&trace.polygon_edge_block_queries, 1);
                        if edge_block_clear(
                            edges.skip(edge_ordinal).take(CLEARANCE_EDGE_BLOCK),
                            bounds,
                            clearance,
                        ) {
                            clear_until = edge_ordinal + CLEARANCE_EDGE_BLOCK;
                        }
                    }
                    trace.add(&trace.edges, 1);
                    let edge = local.points_mm;
                    charge_support_clearance(support_work, cancel)?;
                    let [lower, upper] = project_xyz6_segment_bead_height_mm(
                        edge[0][2],
                        edge[1][2],
                        path.height_mm(),
                    )?;
                    let relevant = match scope {
                        spiling_geometry::ProtectedVoidHeightScope::Bounded {
                            lower_mm,
                            upper_mm,
                        } => lower_mm <= upper && upper_mm >= lower,
                        spiling_geometry::ProtectedVoidHeightScope::OpenBelow { upper_mm } => {
                            upper_mm >= lower
                        }
                    };
                    if !relevant {
                        trace.add(&trace.height_rejects, 1);
                        continue;
                    }
                    let (segment, endpoint_error_mm) = emitted_xy_segment(edge)?;
                    if edge_ordinal < clear_until {
                        // The raw XY envelope includes the command displacement
                        // and physical decimal error. Keep the original height,
                        // representation and logical edge checks above, and the
                        // accounting of a successful prepared bounds proof.
                        native_work.charge(1, cancel).map_err(|failure| {
                            super::native_error(failure, "screen actual emitted support edge")
                        })?;
                        trace.add(&trace.polygon_edge_block_clears, 1);
                        continue;
                    }
                    if encoded.get().is_none() {
                        let _ =
                            encoded.set(encode_polygons(std::iter::once(polygon), 0.0, cancel)?);
                    }
                    let loops = encoded.get().expect("encoded actual forbidden polygon");
                    if prepared.is_none() {
                        let forbidden =
                            spiling_geometry::prepare_planar_forbidden(loops, native_work, cancel)
                            .map_err(|failure| {
                                super::native_error(
                                    failure,
                                    &format!(
                            "actual emitted support layer {} against {label}, scope {scope:?}, segment {segment:?}, width {} mm, XY gap {clearance} mm",
                            layer.index, local.width_mm
                                    ),
                                )
                            })?;
                        let allowance =
                            if memo_bytes >= protected_proof_memo::MemoizedForbidden::MEMO_BYTES {
                                memo_bytes.saturating_sub(loops.iter().fold(
                                    loops.capacity().saturating_mul(std::mem::size_of::<
                                        spiling_geometry::SectionLoopMm,
                                    >(
                                    )),
                                    |bytes, ring| {
                                        bytes.saturating_add(
                                            ring.points_mm
                                                .capacity()
                                                .saturating_mul(std::mem::size_of::<[f64; 3]>()),
                                        )
                                    },
                                ))
                            } else {
                                0
                            };
                        prepared = Some(protected_proof_memo::MemoizedForbidden::new(
                            forbidden, allowance,
                        ));
                    }
                    let policy = spiling_geometry::PlanarCapsuleClearancePolicy {
                        radius_mm: local.width_mm * 0.5,
                        required_gap_mm: clearance,
                        endpoint_error_mm,
                    };
                    let proof = prepared
                        .as_mut()
                        .expect("prepared actual forbidden polygon")
                        .certify(segment, policy, native_work, cancel);
                    if proof.as_ref().is_ok_and(|&cached| cached) {
                        trace.add(&trace.protected_memo_hits, 1);
                    } else {
                        trace.add(&trace.polygon_tests, 1);
                    }
                    if let Err(failure) = proof {
                        if let Some((sweep, access)) = &mut withdrawal
                            && failure.code == spiling_contracts::geometry::GeometryErrorCode::AnalysisIndeterminate
                            && access.covers_bead(ordinal, upper, segment, support_work, cancel)?
                        {
                            sweep.record(segment, policy, upper, native_work, cancel)?;
                        } else {
                            return Err(super::native_error(failure, &format!(
                            "actual emitted support layer {} against {label}, scope {scope:?}, segment {segment:?}, width {} mm, XY gap {clearance} mm",
                            layer.index, local.width_mm
                            )));
                        }
                    }
                }
            }
            Ok(())
        };
        if let (Some(index), Some(bounds)) = (
            deposition.index.and_then(|index| index.layer(ordinal)),
            forbidden_bounds,
        ) {
            index.visit(bounds, clearance, XYZ6_SCREEN_SLACK_MM, cancel, &mut visit)?;
        } else {
            visit(clearance_index::Candidates::Visit(
                clearance_index::PathRange {
                    paths: 0..layer.paths.len(),
                    nonmodel_paths: 0,
                },
            ))?;
        }
    }
    Ok(())
}

fn emitted_xy_segment(edge: &[[f64; 3]]) -> Result<([[f64; 2]; 2], f64), ManufacturingError> {
    let segment = [
        [
            project_xyz6_coordinate_mm(edge[0][0])?,
            project_xyz6_coordinate_mm(edge[0][1])?,
        ],
        [
            project_xyz6_coordinate_mm(edge[1][0])?,
            project_xyz6_coordinate_mm(edge[1][1])?,
        ],
    ];
    let mut error = 0.0f64;
    for point in &segment {
        let radius = xyz6_coordinate_error_mm(point[0])?.hypot(xyz6_coordinate_error_mm(point[1])?);
        error = error.max(if radius == 0.0 { 0.0 } else { radius.next_up() });
    }
    Ok((segment, error))
}

pub(super) fn charge_support_clearance(
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    charge_support_clearance_range(work, 1, cancel)
}

fn charge_support_clearance_range(
    work: &mut usize,
    amount: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    if let Some(total) = work.checked_add(amount) {
        *work = total;
        Ok(())
    } else {
        *work = usize::MAX;
        Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "cumulative actual support-clearance scan work counter overflow",
        ))
    }
}

/// One linear scan derives two scalar vertical representation enclosures from
/// every actual object/support endpoint and the true schedule. No coordinate
/// index/cache or copied path buffer is retained. Uniform extensions preserve
/// monotonicity for charged scope searches, including actual deposited pairs.
fn deposition_vertical_extensions(
    layers: &[PrintLayer],
    schedule: &[LayerElevation],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<DepositionEnvelope>, ManufacturingError> {
    let mut extensions = [0.0f64; 2];
    let mut inventory = layers.len().saturating_add(1);
    let mut any = false;
    for (layer, elevation) in layers.iter().zip(schedule) {
        charge_support_clearance(work, cancel)?;
        inventory = inventory.saturating_add(layer.paths.len());
        for path in &layer.paths {
            charge_support_clearance(work, cancel)?;
            inventory = inventory.saturating_add(path.points_mm.len());
            if path.points_mm.len() < 2 {
                continue;
            }
            any |= matches!(
                path.kind,
                DepositionKind::Support | DepositionKind::SupportInterface
            );
            for point in &path.points_mm {
                charge_support_clearance(work, cancel)?;
                let [lower, upper] = project_xyz6_bead_height_mm(point[2], path.height_mm())?;
                extensions[0] = extensions[0].max((elevation.lower_z - lower).next_up());
                extensions[1] = extensions[1].max((upper - elevation.z).next_up());
            }
        }
    }
    if extensions.iter().any(|value| !value.is_finite()) {
        return Err(error(
            ManufacturingErrorCode::ProgramCoordinateConditioningFailure,
            "actual emitted deposition vertical representation cannot be enclosed",
        ));
    }
    Ok(any.then_some(DepositionEnvelope {
        vertical: extensions,
        inventory,
    }))
}

fn prepare_deposited_screen(
    layers: &[PrintLayer],
    deposition: &DepositionEnvelope,
    available: usize,
    retained_sweep_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<ObjectClearanceIndex>, ManufacturingError> {
    check_cancel(cancel)?;
    let allowance = available.saturating_sub(retained_sweep_bytes);
    if allowance == 0 || !deposition.screen_work_fits(*work) {
        return Ok(None);
    }
    ObjectClearanceIndex::build(
        layers,
        allowance,
        |kind| !kind.is_support(),
        || charge_support_clearance(work, cancel),
    )
}

#[cfg(test)]
fn scoped_support_layers<'a>(
    layers: &'a [PrintLayer],
    schedule: &[LayerElevation],
    scope: spiling_geometry::ProtectedVoidHeightScope,
    extensions: [f64; 2],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<&'a [PrintLayer], ManufacturingError> {
    let range = scoped_support_layer_range(schedule, scope, extensions, work, cancel)?;
    Ok(&layers[range])
}

fn scoped_support_layer_range(
    schedule: &[LayerElevation],
    scope: spiling_geometry::ProtectedVoidHeightScope,
    extensions: [f64; 2],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<std::ops::Range<usize>, ManufacturingError> {
    let (lower, upper) = match scope {
        spiling_geometry::ProtectedVoidHeightScope::Bounded { lower_mm, upper_mm } => {
            (Some(lower_mm), upper_mm)
        }
        spiling_geometry::ProtectedVoidHeightScope::OpenBelow { upper_mm } => (None, upper_mm),
    };
    let first = if let Some(lower) = lower {
        partition_schedule(
            schedule,
            |layer| (layer.z + extensions[1]).next_up() < lower,
            work,
            cancel,
        )?
    } else {
        0
    };
    let end = partition_schedule(
        schedule,
        |layer| (layer.lower_z - extensions[0]).next_down() <= upper,
        work,
        cancel,
    )?;
    if first > end {
        return Err(error(
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "native support-clearance height scope is not ordered",
        ));
    }
    Ok(first..end)
}

fn partition_schedule(
    schedule: &[LayerElevation],
    mut before: impl FnMut(&LayerElevation) -> bool,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    let mut first = 0;
    let mut end = schedule.len();
    while first < end {
        charge_support_clearance(work, cancel)?;
        let middle = first + (end - first) / 2;
        if before(&schedule[middle]) {
            first = middle + 1;
        } else {
            end = middle;
        }
    }
    Ok(first)
}

fn certify_deposited_clearance(
    deposition: &ActualDepositionScope<'_>,
    xy_gap: f64,
    z_gap: f64,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    work: &mut usize,
    trace: &ClearanceTrace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    certify_deposited_layer_range(
        deposition,
        0..deposition.layers.len(),
        xy_gap,
        z_gap,
        native_work,
        work,
        trace,
        cancel,
    )
}

struct DepositedLayerResult {
    result: Result<(), ManufacturingError>,
    geometry_work: u64,
    scan_work: usize,
    trace: ClearanceTrace,
}

/// Each job borrows the immutable actual-deposition inventory and uses the
/// allocation-free capsule-pair predicate. Only one wave's result records
/// are admitted. Reduce in original layer order, including on refusal.
#[expect(clippy::too_many_arguments)]
fn certify_deposited_layers(
    deposition: &ActualDepositionScope<'_>,
    envelope: &DepositionEnvelope,
    available: usize,
    xy_gap: f64,
    z_gap: f64,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    work: &mut usize,
    trace: &ClearanceTrace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let wave = rayon::current_num_threads()
        .min(deposition.layers.len())
        .min(available / std::mem::size_of::<DepositedLayerResult>());
    let mut results = Vec::new();
    if wave < 2
        || !native_work.is_monitored()
        || !envelope.screen_work_fits(*work)
        || results.try_reserve_exact(wave).is_err()
    {
        return certify_deposited_clearance(
            deposition,
            xy_gap,
            z_gap,
            native_work,
            work,
            trace,
            cancel,
        );
    }
    let working_set = crate::working_set::current();
    let trace_enabled = trace.enabled;
    for start in (0..deposition.layers.len()).step_by(wave) {
        check_cancel(cancel)?;
        let end = start.saturating_add(wave).min(deposition.layers.len());
        (start..end)
            .into_par_iter()
            .map(|index| {
                crate::working_set::scope(working_set, || {
                    let mut native = spiling_geometry::GeometryWorkBudget::monitored();
                    let mut scan_work = 0;
                    let local_trace = ClearanceTrace {
                        enabled: trace_enabled,
                        ..Default::default()
                    };
                    let result = certify_deposited_layer_range(
                        deposition,
                        index..index + 1,
                        xy_gap,
                        z_gap,
                        &mut native,
                        &mut scan_work,
                        &local_trace,
                        cancel,
                    );
                    DepositedLayerResult {
                        result,
                        geometry_work: native.consumed(),
                        scan_work,
                        trace: local_trace,
                    }
                })
            })
            .collect_into_vec(&mut results);
        for local in results.drain(..) {
            super::boolean::charge(work, local.scan_work)?;
            native_work
                .charge(local.geometry_work, cancel)
                .map_err(|failure| {
                    super::native_error(failure, "account actual deposited support clearance")
                })?;
            trace.merge_deposited_layer(&local.trace);
            local.result?;
        }
    }
    Ok(())
}

#[expect(clippy::too_many_arguments)]
fn certify_deposited_layer_range(
    deposition: &ActualDepositionScope<'_>,
    range: std::ops::Range<usize>,
    xy_gap: f64,
    z_gap: f64,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    work: &mut usize,
    trace: &ClearanceTrace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let ActualDepositionScope {
        layers,
        schedule,
        vertical,
        screen,
    } = *deposition;
    for index in range {
        let support_layer = &layers[index];
        charge_support_clearance(work, cancel)?;
        let mut applicable = None;
        for support in &support_layer.paths {
            charge_support_clearance(work, cancel)?;
            if support.kind.is_model() || support.points_mm.len() < 2 {
                continue;
            }
            trace.add(&trace.deposited_support_paths, 1);
            if applicable.is_none() {
                let lower = (schedule[index].lower_z - vertical[0]).next_down();
                let upper = (schedule[index].z + vertical[1]).next_up();
                // These are already outward physical bead bounds: expand by
                // the minimum Z gap, never reproject them as XYZ6 coordinates.
                let scope = spiling_geometry::ProtectedVoidHeightScope::Bounded {
                    lower_mm: program_coordinates::enclosed_vertical_gap_mm(lower, z_gap)?,
                    upper_mm: -program_coordinates::enclosed_vertical_gap_mm(-upper, z_gap)?,
                };
                applicable = Some(scoped_support_layer_range(
                    schedule, scope, vertical, work, cancel,
                )?);
            }
            let objects = applicable
                .as_ref()
                .expect("actual support selected a complete applicable layer range")
                .clone();
            let support_edges = support.checked_edges()?;
            super::boolean::charge(work, support.points_mm.len())?;
            let support_frame = path_xy_bounds(support);
            for object_ordinal in objects {
                let object_layer = &layers[object_ordinal];
                charge_support_clearance(work, cancel)?;
                let blocks = screen
                    .filter(|_| support_frame.is_some())
                    .map(|screen| screen.layer(object_ordinal));
                let span = blocks.map_or(object_layer.paths.len().max(1), |_| BLOCK_PATHS);
                for (block_ordinal, paths) in object_layer.paths.chunks(span).enumerate() {
                    if let (Some(blocks), Some((bounds, width))) = (blocks, support_frame) {
                        charge_support_clearance(work, cancel)?;
                        trace.add(&trace.deposited_block_queries, 1);
                        if blocks[block_ordinal].clear(bounds, width, xy_gap) {
                            trace.add(&trace.deposited_block_skipped_slots, paths.len());
                            continue;
                        }
                    }
                    for object in paths {
                        charge_support_clearance(work, cancel)?;
                        if matches!(
                            object.kind,
                            DepositionKind::Support | DepositionKind::SupportInterface
                        ) {
                            continue;
                        }
                        trace.add(&trace.deposited_object_paths, 1);
                        let object_edges = object.checked_edges()?;
                        super::boolean::charge(work, object.points_mm.len())?;
                        if let (
                            Some((support_bounds, support_width)),
                            Some((object_bounds, object_width)),
                        ) = (support_frame, path_xy_bounds(object))
                            && spiling_geometry::planar_bounds_clear(
                                support_bounds,
                                object_bounds,
                                (support_width + object_width) * 0.5 + 2.0 * XYZ6_SCREEN_SLACK_MM,
                                xy_gap,
                            )
                        {
                            trace.add(&trace.deposited_path_rejects, 1);
                            continue;
                        }
                        for support_local in support_edges {
                            let support_edge = support_local.points_mm;
                            charge_support_clearance(work, cancel)?;
                            let support_height = project_xyz6_segment_bead_height_mm(
                                support_edge[0][2],
                                support_edge[1][2],
                                support.height_mm(),
                            )?;
                            let (support_xy, support_error) = emitted_xy_segment(support_edge)?;
                            let support_bounds = edge_xy_bounds(support_edge);
                            for object_local in object_edges {
                                trace.add(&trace.edge_pairs, 1);
                                let object_edge = object_local.points_mm;
                                charge_support_clearance(work, cancel)?;
                                if spiling_geometry::planar_bounds_clear(
                                    support_bounds,
                                    edge_xy_bounds(object_edge),
                                    (support_local.width_mm + object_local.width_mm) * 0.5
                                        + 2.0 * XYZ6_SCREEN_SLACK_MM,
                                    xy_gap,
                                ) {
                                    trace.add(&trace.edge_pair_rejects, 1);
                                    continue;
                                }
                                let object_height = project_xyz6_segment_bead_height_mm(
                                    object_edge[0][2],
                                    object_edge[1][2],
                                    object.height_mm(),
                                )?;
                                let above = program_coordinates::enclosed_vertical_gap_mm(
                                    object_height[0],
                                    support_height[1],
                                )?;
                                let below = program_coordinates::enclosed_vertical_gap_mm(
                                    support_height[0],
                                    object_height[1],
                                )?;
                                // All actual D matters, not only C or intervals
                                // that overlap. Exact zero-gap touching is not an
                                // interior collision when the requested gap is zero.
                                if above >= z_gap || below >= z_gap {
                                    trace.add(&trace.height_pair_rejects, 1);
                                    continue;
                                }
                                let (object_xy, object_error) = emitted_xy_segment(object_edge)?;
                                trace.add(&trace.pair_tests, 1);
                                spiling_geometry::certify_planar_capsule_pair_clearance([
                                spiling_geometry::PlanarCapsuleMm {
                                    segment_xy_mm: support_xy, radius_mm: support_local.width_mm * 0.5,
                                    endpoint_error_mm: support_error,
                                },
                                spiling_geometry::PlanarCapsuleMm {
                                    segment_xy_mm: object_xy, radius_mm: object_local.width_mm * 0.5,
                                    endpoint_error_mm: object_error,
                                },
                            ], xy_gap, native_work, cancel).map_err(|failure| super::native_error(failure,
                                &format!("actual emitted support layer {} versus deposited object/bridge layer {}",
                                    support_layer.index, object_layer.index)))?;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod actual_deposition_tests {
    use super::*;

    #[test]
    fn actual_bridge_and_support_strokes_require_z_gap_even_without_z_overlap() {
        let cancel = AtomicBool::new(false);
        for (gap, required, object_x, support_above, variable_width, accepted) in [
            (0.1, 0.2, 0.0, false, false, false),
            (0.25, 0.2, 0.0, false, false, true),
            (0.25, 0.25, 0.0, false, false, true),
            (0.1, 0.2, 2.0, false, false, true),
            (0.1, 0.2, 0.0, true, false, false),
            (0.25, 0.2, 0.0, true, false, true),
            (0.0, 0.0, 0.0, false, false, true),
            (0.1, 0.2, 2.1, false, false, true),
            (0.1, 0.2, 2.1, false, true, false),
            (0.1, 0.2, 2.1, true, true, false),
        ] {
            let mut layers = Vec::new();
            let mut schedule = Vec::new();
            let mut lower = 0.75;
            let mut push = |kind: DepositionKind, height: f64, x: f64| {
                let z = lower + height;
                let support = matches!(
                    kind,
                    DepositionKind::Support | DepositionKind::SupportInterface
                );
                let object_area = if support { 0.0 } else { 1.0 };
                let support_area = if support { 1.0 } else { 0.0 };
                let volume =
                    (0.5 - height) * height + std::f64::consts::PI * (height * 0.5).powi(2);
                let index = layers.len() as u32;
                layers.push(PrintLayer { skin_command_fits: Vec::new(), index, hatch_course: index,
                z_mm: z,
                lower_z_mm: lower,
                height_mm: height,
                section_z_mm: lower + height * 0.5,
                region_coverage: PlanarRegionCoverage {
            web_roof_domains: Vec::new(), skin_bridge_domains: Vec::new(), process_diaphragms: Vec::new(), diaphragm_area_mm2: 0.0, section_area_mm2: object_area,
                source_section_area_mm2: object_area,
                surface_added_area_mm2: 0.0,
                separator_area_mm2: 0.0,
                process_interfaces: Vec::new(),
                wall_area_mm2: 0.0,
                bottom_skin_area_mm2: 0.0,
                top_skin_area_mm2: 0.0,
                solid_interior_area_mm2: object_area,
                sparse_interior_area_mm2: 0.0,
                intentional_void_area_mm2: 0.0,
                protected_hole_area_mm2: 0.0,
                nominal_deposition_volume_mm3: object_area * volume,
                support_area_mm2: support_area,
                support_nominal_deposition_volume_mm3: support_area * volume,
                unsupported_overhang_mm: 0.0,
                support_omitted_wall_mm: 0.0,
                thin_widened_mm: 0.0,
                thin_omitted_area_mm2: 0.0,
                process_regions: if object_area > 0.0 {
                    vec![ProcessRegionCoverage {
                        functional_solid_zone: None,
                material_domain: None,
                        area_mm2: object_area,
                        fraction: 1.0,
                        contributions: Vec::new(),
                    }]
                } else {
                    Vec::new()
                }, },
                paths: vec![DepositionPath {
                    material_domain: None,
                    kind,
                    owner: match kind { DepositionKind::BottomSkin => Some(spiling_contracts::manufacturing::DepositionOwner::MandatorySkin { skin: spiling_contracts::manufacturing::ProcessSkinKind::BottomSkin }), DepositionKind::TopSkin => Some(spiling_contracts::manufacturing::DepositionOwner::MandatorySkin { skin: spiling_contracts::manufacturing::ProcessSkinKind::TopSkin }), _ => None },
                    width: spiling_contracts::manufacturing::WidthProfile::Constant { width_mm: 0.5 },
                    physical_z_mm: [lower, z],
                    points_mm: vec![[x, 0.0, z], [x + 1.0, 0.0, z]],
                arcs: Vec::new(), exposed: false, connectors: Vec::new() }], });
                schedule.push(LayerElevation {
                    z,
                    lower_z: lower,
                    sample: lower + height * 0.5,
                    height,
                    course: schedule.len() as u32,
                });
                lower = z;
            };
            let support = DepositionKind::SupportInterface;
            let object = DepositionKind::Bridge;
            push(
                if support_above { object } else { support },
                0.25,
                if support_above { object_x } else { 0.0 },
            );
            if gap > 0.0 {
                // Another real far-XY stroke fills the intervening scheduled
                // layer. This is not an invalid globally-empty contact gap.
                push(DepositionKind::SolidFill, gap, 10.0);
            }
            push(
                if support_above { support } else { object },
                0.25,
                if support_above { 0.0 } else { object_x },
            );
            if variable_width {
                let path = layers
                    .iter_mut()
                    .flat_map(|layer| &mut layer.paths)
                    .find(|path| path.kind == object)
                    .unwrap();
                let first = path.points_mm[0];
                path.points_mm
                    .insert(0, [first[0] + 1.0, first[1] + 1.0, first[2]]);
                path.width = WidthProfile::PerEdge {
                    widths_mm: vec![0.5, 2.0],
                };
            }
            let mut work = 0;
            let mut native_work = spiling_geometry::GeometryWorkBudget::new(10_000);
            let vertical = deposition_vertical_extensions(&layers, &schedule, &mut work, &cancel)
                .unwrap()
                .unwrap()
                .vertical;
            // Exercise the actual-deposition consumer and real native capsule
            // proof, independently of C or contact-anchor authority.
            let screen = ObjectClearanceIndex::build(
                &layers,
                64 * 1024,
                |kind| !kind.is_support(),
                || charge_support_clearance(&mut work, &cancel),
            )
            .unwrap()
            .unwrap();
            for screen in [None, Some(&screen)] {
                let result = certify_deposited_clearance(
                    &ActualDepositionScope {
                        layers: &layers,
                        schedule: &schedule,
                        vertical,
                        screen,
                    },
                    0.2,
                    required,
                    &mut native_work,
                    &mut work,
                    &ClearanceTrace::default(),
                    &cancel,
                );
                assert_eq!(
                    result.is_ok(),
                    accepted,
                    "actual Z gap={gap}, required={required}, object X={object_x}, support above={support_above}: {result:?}"
                );
                if !accepted {
                    assert_eq!(
                        result.unwrap_err().code,
                        ManufacturingErrorCode::NativeAnalysisIndeterminate
                    );
                }
            }
        }
    }
}
