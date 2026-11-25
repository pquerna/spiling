// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{LayerElevation, preflight};
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
    /// The support sweep's kept cells: an emitted bead inside them is clear.
    pub kept: &'a super::support::KeptSupport,
}

struct ForbiddenPolygonScope<'a> {
    polygon: &'a geo::Polygon<f64>,
    height: spiling_geometry::ProtectedVoidHeightScope,
    label: &'a str,
}

struct PolygonDeposition<'a> {
    layers: &'a [PrintLayer],
    range: std::ops::Range<usize>,
    index: Option<&'a clearance_index::ClearanceIndex>,
}

/// A diagnostic counter shared by clearance proofs running in parallel.
#[derive(Default)]
struct TraceCount(std::sync::atomic::AtomicUsize);

impl TraceCount {
    fn get(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn set(&self, value: usize) {
        self.0.store(value, std::sync::atomic::Ordering::Relaxed);
    }
}

#[derive(Default)]
struct ClearanceTrace {
    enabled: bool,
    support_edges: TraceCount,
    raster_words: TraceCount,
    escaped_edges: TraceCount,
    escaped_cells: TraceCount,
    occupancy_tests: TraceCount,
    void_tests: TraceCount,
    pair_tests: TraceCount,
    unseen_object_edges: TraceCount,
    polygons: TraceCount,
    layers: TraceCount,
    paths: TraceCount,
    support_paths: TraceCount,
    bounds_points: TraceCount,
    path_rejects: TraceCount,
    edges: TraceCount,
    height_rejects: TraceCount,
    polygon_tests: TraceCount,
    index_bytes: TraceCount,
    skipped_paths: TraceCount,
}

impl ClearanceTrace {
    fn add(&self, counter: &TraceCount, amount: usize) {
        if self.enabled {
            counter
                .0
                .fetch_add(amount, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn report(&self, phase: &str, started: std::time::Instant) {
        if self.enabled {
            eprintln!(
                "{}",
                serde_json::json!({
                    "event": "support_clearance_trace", "phase": phase,
                    "phase_seconds": started.elapsed().as_secs_f64(),
                    "support_edges": self.support_edges.get(), "raster_words": self.raster_words.get(),
                    "escaped_edges": self.escaped_edges.get(), "escaped_cells": self.escaped_cells.get(),
                    "occupancy_tests": self.occupancy_tests.get(), "void_tests": self.void_tests.get(),
                    "pair_tests": self.pair_tests.get(),
                    "unseen_object_edges": self.unseen_object_edges.get(),
                    "index_bytes": self.index_bytes.get(), "skipped_paths": self.skipped_paths.get(),
                    "polygons": self.polygons.get(), "layers": self.layers.get(),
                    "paths": self.paths.get(), "support_paths": self.support_paths.get(),
                    "bounds_points": self.bounds_points.get(), "path_rejects": self.path_rejects.get(),
                    "edges": self.edges.get(), "height_rejects": self.height_rejects.get(),
                    "polygon_tests": self.polygon_tests.get(),
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

/// Final publication gate over actual emitted support capsules.
///
/// The support sweep built each layer's kept cells clear of raw continuous
/// occupancy, protected voids and object beads at the XY clearance, rounded
/// outward. One linear pass rasterizes every emitted support bead outward on
/// that frame; a bead whose cells are all kept is clear by construction. Only
/// the cells that escape are resolved exactly, against just the occupied
/// pieces, voids and object beads near them, with the shared geometry
/// capsule predicates. Object beads that changed after the sweep, and voids a
/// relieved support channel enters (whose beads become the withdrawal), are
/// proved directly. Any unproved gap stays a typed refusal.
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
        kept,
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
    let Some(vertical) = deposition_vertical_extensions(layers, schedule, support_work, cancel)?
    else {
        // Exhausted the actual paths: there are no emitted support segments,
        // rather than trusting a generated-support flag or skipping a proof.
        return Ok(None);
    };
    let started = std::time::Instant::now();
    let mut escapes = collect_escapes(layers, kept, index_bytes, support_work, trace, cancel)?;
    let available = index_bytes.saturating_sub(escape_bytes(&escapes));
    let escaped = EscapeScope::new(
        &mut escapes,
        layers,
        schedule,
        vertical,
        support_work,
        cancel,
    )?;
    trace.report("grid_assertion", started);

    let started = std::time::Instant::now();
    let leased = scopes.iter().any(super::support::VoidScope::has_access);
    let index = if leased {
        clearance_index::ClearanceIndex::build(
            layers.iter().map(|layer| layer.paths.as_slice()),
            available,
            support_work,
            cancel,
        )?
    } else {
        None
    };
    trace
        .index_bytes
        .set(index.as_ref().map_or(0, |index| index.retained_bytes()));
    let mut sweep = withdrawal::Sweep::new(
        available.saturating_sub(index.as_ref().map_or(0, |index| index.retained_bytes())),
    );
    for (polygon, scope) in protected.0.iter().zip(scopes) {
        if !scope.has_access() {
            continue;
        }
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
            },
            xy_clearance_mm,
            native_work,
            support_work,
            trace,
            Some((&mut sweep, scope)),
            cancel,
        )?;
    }
    drop(index);
    trace.report("relieved_voids", started);

    let started = std::time::Instant::now();
    certify_escaped_occupancy(
        &escaped,
        sections,
        xy_clearance_mm,
        native_work,
        support_work,
        trace,
        cancel,
    )?;
    for (polygon, scope) in protected.0.iter().zip(scopes) {
        if scope.has_access() {
            continue;
        }
        let range =
            scoped_support_layer_range(schedule, scope.height, vertical, support_work, cancel)?;
        certify_polygon_escapes(
            &escaped,
            range,
            &ForbiddenPolygonScope {
                polygon,
                height: scope.height,
                label: "projected protected native void",
            },
            xy_clearance_mm,
            native_work,
            support_work,
            &trace.void_tests,
            trace,
            cancel,
        )?;
    }
    trace.report("escaped_exclusions", started);

    let started = std::time::Instant::now();
    let sweep = sweep.certify_source(
        schedule,
        sections,
        xy_clearance_mm,
        native_work,
        support_work,
        cancel,
    )?;
    trace.report("withdrawal", started);

    // A bounded-fidelity M/D prism or emitted bead can extend outside true T.
    // Native target exclusion therefore does not replace this distinct actual
    // nominal-program footprint proof.
    let started = std::time::Instant::now();
    certify_escaped_objects(
        &escaped,
        xy_clearance_mm,
        z_gap_mm,
        native_work,
        support_work,
        trace,
        cancel,
    )?;
    certify_unseen_objects(
        layers,
        &SupportBand { schedule, vertical },
        sections,
        kept,
        [xy_clearance_mm, z_gap_mm],
        native_work,
        support_work,
        trace,
        cancel,
    )?;
    trace.report("deposited_objects", started);
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

/// An emitted edge as its program commands it: the XYZ6 XY segment with
/// its endpoint error enclosure, nominal width and full-bead height.
struct EmittedEdge {
    segment: [[f64; 2]; 2],
    endpoint_error: f64,
    width: f64,
    band: [f64; 2],
}

fn emitted_edge(
    path: &DepositionPath,
    edge: DepositionEdge<'_>,
) -> Result<EmittedEdge, ManufacturingError> {
    let points = edge.points_mm;
    let band = project_xyz6_segment_bead_height_mm(points[0][2], points[1][2], path.height_mm())?;
    let (segment, endpoint_error) = emitted_xy_segment(points)?;
    Ok(EmittedEdge {
        segment,
        endpoint_error,
        width: edge.width_mm,
        band,
    })
}

fn height_meets(scope: spiling_geometry::ProtectedVoidHeightScope, band: [f64; 2]) -> bool {
    match scope {
        spiling_geometry::ProtectedVoidHeightScope::Bounded { lower_mm, upper_mm } => {
            lower_mm <= band[1] && upper_mm >= band[0]
        }
        spiling_geometry::ProtectedVoidHeightScope::OpenBelow { rim_mm, upper_mm } => {
            rim_mm <= band[1] && upper_mm >= band[0]
        }
    }
}

/// One emitted support edge whose outward cells leave its layer's kept
/// grid, and the closed box around the cells that do. Only that part of its
/// bead needs an exact proof: the rest lies in cells built clear.
#[derive(Clone, Copy)]
struct Escaped {
    layer: usize,
    path: usize,
    edge: usize,
    bounds: [[f64; 2]; 2],
}

fn escape_bytes(escapes: &Vec<Escaped>) -> usize {
    escapes
        .capacity()
        .saturating_mul(std::mem::size_of::<Escaped>())
}

/// The grid assertion: every emitted support (and adhesion) bead, rasterized
/// outward on the sweep's frame at its radius plus endpoint error, in layer,
/// path and edge order. Records only the edges whose cells escape, under the
/// caller's byte allowance.
fn collect_escapes(
    layers: &[PrintLayer],
    kept: &super::support::KeptSupport,
    allowance: usize,
    work: &mut usize,
    trace: &ClearanceTrace,
    cancel: &AtomicBool,
) -> Result<Vec<Escaped>, ManufacturingError> {
    /// One layer's escapes, counts, and the first error it met, if any.
    #[derive(Default)]
    struct LayerEscapes {
        escapes: Vec<Escaped>,
        refusal: Option<ManufacturingError>,
        work: usize,
        support_edges: usize,
        raster_words: usize,
        escaped_cells: usize,
    }
    let limit = allowance / std::mem::size_of::<Escaped>();
    let exceeded = || {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "support clearance escapes exceed the remaining admitted geometry bytes",
        )
    };
    // Layers rasterize independently; their escapes join in layer order and
    // the first error in that order is the one reported, as a serial pass.
    let found = layers
        .par_iter()
        .enumerate()
        .map(|(ordinal, layer)| {
            let mut found = LayerEscapes::default();
            let mut scan = || {
                charge_support_clearance(&mut found.work, cancel)?;
                for (index, path) in layer.paths.iter().enumerate() {
                    charge_support_clearance(&mut found.work, cancel)?;
                    if path.kind.is_model() {
                        continue;
                    }
                    for (edge, local) in path.checked_edges()?.enumerate() {
                        charge_support_clearance(&mut found.work, cancel)?;
                        let emitted = emitted_edge(path, local)?;
                        found.support_edges += 1;
                        let radius = (emitted.width * 0.5 + emitted.endpoint_error).next_up();
                        let (escape, words) =
                            kept.escaped(ordinal, emitted.segment, radius, emitted.band)?;
                        charge_support_clearance_range(&mut found.work, words, cancel)?;
                        found.raster_words = found.raster_words.saturating_add(words);
                        let Some(escape) = escape else {
                            continue;
                        };
                        found.escaped_cells = found.escaped_cells.saturating_add(escape.cells);
                        if found.escapes.len() >= limit {
                            return Err(exceeded());
                        }
                        found.escapes.try_reserve(1).map_err(|_| exceeded())?;
                        found.escapes.push(Escaped {
                            layer: ordinal,
                            path: index,
                            edge,
                            bounds: escape.bounds,
                        });
                    }
                }
                Ok(())
            };
            found.refusal = scan().err();
            found
        })
        .collect::<Vec<_>>();
    let mut escapes: Vec<Escaped> = Vec::new();
    for found in found {
        charge_support_clearance_range(work, found.work, cancel)?;
        trace.add(&trace.support_edges, found.support_edges);
        trace.add(&trace.raster_words, found.raster_words);
        trace.add(&trace.escaped_edges, found.escapes.len());
        trace.add(&trace.escaped_cells, found.escaped_cells);
        let total = escapes
            .len()
            .checked_add(found.escapes.len())
            .filter(|total| *total <= limit)
            .ok_or_else(exceeded)?;
        escapes
            .try_reserve_exact(total - escapes.len())
            .map_err(|_| exceeded())?;
        escapes.extend(found.escapes);
        if let Some(refusal) = found.refusal {
            return Err(refusal);
        }
    }
    Ok(escapes)
}

/// The actual schedule with the uniform vertical representation enclosure.
struct SupportBand<'a> {
    schedule: &'a [LayerElevation],
    vertical: [f64; 2],
}

/// Escapes ordered by layer, then by the lower X of their boxes, so a query
/// visits only the escapes of its layers whose X extent can come in reach.
struct EscapeScope<'a> {
    escapes: &'a [Escaped],
    layers: &'a [PrintLayer],
    band: SupportBand<'a>,
    /// Widest escape box in X.
    widest: f64,
}

impl<'a> EscapeScope<'a> {
    fn new(
        escapes: &'a mut [Escaped],
        layers: &'a [PrintLayer],
        schedule: &'a [LayerElevation],
        vertical: [f64; 2],
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        let n = escapes.len();
        charge_support_clearance_range(
            work,
            n.saturating_mul(n.checked_ilog2().unwrap_or(0) as usize + 1),
            cancel,
        )?;
        escapes.sort_unstable_by(|a, b| {
            a.layer
                .cmp(&b.layer)
                .then(a.bounds[0][0].total_cmp(&b.bounds[0][0]))
                .then(a.path.cmp(&b.path))
                .then(a.edge.cmp(&b.edge))
        });
        let widest = escapes
            .iter()
            .map(|escape| escape.bounds[1][0] - escape.bounds[0][0])
            .fold(0.0, f64::max);
        Ok(Self {
            escapes,
            layers,
            band: SupportBand { schedule, vertical },
            widest,
        })
    }

    /// The escaped edge's layer and emitted capsule.
    fn edge(&self, escape: &Escaped) -> Result<(&'a PrintLayer, EmittedEdge), ManufacturingError> {
        let layer = &self.layers[escape.layer];
        let path = &layer.paths[escape.path];
        let local = path.checked_edges()?.edge(escape.edge).ok_or_else(|| {
            error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "actual support clearance lost an escaped emitted edge",
            )
        })?;
        Ok((layer, emitted_edge(path, local)?))
    }

    /// Scans the escapes on `range`'s layers in X order, calling `visit`
    /// on each whose box is not clear of `query` by `reserve + gap` (on all
    /// of them when the query is unknown) until it asks to stop. Returns
    /// the escapes and layer groups examined.
    fn scan<E>(
        &self,
        range: std::ops::Range<usize>,
        query: Option<[[f64; 2]; 2]>,
        reserve: f64,
        gap: f64,
        mut visit: impl FnMut(&Escaped) -> Result<bool, E>,
    ) -> Result<usize, E> {
        let query = query.filter(|query| query.iter().flatten().all(|v| v.is_finite()));
        // A conservative X window: a box outside it is separated in X by
        // more than the full reach, hence clear.
        let reach = ((reserve + gap) * (1.0 + 1.0e-9) + 1.0e-9).next_up();
        let mut examined = 0usize;
        let mut at = self.escapes.partition_point(|e| e.layer < range.start);
        while at < self.escapes.len() && self.escapes[at].layer < range.end {
            examined = examined.saturating_add(1);
            let layer = self.escapes[at].layer;
            let end = at + self.escapes[at..].partition_point(|e| e.layer == layer);
            let group = &self.escapes[at..end];
            let (first, last) = match query {
                Some(query) => (
                    group.partition_point(|e| {
                        e.bounds[0][0] < (query[0][0] - reach - self.widest).next_down()
                    }),
                    (query[1][0] + reach).next_up(),
                ),
                None => (0, f64::INFINITY),
            };
            for escape in &group[first..] {
                if escape.bounds[0][0] > last {
                    break;
                }
                examined = examined.saturating_add(1);
                if query.is_some_and(|query| {
                    spiling_geometry::planar_bounds_clear(escape.bounds, query, reserve, gap)
                }) {
                    continue;
                }
                if visit(escape)? {
                    return Ok(examined);
                }
            }
            at = end;
        }
        Ok(examined)
    }

    /// Each escape on `range`'s layers whose box is not clear of `query` by
    /// `reserve + gap`; every escape there when the query is unknown.
    #[expect(clippy::too_many_arguments)]
    fn visit_near(
        &self,
        range: std::ops::Range<usize>,
        query: Option<[[f64; 2]; 2]>,
        reserve: f64,
        gap: f64,
        work: &mut usize,
        cancel: &AtomicBool,
        mut visit: impl FnMut(&Escaped, &mut usize) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        let examined = self.scan(range, query, reserve, gap, |escape| {
            charge_support_clearance(work, cancel)?;
            visit(escape, work)?;
            Ok(false)
        })?;
        charge_support_clearance_range(work, examined, cancel)
    }

    /// Whether any escape on `range`'s layers is within `gap` of `query`.
    fn any_near(&self, range: std::ops::Range<usize>, query: [[f64; 2]; 2], gap: f64) -> bool {
        let mut near = false;
        let _ = self.scan::<std::convert::Infallible>(range, Some(query), 0.0, gap, |_| {
            near = true;
            Ok(true)
        });
        near
    }
}

/// Resolves the escaped cells against raw continuous occupancy: each span's
/// height-scoped pieces near an escape on a layer the span can reach.
fn certify_escaped_occupancy(
    escaped: &EscapeScope<'_>,
    sections: &[super::SectionSpan],
    gap: f64,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    work: &mut usize,
    trace: &ClearanceTrace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if escaped.escapes.is_empty() {
        return Ok(());
    }
    let schedule = escaped.band.schedule;
    for span in sections {
        charge_support_clearance(work, cancel)?;
        let interval = [
            schedule[span.start as usize].lower_z,
            schedule[(span.end - 1) as usize].z,
        ];
        let range = scoped_support_layer_range(
            schedule,
            spiling_geometry::ProtectedVoidHeightScope::Bounded {
                lower_mm: interval[0],
                upper_mm: interval[1],
            },
            escaped.band.vertical,
            work,
            cancel,
        )?;
        let first = escaped.escapes.partition_point(|e| e.layer < range.start);
        if escaped
            .escapes
            .get(first)
            .is_none_or(|escape| escape.layer >= range.end)
        {
            continue;
        }
        let scoped = |piece: Option<[f64; 2]>| {
            let height = piece.map_or(interval, |height| {
                [interval[0].max(height[0]), interval[1].min(height[1])]
            });
            (height[0] <= height[1]).then_some(
                spiling_geometry::ProtectedVoidHeightScope::Bounded {
                    lower_mm: height[0],
                    upper_mm: height[1],
                },
            )
        };
        let mut charged = 0usize;
        span.visit_occupied_screened(
            cancel,
            |bounds, height| {
                Ok(scoped(height).is_some() && escaped.any_near(range.clone(), bounds, gap))
            },
            |polygon, height| {
                let Some(scope) = scoped(height) else {
                    return Ok(true);
                };
                certify_polygon_escapes(
                    escaped,
                    range.clone(),
                    &ForbiddenPolygonScope {
                        polygon,
                        height: scope,
                        label: "continuous native/finish outer occupancy",
                    },
                    gap,
                    native_work,
                    &mut charged,
                    &trace.occupancy_tests,
                    trace,
                    cancel,
                )?;
                Ok(true)
            },
        )?;
        charge_support_clearance_range(work, charged, cancel)?;
    }
    Ok(())
}

/// Proves every escaped bead on `range`'s layers near one forbidden polygon
/// with the shared exact capsule predicate. The polygon is encoded and
/// prepared once, and only when an escape needs it.
#[expect(clippy::too_many_arguments)]
fn certify_polygon_escapes(
    escaped: &EscapeScope<'_>,
    range: std::ops::Range<usize>,
    forbidden: &ForbiddenPolygonScope<'_>,
    clearance: f64,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    work: &mut usize,
    tests: &TraceCount,
    trace: &ClearanceTrace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let ForbiddenPolygonScope {
        polygon,
        height: scope,
        label,
    } = *forbidden;
    let bounds = geo::BoundingRect::bounding_rect(polygon)
        .map(|b| [[b.min().x, b.min().y], [b.max().x, b.max().y]]);
    let encoded = std::cell::OnceCell::<Vec<spiling_geometry::SectionLoopMm>>::new();
    let mut prepared = None;
    escaped.visit_near(range, bounds, 0.0, clearance, work, cancel, |escape, work| {
        charge_support_clearance(work, cancel)?;
        let (layer, edge) = escaped.edge(escape)?;
        if !height_meets(scope, edge.band) {
            return Ok(());
        }
        let describe = || {
            format!(
                "actual emitted support layer {} against {label}, scope {scope:?}, segment {:?}, width {} mm, XY gap {clearance} mm",
                layer.index, edge.segment, edge.width
            )
        };
        if encoded.get().is_none() {
            let _ = encoded.set(encode_polygons(std::iter::once(polygon), 0.0, cancel)?);
        }
        if prepared.is_none() {
            let loops = encoded.get().expect("encoded actual forbidden polygon");
            prepared = Some(
                spiling_geometry::prepare_planar_forbidden(loops, native_work, cancel)
                    .map_err(|failure| super::native_error(failure, &describe()))?,
            );
        }
        trace.add(tests, 1);
        spiling_geometry::certify_planar_capsule_clearance_prepared(
            edge.segment,
            prepared.as_ref().expect("prepared actual forbidden polygon"),
            spiling_geometry::PlanarCapsuleClearancePolicy {
                radius_mm: edge.width * 0.5,
                required_gap_mm: clearance,
                endpoint_error_mm: edge.endpoint_error,
            },
            native_work,
            cancel,
        )
        .map_err(|failure| super::native_error(failure, &describe()))?;
        Ok(())
    })
}

/// The object/support capsule pair proof within the minimum Z gap. Full
/// bead height governs applicability; only a vertical separation meeting
/// the gap, above or below, skips the XY proof. Exact zero-gap touching is
/// not an interior collision when the requested gap is zero.
fn certify_capsule_pair(
    support: &EmittedEdge,
    object: &EmittedEdge,
    [xy_gap, z_gap]: [f64; 2],
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    trace: &ClearanceTrace,
    cancel: &AtomicBool,
    describe: impl FnOnce() -> String,
) -> Result<(), ManufacturingError> {
    let above = program_coordinates::enclosed_vertical_gap_mm(object.band[0], support.band[1])?;
    let below = program_coordinates::enclosed_vertical_gap_mm(support.band[0], object.band[1])?;
    if above >= z_gap || below >= z_gap {
        return Ok(());
    }
    trace.add(&trace.pair_tests, 1);
    let capsule = |edge: &EmittedEdge| spiling_geometry::PlanarCapsuleMm {
        segment_xy_mm: edge.segment,
        radius_mm: edge.width * 0.5,
        endpoint_error_mm: edge.endpoint_error,
    };
    spiling_geometry::certify_planar_capsule_pair_clearance(
        [capsule(support), capsule(object)],
        xy_gap,
        native_work,
        cancel,
    )
    .map_err(|failure| super::native_error(failure, &describe()))?;
    Ok(())
}

/// The layers whose beads can come within the minimum Z gap of the closed
/// outward height `[lower, upper]`.
fn gap_layer_range(
    band: &SupportBand<'_>,
    [lower, upper]: [f64; 2],
    z_gap: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<std::ops::Range<usize>, ManufacturingError> {
    // These are already outward physical bead bounds: expand by the minimum
    // Z gap, never reproject them as XYZ6 coordinates.
    let scope = spiling_geometry::ProtectedVoidHeightScope::Bounded {
        lower_mm: program_coordinates::enclosed_vertical_gap_mm(lower, z_gap)?,
        upper_mm: -program_coordinates::enclosed_vertical_gap_mm(-upper, z_gap)?,
    };
    scoped_support_layer_range(band.schedule, scope, band.vertical, work, cancel)
}

/// Resolves the escaped cells against actual deposited object and Bridge
/// beads within the minimum Z gap, layer by layer of escaped support.
fn certify_escaped_objects(
    escaped: &EscapeScope<'_>,
    xy_gap: f64,
    z_gap: f64,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    work: &mut usize,
    trace: &ClearanceTrace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut at = 0;
    while at < escaped.escapes.len() {
        let support_ordinal = escaped.escapes[at].layer;
        at += escaped.escapes[at..].partition_point(|e| e.layer == support_ordinal);
        let elevation = &escaped.band.schedule[support_ordinal];
        let applicable = gap_layer_range(
            &escaped.band,
            [
                (elevation.lower_z - escaped.band.vertical[0]).next_down(),
                (elevation.z + escaped.band.vertical[1]).next_up(),
            ],
            z_gap,
            work,
            cancel,
        )?;
        for object_ordinal in applicable {
            let object_layer = &escaped.layers[object_ordinal];
            charge_support_clearance(work, cancel)?;
            for object in &object_layer.paths {
                charge_support_clearance(work, cancel)?;
                if object.kind.is_support() {
                    continue;
                }
                let object_edges = object.checked_edges()?;
                let query = path_xy_bounds(object);
                escaped.visit_near(
                    support_ordinal..support_ordinal + 1,
                    query.map(|(bounds, _)| bounds),
                    query.map_or(0.0, |(_, width)| {
                        (width * 0.5 + XYZ6_SCREEN_SLACK_MM).next_up()
                    }),
                    xy_gap,
                    work,
                    cancel,
                    |escape, work| {
                        let (support_layer, support) = escaped.edge(escape)?;
                        for local in object_edges {
                            charge_support_clearance(work, cancel)?;
                            if spiling_geometry::planar_bounds_clear(
                                escape.bounds,
                                edge_xy_bounds(local.points_mm),
                                (local.width_mm * 0.5 + XYZ6_SCREEN_SLACK_MM).next_up(),
                                xy_gap,
                            ) {
                                continue;
                            }
                            certify_capsule_pair(
                                &support,
                                &emitted_edge(object, local)?,
                                [xy_gap, z_gap],
                                native_work,
                                trace,
                                cancel,
                                || {
                                    format!(
                                        "actual emitted support layer {} versus deposited object/bridge layer {}",
                                        support_layer.index, object_layer.index
                                    )
                                },
                            )?;
                        }
                        Ok(())
                    },
                )?;
            }
        }
    }
    Ok(())
}

/// Proves every object bead the sweep never excluded (see
/// [`super::support::KeptSupport::visit_unseen_object_edges`]) against all
/// actual support beads within the minimum Z gap.
#[expect(clippy::too_many_arguments)]
fn certify_unseen_objects(
    layers: &[PrintLayer],
    band: &SupportBand<'_>,
    sections: &[super::SectionSpan],
    kept: &super::support::KeptSupport,
    gaps: [f64; 2],
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    work: &mut usize,
    trace: &ClearanceTrace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let [xy_gap, z_gap] = gaps;
    // Each section proves with its own pair counter and geometry budget,
    // charged to the job's in section order.
    let states = kept.visit_unseen_object_edges(
        layers,
        sections,
        z_gap,
        work,
        cancel,
        || (0usize, spiling_geometry::GeometryWorkBudget::monitored()),
        |(pairs, native_work), object_ordinal, path, edge| {
            trace.add(&trace.unseen_object_edges, 1);
            let object_layer = &layers[object_ordinal];
            let object = &object_layer.paths[path];
            let Some(local) = object.checked_edges()?.edge(edge) else {
                return Ok(());
            };
            let object_edge = emitted_edge(object, local)?;
            let object_bounds = edge_xy_bounds(local.points_mm);
            let reserve = |width: f64| {
                ((width + local.width_mm) * 0.5 + 2.0 * XYZ6_SCREEN_SLACK_MM).next_up()
            };
            for support_ordinal in gap_layer_range(band, object_edge.band, z_gap, pairs, cancel)?
            {
                let support_layer = &layers[support_ordinal];
                charge_support_clearance(pairs, cancel)?;
                for support in &support_layer.paths {
                    charge_support_clearance(pairs, cancel)?;
                    if support.kind.is_model() {
                        continue;
                    }
                    let support_edges = support.checked_edges()?;
                    if path_xy_bounds(support).is_some_and(|(bounds, width)| {
                        spiling_geometry::planar_bounds_clear(
                            bounds,
                            object_bounds,
                            reserve(width),
                            xy_gap,
                        )
                    }) {
                        continue;
                    }
                    for support_local in support_edges {
                        charge_support_clearance(pairs, cancel)?;
                        if spiling_geometry::planar_bounds_clear(
                            edge_xy_bounds(support_local.points_mm),
                            object_bounds,
                            reserve(support_local.width_mm),
                            xy_gap,
                        ) {
                            continue;
                        }
                        certify_capsule_pair(
                            &emitted_edge(support, support_local)?,
                            &object_edge,
                            gaps,
                            native_work,
                            trace,
                            cancel,
                            || {
                                format!(
                                    "actual emitted support layer {} versus deposited object/bridge layer {}",
                                    support_layer.index, object_layer.index
                                )
                            },
                        )?;
                    }
                }
            }
            Ok(())
        },
    )?;
    for (pairs, spent) in states {
        charge_support_clearance_range(work, pairs, cancel)?;
        native_work
            .charge(spent.consumed(), cancel)
            .map_err(|failure| super::native_error(failure, "support unseen-object proof"))?;
    }
    Ok(())
}

/// The full capsule proof of every non-model path on the deposition's
/// layers against one forbidden polygon: a void a relieved support channel
/// enters. A bead the void's lease covers becomes a withdrawal record
/// instead of a refusal.
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
                for local in edges {
                    trace.add(&trace.edges, 1);
                    charge_support_clearance(support_work, cancel)?;
                    let emitted = emitted_edge(path, local)?;
                    let upper = emitted.band[1];
                    if !height_meets(scope, emitted.band) {
                        trace.add(&trace.height_rejects, 1);
                        continue;
                    }
                    let segment = emitted.segment;
                    let describe = || {
                        format!(
                            "actual emitted support layer {} against {label}, scope {scope:?}, segment {segment:?}, width {} mm, XY gap {clearance} mm",
                            layer.index, local.width_mm
                        )
                    };
                    if encoded.get().is_none() {
                        let _ =
                            encoded.set(encode_polygons(std::iter::once(polygon), 0.0, cancel)?);
                    }
                    if prepared.is_none() {
                        let loops = encoded.get().expect("encoded actual forbidden polygon");
                        prepared = Some(
                            spiling_geometry::prepare_planar_forbidden(loops, native_work, cancel)
                                .map_err(|failure| super::native_error(failure, &describe()))?,
                        );
                    }
                    let policy = spiling_geometry::PlanarCapsuleClearancePolicy {
                        radius_mm: local.width_mm * 0.5,
                        required_gap_mm: clearance,
                        endpoint_error_mm: emitted.endpoint_error,
                    };
                    trace.add(&trace.polygon_tests, 1);
                    let proof = spiling_geometry::certify_planar_capsule_clearance_prepared(
                        segment,
                        prepared
                            .as_ref()
                            .expect("prepared actual forbidden polygon"),
                        policy,
                        native_work,
                        cancel,
                    );
                    if let Err(failure) = proof {
                        if let Some((sweep, access)) = &mut withdrawal
                            && failure.code
                                == spiling_contracts::geometry::GeometryErrorCode::AnalysisIndeterminate
                            && access.covers_bead(ordinal, upper, segment, support_work, cancel)?
                        {
                            sweep.record(segment, policy, upper, native_work, cancel)?;
                        } else {
                            return Err(super::native_error(failure, &describe()));
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

pub(super) fn charge_support_clearance_range(
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
/// `None` when no support segment was emitted.
fn deposition_vertical_extensions(
    layers: &[PrintLayer],
    schedule: &[LayerElevation],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<[f64; 2]>, ManufacturingError> {
    let mut extensions = [0.0f64; 2];
    let mut any = false;
    for (layer, elevation) in layers.iter().zip(schedule) {
        charge_support_clearance(work, cancel)?;
        for path in &layer.paths {
            charge_support_clearance(work, cancel)?;
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
    Ok(any.then_some(extensions))
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
        spiling_geometry::ProtectedVoidHeightScope::OpenBelow { rim_mm, upper_mm } => {
            (rim_mm.is_finite().then_some(rim_mm), upper_mm)
        }
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
            // Exercise the actual-deposition consumer and real native capsule
            // proof, independently of C or contact-anchor authority. With
            // no kept grid every support bead escapes whole.
            for kept_layers in [false, true] {
                let kept = if kept_layers {
                    // Kept cells everywhere still never vouch for object
                    // beads the sweep never saw.
                    let everywhere =
                        MultiPolygon(vec![geo::Rect::new([-5.0, -5.0], [15.0, 5.0]).to_polygon()]);
                    super::super::support::KeptSupport::for_test(
                        [[-5.0, -5.0], [15.0, 5.0]],
                        (0..layers.len()).map(|layer| (layer, [-1.0, 10.0], everywhere.clone())),
                    )
                } else {
                    super::super::support::KeptSupport::default()
                };
                let sections: Vec<_> = (0..layers.len() as u32)
                    .map(|index| crate::planner::SectionSpan {
                        start: index,
                        end: index + 1,
                        material: Arc::new(MultiPolygon(Vec::new())),
                        process_domains: Vec::new(),
                        occupied: None,
                        source_area_mm2: 0.0,
                    })
                    .collect();
                let trace = ClearanceTrace {
                    enabled: true,
                    ..ClearanceTrace::default()
                };
                let result = certify_support_clearance_traced(
                    &layers,
                    &SupportClearanceGeometry {
                        sections: &sections,
                        schedule: &schedule,
                        protected: &MultiPolygon(Vec::new()),
                        scopes: &[],
                        index_bytes: 64 * 1024,
                        kept: &kept,
                    },
                    &SupportPolicy::Breakaway {
                        density: 0.3,
                        xy_clearance_mm: 0.2,
                        z_gap_mm: required,
                        interface_thickness_mm: 0.4,
                    },
                    &mut spiling_geometry::GeometryWorkBudget::new(10_000),
                    &mut 0,
                    &trace,
                    &cancel,
                )
                .map(|_| ());
                assert_eq!(
                    result.is_ok(),
                    accepted,
                    "actual Z gap={gap}, required={required}, object X={object_x}, support above={support_above}, kept={kept_layers}: {result:?}"
                );
                assert_eq!(trace.escaped_edges.get() == 0, kept_layers);
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
