// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{SectionSpan, boolean_budget, region, span_at};
use crate::{check_cancel, error};
use geo::{BooleanOps, BoundingRect, Contains, Distance, Euclidean, MultiPolygon, Polygon};
use spiling_contracts::manufacturing::*;
use std::{
    rc::Rc,
    sync::{Arc, atomic::AtomicBool},
};
mod anchors;
mod beads;
mod grid;
#[cfg(test)]
pub(crate) use anchors::ProjectedAnchorPaths;
pub(super) use anchors::inward_bond_endpoint;
pub(crate) use anchors::strict_capsule_point;
pub(crate) use anchors::{
    AnchorCandidate, AnchorWorkspace, PreparedAnchorPaths, endpoint_bond, projected_edges,
};
pub(in crate::planner) mod strokes;
pub(crate) use strokes::StrokeRuns;
pub(crate) use strokes::{GapKind, gap_kind};
#[cfg(test)]
mod contact_extensions;
mod established;
mod field;
pub(in crate::planner) mod projection;
mod raster;
mod sweep;
pub(in crate::planner) use sweep::KeptSupport;

struct ProjectionTrace {
    enabled: bool,
    started: std::time::Instant,
    section_pieces: Vec<std::cell::Cell<usize>>,
    bed: std::cell::Cell<usize>,
    direct_bed: std::cell::Cell<usize>,
    part: std::cell::Cell<usize>,
    visits: std::cell::Cell<usize>,
    screened: std::cell::Cell<usize>,
    built: std::cell::Cell<usize>,
    staged: std::cell::Cell<usize>,
    cohorts: std::cell::Cell<usize>,
    cohort_retries: std::cell::Cell<usize>,
    section_builds: std::cell::Cell<usize>,
    frame_distance: std::cell::Cell<f64>,
    frame_quantum: std::cell::Cell<f64>,
    old_max_reach: std::cell::Cell<f64>,
    batches: std::cell::Cell<usize>,
    batch_reuse_hits: std::cell::Cell<usize>,
    batch_reuse_misses: std::cell::Cell<usize>,
    batch_reuse_declines: std::cell::Cell<usize>,
    batch_reuse_peak_bytes: std::cell::Cell<usize>,
    batch_reuse_time: std::cell::Cell<std::time::Duration>,
    column_time: std::cell::Cell<std::time::Duration>,
    bead_time: std::cell::Cell<std::time::Duration>,
    sheet_time: std::cell::Cell<std::time::Duration>,
}

impl ProjectionTrace {
    fn new(sections: usize, retained: usize, owned_bytes: usize) -> Self {
        let mut section_pieces = Vec::new();
        let enabled = std::env::var_os("SPILING_TRACE_SUPPORT_PROJECTION").is_some()
            && super::remaining_geometry_units(
                retained.saturating_add(sections),
                owned_bytes.saturating_add(sections.saturating_mul(std::mem::size_of::<usize>())),
            )
            .is_ok()
            && section_pieces.try_reserve_exact(sections).is_ok();
        if enabled {
            section_pieces.resize_with(sections, || std::cell::Cell::new(0));
        }
        Self {
            enabled,
            started: std::time::Instant::now(),
            section_pieces,
            bed: Default::default(),
            direct_bed: Default::default(),
            part: Default::default(),
            visits: Default::default(),
            screened: Default::default(),
            built: Default::default(),
            staged: Default::default(),
            cohorts: Default::default(),
            cohort_retries: Default::default(),
            section_builds: Default::default(),
            frame_distance: Default::default(),
            frame_quantum: Default::default(),
            old_max_reach: Default::default(),
            batches: Default::default(),
            batch_reuse_hits: Default::default(),
            batch_reuse_misses: Default::default(),
            batch_reuse_declines: Default::default(),
            batch_reuse_peak_bytes: Default::default(),
            batch_reuse_time: Default::default(),
            column_time: Default::default(),
            bead_time: Default::default(),
            sheet_time: Default::default(),
        }
    }

    fn report(&self, phase: &str) {
        if self.enabled && (phase != "complete" || self.visits.get() > 0) {
            eprintln!(
                "{}",
                serde_json::json!({
                    "event": "support_projection", "phase": phase,
                    "elapsed_seconds": self.started.elapsed().as_secs_f64(),
                    "bed_columns": self.bed.get(), "direct_bed_columns": self.direct_bed.get(),
                    "part_columns": self.part.get(), "section_visits": self.visits.get(),
                    "piece_visits": self.screened.get(), "distinct_pieces": self.section_pieces.iter().map(std::cell::Cell::get).sum::<usize>(),
                    "pieces_built": self.built.get(), "pieces_staged": self.staged.get(),
                    "cohorts": self.cohorts.get(), "clearance_batches": self.batches.get(),
                    "cohort_retries": self.cohort_retries.get(), "section_builds": self.section_builds.get(),
                    "batch_reuse_hits": self.batch_reuse_hits.get(),
                    "batch_reuse_misses": self.batch_reuse_misses.get(),
                    "batch_reuse_declines": self.batch_reuse_declines.get(),
                    "batch_reuse_peak_bytes": self.batch_reuse_peak_bytes.get(),
                    "batch_reuse_seconds": self.batch_reuse_time.get().as_secs_f64(),
                    "fixed_clearance_distance_mm": self.frame_distance.get(),
                    "fixed_grid_quantum_mm": self.frame_quantum.get(),
                    "old_max_column_reach_mm": self.old_max_reach.get(),
                    "column_seconds": self.column_time.get().as_secs_f64(),
                    "bead_seconds": self.bead_time.get().as_secs_f64(),
                    "sheet_seconds": self.sheet_time.get().as_secs_f64(),
                })
            );
        }
    }
}

impl Drop for ProjectionTrace {
    fn drop(&mut self) {
        self.report("complete");
    }
}

#[derive(Clone)]
struct Demand {
    target: usize,
    failure: strokes::Failure,
    /// The target layer's worst unanchored span at the end of demand analysis.
    worst_unanchored_mm: f64,
}

#[derive(Clone, Copy)]
enum Root {
    Bed,
    Part {
        object_layer: usize,
        first_support_layer: usize,
        landing: usize,
    },
}

pub(super) struct RasterCandidate {
    pub(super) paths: Vec<DepositionPath>,
    pub(super) whole_rows: Vec<DepositionPath>,
    pub(super) process_interfaces: Vec<ProcessInterfaceCoverage>,
    pub(super) skin_bridge_domains: Vec<SkinBridgeDomain>,
    pub(super) skin_command_fits: Vec<SkinCommandFitDomain>,
}

fn authored_component_axis(
    candidate: &RasterCandidate,
    course: u32,
    recipe: &PlanarPrintRecipe,
) -> region::HatchAxis {
    let top = !candidate.paths.is_empty()
        && candidate.paths.iter().all(|path| {
            path.owner
                == Some(DepositionOwner::MandatorySkin {
                    skin: ProcessSkinKind::TopSkin,
                })
        });
    region::HatchAxis::for_policy(
        if top {
            recipe.top_skin_axis
        } else {
            recipe.rectilinear_axis
        },
        course,
    )
}

pub(super) fn component_admission<'a>(
    (candidate, axis, budget, contact_bridge): (
        &mut RasterCandidate,
        region::HatchAxis,
        usize,
        bool,
    ),
    previous: impl Iterator<Item = &'a DepositionPath> + Clone,
    contacts: impl Iterator<Item = &'a DepositionPath> + Clone,
    recipe: &PlanarPrintRecipe,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<super::classification::ComponentAnchorAdmission, ManufacturingError> {
    // One borrowed lower set for every endpoint, row and stroke query below.
    let mut lower = strokes::LowerStrokes::new(previous.clone(), contacts.clone());
    for path in &mut candidate.paths {
        let Some(DepositionOwner::RegionalSkinBridge { skin_first, .. }) = path.owner else {
            continue;
        };
        let [a, transition, b] = path.points_mm.as_slice() else {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "mixed Bridge lost its sole transition",
            ));
        };
        let (a, transition, b) = (*a, *transition, *b);
        let along = if a[0] == b[0] { 1 } else { 0 };
        let t = (transition[along] - a[along]) / (b[along] - a[along]);
        anchors.set_budget(budget);
        let mut endpoint = None;
        for gap in lower.gaps(
            AnchorCandidate {
                endpoints_mm: [a, b],
                bead_width_mm: region::constant_width(path)?,
            },
            path.physical_z_mm[0],
            false,
            anchors,
            work,
            cancel,
        )? {
            if gap.start < t && t < gap.end {
                let parameter = if skin_first && gap.left {
                    Some(gap.start)
                } else if !skin_first && gap.right {
                    Some(gap.end)
                } else {
                    None
                };
                endpoint = parameter.map(|parameter| {
                    std::array::from_fn(|axis| a[axis] + (b[axis] - a[axis]) * parameter)
                });
                break;
            }
        }
        if let Some(endpoint) = endpoint {
            path.points_mm[if skin_first { 0 } else { 2 }] = endpoint;
        }
    }
    if !candidate.whole_rows.is_empty() {
        let owned = candidate
            .paths
            .iter()
            .chain(&candidate.whole_rows)
            .map(|path| path.points_mm.capacity())
            .sum::<usize>();
        let retained = (crate::working_set::units())
            .checked_sub(budget)
            .and_then(|total| total.checked_sub(owned))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "corridor raster retention accounting mismatch",
                )
            })?;
        coalesce_bridge_rows_with(
            candidate,
            BridgeRowContext {
                axis,
                demanded_reorientation: contact_bridge,
                retained_points: retained,
                owned_bytes: 0,
                recipe,
            },
            previous.clone(),
            contacts.clone(),
            &mut lower,
            anchors,
            work,
            cancel,
        )?;
    }
    let paths = &candidate.paths;
    let query_budget = (budget / 2).max(anchors.retained_records());
    let witness_budget = budget.checked_sub(query_budget).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "live component anchor workspace exceeds retention",
        )
    })?;
    let mut proof = super::classification::ComponentAnchorAdmission {
        supported: true,
        bridges: Vec::new(),
        demands: Vec::new(),
        workspace_points: anchors.retained_records(),
        workspace_bytes: anchors.retained_bytes(),
    };
    for (index, path) in paths.iter().enumerate() {
        check_cancel(cancel)?;
        if path.kind == DepositionKind::Perimeter {
            continue;
        }
        if path.points_mm.len() != 2
            && !matches!(path.owner, Some(DepositionOwner::RegionalSkinBridge { .. }))
        {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "component hatch is not a straight stroke",
            ));
        }
        let a = path.points_mm[0];
        let b = *path.points_mm.last().expect("validated component");
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        anchors.set_budget(query_budget);
        let mut bridge = false;
        let mut demand = false;
        for gap in lower.gaps(
            AnchorCandidate {
                endpoints_mm: [a, b],
                bead_width_mm: region::constant_width(path)?,
            },
            path.physical_z_mm[0],
            true,
            anchors,
            work,
            cancel,
        )? {
            match strokes::gap_kind(
                gap,
                (gap.end - gap.start) * length,
                region::constant_width(path)?,
                recipe.process.max_bridge_span_mm,
            ) {
                strokes::GapKind::Supported => {}
                // Only an explicit demand-rooted orthogonal construction may
                // use a real bounded gap ending on eligible support contacts.
                strokes::GapKind::Contact => {
                    bridge |= contact_bridge || path.kind == DepositionKind::Bridge;
                }
                strokes::GapKind::Bridge => bridge = true,
                strokes::GapKind::Demand => {
                    demand = true;
                    reserve_component_record(
                        &mut proof.demands,
                        2,
                        proof.bridges.capacity(),
                        witness_budget,
                    )?;
                    proof.demands.push([
                        std::array::from_fn(|axis| a[axis] + (b[axis] - a[axis]) * gap.start),
                        std::array::from_fn(|axis| a[axis] + (b[axis] - a[axis]) * gap.end),
                    ]);
                }
            }
        }
        if demand {
            proof.supported = false;
        } else if bridge {
            reserve_component_record(
                &mut proof.bridges,
                1,
                proof.demands.capacity() * 2,
                witness_budget,
            )?;
            proof.bridges.push(index);
        }
    }
    proof.workspace_points = anchors.retained_records();
    proof.workspace_bytes = anchors.retained_bytes();
    Ok(proof)
}

pub(super) fn component_budget(
    retained: usize,
    owned_bytes: usize,
    scratch: usize,
    scratch_owned_bytes: usize,
) -> Result<usize, ManufacturingError> {
    workspace_budget(
        retained.saturating_add(scratch),
        owned_bytes.saturating_add(scratch_owned_bytes),
    )
}

fn stroke_proof_reservation(paths: usize) -> usize {
    // The stroke analyzer grows two Vec<usize> of path indices (bridges and
    // unanchored). Reserve their old/new growth arrays; use actual capacity
    // once returned.
    if paths == 0 {
        0
    } else {
        paths
            .saturating_mul(6)
            .max(4)
            .saturating_mul(std::mem::size_of::<usize>())
    }
}

fn workspace_budget(units: usize, bytes: usize) -> Result<usize, ManufacturingError> {
    let remaining = super::remaining_geometry_units(units, bytes)?;
    // Reserve both old/new workspace arrays during growth. These record
    // reservations are separate from measured persistent payload receipts.
    let stride = 2 * std::mem::size_of::<anchors::AnchorGap>().max(std::mem::size_of::<(
        f64,
        f64,
        f64,
        usize,
    )>());
    Ok(remaining.min((crate::working_set::bytes() - bytes) / stride))
}

fn reserve_component_record<T>(
    values: &mut Vec<T>,
    units: usize,
    other: usize,
    budget: usize,
) -> Result<(), ManufacturingError> {
    if values.len() < values.capacity() {
        return Ok(());
    }
    let limit = budget.saturating_sub(other) / units;
    let capacity = values.capacity().saturating_mul(2).max(4).min(limit);
    if capacity <= values.len() {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "component anchor witness retention exceeded",
        ));
    }
    values
        .try_reserve_exact(capacity - values.len())
        .map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "component anchor witness allocation refused",
            )
        })
}

pub(super) struct ProtectedVoidSet {
    pub regions: MultiPolygon<f64>,
    pub scopes: Vec<VoidScope>,
}

pub(super) struct VoidScope {
    pub height: spiling_geometry::ProtectedVoidHeightScope,
    roof: Option<Arc<spiling_geometry::tolerant::TolerantVoidRoof>>,
    access: Vec<RoofAccess>,
}

struct RoofAccess {
    layers: std::ops::Range<usize>,
    column: Rc<OwnedMaterial>,
}

impl VoidScope {
    pub(super) fn new(
        height: spiling_geometry::ProtectedVoidHeightScope,
        roof: Option<Arc<spiling_geometry::tolerant::TolerantVoidRoof>>,
    ) -> Self {
        Self {
            height,
            roof,
            access: Vec::new(),
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn allows_roof_access(
        &self,
        root: Root,
        target: &PrintLayer,
        top: &PrintLayer,
        path: &DepositionPath,
        failure: &strokes::Failure,
        support_width: f64,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<bool, ManufacturingError> {
        let Some(roof) = &self.roof else {
            return Ok(false);
        };
        let [lower, upper] = roof.height_mm;
        if !matches!(root, Root::Bed)
            || !matches!(
                self.height,
                spiling_geometry::ProtectedVoidHeightScope::OpenBelow { .. }
            )
            || top.z_mm > lower
            || target.lower_z_mm > upper
            || target.z_mm < lower
        {
            return Ok(false);
        }
        let start = path.points_mm[failure.first];
        let end = path.points_mm[failure.last];
        let [from, to] = [failure.start, failure.end].map(|at| geo::Coord {
            x: start[0] + (end[0] - start[0]) * at,
            y: start[1] + (end[1] - start[1]) * at,
        });
        let Some(edge) = path.checked_edges()?.edge(failure.first) else {
            return Ok(false);
        };
        let reach = (edge.width_mm + support_width) * 0.5;
        let segment = geo::Line::new(from, to);
        for &facet in &roof.facets {
            super::native::charge_support_clearance(work, cancel)?;
            let contour = geo::Triangle::new(facet[0].into(), facet[1].into(), facet[2].into());
            if contour.contains(&geo::Point::from(from))
                || contour.contains(&geo::Point::from(to))
                || (0..3).any(|index| {
                    Euclidean.distance(
                        &segment,
                        &geo::Line::new(facet[index], facet[(index + 1) % 3]),
                    ) < reach
                })
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Records the relieved support one layer may deposit inside this void.
    /// Adjacent layers share a record only when they share the same region;
    /// each layer is otherwise checked against its own region, never a
    /// neighbour's.
    fn record_access(&mut self, layer: usize, column: Rc<OwnedMaterial>) {
        if let Some(last) = self
            .access
            .last_mut()
            .filter(|last| last.layers.end == layer && Rc::ptr_eq(&last.column, &column))
        {
            last.layers.end += 1;
        } else {
            self.access.push(RoofAccess {
                layers: layer..layer + 1,
                column,
            });
        }
    }

    pub(super) fn has_access(&self) -> bool {
        !self.access.is_empty()
    }

    pub(super) fn covers_bead(
        &self,
        ordinal: usize,
        upper: f64,
        segment: [[f64; 2]; 2],
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<bool, ManufacturingError> {
        if self
            .roof
            .as_ref()
            .is_none_or(|roof| upper > roof.height_mm[1])
        {
            return Ok(false);
        }
        let line = geo::Line::new(segment[0], segment[1]);
        for access in &self.access {
            super::native::charge_support_clearance(work, cancel)?;
            if access.layers.contains(&ordinal) && access.column.contains(&line) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[cfg(test)]
    pub(super) fn allow_test_bead(&mut self, ordinal: usize, column: MultiPolygon<f64>) {
        self.access.push(RoofAccess {
            layers: ordinal..ordinal + 1,
            column: OwnedMaterial::new(column).into_shared(),
        });
    }
}

/// Admission inputs shared by whole-row ownership and deposited-anchor proof.
struct BridgeRowContext<'a> {
    axis: region::HatchAxis,
    demanded_reorientation: bool,
    retained_points: usize,
    owned_bytes: usize,
    recipe: &'a PlanarPrintRecipe,
}

/// Exact payload receipt captured before worker-local sharing.
/// The Cell is temporary inventory state; these owners never cross threads.
struct OwnedMaterial {
    material: MultiPolygon<f64>,
    bytes: usize,
    units: usize,
    header_bytes: usize,
    counted: std::cell::Cell<bool>,
}

impl OwnedMaterial {
    fn new(mut material: MultiPolygon<f64>) -> Self {
        let bytes = super::native::material_bytes(&mut material);
        let units = material_units(&material);
        Self {
            material,
            bytes,
            units,
            header_bytes: 0,
            counted: std::cell::Cell::new(false),
        }
    }

    fn into_shared(mut self) -> Rc<Self> {
        self.header_bytes = std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>();
        Rc::new(self)
    }
}

impl std::ops::Deref for OwnedMaterial {
    type Target = MultiPolygon<f64>;
    fn deref(&self) -> &Self::Target {
        &self.material
    }
}

/// Realized object geometry and the retained native ownership used by support.
pub(super) struct SupportInput<'a> {
    pub(super) build_bounds: [[f64; 2]; 2],
    pub(super) sections: &'a [SectionSpan],
    pub(super) schedule: &'a [super::LayerElevation],
    pub(super) native_points: usize,
    /// Outside owners, including the frozen cache and layer record array,
    /// but excluding every layer's mutable path and coverage payloads. The
    /// projection callback refreshes the native proof component after compaction.
    pub(super) native_owned_bytes: usize,
    pub(super) recipe: &'a PlanarPrintRecipe,
    pub(super) nominal_height_mm: f64,
    pub(super) source_z_allowance_mm: f64,
}

#[derive(Default)]
pub(super) struct SupportWork {
    pub(super) planar: usize,
    /// Support-owned subset of planar work, excluding object raster/skins.
    /// This receipt survives every compile-local schedule retry.
    pub(super) support_owned: usize,
    pub(super) anchors: usize,
}

impl SupportWork {
    fn foreign_work(&self) -> Result<usize, ManufacturingError> {
        self.planar.checked_sub(self.support_owned).ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "support work-domain accounting mismatch",
            )
        })
    }

    fn finish(
        &mut self,
        planar: usize,
        foreign: usize,
        anchors: usize,
    ) -> Result<(), ManufacturingError> {
        self.planar = planar;
        self.anchors = anchors;
        self.support_owned = planar.checked_sub(foreign).ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "support work-domain publication mismatch",
            )
        })?;
        Ok(())
    }
}

// Exclude foreign work from the local policy only. The actual global counter
// is never reset, offset, refunded, or charged a second time.
fn exclude_object_raster_work(
    foreign: &mut usize,
    before: usize,
    after: usize,
) -> Result<(), ManufacturingError> {
    *foreign = after
        .checked_sub(before)
        .and_then(|charged| foreign.checked_add(charged))
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "object raster work-domain accounting mismatch",
            )
        })?;
    Ok(())
}

pub(super) enum SupportAdmission {
    Complete(Option<super::native::SupportWithdrawal>),
    /// Explicit Off's startup pair asks for a thinner first interval.
    RefineUnreachableLayer {
        index: usize,
        failure: ManufacturingError,
    },
}

// Anchors come only from actual deposited capsules. Corridor exclusion consumes
// every lower slab's continuous-target outer occupancy, separately from the
// sampled/scheduled D used by walls and physical-distance skin classification.
#[expect(
    clippy::too_many_arguments,
    reason = "Support orchestration keeps native void projection, raster generation and interface finalization as distinct callbacks"
)]
pub(super) fn realize(
    layers: &mut [PrintLayer],
    input: &SupportInput<'_>,
    segments: &mut usize,
    cancel: &AtomicBool,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    tentative: bool,
    work_bank: &mut SupportWork,
    mut raster: impl FnMut(
        usize,
        region::HatchAxis,
        &mut usize,
        usize,
        usize,
        usize,
        &mut usize,
        Option<&mut super::classification::ComponentAnchorCheck<'_>>,
    ) -> Result<RasterCandidate, ManufacturingError>,
    mut project_voids: impl FnMut(
        &[[[f64; 2]; 2]],
        usize,
        // Complete live bytes, followed by mutable native unit/byte receipts.
        usize,
        &mut usize,
        &mut usize,
        &mut usize,
        &mut spiling_geometry::GeometryWorkBudget,
    ) -> Result<ProtectedVoidSet, ManufacturingError>,
    mut finalize_interface: impl FnMut(
        usize,
        &mut RasterCandidate,
        &mut usize,
        &mut usize,
        usize,
    ) -> Result<bool, ManufacturingError>,
) -> Result<SupportAdmission, ManufacturingError> {
    let mut timing = crate::timing::Clock::new("support.realize");
    let trace = ProjectionTrace::new(
        input.schedule.len(),
        input.native_points,
        input.native_owned_bytes,
    );
    let mut work = work_bank.planar;
    let mut anchor_work = work_bank.anchors;
    let mut foreign_work = work_bank.foreign_work()?;
    let outcome = (|| {
        let SupportInput {
            build_bounds: _,
            sections,
            schedule,
            mut native_points,
            mut native_owned_bytes,
            recipe,
            nominal_height_mm,
            source_z_allowance_mm,
        } = *input;
        native_points = native_points.saturating_add(trace.section_pieces.len());
        native_owned_bytes = native_owned_bytes.saturating_add(vec_bytes(&trace.section_pieces));
        let resolved_recipe = PlanarPrintRecipe {
            layer_height_mm: nominal_height_mm,
            ..recipe.clone()
        };
        let recipe = &resolved_recipe;
        let width = recipe.bead_width_mm;
        let mut demands: Vec<Demand> = Vec::new();
        let mut constructed_points = layers.iter().try_fold(0usize, |total, layer| {
            total
                .checked_add(super::bonds::skin_command_fit_points(
                    &layer.skin_command_fits,
                ))
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "initial skin command fit accounting overflow",
                    )
                })
        })?;
        let mut retained = native_points
            .checked_add(constructed_points)
            .filter(|n| *n <= crate::working_set::units())
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "initial skin command fit retained budget exceeded",
                )
            })?;
        let mut anchors = AnchorWorkspace::default();
        super::remaining_geometry_units(
            retained,
            native_owned_bytes
                .saturating_add(layer_payload_bytes(layers.iter()))
                .saturating_add(layers.len().saturating_mul(std::mem::size_of::<bool>())),
        )?;
        let mut joined_layers = vec![false; layers.len()];
        let mut original_bridge_roles = Vec::new();
        let mut owned_bytes = native_owned_bytes
            .saturating_add(layer_payload_bytes(layers.iter()))
            .saturating_add(joined_layers.capacity() * std::mem::size_of::<bool>());
        anchors.set_budget(workspace_budget(retained, owned_bytes)?);
        if let Some(first) = layers.first_mut()
            && !finalize_layer_interface(
                0,
                first,
                segments,
                &mut work,
                owned_bytes.saturating_add(anchors.retained_bytes()),
                &mut finalize_interface,
            )?
        {
            return Err(super::bonds::unresolved(first));
        }
        let mut split = crate::timing::Split::new("support.demand");
        // Layer payload accounting is read many times per layer; only the
        // current layer changes, so keep a per-layer ledger.
        let mut payload = PayloadLedger::new(layers)?;
        for index in 1..layers.len() {
            check_cancel(cancel)?;
            payload.refresh(index - 1, &layers[index - 1]);
            owned_bytes = native_owned_bytes
                + payload.refresh(index, &layers[index])
                + vec_bytes(&joined_layers)
                + vec_bytes(&demands)
                + vec_bytes(&original_bridge_roles);
            anchors.set_budget(workspace_budget(retained, owned_bytes)?);
            let (prior, remaining) = layers.split_at_mut(index);
            let layer = remaining.first_mut().expect("current support layer");
            check_cancel(cancel)?;
            layer.region_coverage.unsupported_overhang_mm = 0.0;
            let t = split.start();
            let mut interface_bound = finalize_layer_interface(
                index,
                layer,
                segments,
                &mut work,
                owned_bytes.saturating_add(anchors.retained_bytes()),
                &mut finalize_interface,
            )?;
            split.stop("finalize_interface", t);
            owned_bytes = native_owned_bytes
                + payload.refresh(index, layer)
                + vec_bytes(&joined_layers)
                + vec_bytes(&demands)
                + vec_bytes(&original_bridge_roles);
            anchors.set_budget(workspace_budget(retained, owned_bytes)?);
            let authored_axis = region::HatchAxis::for_policy(
                recipe.rectilinear_axis,
                super::hatch_key(schedule, index),
            );
            let previous = prior[index - 1].paths.iter();
            anchors.set_budget(workspace_budget(
                retained,
                owned_bytes.saturating_add(stroke_proof_reservation(layer.paths.len())),
            )?);
            let t = split.start();
            let mut admission = strokes::analyze(
                &layer.paths,
                previous.clone(),
                std::iter::empty(),
                recipe,
                &mut anchors,
                &mut anchor_work,
                cancel,
            )?;
            split.stop("analyze", t);
            let t = split.start();
            if admission.failure.is_some() || !interface_bound {
                // Preserve ordinary authored hatch; only a finite demanded role
                // component may acquire an independently proved Bridge construction.
                let without_old = *segments - path_segments(&layer.paths);
                for recover_components in [false, true] {
                    let axis = authored_axis;
                    let mut staged_segments = without_old;
                    anchors.set_budget(0);
                    let extra_owned_bytes = payload.refresh(index, layer)
                        + vec_bytes(&joined_layers)
                        + vec_bytes(&demands)
                        + vec_bytes(&original_bridge_roles)
                        + vec_bytes(&admission.bridges)
                        + vec_bytes(&admission.unanchored);
                    owned_bytes = native_owned_bytes.saturating_add(extra_owned_bytes);
                    let staged_anchor_bytes = anchors.retained_bytes();
                    let course = super::hatch_key(schedule, index);
                    let mut check_component =
                        |candidate: &mut RasterCandidate,
                         axis,
                         scratch: usize,
                         scratch_owned_bytes: usize| {
                            let contact_bridge =
                                axis != authored_component_axis(candidate, course, recipe);
                            let budget = component_budget(
                                retained,
                                owned_bytes,
                                scratch.saturating_add(candidate_units(candidate)),
                                scratch_owned_bytes
                                    .saturating_add(candidate_owned_bytes(candidate)),
                            )?;
                            component_admission(
                                (candidate, axis, budget, contact_bridge),
                                previous.clone(),
                                std::iter::empty(),
                                recipe,
                                &mut anchors,
                                &mut anchor_work,
                                cancel,
                            )
                        };
                    let recovery: Option<&mut super::classification::ComponentAnchorCheck<'_>> =
                        if recover_components {
                            Some(&mut check_component)
                        } else {
                            None
                        };
                    let raster_before = work;
                    let t2 = split.start();
                    let candidate = raster(
                        index,
                        axis,
                        &mut staged_segments,
                        constructed_points,
                        extra_owned_bytes,
                        staged_anchor_bytes,
                        &mut work,
                        recovery,
                    );
                    exclude_object_raster_work(&mut foreign_work, raster_before, work)?;
                    let mut candidate = candidate?;
                    split.stop(
                        if recover_components {
                            "raster_components"
                        } else {
                            "raster_plain"
                        },
                        t2,
                    );
                    let t2 = split.start();
                    let joined = coalesce_bridge_rows(
                        &mut candidate,
                        BridgeRowContext {
                            axis,
                            demanded_reorientation: false,
                            retained_points: retained,
                            owned_bytes,
                            recipe,
                        },
                        previous.clone(),
                        std::iter::empty(),
                        &mut anchors,
                        &mut anchor_work,
                        cancel,
                    )?;
                    split.stop(
                        if recover_components {
                            "coalesce_components"
                        } else {
                            "coalesce_plain"
                        },
                        t2,
                    );
                    let candidate_bound = finalize_interface(
                        index,
                        &mut candidate,
                        &mut staged_segments,
                        &mut work,
                        owned_bytes.saturating_add(anchors.retained_bytes()),
                    )?;
                    if !candidate_bound {
                        if recover_components && !interface_bound {
                            // Diagnostic only: an unresolved layer always refuses
                            // before it can become the next layer's anchor authority.
                            layer.paths = candidate.paths;
                            layer.region_coverage.process_interfaces = candidate.process_interfaces;
                            {
                                replace_skin_command_fits(
                                    &mut layer.skin_command_fits,
                                    candidate.skin_command_fits,
                                    &mut retained,
                                    &mut constructed_points,
                                )?;
                                replace_skin_bridge_domains(
                                    &mut layer.region_coverage,
                                    candidate.skin_bridge_domains,
                                    &mut retained,
                                    &mut constructed_points,
                                )?;
                            };
                        }
                        continue;
                    }
                    anchors.set_budget(candidate_budget(
                        &candidate.paths,
                        retained,
                        owned_bytes
                            .saturating_add(candidate_aux_bytes(&candidate))
                            .saturating_add(stroke_proof_reservation(candidate.paths.len())),
                    )?);
                    // Paths are judged independently against the same committed
                    // lower layer, so an attempt that rebuilt the layer's current
                    // paths unchanged keeps the admission already proved for them.
                    let t2 = split.start();
                    let other = if candidate.paths == layer.paths {
                        admission.clone()
                    } else {
                        strokes::analyze(
                            &candidate.paths,
                            previous.clone(),
                            std::iter::empty(),
                            recipe,
                            &mut anchors,
                            &mut anchor_work,
                            cancel,
                        )?
                    };
                    split.stop(
                        if recover_components {
                            "reanalyze_components"
                        } else {
                            "reanalyze_plain"
                        },
                        t2,
                    );
                    let complete = other.failure.is_none();
                    // Same-axis joins preserve every required piece and have their
                    // own direct-anchor proof. Keep that monotone improvement even
                    // when another stroke still needs support; do not demand a
                    // corridor for fragments that we can already anchor.
                    let component_bridges = candidate
                        .paths
                        .iter()
                        .any(|path| path.kind == DepositionKind::Bridge);
                    if complete || joined || component_bridges {
                        *segments = without_old + path_segments(&candidate.paths);
                        layer.paths = candidate.paths;
                        layer.region_coverage.process_interfaces = candidate.process_interfaces;
                        {
                            replace_skin_command_fits(
                                &mut layer.skin_command_fits,
                                candidate.skin_command_fits,
                                &mut retained,
                                &mut constructed_points,
                            )?;
                            replace_skin_bridge_domains(
                                &mut layer.region_coverage,
                                candidate.skin_bridge_domains,
                                &mut retained,
                                &mut constructed_points,
                            )?;
                        };
                        interface_bound = true;
                        joined_layers[index] = joined || component_bridges;
                        admission = other;
                        if complete {
                            break;
                        }
                    }
                }
                owned_bytes = native_owned_bytes
                    + payload.refresh(index, layer)
                    + vec_bytes(&joined_layers)
                    + vec_bytes(&demands)
                    + vec_bytes(&original_bridge_roles);
                anchors.set_budget(workspace_budget(retained, owned_bytes)?);
            }
            split.stop("recovery", t);
            if !interface_bound {
                return Err(super::bonds::unresolved(layer));
            }
            // Permanent construction and its first upper landing may never be
            // repaired by temporary support, even when unrelated support is enabled.
            // Paths are analyzed independently, so the layer's admission already
            // names every path whose own proof would fail.
            for path in admission.unanchored.iter().map(|&path| &layer.paths[path]) {
                let Some(DepositionOwner::Regional { region_index }) = path.owner else {
                    continue;
                };
                let plate_owned = layer
                    .region_coverage
                    .process_diaphragms
                    .iter()
                    .any(|plate| plate.region_index == region_index);
                let landing_owned = prior[index - 1]
                    .region_coverage
                    .process_diaphragms
                    .iter()
                    .filter(|plate| plate.physical_z_mm[1] == layer.lower_z_mm)
                    .any(|plate| {
                        layer
                            .region_coverage
                            .process_regions
                            .get(usize::from(region_index))
                            .is_some_and(|region| {
                                region.contributions.iter().any(|source| {
                                    source.occurrence_id == plate.occurrence_id
                                        && source.definition_id == plate.definition_id
                                })
                            })
                    });
                if plate_owned || landing_owned {
                    return Err(error(
                        ManufacturingErrorCode::UnsupportedGeometry,
                        format!(
                            "UnsupportedInterface: permanent diaphragm or upper landing at layer {} Z={} mm lacks actual lower anchors; temporary support is not authorized",
                            layer.index, layer.z_mm
                        ),
                    ));
                }
            }
            anchors.set_budget(workspace_budget(
                retained,
                owned_bytes
                    .saturating_add(vec_bytes(&admission.bridges))
                    .saturating_add(vec_bytes(&admission.unanchored))
                    .saturating_add(stroke_proof_reservation(layer.paths.len())),
            )?);
            if let Some(failure) = admission.failure.take() {
                if matches!(recipe.supports, SupportPolicy::Disabled) {
                    if tentative && index > 0 {
                        let failure =
                            admission_error(index, layer.z_mm, &layer.paths, &failure, recipe);
                        return Ok(SupportAdmission::RefineUnreachableLayer { index, failure });
                    }
                    // Explicit Off retains authored overhangs and reports them.
                    layer.region_coverage.unsupported_overhang_mm = admission.worst_unanchored_mm;
                } else {
                    let live = native_owned_bytes
                        + payload.refresh(index, layer)
                        + vec_bytes(&joined_layers)
                        + vec_bytes(&demands)
                        + vec_bytes(&original_bridge_roles)
                        + anchors.retained_bytes()
                        + vec_bytes(&admission.bridges)
                        + vec_bytes(&admission.unanchored);
                    reserve_owned(&mut demands, 1, live)?;
                    demands.push(Demand {
                        target: index,
                        failure,
                        worst_unanchored_mm: admission.worst_unanchored_mm,
                    });
                }
            }
            let live = native_owned_bytes
                + payload.refresh(index, layer)
                + vec_bytes(&joined_layers)
                + vec_bytes(&demands)
                + vec_bytes(&original_bridge_roles)
                + anchors.retained_bytes()
                + vec_bytes(&admission.bridges)
                + vec_bytes(&admission.unanchored);
            reserve_owned(&mut original_bridge_roles, admission.bridges.len(), live)?;
            for path in admission.bridges {
                original_bridge_roles.push((index, path, layer.paths[path].kind));
                layer.paths[path].kind = DepositionKind::Bridge;
            }
        }
        drop(split);
        drop(payload);
        if demands.is_empty() {
            return Ok(SupportAdmission::Complete(None));
        }
        anchors.set_budget(0);
        let SupportPolicy::Breakaway {
            density,
            xy_clearance_mm,
            z_gap_mm,
            interface_thickness_mm,
        } = recipe.supports
        else {
            unreachable!("disabled support demand refused above")
        };
        // The gap is measured against the sliced object layers, which ADR012
        // places within the recorded tolerance of the BREP; widening it by the
        // whole enclosure margin would drop a full layer below every contact.
        let selection_gap_mm = z_gap_mm;
        // Native all-Z void branches are projected only after actual strokes prove
        // demand. Scheduled hole samples cannot certify a protected projection.
        owned_bytes = native_owned_bytes
            + layer_payload_bytes(layers.iter())
            + vec_bytes(&joined_layers)
            + vec_bytes(&demands)
            + vec_bytes(&original_bridge_roles)
            + anchors.retained_bytes();
        super::remaining_geometry_units(
            retained,
            owned_bytes.saturating_add(
                demands
                    .len()
                    .saturating_mul(std::mem::size_of::<[[f64; 2]; 2]>()),
            ),
        )?;
        let mut demand_bounds = Vec::with_capacity(demands.len());
        owned_bytes = owned_bytes.saturating_add(vec_bytes(&demand_bounds));
        // Enclose the half-bead placement search with the existing offset
        // construction allowance before querying any nearby protected void.
        // Query every forbidden mask that can meet the proposed full bead and
        // its clearance, including tolerant source enclosure outside raw bounds.
        let search_bound = (width / 2.0
            + xy_clearance_mm
            + source_z_allowance_mm
            + super::OFFSET_TOLERANCE_MM * 0.5)
            .next_up();
        for demand in &demands {
            check_cancel(cancel)?;
            let candidate = &sections[span_at(sections, demand.target as u32)].material;
            let bounds = candidate.bounding_rect().ok_or_else(|| {
                error(
                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                    "actual support demand has no material footprint bounds",
                )
            })?;
            demand_bounds.push([
                [
                    (bounds.min().x - search_bound).next_down(),
                    (bounds.max().x + search_bound).next_up(),
                ],
                [
                    (bounds.min().y - search_bound).next_down(),
                    (bounds.max().y + search_bound).next_up(),
                ],
            ]);
        }
        let native_points_before = native_points;
        let native_bytes_before = native_owned_bytes;
        timing.mark("demand_layers");
        let projected = project_voids(
            &demand_bounds,
            retained,
            owned_bytes,
            &mut native_points,
            &mut native_owned_bytes,
            &mut work,
            native_work,
        )?;
        retained = retained
            .checked_sub(native_points_before)
            .and_then(|n| n.checked_add(native_points))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native void proof-unit handoff accounting mismatch",
                )
            })?;
        owned_bytes = owned_bytes
            .checked_sub(native_bytes_before)
            .and_then(|n| n.checked_add(native_owned_bytes))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native void owner handoff accounting mismatch",
                )
            })?;
        super::remaining_geometry_units(retained, owned_bytes)?;
        let ProtectedVoidSet {
            regions: protected,
            mut scopes,
        } = projected;
        let protected = OwnedMaterial::new(protected);
        if protected.0.len() != scopes.len() {
            return Err(error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "native protected void scopes do not match their outer masks",
            ));
        }
        region::check_size(&protected)?;
        let layer_metadata_bytes = layers.len().saturating_mul(
            2 * std::mem::size_of::<Rc<OwnedMaterial>>()
                + 2 * std::mem::size_of::<usize>()
                + std::mem::size_of::<Option<usize>>(),
        );
        super::remaining_geometry_units(
            retained,
            owned_bytes
                .saturating_add(vec_bytes(&scopes))
                .saturating_add(protected.bytes)
                .saturating_add(layer_metadata_bytes)
                .saturating_add(
                    std::mem::size_of::<OwnedMaterial>() + 2 * std::mem::size_of::<usize>(),
                ),
        )?;
        let empty = OwnedMaterial::new(MultiPolygon(Vec::new())).into_shared();
        let support_regions = vec![Rc::clone(&empty); layers.len()];
        let interfaces = vec![empty; layers.len()];
        let mut object_start = vec![0; layers.len()];
        let mut support_wall_paths = vec![0; layers.len()];
        trace.report("analysis_complete");
        timing.mark("demand_and_void_analysis");
        let mut contact_tops = vec![None; layers.len()];
        let mut roots = Vec::new();
        let mut root_landings = Vec::new();
        let mut landing_bytes = 0usize;
        for object_layer in (0..layers.len().saturating_sub(1)).rev() {
            if sections[span_at(sections, object_layer as u32)].end as usize != object_layer + 1 {
                continue;
            }
            let below = &sections[span_at(sections, object_layer as u32)].material;
            let above = &sections[span_at(sections, object_layer as u32 + 1)].material;
            if Arc::ptr_eq(below, above) {
                continue;
            }
            charge_boolean(below, above, &mut work, foreign_work, cancel)?;
            let landing = landing_output(below.difference(above.as_ref()))?;
            if landing.0.is_empty() {
                continue;
            }
            let mut object_top = f64::NEG_INFINITY;
            for path in layers[object_layer]
                .paths
                .iter()
                .filter(|path| path.kind.is_model())
            {
                super::native::charge_support_clearance(&mut work, cancel)?;
                object_top = object_top.max(
                    program_coordinates::project_xyz6_bead_height_mm(
                        path.physical_z_mm[1],
                        path.height_mm(),
                    )?[1],
                );
            }
            if !object_top.is_finite() {
                continue;
            }
            for (first_support_layer, layer) in layers.iter().enumerate().skip(object_layer + 1) {
                super::native::charge_support_clearance(&mut work, cancel)?;
                let floor =
                    program_coordinates::project_xyz6_bead_height_mm(layer.z_mm, layer.height_mm)?
                        [0];
                if program_coordinates::enclosed_vertical_gap_mm(floor, object_top)? >= z_gap_mm {
                    let live_bytes = owned_bytes
                        .saturating_add(vec_bytes(&roots))
                        .saturating_add(vec_bytes(&root_landings))
                        .saturating_add(landing_bytes)
                        .saturating_add(landing.bytes);
                    reserve_owned(&mut roots, 1, live_bytes)?;
                    reserve_owned(
                        &mut root_landings,
                        1,
                        live_bytes.saturating_add(vec_bytes(&roots)),
                    )?;
                    retained = retained.checked_add(landing.units).ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "part landing geometry accounting overflow",
                        )
                    })?;
                    landing_bytes = landing_bytes.saturating_add(landing.bytes);
                    roots.push(Root::Part {
                        object_layer,
                        first_support_layer,
                        landing: root_landings.len(),
                    });
                    root_landings.push(landing);
                    break;
                }
            }
        }
        let mut support_metadata_bytes = vec_bytes(&joined_layers)
            + vec_bytes(&demands)
            + vec_bytes(&original_bridge_roles)
            + vec_bytes(&demand_bounds)
            + vec_bytes(&scopes)
            + vec_bytes(&support_regions)
            + vec_bytes(&interfaces)
            + vec_bytes(&object_start)
            + vec_bytes(&support_wall_paths)
            + vec_bytes(&contact_tops)
            + vec_bytes(&roots)
            + vec_bytes(&root_landings)
            + landing_bytes;
        owned_bytes = native_owned_bytes
            + layer_payload_bytes(layers.iter())
            + support_metadata_bytes
            + anchors.retained_bytes();
        remaining_geometry_budget(
            retained,
            owned_bytes,
            &support_regions,
            &interfaces,
            &[&protected],
        )?;
        // Borrow each final-D owner once. Its nominal bead prism remains an outer
        // exclusion after any later re-hatching; it never supplies an anchor.
        let available = remaining_geometry_budget(
            retained,
            owned_bytes,
            &support_regions,
            &interfaces,
            &[&protected],
        )?;
        if sections.len() > available {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "nominal object scope retention exceeded",
            ));
        }
        super::remaining_geometry_units(
            retained,
            owned_bytes
                .saturating_add(geometry_receipt(&support_regions, &interfaces, &[&protected]).1)
                .saturating_add(
                    sections
                        .len()
                        .saturating_mul(std::mem::size_of::<NominalScope>()),
                ),
        )?;
        let mut nominal: Vec<NominalScope> = Vec::new();
        nominal.try_reserve_exact(sections.len()).map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "nominal object scope allocation refused",
            )
        })?;
        owned_bytes = owned_bytes.saturating_add(vec_bytes(&nominal));
        for (index, section) in sections.iter().enumerate() {
            check_cancel(cancel)?;
            if section.material.0.is_empty() {
                continue;
            }
            let mut floor: Option<f64> = None;
            let mut ceiling = f64::NEG_INFINITY;
            let mut beads: Option<[[f64; 2]; 2]> = None;
            let objects = layers
                .get(section.start as usize..(section.end as usize).min(layers.len()))
                .unwrap_or(&[]);
            for object in objects {
                super::native::charge_support_clearance(&mut work, cancel)?;
                if object.paths.is_empty() {
                    continue;
                }
                for path in &object.paths {
                    let lower = program_coordinates::project_xyz6_bead_height_mm(
                        path.physical_z_mm[1],
                        path.physical_z_mm[1] - path.physical_z_mm[0],
                    )?[0];
                    floor = Some(floor.map_or(lower, |value| value.min(lower)));
                    ceiling = ceiling.max(
                        program_coordinates::project_xyz6_bead_height_mm(
                            path.physical_z_mm[1],
                            path.height_mm(),
                        )?[1],
                    );
                    // Outward XY bounds of the actual nominal bead footprint,
                    // which can extend outside D; screens only.
                    let Some(bounds) = bead_bounds(path, &mut work, cancel)? else {
                        continue;
                    };
                    beads = Some(union_frame(beads, bounds));
                }
            }
            if let Some(floor) = floor {
                nominal.push(NominalScope {
                    index,
                    floor,
                    ceiling,
                    beads,
                });
            }
        }
        super::boolean::charge(
            &mut work,
            nominal.len().checked_mul(nominal.len()).ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "nominal object scope sorting work overflow",
                )
            })?,
        )?;
        nominal.sort_unstable_by(|a, b| a.floor.total_cmp(&b.floor).then(a.index.cmp(&b.index)));
        check_cancel(cancel)?;
        let base_retained = retained;
        let retained = retained
            .checked_add(nominal.capacity())
            .filter(|n| *n <= crate::working_set::units())
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "nominal object scope retention exceeded",
                )
            })?;
        remaining_geometry_budget(
            retained,
            owned_bytes,
            &support_regions,
            &interfaces,
            &[&protected],
        )?;
        let frame = projection::Frame::new(
            input,
            xy_clearance_mm,
            owned_bytes
                .saturating_add(geometry_receipt(&support_regions, &interfaces, &[&protected]).1),
            cancel,
        )?;
        let output = sweep::Sweep {
            layers,
            sections,
            schedule,
            recipe,
            frame: &frame,
            demands: &demands,
            roots: &roots,
            landings: &root_landings,
            protected: &protected,
            nominal: &nominal,
            gap: selection_gap_mm,
            requested: xy_clearance_mm,
            interface_thickness: interface_thickness_mm,
            retained,
            owned_bytes,
            foreign_work,
        }
        .run(&scopes, &mut work, cancel)?;
        let grid_frame = output.frame;
        let grids = output.grids;
        for (target, top) in output.contact_tops.into_iter().enumerate() {
            if top.is_some() {
                contact_tops[target] = top;
            }
        }
        let tops = output.tops;
        // A layer holding relieved roof-access support takes no exact
        // clearance field: that support enters its void.
        let mut relieved_layers = Vec::new();
        reserve_owned(&mut relieved_layers, layers.len(), owned_bytes)?;
        relieved_layers.resize(layers.len(), false);
        for (ordinal, layer, column) in output.access {
            relieved_layers[layer] = true;
            scopes[ordinal].record_access(layer, column);
        }
        let mut void_heights = Vec::new();
        reserve_owned(&mut void_heights, scopes.len(), owned_bytes)?;
        void_heights.extend(
            protected
                .0
                .iter()
                .zip(&scopes)
                .map(|(mask, scope)| (mask, scope.height)),
        );
        // Final clearance asserts emitted support against these kept cells;
        // they stay admitted until it consumes them.
        let mut kept = output.kept;
        support_metadata_bytes += kept.bytes();
        // The sweep's proposals do not grant final landing authority.
        let part_contacts = output.part_contacts;
        support_metadata_bytes += part_contacts
            .iter()
            .flatten()
            .map(sweep::PartContact::bytes)
            .fold(vec_bytes(&part_contacts), usize::saturating_add);
        trace.report("corridors_complete");
        timing.mark("roots_and_corridors");
        drop(nominal);
        let mut retained = base_retained;
        // A layer's walls and rows depend only on its own kept cells and its
        // preferred axis, so they are staged in parallel and admitted
        // together; anchoring to lower layers stays in layer order below.
        let mut prebuilt: Vec<Option<(Vec<DepositionPath>, raster::WallReceipt)>> = {
            use rayon::prelude::*;
            let grid_bytes = grids
                .iter()
                .flatten()
                .map(sweep::LayerGrid::bytes)
                .fold(vec_bytes(&grids), usize::saturating_add);
            let geometry = geometry_receipt(&support_regions, &interfaces, &[&protected]);
            let base_retained = retained + geometry.0;
            let base_owned =
                (native_owned_bytes + layer_payload_bytes(layers.iter()) + support_metadata_bytes)
                    .saturating_add(geometry.1)
                    .saturating_add(grid_bytes);
            let hatches: Vec<_> = layers
                .iter()
                .enumerate()
                .map(|(i, layer)| {
                    let axis = region::HatchAxis::for_policy(
                        RectilinearAxisPolicy::AlternatingXY,
                        super::hatch_key(schedule, i),
                    );
                    (layer.z_mm, layer.lower_z_mm, axis)
                })
                .collect();
            let admit = |extra: usize| {
                super::remaining_geometry_units(base_retained, base_owned.saturating_add(extra))
                    .map(|_| ())
            };
            let exclusions = sweep::Exclusions {
                retained: base_retained,
                owned_bytes: base_owned,
                layers,
                sections,
                schedule,
                kept: &kept,
                voids: &void_heights,
                tops: &tops,
                relieved: &relieved_layers,
                distance: frame.distance,
                frame_bounds: frame.bounds,
                requested: xy_clearance_mm,
                gap: selection_gap_mm,
            };
            let built: Vec<Result<Option<_>, ManufacturingError>> = match grid_frame {
                None => Vec::new(),
                Some(frame) => {
                    let raster = grid::Raster {
                        frame,
                        cancel,
                        admit: &admit,
                        work: std::sync::atomic::AtomicUsize::new(0),
                    };
                    let working_set = crate::working_set::current();
                    // Each span's occupied pieces load once, on their first
                    // reader, and drop after their last.
                    let bounds =
                        grids
                            .iter()
                            .flatten()
                            .fold(None, |bounds: Option<[[f64; 2]; 2]>, grid| {
                                let (Some(x), Some(y)) = (
                                    grid.support.extent(&frame, 0),
                                    grid.support.extent(&frame, 1),
                                ) else {
                                    return bounds;
                                };
                                let extent = [[x[0], y[0]], [x[1], y[1]]];
                                Some(bounds.map_or(extent, |b| union_frame(Some(b), extent)))
                            });
                    let spans = match bounds {
                        Some(bounds) => Some(sweep::SpanCache::new(
                            &exclusions,
                            grids
                                .iter()
                                .enumerate()
                                .filter(|(_, grid)| grid.is_some())
                                .map(|(l, _)| l),
                            bounds,
                            FIELD_SCREEN_MM,
                        )?),
                        None => None,
                    };
                    let built = grids
                        .par_iter()
                        .zip(&hatches)
                        .enumerate()
                        .map(|(l, (grid, &(z, lower_z, axis)))| {
                            let Some(grid) = grid else {
                                return Ok(None);
                            };
                            crate::working_set::scope(working_set, || {
                                let mut local = 0usize;
                                let hatch = region::HatchLayer {
                                    roof: None,
                                    width_mm: width,
                                    z,
                                    lower_z,
                                    height: z - lower_z,
                                    axis,
                                    top_axis: axis,
                                    recipe,
                                };
                                let loaded = match &spans {
                                    Some(cache) => cache.acquire(l, cancel)?,
                                    None => Vec::new(),
                                };
                                let feed = |builder: &mut field::FieldBuilder<'_, '_>| {
                                    exclusions.feed(l, builder, &loaded, cancel)
                                };
                                let staged = raster::paths(
                                    &raster,
                                    &grid.support,
                                    &grid.interface,
                                    Some(&feed),
                                    density,
                                    &hatch,
                                    0,
                                    base_retained,
                                    base_owned,
                                    &mut local,
                                    0,
                                    cancel,
                                )
                                .map(|(paths, receipt)| Some((paths, receipt, local)));
                                drop(loaded);
                                if let Some(cache) = &spans {
                                    cache.release(l);
                                }
                                staged
                            })
                        })
                        .collect();
                    drop(spans);
                    charge_owned_work(&mut work, foreign_work, raster.take_work(), cancel)?;
                    built
                }
            };
            drop(grids);
            drop(hatches);
            let mut prebuilt = Vec::new();
            reserve_owned(&mut prebuilt, layers.len(), base_owned)?;
            let (mut units, mut bytes) = (0usize, 0usize);
            for result in built {
                let staged = result?.map(|(paths, receipt, local)| {
                    units = paths.iter().fold(units, |total, path| {
                        total.saturating_add(path.points_mm.capacity())
                    });
                    bytes = bytes
                        .saturating_add(super::bonds::path_bytes(&paths))
                        .saturating_add(vec_bytes(&receipt.certified));
                    (paths, receipt, local)
                });
                if let Some((_, _, local)) = &staged {
                    charge_owned_work(&mut work, foreign_work, *local, cancel)?;
                }
                prebuilt.push(staged.map(|(paths, receipt, _)| (paths, receipt)));
            }
            prebuilt.resize_with(layers.len(), || None);
            super::remaining_geometry_units(
                base_retained.saturating_add(units),
                base_owned
                    .saturating_sub(grid_bytes)
                    .saturating_add(vec_bytes(&prebuilt))
                    .saturating_add(bytes),
            )?;
            prebuilt
        };
        let mut staged_bytes = prebuilt
            .iter()
            .flatten()
            .map(|(paths, _)| super::bonds::path_bytes(paths))
            .fold(vec_bytes(&prebuilt), usize::saturating_add);
        let mut payload = PayloadLedger::new(layers)?;
        for i in 0..layers.len() {
            if i > 0 {
                payload.refresh(i - 1, &layers[i - 1]);
            }
            owned_bytes =
                native_owned_bytes + payload.refresh(i, &layers[i]) + support_metadata_bytes;
            anchors.set_budget(0);
            let (mut paths, mut wall_receipt) = prebuilt[i].take().unwrap_or_default();
            staged_bytes = staged_bytes.saturating_sub(super::bonds::path_bytes(&paths));
            let (prior, remaining) = layers.split_at_mut(i);
            let layer = remaining.first_mut().expect("current support layer");
            let geometry = geometry_receipt(&support_regions, &interfaces, &[&protected]);
            let retained_domains = retained + geometry.0;
            let live_bytes = owned_bytes
                .saturating_add(geometry.1)
                .saturating_add(staged_bytes);
            // The bed roots the first layer. Above it, every stroke needs
            // actual anchors on the support below or, for interface, on an
            // established part contact. The preferred axis is the only one
            // tried: what it cannot anchor is omitted and reported.
            if i > 0 && !paths.is_empty() {
                let paths_bytes = super::bonds::path_bytes(&paths);
                let admit = |extra: usize| {
                    super::remaining_geometry_units(
                        retained_domains,
                        live_bytes.saturating_add(paths_bytes).saturating_add(extra),
                    )
                    .map(|_| ())
                };
                let proposal = part_contacts.get(i).and_then(Option::as_ref);
                let contact_raster =
                    grid_frame
                        .filter(|_| proposal.is_some())
                        .map(|frame| grid::Raster {
                            frame,
                            cancel,
                            admit: &admit,
                            work: std::sync::atomic::AtomicUsize::new(0),
                        });
                let contacts = contact_raster
                    .as_ref()
                    .zip(proposal)
                    .map(|(raster, proposal)| established::CellContacts {
                        raster,
                        cells: &proposal.cells,
                        donors: &[],
                    });
                // One lower inventory serves every anchor query on this layer.
                let mut lower = strokes::LowerStrokes::new(
                    prior[i - 1]
                        .paths
                        .iter()
                        .filter(|path| support_role(path.kind)),
                    std::iter::empty(),
                );
                let kept = raster::retain_anchored(
                    &mut paths,
                    wall_receipt.paths,
                    &mut lower,
                    contacts.as_ref(),
                    recipe,
                    retained_domains,
                    live_bytes,
                    &mut anchors,
                    &mut anchor_work,
                    cancel,
                )?;
                if let Some(raster) = &contact_raster {
                    charge_owned_work(&mut work, foreign_work, raster.take_work(), cancel)?;
                }
                wall_receipt.paths = kept.paths;
                wall_receipt.omitted_mm += kept.omitted_mm;
            }
            // Final clearance takes the edges the raster proved clear exactly.
            support_metadata_bytes += kept.certify(i, std::mem::take(&mut wall_receipt.certified));
            layer.region_coverage.support_omitted_wall_mm = wall_receipt.omitted_mm;
            support_wall_paths[i] = wall_receipt.paths;
            *segments = segments
                .checked_add(path_segments(&paths))
                .filter(|count| *count <= crate::working_set::units())
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "retained deposition segment limit exceeded",
                    )
                })?;
            anchors.set_budget(0);
            layer.region_coverage.support_nominal_deposition_volume_mm3 =
                raster::nominal_volume(&paths, layer.index, recipe)?;
            layer.region_coverage.support_area_mm2 =
                layer.region_coverage.support_nominal_deposition_volume_mm3
                    / (layer.z_mm - layer.lower_z_mm);
            object_start[i] = paths.len();
            let paths_live = live_bytes.saturating_add(super::bonds::path_bytes(&paths));
            reserve_owned(&mut paths, layer.paths.len(), paths_live)?;
            paths.append(&mut layer.paths);
            layer.paths = paths;
            let geometry = geometry_receipt(&support_regions, &interfaces, &[&protected]);
            owned_bytes = native_owned_bytes
                + support_metadata_bytes
                + geometry.1
                + payload.refresh(i, layer);
        }
        drop(prebuilt);
        drop(payload);
        timing.mark("support_raster");
        let mut outstanding_roles = original_bridge_roles.as_slice();
        let mut outstanding_demands = demands.as_slice();
        let mut unchanged_lower = object_start[0] == 0;
        let mut retained_regions =
            retained + geometry_receipt(&support_regions, &interfaces, &[&protected]).0;
        // Raster staging must include these live support/protected domains without
        // counting the caller's object/cache reservation twice.
        let retained_support_points = retained_regions.checked_sub(retained).ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "support retained point accounting underflow",
            )
        })?;
        let mut split = crate::timing::Split::new("support.regeneration");
        let mut payload = PayloadLedger::new(layers)?;
        for index in 1..layers.len() {
            let t = split.start();
            support_metadata_bytes += reconcile_layer_roots(
                layers,
                &mut payload,
                FinalSupportInput {
                    index,
                    roots: &roots,
                    part_contact: part_contacts.get(index).and_then(Option::as_ref),
                    contact_tops: &contact_tops,
                    z_gap: z_gap_mm,
                    frame: grid_frame,
                    wall_paths: &mut support_wall_paths,
                    recipe,
                    retained_units: retained_regions,
                    owned_bytes: native_owned_bytes
                        + support_metadata_bytes
                        + geometry_receipt(&support_regions, &interfaces, &[&protected]).1,
                },
                segments,
                &mut anchors,
                &mut anchor_work,
                cancel,
            )?;
            split.stop("reconcile_roots", t);
            object_start[index] = layers[index]
                .paths
                .iter()
                .take_while(|path| support_role(path.kind))
                .count();
            let (original_roles, later_roles) = outstanding_roles
                .split_at(outstanding_roles.partition_point(|&(layer, _, _)| layer == index));
            outstanding_roles = later_roles;
            let (original_demands, later_demands) = outstanding_demands
                .split_at(outstanding_demands.partition_point(|demand| demand.target == index));
            outstanding_demands = later_demands;
            let (prior, remaining) = layers.split_at_mut(index);
            let layer = remaining.first_mut().expect("current support layer");
            let begin = object_start[index];
            let contact_paths = contact_tops[index]
                .map(|top| prior[top].paths.as_slice())
                .unwrap_or(&[]);
            let contact = contact_paths.iter().filter(|path| support_role(path.kind));
            let mut replaced_object = joined_layers[index];
            if unchanged_lower && !replaced_object && contact.clone().next().is_none() {
                // Demand analysis already judged and tried to recover these
                // same object paths against this same lower layer without
                // contact; repeating it reproduces its result. A layer whose
                // recovery had succeeded has no demand, and one it replaced
                // is joined.
                for demand in original_demands {
                    layer.region_coverage.unsupported_overhang_mm = demand.worst_unanchored_mm;
                }
                unchanged_lower = begin == 0;
                continue;
            }
            for &(_, path, kind) in original_roles {
                layer.paths[begin + path].kind = kind;
            }
            let extra_owned_bytes = payload.refresh(index, layer)
                + support_metadata_bytes
                + geometry_receipt(&support_regions, &interfaces, &[&protected]).1;
            owned_bytes = native_owned_bytes.saturating_add(extra_owned_bytes);
            anchors.set_budget(workspace_budget(retained_regions, owned_bytes)?);
            let authored_axis = region::HatchAxis::for_policy(
                recipe.rectilinear_axis,
                super::hatch_key(schedule, index),
            );
            let previous = prior[index - 1].paths.iter();
            let t = split.start();
            if joined_layers[index] {
                // Other support demands can change the eventual previous footprints.
                // Recreate ownership pieces; never retain a stale composite Bridge.
                let without_old = *segments - path_segments(&layer.paths[begin..]);
                let mut staged_segments = without_old;
                anchors.set_budget(0);
                let extra_owned_bytes = payload.refresh(index, layer)
                    + support_metadata_bytes
                    + geometry_receipt(&support_regions, &interfaces, &[&protected]).1;
                let staged_anchor_bytes = anchors.retained_bytes();
                let course = super::hatch_key(schedule, index);
                let mut check_component =
                    |candidate: &mut RasterCandidate,
                     axis,
                     scratch: usize,
                     scratch_owned_bytes: usize| {
                        let contact_bridge =
                            axis != authored_component_axis(candidate, course, recipe);
                        let budget = component_budget(
                            retained_regions,
                            native_owned_bytes + extra_owned_bytes,
                            scratch.saturating_add(candidate_units(candidate)),
                            scratch_owned_bytes.saturating_add(candidate_owned_bytes(candidate)),
                        )?;
                        component_admission(
                            (candidate, axis, budget, contact_bridge),
                            previous.clone(),
                            contact.clone(),
                            recipe,
                            &mut anchors,
                            &mut anchor_work,
                            cancel,
                        )
                    };
                let raster_before = work;
                let candidate = raster(
                    index,
                    authored_axis,
                    &mut staged_segments,
                    retained_support_points + constructed_points,
                    extra_owned_bytes,
                    staged_anchor_bytes,
                    &mut work,
                    Some(&mut check_component),
                );
                exclude_object_raster_work(&mut foreign_work, raster_before, work)?;
                let mut candidate = candidate?;
                coalesce_bridge_rows(
                    &mut candidate,
                    BridgeRowContext {
                        axis: authored_axis,
                        demanded_reorientation: false,
                        retained_points: retained_regions,
                        owned_bytes: native_owned_bytes + extra_owned_bytes,
                        recipe,
                    },
                    previous.clone(),
                    contact.clone(),
                    &mut anchors,
                    &mut anchor_work,
                    cancel,
                )?;
                if !finalize_interface(
                    index,
                    &mut candidate,
                    &mut staged_segments,
                    &mut work,
                    owned_bytes.saturating_add(anchors.retained_bytes()),
                )? {
                    layer.paths = candidate.paths;
                    layer.region_coverage.process_interfaces = candidate.process_interfaces;
                    {
                        layer.region_coverage.skin_bridge_domains = candidate.skin_bridge_domains;
                        layer.skin_command_fits = candidate.skin_command_fits;
                    };
                    return Err(super::bonds::unresolved(layer));
                }
                *segments = without_old + path_segments(&candidate.paths);
                layer.paths.truncate(begin);
                let live_bytes = native_owned_bytes
                    + support_metadata_bytes
                    + payload.refresh(index, layer)
                    + geometry_receipt(&support_regions, &interfaces, &[&protected]).1
                    + super::bonds::path_bytes(&candidate.paths)
                    + candidate_aux_bytes(&candidate)
                    + anchors.retained_bytes();
                reserve_owned(&mut layer.paths, candidate.paths.len(), live_bytes)?;
                layer.paths.extend(candidate.paths);
                layer.region_coverage.process_interfaces = candidate.process_interfaces;
                {
                    replace_skin_command_fits(
                        &mut layer.skin_command_fits,
                        candidate.skin_command_fits,
                        &mut retained,
                        &mut constructed_points,
                    )?;
                    replace_skin_bridge_domains(
                        &mut layer.region_coverage,
                        candidate.skin_bridge_domains,
                        &mut retained,
                        &mut constructed_points,
                    )?;
                };
                retained_regions =
                    retained + geometry_receipt(&support_regions, &interfaces, &[&protected]).0;
                owned_bytes = native_owned_bytes
                    + support_metadata_bytes
                    + payload.refresh(index, layer)
                    + geometry_receipt(&support_regions, &interfaces, &[&protected]).1;
                anchors.set_budget(workspace_budget(retained_regions, owned_bytes)?);
            }
            split.stop("joined_rebuild", t);
            anchors.set_budget(workspace_budget(
                retained_regions,
                owned_bytes.saturating_add(stroke_proof_reservation(layer.paths.len() - begin)),
            )?);
            let t = split.start();
            let mut admission = strokes::analyze(
                &layer.paths[begin..],
                previous.clone(),
                contact.clone(),
                recipe,
                &mut anchors,
                &mut anchor_work,
                cancel,
            )?;
            split.stop("analyze", t);
            let t = split.start();
            if admission.failure.is_some() {
                let without_old = *segments - path_segments(&layer.paths[begin..]);
                for recover_components in [false, true] {
                    if recover_components && joined_layers[index] {
                        // This layer was just rebuilt by this same component
                        // recovery against this same lower layer and contact,
                        // and that rebuild has just failed analysis.
                        break;
                    }
                    let axis = authored_axis;
                    let mut staged_segments = without_old;
                    anchors.set_budget(0);
                    let extra_owned_bytes = payload.refresh(index, layer)
                        + support_metadata_bytes
                        + geometry_receipt(&support_regions, &interfaces, &[&protected]).1
                        + vec_bytes(&admission.bridges);
                    owned_bytes = native_owned_bytes.saturating_add(extra_owned_bytes);
                    let staged_anchor_bytes = anchors.retained_bytes();
                    let course = super::hatch_key(schedule, index);
                    let mut check_component =
                        |candidate: &mut RasterCandidate,
                         axis,
                         scratch: usize,
                         scratch_owned_bytes: usize| {
                            let contact_bridge =
                                axis != authored_component_axis(candidate, course, recipe);
                            let budget = component_budget(
                                retained_regions,
                                owned_bytes,
                                scratch.saturating_add(candidate_units(candidate)),
                                scratch_owned_bytes
                                    .saturating_add(candidate_owned_bytes(candidate)),
                            )?;
                            component_admission(
                                (candidate, axis, budget, contact_bridge),
                                previous.clone(),
                                contact.clone(),
                                recipe,
                                &mut anchors,
                                &mut anchor_work,
                                cancel,
                            )
                        };
                    let recovery: Option<&mut super::classification::ComponentAnchorCheck<'_>> =
                        if recover_components {
                            Some(&mut check_component)
                        } else {
                            None
                        };
                    let raster_before = work;
                    let t2 = split.start();
                    let candidate = raster(
                        index,
                        axis,
                        &mut staged_segments,
                        retained_support_points + constructed_points,
                        extra_owned_bytes,
                        staged_anchor_bytes,
                        &mut work,
                        recovery,
                    );
                    exclude_object_raster_work(&mut foreign_work, raster_before, work)?;
                    let mut candidate = candidate?;
                    split.stop(
                        if recover_components {
                            "raster_components"
                        } else {
                            "raster_plain"
                        },
                        t2,
                    );
                    let t2 = split.start();
                    coalesce_bridge_rows(
                        &mut candidate,
                        BridgeRowContext {
                            axis,
                            demanded_reorientation: false,
                            retained_points: retained_regions,
                            owned_bytes,
                            recipe,
                        },
                        previous.clone(),
                        contact.clone(),
                        &mut anchors,
                        &mut anchor_work,
                        cancel,
                    )?;
                    split.stop(
                        if recover_components {
                            "coalesce_components"
                        } else {
                            "coalesce_plain"
                        },
                        t2,
                    );
                    if !finalize_interface(
                        index,
                        &mut candidate,
                        &mut staged_segments,
                        &mut work,
                        owned_bytes.saturating_add(anchors.retained_bytes()),
                    )? {
                        continue;
                    }
                    anchors.set_budget(candidate_budget(
                        &candidate.paths,
                        retained_regions,
                        owned_bytes
                            .saturating_add(candidate_aux_bytes(&candidate))
                            .saturating_add(stroke_proof_reservation(candidate.paths.len())),
                    )?);
                    let t2 = split.start();
                    // Paths are judged independently against the same lower
                    // layer and contact: rebuilt unchanged paths keep their
                    // admission.
                    let other = if candidate.paths == layer.paths[begin..] {
                        admission.clone()
                    } else {
                        strokes::analyze(
                            &candidate.paths,
                            previous.clone(),
                            contact.clone(),
                            recipe,
                            &mut anchors,
                            &mut anchor_work,
                            cancel,
                        )?
                    };
                    split.stop(
                        if recover_components {
                            "reanalyze_components"
                        } else {
                            "reanalyze_plain"
                        },
                        t2,
                    );
                    if other.failure.is_none() {
                        replaced_object = true;
                        *segments = without_old + path_segments(&candidate.paths);
                        layer.paths.truncate(begin);
                        let live_bytes = native_owned_bytes
                            + support_metadata_bytes
                            + payload.refresh(index, layer)
                            + geometry_receipt(&support_regions, &interfaces, &[&protected]).1
                            + super::bonds::path_bytes(&candidate.paths)
                            + candidate_aux_bytes(&candidate)
                            + anchors.retained_bytes();
                        reserve_owned(&mut layer.paths, candidate.paths.len(), live_bytes)?;
                        layer.paths.extend(candidate.paths);
                        layer.region_coverage.process_interfaces = candidate.process_interfaces;
                        {
                            replace_skin_command_fits(
                                &mut layer.skin_command_fits,
                                candidate.skin_command_fits,
                                &mut retained,
                                &mut constructed_points,
                            )?;
                            replace_skin_bridge_domains(
                                &mut layer.region_coverage,
                                candidate.skin_bridge_domains,
                                &mut retained,
                                &mut constructed_points,
                            )?;
                        };
                        retained_regions = retained
                            + geometry_receipt(&support_regions, &interfaces, &[&protected]).0;
                        admission = other;
                        break;
                    }
                }
                owned_bytes = native_owned_bytes
                    + support_metadata_bytes
                    + payload.refresh(index, layer)
                    + geometry_receipt(&support_regions, &interfaces, &[&protected]).1;
                anchors.set_budget(workspace_budget(retained_regions, owned_bytes)?);
            }
            split.stop("recovery", t);
            layer.region_coverage.unsupported_overhang_mm = admission.worst_unanchored_mm;
            unchanged_lower = begin == 0
                && !replaced_object
                && admission
                    .bridges
                    .iter()
                    .copied()
                    .eq(original_roles.iter().map(|&(_, path, _)| path));
            for path in admission.bridges {
                layer.paths[begin + path].kind = DepositionKind::Bridge;
            }
        }
        drop(split);
        drop(payload);
        support_metadata_bytes -= vec_bytes(&original_bridge_roles);
        drop(original_bridge_roles);
        support_metadata_bytes = support_metadata_bytes.saturating_sub(
            part_contacts
                .iter()
                .flatten()
                .map(sweep::PartContact::bytes)
                .fold(vec_bytes(&part_contacts), usize::saturating_add),
        );
        drop(part_contacts);
        // Contact selection is only a proposal until actual final paths exist.
        // Check the same closed, physical-decimal bead enclosure used by the XY
        // exclusion gates; no schedule epsilon can authorize a smaller Z gap.
        timing.mark("regeneration_and_anchor_reconciliation");
        for (target, top) in contact_tops.iter().enumerate() {
            let Some(top) = *top else {
                continue;
            };
            let mut object_bottom = f64::INFINITY;
            let mut support_top = f64::NEG_INFINITY;
            for path in &layers[target].paths {
                if support_role(path.kind) {
                    continue;
                }
                for edge in path.points_mm.windows(2) {
                    super::native::charge_support_clearance(&mut work, cancel)?;
                    object_bottom = object_bottom.min(
                        program_coordinates::project_xyz6_segment_bead_height_mm(
                            edge[0][2],
                            edge[1][2],
                            path.physical_z_mm[1] - path.physical_z_mm[0],
                        )?[0],
                    );
                }
            }
            for path in &layers[top].paths {
                if !support_role(path.kind) {
                    continue;
                }
                for edge in path.points_mm.windows(2) {
                    super::native::charge_support_clearance(&mut work, cancel)?;
                    support_top = support_top.max(
                        program_coordinates::project_xyz6_segment_bead_height_mm(
                            edge[0][2],
                            edge[1][2],
                            path.physical_z_mm[1] - path.physical_z_mm[0],
                        )?[1],
                    );
                }
            }
            // No support was emitted under this layer after all: there is no
            // contact gap to prove, and its strokes are reported unsupported.
            if !support_top.is_finite() {
                continue;
            }
            if !object_bottom.is_finite()
                || program_coordinates::enclosed_vertical_gap_mm(object_bottom, support_top)?
                    < z_gap_mm
            {
                return Err(error(
                    ManufacturingErrorCode::ProgramCoordinateConditioningFailure,
                    format!(
                        "layer {target}: actual emitted contact does not prove the minimum Z gap {z_gap_mm} mm"
                    ),
                ));
            }
        }
        // Offset/overlay corridor masks only propose candidates. Certify every
        // final actual emitted support capsule against the unchanged raw all-Z C
        // and scoped positive void masks, after both raster/anchor alternatives.
        timing.mark("contact_gap");
        let index_bytes = remaining_geometry_budget(
            retained,
            native_owned_bytes
                .saturating_add(support_metadata_bytes)
                .saturating_add(layer_payload_bytes(layers.iter()))
                .saturating_add(anchors.retained_bytes()),
            &support_regions,
            &interfaces,
            &[&protected],
        )
        .unwrap_or(0)
        .saturating_mul(std::mem::size_of::<geo::Coord<f64>>())
            / 8;
        let withdrawal = super::native::certify_support_clearance(
            layers,
            &super::native::SupportClearanceGeometry {
                sections,
                schedule,
                protected: &protected,
                scopes: &scopes,
                index_bytes,
                kept: &kept,
            },
            &recipe.supports,
            native_work,
            &mut work,
            cancel,
        )?;
        timing.mark("final_actual_clearance");
        Ok(SupportAdmission::Complete(withdrawal))
    })();
    work_bank.finish(work, foreign_work, anchor_work)?;
    outcome
}

struct FinalSupportInput<'a> {
    index: usize,
    roots: &'a [Root],
    part_contact: Option<&'a sweep::PartContact>,
    contact_tops: &'a [Option<usize>],
    z_gap: f64,
    /// The support grid frame; `None` when the sweep had no demand.
    frame: Option<grid::CellFrame>,
    wall_paths: &'a mut [usize],
    recipe: &'a PlanarPrintRecipe,
    retained_units: usize,
    owned_bytes: usize,
}

fn reconcile_layer_roots(
    layers: &mut [PrintLayer],
    payload: &mut PayloadLedger,
    input: FinalSupportInput<'_>,
    segments: &mut usize,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    check_cancel(cancel)?;
    let FinalSupportInput {
        index,
        roots,
        part_contact,
        contact_tops,
        z_gap,
        frame,
        wall_paths,
        recipe,
        retained_units,
        owned_bytes,
    } = input;
    let added_bytes = 0usize;
    {
        check_cancel(cancel)?;
        let payload_bytes = payload.refresh(index, &layers[index]);
        anchors.set_budget(workspace_budget(
            retained_units,
            owned_bytes
                .saturating_add(added_bytes)
                .saturating_add(payload_bytes),
        )?);
        let (prior, remaining) = layers.split_at_mut(index);
        let layer = remaining.first_mut().expect("final support layer");
        let mut continuing = true;
        if index > 0 {
            // One lower inventory for every path on this layer.
            let mut lower = strokes::LowerStrokes::new(
                prior[index - 1]
                    .paths
                    .iter()
                    .filter(|path| support_role(path.kind)),
                std::iter::empty(),
            );
            for path in layer.paths.iter().filter(|path| support_role(path.kind)) {
                if raster::scan_with_lower(
                    std::slice::from_ref(path),
                    &mut lower,
                    None,
                    recipe,
                    anchors,
                    work,
                    cancel,
                    true,
                )?
                .is_some()
                {
                    continuing = false;
                    break;
                }
            }
        }
        if !continuing {
            let layer_bytes = super::bonds::path_bytes(&layer.paths);
            let outside = owned_bytes
                .saturating_add(added_bytes)
                .saturating_add(payload.others(index))
                .saturating_add(layer_bytes);
            let admit = |extra: usize| {
                super::remaining_geometry_units(retained_units, outside.saturating_add(extra))
                    .map(|_| ())
            };
            let contact_raster =
                frame
                    .filter(|_| part_contact.is_some())
                    .map(|frame| grid::Raster {
                        frame,
                        cancel,
                        admit: &admit,
                        work: std::sync::atomic::AtomicUsize::new(0),
                    });
            let mut donors = Vec::new();
            let contact_cells = match contact_raster.as_ref().zip(part_contact) {
                Some((raster, proposal)) => {
                    for &root_index in &proposal.roots {
                        let Root::Part {
                            object_layer,
                            first_support_layer,
                            ..
                        } = roots[root_index]
                        else {
                            continue;
                        };
                        if first_support_layer != index
                            || object_layer >= index
                            || !established::new_contact_band(prior, layer, object_layer, z_gap)?
                        {
                            continue;
                        }
                        established::local_donors(
                            prior,
                            object_layer,
                            contact_tops[object_layer],
                            recipe,
                            raster,
                            outside,
                            &mut donors,
                        )?;
                    }
                    let cells = established::inward_donors(raster, &donors)?;
                    *work = work.saturating_add(raster.take_work());
                    Some(cells)
                }
                None => None,
            };
            let contacts =
                contact_raster
                    .as_ref()
                    .zip(contact_cells.as_ref())
                    .map(|(raster, cells)| established::CellContacts {
                        raster,
                        cells,
                        donors: &donors,
                    });
            let old_segments = path_segments(&layer.paths);
            let kept = raster::retain_anchored(
                &mut layer.paths,
                wall_paths[index],
                &mut strokes::LowerStrokes::new(
                    prior[index - 1]
                        .paths
                        .iter()
                        .filter(|path| support_role(path.kind)),
                    std::iter::empty(),
                ),
                contacts.as_ref(),
                recipe,
                retained_units,
                outside
                    .saturating_sub(layer_bytes)
                    .saturating_add(vec_bytes(&donors))
                    .saturating_add(contact_cells.as_ref().map_or(0, grid::Bits::bytes)),
                anchors,
                work,
                cancel,
            )?;
            if let Some(raster) = &contact_raster {
                *work = work.saturating_add(raster.take_work());
            }
            wall_paths[index] = kept.paths;
            layer.region_coverage.support_omitted_wall_mm += kept.omitted_mm;
            *segments = segments
                .checked_sub(old_segments)
                .and_then(|count| count.checked_add(path_segments(&layer.paths)))
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "final support segment accounting overflow",
                    )
                })?;
        }
        let support_count = layer
            .paths
            .iter()
            .take_while(|path| support_role(path.kind))
            .count();
        layer.region_coverage.support_nominal_deposition_volume_mm3 =
            raster::nominal_volume(&layer.paths[..support_count], layer.index, recipe)?;
        layer.region_coverage.support_area_mm2 =
            layer.region_coverage.support_nominal_deposition_volume_mm3
                / (layer.z_mm - layer.lower_z_mm);
    }
    Ok(added_bytes)
}

fn contact_layer(
    layers: &[PrintLayer],
    target: usize,
    path: &DepositionPath,
    z_gap_mm: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<usize>, ManufacturingError> {
    let floor = program_coordinates::project_xyz6_bead_height_mm(
        path.physical_z_mm[1],
        path.physical_z_mm[1] - path.physical_z_mm[0],
    )?[0];
    for i in (0..target).rev() {
        super::native::charge_support_clearance(work, cancel)?;
        let upper = program_coordinates::project_xyz6_bead_height_mm(
            layers[i].z_mm,
            layers[i].z_mm - layers[i].lower_z_mm,
        )?[1];
        if program_coordinates::enclosed_vertical_gap_mm(floor, upper)? >= z_gap_mm {
            return Ok(Some(i));
        }
    }
    Ok(None)
}

/// Move existing buffers, never copy geometry. Final construction is complete
/// before this layer can supply lower anchors to its successor.
fn finalize_layer_interface(
    index: usize,
    layer: &mut PrintLayer,
    segments: &mut usize,
    work: &mut usize,
    outside_owned_bytes: usize,
    finalize: &mut impl FnMut(
        usize,
        &mut RasterCandidate,
        &mut usize,
        &mut usize,
        usize,
    ) -> Result<bool, ManufacturingError>,
) -> Result<bool, ManufacturingError> {
    if layer.region_coverage.process_interfaces.is_empty() {
        return Ok(true);
    }
    let mut candidate = RasterCandidate {
        skin_command_fits: std::mem::take(&mut layer.skin_command_fits),
        skin_bridge_domains: std::mem::take(&mut layer.region_coverage.skin_bridge_domains),
        paths: std::mem::take(&mut layer.paths),
        whole_rows: Vec::new(),
        process_interfaces: std::mem::take(&mut layer.region_coverage.process_interfaces),
    };
    let result = match outside_owned_bytes.checked_sub(candidate_owned_bytes(&candidate)) {
        Some(outside) => finalize(index, &mut candidate, segments, work, outside),
        None => Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "moved interface candidate exceeds its live ownership receipt",
        )),
    };
    layer.paths = candidate.paths;
    layer.region_coverage.process_interfaces = candidate.process_interfaces;
    {
        layer.region_coverage.skin_bridge_domains = candidate.skin_bridge_domains;
        layer.skin_command_fits = candidate.skin_command_fits;
    };
    result
}

fn replace_skin_bridge_domains(
    coverage: &mut PlanarRegionCoverage,
    domains: Vec<SkinBridgeDomain>,
    retained: &mut usize,
    constructed: &mut usize,
) -> Result<(), ManufacturingError> {
    let old = super::bonds::skin_bridge_points(&coverage.skin_bridge_domains);
    let new = super::bonds::skin_bridge_points(&domains);
    let revised = retained
        .checked_sub(old)
        .and_then(|n| n.checked_add(new))
        .filter(|n| *n <= crate::working_set::units())
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "retained mixed bridge domain budget exceeded",
            )
        })?;
    *constructed = constructed
        .checked_sub(old)
        .and_then(|n| n.checked_add(new))
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "mixed bridge domain accounting mismatch",
            )
        })?;
    *retained = revised;
    coverage.skin_bridge_domains = domains;
    Ok(())
}

/// A fitted short skin row absorbed into a coalesced row is no longer its
/// own command. Its fit witness stays only while the coalesced row is still a
/// mandatory-skin Bridge of the same skin, the construction the verifier binds
/// a fit to. A row whose owners disagree is an ordinary Bridge: it carries
/// that skin material and its own command length, so the witness is released
/// rather than left naming a command that no longer exists.
fn release_absorbed_fit(
    domains: &mut Vec<SkinCommandFitDomain>,
    piece: &DepositionPath,
    row: &DepositionPath,
) {
    if piece.points_mm.len() != 2 {
        return;
    }
    let Some(DepositionOwner::MandatorySkin { skin }) = piece.owner else {
        return;
    };
    if row.kind == DepositionKind::Bridge
        && row.owner == Some(DepositionOwner::MandatorySkin { skin })
    {
        return;
    }
    let ends = [piece.points_mm[0], piece.points_mm[1]];
    for domain in domains.iter_mut().filter(|domain| domain.skin == skin) {
        domain
            .fits
            .retain(|fit| fit.fitted_mm != ends && fit.fitted_mm != [ends[1], ends[0]]);
    }
    domains.retain(|domain| !domain.fits.is_empty());
}

fn replace_skin_command_fits(
    current: &mut Vec<SkinCommandFitDomain>,
    domains: Vec<SkinCommandFitDomain>,
    retained: &mut usize,
    constructed: &mut usize,
) -> Result<(), ManufacturingError> {
    let old = super::bonds::skin_command_fit_points(current);
    let new = super::bonds::skin_command_fit_points(&domains);
    let revised = retained
        .checked_sub(old)
        .and_then(|n| n.checked_add(new))
        .filter(|n| *n <= crate::working_set::units())
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "retained skin command fit budget exceeded",
            )
        })?;
    let revised_constructed = constructed
        .checked_sub(old)
        .and_then(|n| n.checked_add(new))
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "skin command fit accounting mismatch",
            )
        })?;
    *retained = revised;
    *constructed = revised_constructed;
    *current = domains;
    Ok(())
}

fn support_role(kind: DepositionKind) -> bool {
    matches!(
        kind,
        DepositionKind::Support | DepositionKind::SupportInterface
    )
}

fn path_segments(paths: &[DepositionPath]) -> usize {
    paths.iter().map(|path| path.points_mm.len() - 1).sum()
}

fn candidate_budget(
    paths: &Vec<DepositionPath>,
    retained: usize,
    owned_bytes: usize,
) -> Result<usize, ManufacturingError> {
    let units = paths.iter().fold(retained, |total, path| {
        total.saturating_add(path.points_mm.capacity())
    });
    workspace_budget(
        units,
        owned_bytes.saturating_add(super::bonds::path_bytes(paths)),
    )
}

fn candidate_aux_bytes(candidate: &RasterCandidate) -> usize {
    super::bonds::interface_owned_bytes(&candidate.process_interfaces)
        + vec_bytes(&candidate.skin_bridge_domains)
        + candidate
            .skin_bridge_domains
            .iter()
            .map(SkinBridgeDomain::allocated_bytes)
            .sum::<usize>()
        + super::bonds::skin_command_fit_bytes(&candidate.skin_command_fits)
        + super::bonds::path_bytes(&candidate.whole_rows)
}

fn candidate_owned_bytes(candidate: &RasterCandidate) -> usize {
    super::bonds::path_bytes(&candidate.paths).saturating_add(candidate_aux_bytes(candidate))
}

fn candidate_units(candidate: &RasterCandidate) -> usize {
    candidate_owned_bytes(candidate).div_ceil(std::mem::size_of::<geo::Coord<f64>>())
}

fn matches_skin_witness(path: &DepositionPath, bond: &ProcessEndpointBond) -> bool {
    let ProcessEndpointBond::Skin {
        skin_kind,
        process_kind,
        skin_segment_mm,
        ..
    } = bond
    else {
        return false;
    };
    if path.kind != *process_kind
        || path.owner != Some(DepositionOwner::MandatorySkin { skin: *skin_kind })
        || path.points_mm.len() != 2
    {
        return false;
    }
    let a = [path.points_mm[0][0], path.points_mm[0][1]];
    let b = [path.points_mm[1][0], path.points_mm[1][1]];
    [a, b] == *skin_segment_mm || [b, a] == *skin_segment_mm
}

fn coalesce_bridge_rows<'a>(
    candidate: &mut RasterCandidate,
    context: BridgeRowContext<'_>,
    previous: impl Iterator<Item = &'a DepositionPath> + Clone,
    contacts: impl Iterator<Item = &'a DepositionPath> + Clone,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let mut lower = strokes::LowerStrokes::new(previous.clone(), contacts.clone());
    coalesce_bridge_rows_with(
        candidate, context, previous, contacts, &mut lower, anchors, work, cancel,
    )
}

/// Bridge-row coalescing over one borrowed lower set shared by every row proof.
#[allow(clippy::too_many_arguments)]
fn coalesce_bridge_rows_with<'a, P, C>(
    candidate: &mut RasterCandidate,
    context: BridgeRowContext<'_>,
    previous: P,
    contacts: C,
    lower: &mut strokes::LowerStrokes<'a, P, C>,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError>
where
    P: Iterator<Item = &'a DepositionPath> + Clone,
    C: Iterator<Item = &'a DepositionPath> + Clone,
{
    let mut joined = false;
    for axis in [context.axis, context.axis.orthogonal()] {
        let fixed = usize::from(axis == region::HatchAxis::X);
        if !candidate.whole_rows.iter().any(|row| {
            row.points_mm.len() == 2 && row.points_mm[0][fixed] == row.points_mm[1][fixed]
        }) {
            continue;
        }
        joined |= coalesce_bridge_rows_on_axis(
            candidate,
            BridgeRowContext {
                axis,
                demanded_reorientation: context.demanded_reorientation,
                retained_points: context.retained_points,
                owned_bytes: context.owned_bytes,
                recipe: context.recipe,
            },
            previous.clone(),
            contacts.clone(),
            lower,
            anchors,
            work,
            cancel,
        )?;
    }
    Ok(joined)
}

#[expect(
    clippy::too_many_arguments,
    reason = "Row candidate, borrowed anchor inventory and caller-owned work stay explicit"
)]
fn coalesce_bridge_rows_on_axis<'a, P, C>(
    candidate: &mut RasterCandidate,
    context: BridgeRowContext<'_>,
    previous: P,
    contacts: C,
    lower: &mut strokes::LowerStrokes<'a, P, C>,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError>
where
    P: Iterator<Item = &'a DepositionPath> + Clone,
    C: Iterator<Item = &'a DepositionPath> + Clone,
{
    let BridgeRowContext {
        axis,
        demanded_reorientation,
        retained_points,
        owned_bytes,
        recipe,
    } = context;
    if candidate.whole_rows.is_empty() {
        return Ok(false);
    }
    let fixed = if axis == region::HatchAxis::X { 1 } else { 0 };
    let along = 1 - fixed;
    // Compare actual emitted-grid coordinates, not a widened anchor radius or
    // native tolerance. Whole rows themselves come from authoritative fill.
    let grid = program_coordinates::project_xyz6_coordinate_mm;
    let row_units = candidate
        .whole_rows
        .iter()
        .map(|path| path.points_mm.capacity())
        .sum::<usize>();
    let budget = candidate_budget(
        &candidate.paths,
        retained_points.saturating_add(row_units),
        owned_bytes
            .saturating_add(candidate_aux_bytes(candidate))
            .saturating_add(2 * std::mem::size_of::<[f64; 3]>() + 2 * stroke_proof_reservation(1)),
    )?;
    let original_len = candidate.paths.len();
    let workspace_budget = budget.checked_sub(original_len).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "bridge row ownership workspace exceeded",
        )
    })?;
    let query_budget = (workspace_budget / 2).max(anchors.retained_records());
    let pieces_budget = workspace_budget.checked_sub(query_budget).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "live bridge anchor workspace exceeded",
        )
    })?;
    anchors.set_budget(query_budget);
    let mut pieces = Vec::new();
    for (index, path) in candidate.paths.iter().enumerate() {
        check_cancel(cancel)?;
        if path.points_mm.len() != 2
            || matches!(
                path.kind,
                DepositionKind::Perimeter | DepositionKind::SeparatorWall
            )
        {
            continue;
        }
        let a = path.points_mm[0];
        let b = path.points_mm[1];
        let row = grid(a[fixed])?;
        if row != grid(b[fixed])? {
            continue;
        }
        let lo = grid(a[along])?.min(grid(b[along])?);
        let hi = grid(a[along])?.max(grid(b[along])?);
        reserve_component_record(&mut pieces, 2, 0, pieces_budget)?;
        pieces.push((row, lo, hi, index));
    }
    pieces.sort_unstable_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then(a.1.total_cmp(&b.1))
            .then(a.2.total_cmp(&b.2))
    });
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Ownership {
        Required,
        Frozen,
        Replaced,
        Removed,
    }
    let mut ownership = Vec::new();
    ownership.try_reserve_exact(original_len).map_err(|_| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "bridge row ownership allocation refused",
        )
    })?;
    ownership.resize(original_len, Ownership::Required);
    for &(_, _, _, index) in &pieces {
        for bond in candidate
            .process_interfaces
            .iter()
            .flat_map(|interface| interface.endpoint_bonds.iter().flatten())
        {
            check_cancel(cancel)?;
            *work = work.checked_add(1).ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "cumulative bridge witness ownership work counter overflow",
                )
            })?;
            if matches_skin_witness(&candidate.paths[index], bond) {
                // The actual continuous wall connection is already chosen.
                // Freeze construction, not lower-anchor/support admission.
                ownership[index] = Ownership::Frozen;
                break;
            }
        }
    }
    let construction = matches!(
        recipe.sparse_infill,
        SparseInfillPolicy::Grid | SparseInfillPolicy::Gyroid | SparseInfillPolicy::WebRoof { .. }
    );
    let retained_infill = |path: &DepositionPath| {
        construction
            && (path.kind == DepositionKind::SparseFill
                || path.kind == DepositionKind::Bridge
                    && matches!(path.owner, Some(DepositionOwner::Regional { .. })))
    };
    if construction {
        for &(_, _, _, index) in &pieces {
            if retained_infill(&candidate.paths[index]) {
                ownership[index] = Ownership::Frozen;
            }
        }
    }
    let mut joined = false;
    for mut row in candidate.whole_rows.extract_if(.., |row| {
        row.points_mm.len() == 2 && row.points_mm[0][fixed] == row.points_mm[1][fixed]
    }) {
        check_cancel(cancel)?;
        if retained_infill(&row) {
            continue;
        }
        let key = grid(row.points_mm[0][fixed])?;
        let lo = grid(row.points_mm[0][along])?.min(grid(row.points_mm[1][along])?);
        let hi = grid(row.points_mm[0][along])?.max(grid(row.points_mm[1][along])?);
        let whole_start = row.points_mm[0][along].min(row.points_mm[1][along]);
        let whole_end = row.points_mm[0][along].max(row.points_mm[1][along]);
        let begin = pieces.partition_point(|piece| piece.0 < key);
        let end = pieces.partition_point(|piece| piece.0 <= key);
        // Each piece is visited at most twice while finding disjoint groups.
        *work = work.checked_add((end - begin) * 2).ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "cumulative bridge row ownership work counter overflow",
            )
        })?;
        let mut cursor = begin;
        let mut previous_required_end = None;
        let mut previous_emitted_end = None;
        while cursor < end {
            check_cancel(cancel)?;
            let (_, start, stop, index) = pieces[cursor];
            if ownership[index] == Ownership::Frozen
                || candidate.paths[index].width != row.width
                || candidate.paths[index].physical_z_mm != row.physical_z_mm
            {
                if stop > lo && start < hi {
                    let points = &candidate.paths[index].points_mm;
                    let required_end = points[0][along].max(points[1][along]);
                    previous_required_end = Some(
                        previous_required_end
                            .map_or(required_end, |previous: f64| previous.max(required_end)),
                    );
                    previous_emitted_end =
                        Some(previous_emitted_end.map_or(stop, |previous: f64| previous.max(stop)));
                }
                cursor += 1;
                continue;
            }
            let group_begin = cursor;
            let mut first = None;
            let mut last = None;
            let mut covered: f64 = f64::NEG_INFINITY;
            let mut required_start = f64::INFINITY;
            let mut required_end = f64::NEG_INFINITY;
            let mut count = 0;
            let mut core_owner = None;
            while cursor < end {
                check_cancel(cancel)?;
                let (_, start, stop, index) = pieces[cursor];
                if (ownership[index] == Ownership::Frozen
                    || candidate.paths[index].width != row.width
                    || candidate.paths[index].physical_z_mm != row.physical_z_mm)
                    && stop > lo
                    && start < hi
                {
                    // Retain frozen rows as required barriers so a neighbor
                    // cannot consume their geometry as an unrequired extension.
                    break;
                }
                if ownership[index] != Ownership::Required
                    || start < lo
                    || stop > hi
                    || stop <= start
                {
                    cursor += 1;
                    continue;
                }
                if let Some(last) = last
                    && start > covered
                {
                    let previous_piece: &DepositionPath = &candidate.paths[last];
                    let current_piece = &candidate.paths[index];
                    let endpoint =
                        if current_piece.points_mm[0][along] <= current_piece.points_mm[1][along] {
                            current_piece.points_mm[0]
                        } else {
                            current_piece.points_mm[1]
                        };
                    // A same-row centerline gap is not missing material when
                    // the actual emitted current bead capsules already overlap
                    // positively. Tangency is not connectivity. This candidate
                    // connector still needs independent previous-anchor proof.
                    if !(AnchorCandidate {
                        endpoints_mm: [previous_piece.points_mm[0], previous_piece.points_mm[1]],
                        bead_width_mm: region::constant_width(previous_piece)?,
                    })
                    .emitted_endpoint_footprint_overlaps(
                        endpoint,
                        region::constant_width(current_piece)?,
                    )? {
                        break;
                    }
                }
                first.get_or_insert(index);
                let piece_owner = candidate.paths[index].owner;
                if count == 0 {
                    core_owner = piece_owner;
                } else if core_owner != piece_owner {
                    // Every required core owner must agree. Anchor-only end
                    // extensions do not contribute a new mandatory owner.
                    core_owner = None;
                }
                // Nested or overlapping owners cannot move the union's end
                // backwards or discard material owned by an earlier piece.
                if stop > covered {
                    covered = stop;
                    last = Some(index);
                }
                for point in &candidate.paths[index].points_mm {
                    required_start = required_start.min(point[along]);
                    required_end = required_end.max(point[along]);
                }
                count += 1;
                cursor += 1;
            }
            let Some(first) = first else {
                continue;
            };
            // Neighboring required groups bound each search, but are not
            // anchors. A group may reach the nearest real previous interval
            // inside an unrequired gap without filling that gap completely.
            let left_emitted_end = previous_emitted_end;
            let next_required_start = if cursor < end {
                let next = &candidate.paths[pieces[cursor].3].points_mm;
                Some(next[0][along].min(next[1][along]))
            } else {
                None
            };
            let extension_start = previous_required_end.unwrap_or(whole_start);
            let extension_end = next_required_start.unwrap_or(whole_end);
            previous_required_end = Some(required_end);
            previous_emitted_end = Some(grid(required_end)?);
            row.points_mm[0][along] = required_start;
            row.points_mm[1][along] = required_end;
            row.owner = core_owner;
            let contact_extension = demanded_reorientation
                && matches!(core_owner, Some(DepositionOwner::MandatorySkin { .. }));
            let required_proof =
                lower.analyze(std::slice::from_ref(&row), recipe, anchors, work, cancel)?;
            // A short one-ended overhang may already satisfy ordinary support.
            // In a staged orthogonal proposal it still needs a real second
            // anchor before demand-bound band selection can consume it as Bridge.
            let construct_ends = required_proof.failure.is_some()
                || (demanded_reorientation && required_proof.bridges.is_empty());
            let proof = if construct_ends {
                // End extension cannot repair an over-limit interior span.
                if required_proof
                    .failure
                    .as_ref()
                    .is_some_and(|failure| failure.start > 0.0 && failure.end < 1.0)
                {
                    continue;
                }
                let whole = AnchorCandidate {
                    endpoints_mm: [
                        {
                            let mut p = row.points_mm[0];
                            p[along] = extension_start;
                            p
                        },
                        {
                            let mut p = row.points_mm[1];
                            p[along] = extension_end;
                            p
                        },
                    ],
                    bead_width_mm: region::constant_width(&row)?,
                };
                let length = extension_end - extension_start;
                let mut start = ((required_start - extension_start) / length).clamp(0.0, 1.0);
                let mut end = ((required_end - extension_start) / length).clamp(0.0, 1.0);
                let mut anchored = true;
                let gaps = if contact_extension {
                    anchors.gaps(
                        whole,
                        previous
                            .clone()
                            .filter(|lower| lower.physical_z_mm[1] == row.physical_z_mm[0]),
                        contacts.clone().filter(|lower| {
                            support_role(lower.kind)
                                && lower.physical_z_mm[1] <= row.physical_z_mm[0]
                        }),
                        work,
                        cancel,
                    )?
                } else {
                    lower.gaps(whole, row.physical_z_mm[0], false, anchors, work, cancel)?
                };
                for gap in gaps {
                    if gap.start <= start && start < gap.end {
                        anchored &= gap.left;
                        start = gap.start;
                    }
                    if gap.start < end && end <= gap.end {
                        anchored &= gap.right;
                        end = gap.end;
                    }
                }
                if !anchored {
                    continue;
                }
                row.points_mm[0][along] = extension_start + start * length;
                row.points_mm[1][along] = extension_start + end * length;
                lower.analyze(std::slice::from_ref(&row), recipe, anchors, work, cancel)?
            } else {
                if count < 2 || required_proof.bridges.is_empty() {
                    continue;
                }
                required_proof
            };
            if proof.failure.is_some() || proof.bridges.is_empty() {
                continue;
            }
            let emitted_start = grid(row.points_mm[0][along])?;
            let emitted_end = grid(row.points_mm[1][along])?;
            if let Some(left) = left_emitted_end
                && emitted_start < grid(required_start)?
                && anchors::emitted_coordinate_separation_lower_mm(emitted_start, left)?
                    <= region::constant_width(&row)?
            {
                continue;
            }
            if let Some(right) = next_required_start
                && emitted_end > grid(required_end)?
                && anchors::emitted_coordinate_separation_lower_mm(grid(right)?, emitted_end)?
                    <= region::constant_width(&row)?
            {
                continue;
            }
            // Preserve an actual footprint gap between independent groups,
            // including after an earlier group acquired its own extension.
            previous_emitted_end = Some(emitted_end);
            for &(_, start, stop, index) in &pieces[group_begin..cursor] {
                if ownership[index] == Ownership::Required && start >= lo && stop <= hi {
                    ownership[index] = Ownership::Removed;
                    release_absorbed_fit(
                        &mut candidate.skin_command_fits,
                        &candidate.paths[index],
                        &row,
                    );
                }
            }
            // Reuse an existing two-point owner instead of retaining both all
            // original pieces and newly allocated group paths until compaction.
            candidate.paths[first].kind = row.kind;
            candidate.paths[first].owner = row.owner;
            candidate.paths[first]
                .points_mm
                .copy_from_slice(&row.points_mm);
            ownership[first] = Ownership::Replaced;
            joined = true;
        }
    }
    let mut index = 0;
    candidate.paths.retain(|_| {
        let keep = ownership[index] != Ownership::Removed;
        index += 1;
        keep
    });
    Ok(joined)
}

fn admission_error(
    index: usize,
    z: f64,
    paths: &[DepositionPath],
    failure: &strokes::Failure,
    recipe: &PlanarPrintRecipe,
) -> ManufacturingError {
    let path = &paths[failure.path];
    error(
        ManufacturingErrorCode::UnsupportedGeometry,
        format!(
            "layer {index} Z {z:.6}: neither orthogonal native raster has bounded deposited anchors (bridge limit {:.3} mm); role {:?}, stroke {:?} to {:?}, gap ({:.12},{:.12}) span {:.12}; enable breakaway supports or revise placement/infill",
            recipe.process.max_bridge_span_mm,
            path.kind,
            path.points_mm[failure.first],
            path.points_mm[failure.last],
            failure.start,
            failure.end,
            failure.span
        ),
    )
}

pub(crate) fn object_volumes(
    layers: &mut [PrintLayer],
    intent: &ManufacturingIntent,
) -> Result<(), ManufacturingError> {
    for layer in layers {
        layer.region_coverage.nominal_deposition_volume_mm3 = layer
            .paths
            .iter()
            .filter(|path| path.kind.is_model())
            .try_fold(0.0, |sum, path| {
                let process = intent.process_for_path(path)?;
                Ok::<_, ManufacturingError>(
                    sum + region::nominal_volume_with_process(
                        path,
                        layer.index,
                        process.flow_multiplier,
                        process.policy,
                    )?,
                )
            })?;
    }
    Ok(())
}

fn charge_boolean(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    boolean_budget(a, b, cancel)?;
    let pairs = region::point_count(a)
        .checked_mul(region::point_count(b))
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "support Boolean work overflow",
            )
        })?;
    charge_owned_work(work, foreign_work, pairs, cancel)
}

fn charge_owned_work(
    work: &mut usize,
    foreign: usize,
    amount: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    let owned = work.checked_sub(foreign).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "support work-domain charge mismatch",
        )
    })?;
    let next = work.checked_add(amount)
        .ok_or_else(|| error(
            ManufacturingErrorCode::ResourceLimit,
            format!(
                "cumulative planar work counter overflow in support; global {}, support-owned {owned}, requested {amount}",
                *work,
            ),
        ))?;
    *work = next;
    Ok(())
}

fn reserve_owned<T>(
    values: &mut Vec<T>,
    additional: usize,
    live_bytes: usize,
) -> Result<(), ManufacturingError> {
    let needed = values.len().checked_add(additional).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "support capacity overflow",
        )
    })?;
    if needed <= values.capacity() {
        return Ok(());
    }
    let capacity = needed.max(values.capacity().saturating_mul(2)).max(4);
    let old_bytes = vec_bytes(values);
    let requested_bytes = capacity
        .checked_mul(std::mem::size_of::<T>())
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "support allocation byte overflow",
            )
        })?;
    // Both old and replacement arrays may coexist during reallocation.
    super::remaining_geometry_units(0, live_bytes.saturating_add(requested_bytes))?;
    values
        .try_reserve_exact(capacity - values.len())
        .map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "support allocation refused",
            )
        })?;
    super::remaining_geometry_units(
        0,
        live_bytes
            .saturating_sub(old_bytes)
            .saturating_add(vec_bytes(values)),
    )?;
    Ok(())
}

fn vec_bytes<T>(values: &Vec<T>) -> usize {
    values.capacity().saturating_mul(std::mem::size_of::<T>())
}

fn material_units(material: &MultiPolygon<f64>) -> usize {
    material
        .0
        .iter()
        .map(|polygon| {
            polygon.exterior().0.capacity()
                + polygon
                    .interiors()
                    .iter()
                    .map(|ring| ring.0.capacity())
                    .sum::<usize>()
        })
        .sum()
}

/// Per-layer payload bytes and their running total. Accounting inside a
/// layer loop reads it instead of rescanning every layer; each read first
/// re-measures the one layer the loop may have changed. Its own entries are
/// counted in the total it reports.
struct PayloadLedger {
    per_layer: Vec<usize>,
    total: usize,
}

impl PayloadLedger {
    fn new(layers: &[PrintLayer]) -> Result<Self, ManufacturingError> {
        let mut per_layer = Vec::new();
        per_layer.try_reserve_exact(layers.len()).map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "layer payload ledger allocation refused",
            )
        })?;
        per_layer.extend(
            layers
                .iter()
                .map(|layer| layer_payload_bytes(std::iter::once(layer))),
        );
        let total = per_layer.iter().copied().fold(0, usize::saturating_add);
        Ok(Self { per_layer, total })
    }

    /// Re-measure layer `index` and return every layer's payload.
    fn refresh(&mut self, index: usize, layer: &PrintLayer) -> usize {
        let bytes = layer_payload_bytes(std::iter::once(layer));
        self.total = self.total - self.per_layer[index] + bytes;
        self.per_layer[index] = bytes;
        self.total.saturating_add(vec_bytes(&self.per_layer))
    }

    /// Every layer's payload except `index`, as last measured.
    fn others(&self, index: usize) -> usize {
        (self.total - self.per_layer[index]).saturating_add(vec_bytes(&self.per_layer))
    }
}

fn layer_payload_bytes<'a>(layers: impl Iterator<Item = &'a PrintLayer>) -> usize {
    layers
        .map(|layer| {
            super::bonds::path_bytes(&layer.paths)
                .saturating_add(super::bonds::coverage_owned_bytes(&layer.region_coverage))
                .saturating_add(super::bonds::skin_command_fit_bytes(
                    &layer.skin_command_fits,
                ))
        })
        .fold(0usize, usize::saturating_add)
}

fn geometry_receipt(
    support: &[Rc<OwnedMaterial>],
    interfaces: &[Rc<OwnedMaterial>],
    temporary: &[&OwnedMaterial],
) -> (usize, usize) {
    // Owner-local marks avoid an allocating pointer set and all-pairs scans.
    for owner in support
        .iter()
        .chain(interfaces)
        .map(Rc::as_ref)
        .chain(temporary.iter().copied())
    {
        owner.counted.set(false);
    }
    let mut units = 0usize;
    let mut bytes = 0usize;
    for owner in support
        .iter()
        .chain(interfaces)
        .map(Rc::as_ref)
        .chain(temporary.iter().copied())
    {
        if owner.counted.replace(true) {
            continue;
        }
        units = units.saturating_add(owner.units);
        bytes = bytes
            .saturating_add(owner.bytes)
            .saturating_add(owner.header_bytes);
    }
    (units, bytes)
}

fn remaining_geometry_budget(
    native: usize,
    owned_bytes: usize,
    support: &[Rc<OwnedMaterial>],
    interfaces: &[Rc<OwnedMaterial>],
    temporary: &[&OwnedMaterial],
) -> Result<usize, ManufacturingError> {
    let (units, bytes) = geometry_receipt(support, interfaces, temporary);
    super::remaining_geometry_units(
        native.saturating_add(units),
        owned_bytes.saturating_add(bytes),
    )
}

fn union_frame(frame: Option<[[f64; 2]; 2]>, other: [[f64; 2]; 2]) -> [[f64; 2]; 2] {
    match frame {
        None => other,
        Some(f) => [
            [f[0][0].min(other[0][0]), f[0][1].min(other[0][1])],
            [f[1][0].max(other[1][0]), f[1][1].max(other[1][1])],
        ],
    }
}

/// Object beads reaching at most this far outside D stay covered by the D
/// exclusion, whose clearance distance already adds the offset tolerance.
const BEAD_OUTSIDE_ALLOWANCE_MM: f64 = super::OFFSET_TOLERANCE_MM * 0.4;

/// One section's admitted object floor and the outward XY bounds of its
/// actual nominal bead footprints.
#[derive(Clone, Copy)]
struct NominalScope {
    index: usize,
    floor: f64,
    ceiling: f64,
    beads: Option<[[f64; 2]; 2]>,
}

/// Outward XY bounds of a path's nominal bead capsules; `None` for paths that
/// deposit no edge. Malformed paths are left to ordinary validation.
fn bead_bounds(
    path: &DepositionPath,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<[[f64; 2]; 2]>, ManufacturingError> {
    if path.points_mm.len() < 2 || support_role(path.kind) {
        return Ok(None);
    }
    let width = match &path.width {
        spiling_contracts::manufacturing::WidthProfile::Constant { width_mm } => *width_mm,
        spiling_contracts::manufacturing::WidthProfile::PerEdge { widths_mm } => {
            widths_mm.iter().copied().fold(0.0, f64::max)
        }
    };
    let mut bounds = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
    for point in &path.points_mm {
        super::native::charge_support_clearance(work, cancel)?;
        bounds = [
            [bounds[0][0].min(point[0]), bounds[0][1].min(point[1])],
            [bounds[1][0].max(point[0]), bounds[1][1].max(point[1])],
        ];
    }
    let half = (width * 0.5).next_up();
    let bounds = [
        bounds[0].map(|v| (v - half).next_down()),
        bounds[1].map(|v| (v + half).next_up()),
    ];
    Ok(bounds
        .iter()
        .flatten()
        .all(|v| v.is_finite())
        .then_some(bounds))
}

/// How far beyond the kept support a loaded occupied piece's
/// clearance may reach and still bound one of its walls: above any field
/// query spread plus its exact reach.
const FIELD_SCREEN_MM: f64 = 2.0;

fn void_reaches_corridor(
    scope: &spiling_geometry::ProtectedVoidHeightScope,
    corridor_lower: f64,
    corridor_upper: f64,
) -> bool {
    match scope {
        spiling_geometry::ProtectedVoidHeightScope::Bounded { lower_mm, upper_mm } => {
            *lower_mm <= corridor_upper && *upper_mm >= corridor_lower
        }
        spiling_geometry::ProtectedVoidHeightScope::OpenBelow { upper_mm } => {
            *upper_mm >= corridor_lower
        }
    }
}

fn material_bounds(material: &MultiPolygon<f64>) -> Option<[[f64; 2]; 2]> {
    material.bounding_rect().map(|bounds| {
        [
            [bounds.min().x, bounds.min().y],
            [bounds.max().x, bounds.max().y],
        ]
    })
}

fn candidate_clearance_distance_in_bounds(
    occupied: &Polygon<f64>,
    column: Option<[[f64; 2]; 2]>,
    requested: f64,
) -> Result<Option<f64>, ManufacturingError> {
    let a = occupied.bounding_rect().ok_or_else(|| {
        error(
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "native outer operand has no finite bounds",
        )
    })?;
    let Some(bounds) = column else {
        return Ok(None);
    };
    let distance = clearance_distance_in_frame(
        [[a.min().x, a.min().y], [a.max().x, a.max().y]],
        bounds,
        requested,
    )?;
    // A disjoint outward expanded box cannot affect this candidate. Avoid
    // offset/overlay work, while final raw-source capsule proof stays separate.
    if (a.max().x + distance).next_up() < bounds[0][0]
        || (a.min().x - distance).next_down() > bounds[1][0]
        || (a.max().y + distance).next_up() < bounds[0][1]
        || (a.min().y - distance).next_down() > bounds[1][1]
    {
        return Ok(None);
    }
    Ok(Some(distance))
}

fn clearance_distance_in_frame(
    a: [[f64; 2]; 2],
    b: [[f64; 2]; 2],
    requested: f64,
) -> Result<f64, ManufacturingError> {
    // Offset allowance and Boolean padding only propose a candidate work
    // frame. They do not prove raw-C containment after overlay. Final actual
    // XYZ6 support capsules have a separate shared geometry proof.
    let reserve = super::OFFSET_TOLERANCE_MM + 2.0f64.sqrt() * (super::LAYER_QUANTIZATION_MM * 0.5);
    let base = (requested + reserve).next_up();
    let min = [a[0][0].min(b[0][0]), a[0][1].min(b[0][1])];
    let max = [a[1][0].max(b[1][0]), a[1][1].max(b[1][1])];
    let padding = super::boolean::overlay_padding(
        min.map(|v| (v - base - super::OFFSET_TOLERANCE_MM).next_down()),
        max.map(|v| (v + base + super::OFFSET_TOLERANCE_MM).next_up()),
    )?;
    let guard = (2.0 * 2.0f64.sqrt() * padding).next_up();
    let distance = (base + guard).next_up();
    let actual_padding = super::boolean::overlay_padding(
        min.map(|v| (v - distance - super::OFFSET_TOLERANCE_MM).next_down()),
        max.map(|v| (v + distance + super::OFFSET_TOLERANCE_MM).next_up()),
    )?;
    if !distance.is_finite() || (2.0f64.sqrt() * actual_padding).next_up() > guard {
        return Err(error(
            ManufacturingErrorCode::NativeConditioningFailure,
            "support exclusion candidate cannot admit its offset/overlay/XYZ6 work frame",
        ));
    }
    Ok(distance)
}

fn landing_output(mut material: MultiPolygon<f64>) -> Result<OwnedMaterial, ManufacturingError> {
    use geo::Area;
    use geo::kernels::{Kernel, Orientation, RobustKernel};
    for polygon in &material.0 {
        if std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .flat_map(|ring| &ring.0)
            .any(|point| !point.x.is_finite() || !point.y.is_finite())
            || !polygon.unsigned_area().is_finite()
        {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "support landing difference returned nonfinite geometry",
            ));
        }
    }
    material.0.retain(|polygon| {
        polygon.unsigned_area() != 0.0
            || std::iter::once(polygon.exterior())
                .chain(polygon.interiors())
                .any(|ring| {
                    let Some(&first) = ring.0.first() else {
                        return false;
                    };
                    let Some(&second) = ring.0.iter().find(|&&point| point != first) else {
                        return false;
                    };
                    ring.0.iter().any(|&point| {
                        RobustKernel::orient2d(first, second, point) != Orientation::Collinear
                    })
                })
    });
    for polygon in &mut material.0 {
        if polygon.unsigned_area() == 0.0 {
            region::canonicalize_polygon(polygon);
        }
    }
    region::check_size(&material)?;
    Ok(OwnedMaterial::new(material))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{line_string, polygon};

    #[test]
    fn roof_access_checks_each_layer_against_its_own_region() {
        // A relieved bed column widens toward the bed: layer 0 may deposit a
        // road that the narrower region at layer 1 would not contain.
        let wide = OwnedMaterial::new(MultiPolygon(vec![
            polygon![(x: 0., y: 0.), (x: 30., y: 0.), (x: 30., y: 4.), (x: 0., y: 4.)],
        ]))
        .into_shared();
        let narrow = OwnedMaterial::new(MultiPolygon(vec![
            polygon![(x: 10., y: 0.), (x: 20., y: 0.), (x: 20., y: 4.), (x: 10., y: 4.)],
        ]))
        .into_shared();
        let mut scope = VoidScope::new(
            spiling_geometry::ProtectedVoidHeightScope::OpenBelow { upper_mm: 5.0 },
            Some(Arc::new(spiling_geometry::tolerant::TolerantVoidRoof {
                height_mm: [4.0, 5.0],
                facets: Vec::new(),
            })),
        );
        scope.record_access(0, Rc::clone(&wide));
        scope.record_access(1, Rc::clone(&narrow));
        scope.record_access(2, Rc::clone(&narrow));
        assert_eq!(scope.access.len(), 2, "same region shares one record");
        let cancel = AtomicBool::new(false);
        let road = [[1.0, 2.0], [29.0, 2.0]];
        assert!(scope.covers_bead(0, 0.2, road, &mut 0, &cancel).unwrap());
        assert!(!scope.covers_bead(1, 0.4, road, &mut 0, &cancel).unwrap());
        assert!(
            scope
                .covers_bead(2, 0.6, [[11.0, 2.0], [19.0, 2.0]], &mut 0, &cancel)
                .unwrap()
        );
    }

    #[test]
    fn landing_difference_discards_only_zero_area_output() {
        let zero = polygon![(x: 0., y: 0.), (x: 1., y: 1.), (x: 2., y: 2.)];
        let tiny = polygon![(x: 0., y: 0.), (x: 1e-8, y: 0.), (x: 1e-8, y: 1e-8)];
        let holed = Polygon::new(
            line_string![(x: 0., y: 0.), (x: 4., y: 0.), (x: 4., y: 4.), (x: 0., y: 4.), (x: 0., y: 0.)],
            vec![
                line_string![(x: 1., y: 1.), (x: 2., y: 1.), (x: 2., y: 2.), (x: 1., y: 2.), (x: 1., y: 1.)],
            ],
        );
        let result = landing_output(MultiPolygon(vec![zero, tiny.clone(), holed.clone()])).unwrap();
        assert_eq!(result.material.0, vec![tiny, holed]);
        assert!(
            landing_output(MultiPolygon(Vec::new()))
                .unwrap()
                .0
                .is_empty()
        );
        for nonfinite in [f64::NAN, f64::INFINITY] {
            let invalid = polygon![(x: 0., y: 0.), (x: nonfinite, y: 1.), (x: 2., y: 2.)];
            assert_eq!(
                landing_output(MultiPolygon(vec![invalid]))
                    .err()
                    .unwrap()
                    .code,
                ManufacturingErrorCode::UnsupportedGeometry
            );
        }
    }

    #[test]
    fn landing_difference_preserves_positive_triangle_hidden_by_area_cancellation() {
        use geo::Area;
        let sliver = polygon![
            (x: 103.03553390940172, y: 119.66446610950895),
            (x: 94.83403265913469, y: 127.86596730017133),
            (x: 94.83403271873934, y: 127.86596724056669),
        ];
        assert_eq!(sliver.unsigned_area(), 0.);
        let result = landing_output(MultiPolygon(vec![sliver.clone()])).unwrap();
        assert_eq!(result.0.len(), 1);
        assert!(result.0[0].unsigned_area() > 0.);
        assert_eq!(result.0[0].exterior().0.len(), sliver.exterior().0.len());
        for point in sliver.exterior().points() {
            assert!(result.0[0].exterior().points().any(|kept| kept == point));
        }
    }

    #[test]
    fn support_never_splits_a_layer_and_keeps_accessible_contacts() {
        let cancel = AtomicBool::new(false);
        for shift in [0.0, 0.37] {
            for (target, disabled) in [(5, false), (1, false), (5, true)] {
                let mut recipe = crate::planner::tests::recipe(1.0);
                recipe.supports = if disabled {
                    SupportPolicy::Disabled
                } else {
                    SupportPolicy::Breakaway {
                        density: 0.3,
                        xy_clearance_mm: 0.2,
                        z_gap_mm: 0.2,
                        interface_thickness_mm: 0.4,
                    }
                };
                let schedule: Vec<_> = (0..=target as u32)
                    .map(|index| super::super::LayerElevation {
                        lower_z: index as f64 * 0.2,
                        z: (index + 1) as f64 * 0.2,
                        sample: (index as f64 + 0.5) * 0.2,
                        height: 0.2,
                        course: index,
                    })
                    .collect();
                let mut layers: Vec<_> = schedule
                    .iter()
                    .enumerate()
                    .map(|(index, elevation)| PrintLayer {
                        index: index as u32,
                        hatch_course: elevation.course,
                        lower_z_mm: elevation.lower_z,
                        z_mm: elevation.z,
                        section_z_mm: elevation.sample,
                        height_mm: elevation.height,
                        paths: vec![DepositionPath {
                            material_domain: None,
                            kind: DepositionKind::Perimeter,
                            owner: Some(DepositionOwner::Wall),
                            width: WidthProfile::Constant {
                                width_mm: recipe.bead_width_mm,
                            },
                            physical_z_mm: [elevation.lower_z, elevation.z],
                            points_mm: vec![
                                [-0.5 + shift, shift, elevation.z],
                                [
                                    if index == target { 15.5 } else { 0.5 } + shift,
                                    shift,
                                    elevation.z,
                                ],
                            ],
                            arcs: Vec::new(),
                            exposed: false,
                            connectors: Vec::new(),
                        }],
                        region_coverage: super::super::empty_coverage(1.0),
                        skin_command_fits: Vec::new(),
                    })
                    .collect();
                let candidates: Vec<_> = layers.iter().map(|layer| layer.paths.clone()).collect();
                let sections: Vec<_> = [
                    (0, target as u32, 1.0),
                    (target as u32, target as u32 + 1, 16.0),
                ]
                .into_iter()
                .map(|(start, end, right)| SectionSpan {
                    start,
                    end,
                    material: Arc::new(MultiPolygon(vec![polygon![
                        (x: -1.0 + shift, y: -2.0 + shift), (x: right + shift, y: -2.0 + shift),
                        (x: right + shift, y: 2.0 + shift), (x: -1.0 + shift, y: 2.0 + shift),
                    ]])),
                    process_domains: Vec::new(),
                    occupied: None,
                    source_area_mm2: (right + 1.0) * 4.0,
                })
                .collect();
                let mut bank = SupportWork::default();
                let result = realize(
                    &mut layers,
                    &SupportInput {
                        build_bounds: [[0.0; 2]; 2],
                        sections: &sections,
                        schedule: &schedule,
                        native_points: 0,
                        native_owned_bytes: 0,
                        recipe: &recipe,
                        nominal_height_mm: 0.2,
                        source_z_allowance_mm: 0.0,
                    },
                    &mut (target + 1),
                    &cancel,
                    &mut spiling_geometry::GeometryWorkBudget::compile(),
                    false,
                    &mut bank,
                    |index, _, _, _, _, _, _, _| {
                        Ok(RasterCandidate {
                            paths: candidates[index].clone(),
                            whole_rows: Vec::new(),
                            process_interfaces: Vec::new(),
                            skin_command_fits: Vec::new(),
                            skin_bridge_domains: Vec::new(),
                        })
                    },
                    |_, _, _, _, _, _, _| {
                        Ok(ProtectedVoidSet {
                            regions: MultiPolygon(Vec::new()),
                            scopes: Vec::new(),
                        })
                    },
                    |_, _, _, _, _| Ok(true),
                )
                .unwrap();
                assert!(matches!(result, SupportAdmission::Complete(None)));
                let has_support = layers
                    .iter()
                    .flat_map(|layer| &layer.paths)
                    .any(|path| support_role(path.kind));
                if target == 5 && !disabled {
                    assert!(has_support, "an accessible overhang is carried by support");
                    assert_eq!(layers[target].region_coverage.unsupported_overhang_mm, 0.0);
                } else {
                    assert!(!has_support);
                    assert!(layers[target].region_coverage.unsupported_overhang_mm > 0.0);
                }
                assert!(
                    layers[target]
                        .paths
                        .iter()
                        .any(|path| path.points_mm == candidates[target][0].points_mm)
                );
            }
        }
    }

    #[test]
    fn regenerated_part_roots_prune_only_their_dependent_support_and_report_real_demand() {
        let mut recipe = crate::planner::tests::recipe(1.0);
        recipe.process.max_bridge_span_mm = 20.0;
        let cancel = AtomicBool::new(false);
        for shift in [0.0, 0.37] {
            let path = |kind, start: f64, end: f64, index: usize| DepositionPath {
                material_domain: None,
                kind,
                width: WidthProfile::Constant { width_mm: 0.6 },
                physical_z_mm: [index as f64 * 0.2, (index + 1) as f64 * 0.2],
                owner: (kind == DepositionKind::Perimeter).then_some(DepositionOwner::Wall),
                points_mm: vec![
                    [start + shift, shift, (index + 1) as f64 * 0.2],
                    [end + shift, shift, (index + 1) as f64 * 0.2],
                ],
                arcs: Vec::new(),
                exposed: false,
                connectors: Vec::new(),
            };
            let mut layers: Vec<_> = (0..5)
                .map(|index| {
                    let mut paths = if index < 4 {
                        vec![path(DepositionKind::Support, 20.0, 30.0, index)]
                    } else {
                        Vec::new()
                    };
                    if index < 2 {
                        paths.push(path(DepositionKind::Perimeter, 10.0, 15.0, index));
                    }
                    if index == 2 {
                        paths.push(path(DepositionKind::SupportInterface, 0.0, 5.0, index));
                    }
                    if index == 3 {
                        paths.push(path(DepositionKind::Support, 0.0, 5.0, index));
                    }
                    if index == 4 {
                        paths.extend([
                            path(DepositionKind::Perimeter, 0.0, 5.0, index),
                            path(DepositionKind::Perimeter, 20.0, 30.0, index),
                            path(DepositionKind::Perimeter, 4.0, 21.0, index),
                        ]);
                    }
                    PrintLayer {
                        index: index as u32,
                        hatch_course: index as u32,
                        z_mm: (index + 1) as f64 * 0.2,
                        lower_z_mm: index as f64 * 0.2,
                        height_mm: 0.2,
                        section_z_mm: (index as f64 + 0.5) * 0.2,
                        paths,
                        region_coverage: super::super::empty_coverage(1.0),
                        skin_command_fits: Vec::new(),
                    }
                })
                .collect();
            let required: Vec<_> = layers
                .iter()
                .flat_map(|layer| {
                    layer
                        .paths
                        .iter()
                        .filter(|path| path.kind.is_model())
                        .cloned()
                })
                .collect();
            let mut wall_paths = [1, 1, 2, 2, 0];
            let roots = [Root::Part {
                object_layer: 0,
                first_support_layer: 2,
                landing: 0,
            }];
            let cell_frame =
                grid::CellFrame::new([[-5.0, -5.0], [35.0, 5.0]], grid::SUPPORT_CELL_MM).unwrap();
            let frame = Some(cell_frame);
            // The sweep's landing cells: the bed-established object road.
            let admit = |_: usize| Ok(());
            let contacts = [None, None, None, None, Some(2)];
            let raster = grid::Raster {
                frame: cell_frame,
                cancel: &cancel,
                admit: &admit,
                work: std::sync::atomic::AtomicUsize::new(0),
            };
            let part_contact = sweep::PartContact {
                cells: raster
                    .inward(
                        &established::proposal_geometry(
                            &raster,
                            layers[0].paths.iter().filter(|path| path.kind.is_model()),
                            recipe.bead_width_mm * 0.5,
                            0,
                        )
                        .unwrap()
                        .0,
                    )
                    .unwrap(),
                roots: vec![0],
            };
            let mut segments = layers.iter().map(|layer| path_segments(&layer.paths)).sum();
            let mut anchors = AnchorWorkspace::default();
            anchors.set_budget(usize::MAX);
            let before = strokes::analyze(
                &layers[4].paths[2..],
                layers[3].paths.iter(),
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut 0,
                &cancel,
            )
            .unwrap();
            assert!(before.failure.is_none() && !before.bridges.is_empty());
            let mut payload = PayloadLedger::new(&layers).unwrap();
            for index in 1..layers.len() {
                reconcile_layer_roots(
                    &mut layers,
                    &mut payload,
                    FinalSupportInput {
                        index,
                        roots: &roots,
                        part_contact: (index == 2).then_some(&part_contact),
                        contact_tops: &contacts,
                        z_gap: 0.125,
                        frame,
                        wall_paths: &mut wall_paths,
                        recipe: &recipe,
                        retained_units: 0,
                        owned_bytes: 0,
                    },
                    &mut segments,
                    &mut anchors,
                    &mut 0,
                    &cancel,
                )
                .unwrap();
            }
            anchors.set_budget(usize::MAX);
            let final_admission = strokes::analyze(
                &layers[4].paths,
                layers[3].paths.iter(),
                layers[2]
                    .paths
                    .iter()
                    .filter(|path| support_role(path.kind)),
                &recipe,
                &mut anchors,
                &mut 0,
                &cancel,
            )
            .unwrap();
            assert!(final_admission.failure.is_some());
            assert!(final_admission.bridges.is_empty());
            assert!(final_admission.worst_unanchored_mm > 0.0);
            let retained: Vec<_> = layers
                .iter()
                .flat_map(|layer| {
                    layer
                        .paths
                        .iter()
                        .filter(|path| path.kind.is_model())
                        .cloned()
                })
                .collect();
            assert_eq!(required, retained);
            assert!(layers[2..4].iter().all(|layer| {
                layer
                    .paths
                    .iter()
                    .filter(|path| support_role(path.kind))
                    .all(|path| path.points_mm[0][0] >= 20.0 + shift)
            }));
            assert!(layers[2..4].iter().all(|layer| {
                layer.region_coverage.support_nominal_deposition_volume_mm3 > 0.0
                    && layer.region_coverage.support_omitted_wall_mm > 0.0
            }));
            assert_eq!(
                segments,
                layers
                    .iter()
                    .map(|layer| path_segments(&layer.paths))
                    .sum::<usize>()
            );
            let captured = layers.clone();
            let mut payload = PayloadLedger::new(&layers).unwrap();
            assert_eq!(
                reconcile_layer_roots(
                    &mut layers,
                    &mut payload,
                    FinalSupportInput {
                        index: 1,
                        roots: &roots,
                        part_contact: None,
                        contact_tops: &contacts,
                        z_gap: 0.125,
                        frame,
                        wall_paths: &mut wall_paths,
                        recipe: &recipe,
                        retained_units: 0,
                        owned_bytes: 0
                    },
                    &mut segments,
                    &mut anchors,
                    &mut 0,
                    &AtomicBool::new(true)
                )
                .unwrap_err()
                .code,
                ManufacturingErrorCode::Cancelled
            );
            assert_eq!(layers, captured);
        }
    }

    #[test]
    fn support_owned_work_refuses_global_planar_counter_overflow() {
        let material = MultiPolygon(vec![polygon![
            (x: 0.0, y: 0.0), (x: 3.0, y: 0.0),
            (x: 3.0, y: 2.0), (x: 0.0, y: 2.0),
        ]]);
        let pairs = region::point_count(&material).pow(2);
        let mut bank = SupportWork {
            planar: usize::MAX - pairs + 1,
            ..SupportWork::default()
        };
        let mut work = bank.planar;
        let foreign = bank.foreign_work().unwrap();
        let failure = charge_boolean(
            &material,
            &material,
            &mut work,
            foreign,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(failure.code, ManufacturingErrorCode::ResourceLimit);
        let before = bank.planar;
        bank.finish(work, foreign, 0).unwrap();
        assert_eq!(bank.planar, before);
        assert_eq!(bank.support_owned, 0);
    }

    #[test]
    fn retained_support_capacity_is_byte_bounded_and_shared_once() {
        let owner = || {
            OwnedMaterial::new(MultiPolygon(vec![Polygon::new(
                geo::LineString(Vec::with_capacity(60_001)),
                Vec::new(),
            )]))
            .into_shared()
        };
        let a = owner();
        let b = owner();
        let support = vec![Rc::clone(&a), Rc::clone(&a), Rc::clone(&b)];
        let interfaces = vec![Rc::clone(&b), Rc::clone(&a)];
        let (units, bytes) = geometry_receipt(&support, &interfaces, &[]);
        assert_eq!(units, a.units + b.units);
        assert_eq!(bytes, a.bytes + a.header_bytes + b.bytes + b.header_bytes);
        assert!(remaining_geometry_budget(0, 0, &support, &interfaces, &[]).is_ok());
        assert!(
            remaining_geometry_budget(0, crate::working_set::bytes(), &support, &interfaces, &[])
                .is_err()
        );
    }

    fn row(kind: DepositionKind, a: f64, b: f64, z: f64) -> DepositionPath {
        DepositionPath {
            material_domain: None,
            kind,
            width: WidthProfile::Constant { width_mm: 0.6 },
            physical_z_mm: [z - 0.4, z],
            owner: match kind {
                DepositionKind::BottomSkin => Some(DepositionOwner::MandatorySkin {
                    skin: ProcessSkinKind::BottomSkin,
                }),
                DepositionKind::TopSkin => Some(DepositionOwner::MandatorySkin {
                    skin: ProcessSkinKind::TopSkin,
                }),
                _ => None,
            },
            points_mm: vec![[a, 0.0, z], [b, 0.0, z]],
            arcs: Vec::new(),
            exposed: false,
            connectors: Vec::new(),
        }
    }

    #[test]
    fn genuine_bridge_rows_join_existing_ownership_pieces_without_extra_fill() {
        let mut candidate = RasterCandidate {
            skin_command_fits: Vec::new(),
            skin_bridge_domains: Vec::new(),
            paths: vec![
                row(DepositionKind::SolidFill, 1.0, 3.0, 0.8),
                row(DepositionKind::BottomSkin, 3.0, 7.0, 0.8),
                row(DepositionKind::SolidFill, 7.0, 9.0, 0.8),
            ],
            whole_rows: vec![row(DepositionKind::Bridge, 1.0, 9.0, 0.8)],
            process_interfaces: Vec::new(),
        };
        let previous = [
            row(DepositionKind::SolidFill, 0.0, 3.0, 0.4),
            row(DepositionKind::SolidFill, 7.0, 10.0, 0.4),
        ];
        let recipe = crate::planner::tests::recipe(1.0);
        let mut query = AnchorWorkspace::default();
        assert!(
            coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: region::HatchAxis::X,
                    demanded_reorientation: false,
                    retained_points: 0,
                    owned_bytes: 0,
                    recipe: &recipe,
                },
                previous.iter(),
                std::iter::empty(),
                &mut query,
                &mut 0,
                &AtomicBool::new(false)
            )
            .unwrap()
        );
        assert_eq!(candidate.paths.len(), 1);
        assert_eq!(candidate.paths[0].kind, DepositionKind::Bridge);
        assert_eq!(candidate.paths[0].owner, None);
        assert_eq!(
            candidate.paths[0].points_mm,
            [[1.0, 0.0, 0.8], [9.0, 0.0, 0.8]]
        );
    }

    #[test]
    fn structured_infill_keeps_roads_while_skin_rows_join() {
        for policy in [
            SparseInfillPolicy::Grid,
            SparseInfillPolicy::Gyroid,
            SparseInfillPolicy::web_roof(),
        ] {
            let mut sparse = row(DepositionKind::SparseFill, 12.0, 15.0, 0.8);
            sparse.owner = Some(DepositionOwner::Regional { region_index: 0 });
            let mut sparse_row = sparse.clone();
            sparse_row.kind = DepositionKind::Bridge;
            let mut candidate = RasterCandidate {
                skin_command_fits: Vec::new(),
                skin_bridge_domains: Vec::new(),
                paths: vec![
                    row(DepositionKind::SolidFill, 1.0, 3.0, 0.8),
                    row(DepositionKind::BottomSkin, 3.0, 7.0, 0.8),
                    row(DepositionKind::SolidFill, 7.0, 9.0, 0.8),
                    sparse.clone(),
                ],
                whole_rows: vec![row(DepositionKind::Bridge, 1.0, 9.0, 0.8), sparse_row],
                process_interfaces: Vec::new(),
            };
            let previous = [
                row(DepositionKind::SolidFill, 0.0, 3.0, 0.4),
                row(DepositionKind::SolidFill, 7.0, 16.0, 0.4),
            ];
            let mut recipe = crate::planner::tests::recipe(0.08);
            recipe.sparse_infill = policy;
            assert!(
                coalesce_bridge_rows(
                    &mut candidate,
                    BridgeRowContext {
                        axis: region::HatchAxis::X,
                        demanded_reorientation: false,
                        retained_points: 0,
                        owned_bytes: 0,
                        recipe: &recipe,
                    },
                    previous.iter(),
                    std::iter::empty(),
                    &mut AnchorWorkspace::default(),
                    &mut 0,
                    &AtomicBool::new(false),
                )
                .unwrap()
            );
            assert_eq!(candidate.paths.len(), 2);
            assert!(candidate.paths.contains(&sparse));
            let bridge = candidate
                .paths
                .iter()
                .find(|path| path.kind == DepositionKind::Bridge)
                .unwrap();
            assert_eq!(bridge.points_mm, [[1.0, 0.0, 0.8], [9.0, 0.0, 0.8]]);
        }
    }

    #[test]
    fn structured_infill_is_a_barrier_to_skin_row_joining() {
        for policy in [
            SparseInfillPolicy::Grid,
            SparseInfillPolicy::Gyroid,
            SparseInfillPolicy::web_roof(),
        ] {
            let mut sparse = row(DepositionKind::SparseFill, 3.0, 7.0, 0.8);
            sparse.owner = Some(DepositionOwner::Regional { region_index: 0 });
            let mut candidate = RasterCandidate {
                skin_command_fits: Vec::new(),
                skin_bridge_domains: Vec::new(),
                paths: vec![
                    row(DepositionKind::BottomSkin, 1.0, 3.0, 0.8),
                    sparse.clone(),
                    row(DepositionKind::BottomSkin, 7.0, 9.0, 0.8),
                ],
                whole_rows: vec![row(DepositionKind::Bridge, 1.0, 9.0, 0.8)],
                process_interfaces: Vec::new(),
            };
            let previous = [
                row(DepositionKind::SolidFill, 0.0, 1.2, 0.4),
                row(DepositionKind::SolidFill, 2.8, 7.2, 0.4),
                row(DepositionKind::SolidFill, 8.8, 10.0, 0.4),
            ];
            let mut recipe = crate::planner::tests::recipe(0.08);
            recipe.sparse_infill = policy;
            assert!(
                !coalesce_bridge_rows(
                    &mut candidate,
                    BridgeRowContext {
                        axis: region::HatchAxis::X,
                        demanded_reorientation: false,
                        retained_points: 0,
                        owned_bytes: 0,
                        recipe: &recipe,
                    },
                    previous.iter(),
                    std::iter::empty(),
                    &mut AnchorWorkspace::default(),
                    &mut 0,
                    &AtomicBool::new(false),
                )
                .unwrap()
            );
            assert!(candidate.paths.contains(&sparse));
            assert_eq!(candidate.paths.len(), 3);
            assert!(
                candidate
                    .paths
                    .iter()
                    .filter(|path| path.kind == DepositionKind::Bridge)
                    .all(|path| {
                        let start = path.points_mm[0][0].min(path.points_mm[1][0]);
                        let stop = path.points_mm[0][0].max(path.points_mm[1][0]);
                        stop <= 3.0 || start >= 7.0
                    })
            );
        }
    }

    #[test]
    fn bridge_core_origin_requires_unanimous_original_mandatory_skin() {
        use DepositionKind::{BottomSkin, Bridge, SparseFill, TopSkin};
        let previous = [
            row(DepositionKind::SolidFill, 0.0, 3.0, 0.4),
            row(DepositionKind::SolidFill, 7.0, 10.0, 0.4),
        ];
        let recipe = crate::planner::tests::recipe(1.0);
        for (kinds, expected) in [
            (
                [TopSkin, TopSkin, TopSkin],
                Some(DepositionOwner::MandatorySkin {
                    skin: ProcessSkinKind::TopSkin,
                }),
            ),
            (
                [BottomSkin, BottomSkin, BottomSkin],
                Some(DepositionOwner::MandatorySkin {
                    skin: ProcessSkinKind::BottomSkin,
                }),
            ),
            ([TopSkin, BottomSkin, TopSkin], None),
            ([SparseFill, TopSkin, TopSkin], None),
            ([TopSkin, TopSkin, SparseFill], None),
            ([Bridge, Bridge, Bridge], None),
        ] {
            for converted in [false, true] {
                let mut paths = vec![
                    row(kinds[0], 1.0, 3.0, 0.8),
                    row(kinds[1], 3.0, 7.0, 0.8),
                    row(kinds[2], 7.0, 9.0, 0.8),
                ];
                if converted {
                    for path in &mut paths {
                        path.kind = Bridge;
                    }
                }
                let mut candidate = RasterCandidate {
                    skin_command_fits: Vec::new(),
                    skin_bridge_domains: Vec::new(),
                    paths,
                    whole_rows: vec![row(Bridge, 1.0, 9.0, 0.8)],
                    process_interfaces: Vec::new(),
                };
                assert!(
                    coalesce_bridge_rows(
                        &mut candidate,
                        BridgeRowContext {
                            axis: region::HatchAxis::X,
                            demanded_reorientation: false,
                            retained_points: 0,
                            owned_bytes: 0,
                            recipe: &recipe,
                        },
                        previous.iter(),
                        std::iter::empty(),
                        &mut AnchorWorkspace::default(),
                        &mut 0,
                        &AtomicBool::new(false),
                    )
                    .unwrap()
                );
                assert_eq!(candidate.paths.len(), 1);
                assert_eq!(candidate.paths[0].kind, Bridge);
                assert_eq!(candidate.paths[0].owner, expected);
            }
        }
    }

    /// A fitted short skin row absorbed into a mixed-owner Bridge is no
    /// longer its own command: its fit witness is released, never left
    /// naming a construction the verifier cannot find. An all-skin Bridge
    /// keeps the witness the verifier binds to it.
    #[test]
    fn absorbed_skin_fit_is_released_only_from_a_mixed_owner_bridge() {
        use DepositionKind::{BottomSkin, Bridge, SolidFill};
        let previous = [
            row(SolidFill, 0.0, 3.0, 0.4),
            row(SolidFill, 7.0, 10.0, 0.4),
        ];
        let recipe = crate::planner::tests::recipe(1.0);
        for (kinds, keeps) in [
            ([SolidFill, BottomSkin, SolidFill], false),
            ([BottomSkin, BottomSkin, BottomSkin], true),
        ] {
            let paths = vec![
                row(kinds[0], 1.0, 3.0, 0.8),
                row(kinds[1], 3.0, 7.0, 0.8),
                row(kinds[2], 7.0, 9.0, 0.8),
            ];
            let fitted = [paths[1].points_mm[0], paths[1].points_mm[1]];
            let domain = SkinCommandFitDomain {
                region_index: 0,
                region: ProcessRegionCoverage {
                    functional_solid_zone: None,
                    material_domain: None,
                    area_mm2: 1.0,
                    fraction: 1.0,
                    contributions: Vec::new(),
                },
                skin: ProcessSkinKind::BottomSkin,
                regional: Vec::new(),
                mandatory: Vec::new(),
                source_owner: Vec::new(),
                fits: vec![SkinCommandFit {
                    required_mm: fitted,
                    fitted_mm: fitted,
                }],
            };
            let mut candidate = RasterCandidate {
                skin_command_fits: vec![domain],
                skin_bridge_domains: Vec::new(),
                paths,
                whole_rows: vec![row(Bridge, 1.0, 9.0, 0.8)],
                process_interfaces: Vec::new(),
            };
            assert!(
                coalesce_bridge_rows(
                    &mut candidate,
                    BridgeRowContext {
                        axis: region::HatchAxis::X,
                        demanded_reorientation: false,
                        retained_points: 0,
                        owned_bytes: 0,
                        recipe: &recipe,
                    },
                    previous.iter(),
                    std::iter::empty(),
                    &mut AnchorWorkspace::default(),
                    &mut 0,
                    &AtomicBool::new(false),
                )
                .unwrap()
            );
            assert_eq!(candidate.paths.len(), 1);
            assert_eq!(candidate.paths[0].kind, Bridge);
            assert_eq!(candidate.skin_command_fits.len(), usize::from(keeps));
        }
    }

    #[test]
    fn chosen_skin_witness_matches_only_unchanged_role_origin_and_exact_xy() {
        let bond = ProcessEndpointBond::Skin {
            skin_kind: ProcessSkinKind::TopSkin,
            process_kind: DepositionKind::Bridge,
            skin_segment_mm: [[3.0, 0.0], [7.0, 0.0]],
            wall_segment_mm: [[3.0, -1.0], [3.0, 1.0]],
        };
        let mut path = row(DepositionKind::TopSkin, 3.0, 7.0, 0.8);
        path.kind = DepositionKind::Bridge;
        assert!(matches_skin_witness(&path, &bond));
        path.points_mm.reverse();
        assert!(matches_skin_witness(&path, &bond));
        path.points_mm[0][0] += 0.000001;
        assert!(!matches_skin_witness(&path, &bond));
        path.points_mm[0][0] = 7.0;
        path.kind = DepositionKind::TopSkin;
        assert!(!matches_skin_witness(&path, &bond));
        path.kind = DepositionKind::Bridge;
        path.owner = None;
        assert!(!matches_skin_witness(&path, &bond));
        path.owner = Some(DepositionOwner::MandatorySkin {
            skin: ProcessSkinKind::BottomSkin,
        });
        assert!(!matches_skin_witness(&path, &bond));
    }

    #[test]
    fn bridge_row_candidates_cannot_fill_a_missing_sparse_piece() {
        let mut candidate = RasterCandidate {
            skin_command_fits: Vec::new(),
            skin_bridge_domains: Vec::new(),
            paths: vec![
                row(DepositionKind::SparseFill, 1.0, 3.0, 0.8),
                row(DepositionKind::SparseFill, 7.0, 9.0, 0.8),
            ],
            whole_rows: vec![row(DepositionKind::Bridge, 1.0, 9.0, 0.8)],
            process_interfaces: Vec::new(),
        };
        let previous = [
            row(DepositionKind::SolidFill, 0.0, 3.0, 0.4),
            row(DepositionKind::SolidFill, 7.0, 10.0, 0.4),
        ];
        let recipe = crate::planner::tests::recipe(0.2);
        let mut query = AnchorWorkspace::default();
        assert!(
            !coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: region::HatchAxis::X,
                    demanded_reorientation: false,
                    retained_points: 0,
                    owned_bytes: 0,
                    recipe: &recipe,
                },
                previous.iter(),
                std::iter::empty(),
                &mut query,
                &mut 0,
                &AtomicBool::new(false)
            )
            .unwrap()
        );
        assert_eq!(candidate.paths.len(), 2);
        assert!(
            candidate
                .paths
                .iter()
                .all(|path| path.kind == DepositionKind::SparseFill)
        );
    }

    #[test]
    fn overlapping_row_groups_preserve_required_material_and_unrequested_gaps() {
        let recipe = crate::planner::tests::recipe(0.2);
        let cancel = AtomicBool::new(false);
        let previous = [3.0, 7.5].map(|x| DepositionPath {
            material_domain: None,
            kind: DepositionKind::SparseFill,
            width: WidthProfile::Constant { width_mm: 0.6 },
            physical_z_mm: [0.0, 0.4],
            owner: None,
            points_mm: vec![[x, -1.0, 0.4], [x, 1.0, 0.4]],
            arcs: Vec::new(),
            exposed: false,
            connectors: Vec::new(),
        });
        for nested in [false, true] {
            let required = vec![
                row(DepositionKind::SparseFill, 1.0, 1.1, 0.8),
                row(
                    DepositionKind::TopSkin,
                    3.2,
                    if nested { 6.8 } else { 5.1 },
                    0.8,
                ),
                row(
                    DepositionKind::SparseFill,
                    if nested { 4.0 } else { 6.8 },
                    if nested { 3.4 } else { 4.9 },
                    0.8,
                ),
            ];
            let mut candidate = RasterCandidate {
                skin_command_fits: Vec::new(),
                skin_bridge_domains: Vec::new(),
                paths: required.clone(),
                whole_rows: vec![row(DepositionKind::Bridge, 0.5, 9.0, 0.8)],
                process_interfaces: Vec::new(),
            };
            let mut anchors = AnchorWorkspace::default();
            let mut work = 0;
            assert!(
                coalesce_bridge_rows(
                    &mut candidate,
                    BridgeRowContext {
                        axis: region::HatchAxis::X,
                        demanded_reorientation: false,
                        retained_points: 0,
                        owned_bytes: 0,
                        recipe: &recipe,
                    },
                    previous.iter(),
                    std::iter::empty(),
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap()
            );
            for owner in &required {
                let start = owner.points_mm[0][0].min(owner.points_mm[1][0]);
                let end = owner.points_mm[0][0].max(owner.points_mm[1][0]);
                assert!(
                    candidate.paths.iter().any(|actual| {
                        actual.points_mm[0][0].min(actual.points_mm[1][0]) <= start
                            && actual.points_mm[0][0].max(actual.points_mm[1][0]) >= end
                    }),
                    "overlap union must retain every required owner"
                );
            }
            assert!(
                candidate.paths.iter().all(|actual| {
                    actual.points_mm[0][0].max(actual.points_mm[1][0]) <= 1.1
                        || actual.points_mm[0][0].min(actual.points_mm[1][0]) >= 3.2
                }),
                "a separate required group must not fill the unrequested interval"
            );
            let repaired: Vec<_> = candidate
                .paths
                .iter()
                .filter(|path| path.points_mm.iter().all(|point| point[0] >= 3.2))
                .cloned()
                .collect();
            assert!(
                strokes::analyze(
                    &repaired,
                    previous.iter(),
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .failure
                .is_none(),
                "an unrelated unanchored group must not prevent a real bridge"
            );
            assert!(
                strokes::analyze(
                    &candidate.paths,
                    previous.iter(),
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .failure
                .is_some(),
                "repairing one group cannot suppress another group's demand"
            );
        }
    }

    #[test]
    fn separate_required_groups_extend_only_to_nearest_real_interior_anchors() {
        let recipe = crate::planner::tests::recipe(0.2);
        let cancel = AtomicBool::new(false);
        let required = vec![
            row(DepositionKind::TopSkin, 2.0, 2.4, 0.8),
            row(DepositionKind::SparseFill, 8.0, 9.0, 0.8),
        ];
        for interior_anchor in [None, Some(4.0), Some(7.7)] {
            let previous: Vec<_> = [Some(1.7), interior_anchor, Some(8.5)]
                .into_iter()
                .flatten()
                .map(|x| DepositionPath {
                    material_domain: None,
                    kind: DepositionKind::SparseFill,
                    width: WidthProfile::Constant { width_mm: 0.6 },
                    physical_z_mm: [0.0, 0.4],
                    owner: None,
                    points_mm: vec![[x, -1.0, 0.4], [x, 1.0, 0.4]],
                    arcs: Vec::new(),
                    exposed: false,
                    connectors: Vec::new(),
                })
                .collect();
            let mut candidate = RasterCandidate {
                skin_command_fits: Vec::new(),
                skin_bridge_domains: Vec::new(),
                paths: required.clone(),
                whole_rows: vec![row(DepositionKind::Bridge, 1.0, 10.0, 0.8)],
                process_interfaces: Vec::new(),
            };
            let mut anchors = AnchorWorkspace::default();
            let mut work = 0;
            assert!(
                strokes::analyze(
                    &candidate.paths,
                    previous.iter(),
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .failure
                .is_some(),
                "one endpoint anchor cannot support the original over-limit overhang"
            );
            coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: region::HatchAxis::X,
                    demanded_reorientation: false,
                    retained_points: 0,
                    owned_bytes: 0,
                    recipe: &recipe,
                },
                previous.iter(),
                std::iter::empty(),
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            let admitted = strokes::analyze(
                &candidate.paths,
                previous.iter(),
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            if interior_anchor == Some(4.0) {
                assert!(admitted.failure.is_none());
                assert!(
                    candidate.paths.iter().all(|path| {
                        path.points_mm.iter().all(|point| point[0] <= 4.0)
                            || path.points_mm.iter().all(|point| point[0] >= 8.0)
                    }),
                    "the nearest anchor must stop extension well before the unrequested gap is filled"
                );
                for owner in &required {
                    let start = owner.points_mm[0][0].min(owner.points_mm[1][0]);
                    let end = owner.points_mm[0][0].max(owner.points_mm[1][0]);
                    assert!(candidate.paths.iter().any(|actual| {
                        actual.points_mm[0][0].min(actual.points_mm[1][0]) <= start
                            && actual.points_mm[0][0].max(actual.points_mm[1][0]) >= end
                    }));
                }
            } else {
                assert!(admitted.failure.is_some());
                assert_eq!(
                    candidate.paths, required,
                    "absent interior anchors or an extension that erases the current footprint gap cannot repair demand"
                );
            }
        }
    }

    #[test]
    fn current_row_footprint_overlap_is_strict_and_never_a_previous_anchor() {
        let mut recipe = crate::planner::tests::recipe(0.2);
        let cancel = AtomicBool::new(false);
        let unit = super::super::LAYER_QUANTIZATION_MM;
        let end = 4.0;
        for (axis, width) in [
            (region::HatchAxis::X, 0.5),
            (region::HatchAxis::Y, 0.5),
            (region::HatchAxis::X, 0.6),
            (region::HatchAxis::Y, 0.6),
        ] {
            recipe.bead_width_mm = width;
            let orient = |mut path: DepositionPath| {
                path.width = WidthProfile::Constant { width_mm: width };
                if axis == region::HatchAxis::Y {
                    for point in &mut path.points_mm {
                        point.swap(0, 1);
                    }
                }
                path
            };
            let previous = [2.5, 7.5].map(|x| {
                orient(DepositionPath {
                    material_domain: None,
                    kind: DepositionKind::SparseFill,
                    width: WidthProfile::Constant { width_mm: width },
                    physical_z_mm: [0.0, 0.4],
                    owner: None,
                    points_mm: vec![[x, -1.0, 0.4], [x, 1.0, 0.4]],
                    arcs: Vec::new(),
                    exposed: false,
                    connectors: Vec::new(),
                })
            });
            for (left_end, right_start, connected) in [
                (end, end + recipe.bead_width_mm - unit, true),
                // Non-dyadic decimal tangency must not become a positive
                // overlap merely because decoded subtraction rounds below it.
                (end, end + recipe.bead_width_mm, false),
                (end, end + recipe.bead_width_mm + unit, false),
                // A nominal overlap may disappear on the actual emitted row.
                (
                    end + 0.49 * unit,
                    end + recipe.bead_width_mm + 0.01 * unit,
                    false,
                ),
                // A tiny nominal gap may straddle the rounding half-grid.
                (end + 0.49 * unit, end + 0.51 * unit, true),
            ] {
                for missing_previous in [false, true] {
                    let required = vec![
                        orient(row(DepositionKind::TopSkin, 3.2, left_end, 0.8)),
                        orient(row(DepositionKind::SparseFill, right_start, 6.8, 0.8)),
                    ];
                    let mut candidate = RasterCandidate {
                        skin_command_fits: Vec::new(),
                        skin_bridge_domains: Vec::new(),
                        paths: required.clone(),
                        whole_rows: vec![orient(row(DepositionKind::Bridge, 1.0, 9.0, 0.8))],
                        process_interfaces: Vec::new(),
                    };
                    let prior = previous.iter().filter(|_| !missing_previous);
                    let mut anchors = AnchorWorkspace::default();
                    let mut work = 0;
                    assert!(
                        strokes::analyze(
                            &candidate.paths,
                            prior.clone(),
                            std::iter::empty(),
                            &recipe,
                            &mut anchors,
                            &mut work,
                            &cancel,
                        )
                        .unwrap()
                        .failure
                        .is_some()
                    );
                    coalesce_bridge_rows(
                        &mut candidate,
                        BridgeRowContext {
                            axis,
                            demanded_reorientation: false,
                            retained_points: 0,
                            owned_bytes: 0,
                            recipe: &recipe,
                        },
                        prior.clone(),
                        std::iter::empty(),
                        &mut anchors,
                        &mut work,
                        &cancel,
                    )
                    .unwrap();
                    let admitted = strokes::analyze(
                        &candidate.paths,
                        prior,
                        std::iter::empty(),
                        &recipe,
                        &mut anchors,
                        &mut work,
                        &cancel,
                    )
                    .unwrap();
                    assert_eq!(
                        admitted.failure.is_none(),
                        connected && !missing_previous,
                        "only positive current footprint connectivity plus real previous anchors can repair the row"
                    );
                    if !connected || missing_previous {
                        assert_eq!(
                            candidate.paths, required,
                            "tangency, missing current material or absent previous anchors cannot create a connector"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn demanded_reorientation_constructs_real_bridges_from_supported_overhangs() {
        let cancel = AtomicBool::new(false);
        let fill = MultiPolygon(vec![polygon![
            (x: 1.1, y: -2.0), (x: 4.0, y: -2.0),
            (x: 4.0, y: 2.0), (x: 1.1, y: 2.0), (x: 1.1, y: -2.0),
        ]]);
        // Ordinary admission allows this short one-ended overhang. Reorienting
        // another demanded row cannot simply relabel that supported alternative.
        let required = vec![row(DepositionKind::TopSkin, 1.1, 1.355, 0.8)];
        let prior = [0.75, 3.0].map(|x| DepositionPath {
            material_domain: None,
            kind: DepositionKind::SparseFill,
            owner: None,
            width: WidthProfile::PerEdge {
                widths_mm: vec![0.6],
            },
            physical_z_mm: [0.0, 0.4],
            points_mm: vec![[x, -1.0, 0.4], [x, 1.0, 0.4]],
            arcs: Vec::new(),
            exposed: false,
            connectors: Vec::new(),
        });
        for (reorientation, missing_anchor, limit) in [
            (false, false, 6.0),
            (true, false, 6.0),
            (true, true, 6.0),
            (true, false, 1.0),
        ] {
            let mut recipe = crate::planner::tests::recipe(0.1);
            recipe.process.max_bridge_span_mm = limit;
            let layer = region::HatchLayer {
                roof: None,
                width_mm: recipe.bead_width_mm,
                z: 0.8,
                lower_z: 0.4,
                height: 0.4,
                axis: region::HatchAxis::X,
                top_axis: region::HatchAxis::X,
                recipe: &recipe,
            };
            let previous = prior
                .iter()
                .enumerate()
                .filter(|(index, _)| !missing_anchor || *index == 0)
                .map(|(_, path)| path);
            let mut candidate = RasterCandidate {
                paths: required.clone(),
                whole_rows: region::bridge_rows(&fill, &[], &required, &layer, &mut 0, &cancel)
                    .unwrap(),
                process_interfaces: Vec::new(),
                skin_bridge_domains: Vec::new(),
                skin_command_fits: Vec::new(),
            };
            let mut anchors = AnchorWorkspace::default();
            let mut work = 0;
            let proof = component_admission(
                (&mut candidate, region::HatchAxis::X, 100_000, reorientation),
                previous.clone(),
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(proof.supported);
            if reorientation && !missing_anchor && limit == 6.0 {
                assert_eq!(proof.bridges, [0]);
                let path = &candidate.paths[0];
                assert_eq!(path.kind, DepositionKind::Bridge);
                assert_eq!(path.owner, required[0].owner);
                assert_eq!(path.width, required[0].width);
                assert_eq!(path.physical_z_mm, required[0].physical_z_mm);
                assert_eq!(path.points_mm[0], required[0].points_mm[0]);
                assert!(path.points_mm[1][0] > required[0].points_mm[1][0]);
                assert!(path.points_mm[1][0] < prior[1].points_mm[0][0]);
                let actual = strokes::analyze(
                    &candidate.paths,
                    previous,
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap();
                assert!(actual.failure.is_none());
                assert_eq!(actual.bridges, [0]);
            } else {
                assert!(proof.bridges.is_empty());
                assert_eq!(candidate.paths, required);
            }
        }
    }

    #[test]
    fn top_skin_rows_extend_only_to_actual_sparse_anchors() {
        let cancel = AtomicBool::new(false);
        let recipe = crate::planner::tests::recipe(0.1);
        let fill = MultiPolygon(vec![polygon![
            (x: 1.0, y: -2.0), (x: 9.0, y: -2.0),
            (x: 9.0, y: 2.0), (x: 1.0, y: 2.0), (x: 1.0, y: -2.0),
        ]]);
        let required = vec![row(DepositionKind::TopSkin, 3.2, 6.8, 0.8)];
        let prior = [2.5, 7.5].map(|x| DepositionPath {
            material_domain: None,
            kind: DepositionKind::SparseFill,
            width: WidthProfile::Constant { width_mm: 0.6 },
            physical_z_mm: [0.0, 0.4],
            owner: None,
            points_mm: vec![[x, -1.0, 0.4], [x, 1.0, 0.4]],
            arcs: Vec::new(),
            exposed: false,
            connectors: Vec::new(),
        });
        let layer = region::HatchLayer {
            roof: None,
            width_mm: recipe.bead_width_mm,
            z: 0.8,
            lower_z: 0.4,
            height: 0.4,
            axis: region::HatchAxis::X,
            top_axis: region::HatchAxis::X,
            recipe: &recipe,
        };
        for missing_anchor in [false, true] {
            let previous = prior.iter().filter(|_| !missing_anchor);
            let mut candidate = RasterCandidate {
                skin_command_fits: Vec::new(),
                skin_bridge_domains: Vec::new(),
                whole_rows: region::bridge_rows(&fill, &[], &required, &layer, &mut 0, &cancel)
                    .unwrap(),
                paths: required.clone(),
                process_interfaces: Vec::new(),
            };
            let mut anchors = AnchorWorkspace::default();
            let mut work = 0;
            assert!(
                strokes::analyze(
                    &candidate.paths,
                    previous.clone(),
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .failure
                .is_some()
            );
            coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: region::HatchAxis::X,
                    demanded_reorientation: false,
                    retained_points: 0,
                    owned_bytes: 0,
                    recipe: &recipe,
                },
                previous.clone(),
                std::iter::empty(),
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            let admission = strokes::analyze(
                &candidate.paths,
                previous,
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            if missing_anchor {
                assert!(
                    admission.failure.is_some(),
                    "source fill cannot supply an anchor"
                );
            } else {
                assert!(admission.failure.is_none());
                assert!(
                    candidate.paths.iter().any(|actual| {
                        actual.kind == DepositionKind::Bridge
                            && actual.owner
                                == Some(DepositionOwner::MandatorySkin {
                                    skin: ProcessSkinKind::TopSkin,
                                })
                            && actual.points_mm[0][0].min(actual.points_mm[1][0]) <= 3.2
                            && actual.points_mm[0][0].max(actual.points_mm[1][0]) >= 6.8
                            && actual
                                .points_mm
                                .iter()
                                .all(|point| (1.0..=9.0).contains(&point[0]))
                    }),
                    "a genuinely anchored bridge must retain the required skin inside positive fill"
                );
            }
        }
    }

    #[test]
    fn pre_floor_skin_deposits_anchors_under_upcoming_pocket_perimeters() {
        use crate::planner::classification::{ClassifiedRegions, fixed_owned_budget};
        let cancel = AtomicBool::new(false);
        let recipe = crate::planner::tests::recipe(0.25);
        let mut section = MultiPolygon(vec![polygon![
            (x: 0.0, y: 0.0), (x: 10.0, y: 0.0),
            (x: 10.0, y: 10.0), (x: 0.0, y: 10.0), (x: 0.0, y: 0.0),
        ]]);
        let mut upper = MultiPolygon(vec![Polygon::new(
            section.0[0].exterior().clone(),
            vec![line_string![
                (x: 4.0, y: 4.0), (x: 4.0, y: 6.0),
                (x: 6.0, y: 6.0), (x: 6.0, y: 4.0), (x: 4.0, y: 4.0),
            ]],
        )]);
        let layout = |material: &MultiPolygon<f64>| {
            std::sync::Arc::new(
                crate::planner::tests::wall_plan(
                    material,
                    &region::RequiredMaterial::from_section(material),
                    &recipe,
                    region::point_count(material),
                    &cancel,
                )
                .unwrap(),
            )
        };
        let inset = (recipe.perimeter_count as f64 - 0.5) * recipe.bead_width_mm;
        let mut section_inset = region::offset(&section, -inset, &cancel).unwrap();
        let mut upper_inset = region::offset(&upper, -inset, &cancel).unwrap();
        let fixture_bytes = crate::planner::native::material_bytes(&mut section)
            + crate::planner::native::material_bytes(&mut upper)
            + crate::planner::native::material_bytes(&mut section_inset)
            + crate::planner::native::material_bytes(&mut upper_inset);
        let fixture_units = material_units(&section)
            + material_units(&upper)
            + material_units(&section_inset)
            + material_units(&upper_inset);
        let previous_layout = layout(&section);
        let previous_units = fixture_units + previous_layout.retained_points;
        let previous_bytes =
            fixture_bytes + previous_layout.retained_bytes + 2 * std::mem::size_of::<usize>();
        let mut previous_regions = ClassifiedRegions::new(
            previous_layout,
            &section_inset,
            &upper_inset,
            fixed_owned_budget(previous_units, previous_bytes),
            &recipe,
            &cancel,
        )
        .unwrap();
        let classified_bytes = previous_regions.retained_bytes();
        let previous = previous_regions
            .paths(
                0.4,
                0.0,
                region::HatchAxis::X,
                region::HatchAxis::X,
                &recipe,
                None,
                region::OrdinaryHatchPhase::Centered,
                classified_bytes,
                &[],
                &mut 0,
                |units, bytes, _| {
                    crate::planner::remaining_geometry_units(units, bytes).map(|_| ())
                },
                &cancel,
            )
            .unwrap()
            .0;
        let upcoming_layout = layout(&upper);
        let upcoming_units = fixture_units
            + upcoming_layout.retained_points
            + previous
                .iter()
                .map(|path| path.points_mm.capacity())
                .sum::<usize>();
        let upcoming_bytes = fixture_bytes
            + upcoming_layout.retained_bytes
            + 2 * std::mem::size_of::<usize>()
            + crate::planner::bonds::path_bytes(&previous);
        let mut upcoming_regions = ClassifiedRegions::new(
            upcoming_layout,
            &section_inset,
            &upper_inset,
            fixed_owned_budget(upcoming_units, upcoming_bytes),
            &recipe,
            &cancel,
        )
        .unwrap();
        let classified_bytes = upcoming_regions.retained_bytes();
        let upcoming = upcoming_regions
            .paths(
                0.8,
                0.4,
                region::HatchAxis::Y,
                region::HatchAxis::Y,
                &recipe,
                None,
                region::OrdinaryHatchPhase::Centered,
                classified_bytes,
                &[],
                &mut 0,
                |units, bytes, _| {
                    crate::planner::remaining_geometry_units(units, bytes).map(|_| ())
                },
                &cancel,
            )
            .unwrap()
            .0
            .into_iter()
            .filter(|path| path.kind == DepositionKind::Perimeter)
            .collect::<Vec<_>>();
        let admission = strokes::analyze(
            &upcoming,
            previous.iter(),
            std::iter::empty(),
            &recipe,
            &mut AnchorWorkspace::default(),
            &mut 0,
            &cancel,
        )
        .unwrap();
        assert!(
            admission.failure.is_none(),
            "future walls need real deposited anchors"
        );
        assert!(
            admission.bridges.is_empty(),
            "a curved pocket rim cannot borrow straight-bridge authority"
        );
    }

    #[test]
    fn anchor_extension_cannot_cross_a_positive_fill_hole() {
        let cancel = AtomicBool::new(false);
        let recipe = crate::planner::tests::recipe(0.1);
        let exterior = geo::line_string![
            (x: 1.0, y: -2.0), (x: 9.0, y: -2.0),
            (x: 9.0, y: 2.0), (x: 1.0, y: 2.0), (x: 1.0, y: -2.0),
        ];
        let hole = geo::line_string![
            (x: 4.2, y: -1.0), (x: 4.2, y: 1.0),
            (x: 5.8, y: 1.0), (x: 5.8, y: -1.0), (x: 4.2, y: -1.0),
        ];
        let fill = MultiPolygon(vec![Polygon::new(exterior, vec![hole])]);
        let required = vec![
            row(DepositionKind::TopSkin, 3.0, 4.0, 0.8),
            row(DepositionKind::TopSkin, 6.0, 7.0, 0.8),
        ];
        let mut candidate = RasterCandidate {
            skin_command_fits: Vec::new(),
            skin_bridge_domains: Vec::new(),
            whole_rows: region::bridge_rows(
                &fill,
                &[],
                &required,
                &region::HatchLayer {
                    roof: None,
                    width_mm: recipe.bead_width_mm,
                    z: 0.8,
                    lower_z: 0.4,
                    height: 0.4,
                    axis: region::HatchAxis::X,
                    top_axis: region::HatchAxis::X,
                    recipe: &recipe,
                },
                &mut 0,
                &cancel,
            )
            .unwrap(),
            paths: required.clone(),
            process_interfaces: Vec::new(),
        };
        assert_eq!(candidate.whole_rows.len(), 2);
        let previous = [
            row(DepositionKind::SparseFill, 1.0, 2.0, 0.4),
            row(DepositionKind::SparseFill, 8.0, 9.0, 0.4),
        ];
        let mut anchors = AnchorWorkspace::default();
        assert!(
            !coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: region::HatchAxis::X,
                    demanded_reorientation: false,
                    retained_points: 0,
                    owned_bytes: 0,
                    recipe: &recipe,
                },
                previous.iter(),
                std::iter::empty(),
                &mut anchors,
                &mut 0,
                &cancel,
            )
            .unwrap()
        );
        assert_eq!(candidate.paths, required);
        assert!(
            strokes::analyze(
                &candidate.paths,
                previous.iter(),
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut 0,
                &cancel,
            )
            .unwrap()
            .failure
            .is_some()
        );
    }

    #[test]
    fn auxiliary_bridge_bounds_ignore_unrequested_tangent_and_thin_intervals() {
        let cancel = AtomicBool::new(false);
        let recipe = crate::planner::tests::recipe(0.1);
        let layer = region::HatchLayer {
            roof: None,
            width_mm: recipe.bead_width_mm,
            z: 0.8,
            lower_z: 0.4,
            height: 0.4,
            axis: region::HatchAxis::X,
            top_axis: region::HatchAxis::X,
            recipe: &recipe,
        };
        let required = vec![DepositionPath {
            material_domain: None,
            kind: DepositionKind::TopSkin,
            width: WidthProfile::Constant {
                width_mm: recipe.bead_width_mm,
            },
            physical_z_mm: [0.4, 0.8],
            owner: Some(DepositionOwner::MandatorySkin {
                skin: ProcessSkinKind::TopSkin,
            }),
            points_mm: vec![[1.0, 2.0, 0.8], [9.0, 2.0, 0.8]],
            arcs: Vec::new(),
            exposed: false,
            connectors: Vec::new(),
        }];
        let polygon = |points: Vec<(f64, f64)>| Polygon::new(geo::LineString::from(points), vec![]);
        for tip in [2.0, 2.0 - 0.00000001] {
            let isolated = MultiPolygon(vec![
                polygon(vec![
                    (0.0, 0.0),
                    (10.0, 0.0),
                    (10.0, 4.0),
                    (0.0, 4.0),
                    (0.0, 0.0),
                ]),
                polygon(vec![(20.0, 4.0), (22.0, tip), (24.0, 4.0), (20.0, 4.0)]),
            ]);
            let connected = MultiPolygon(vec![polygon(vec![
                (0.0, 0.0),
                (10.0, 0.0),
                (10.0, 3.0),
                (20.0, 3.0),
                (22.0, tip),
                (24.0, 3.0),
                (24.0, 4.0),
                (0.0, 4.0),
                (0.0, 0.0),
            ])]);
            for fill in [&isolated, &connected] {
                let rows =
                    region::bridge_rows(fill, &[], &required, &layer, &mut 0, &cancel).unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].points_mm, [[0.0, 2.0, 0.8], [10.0, 2.0, 0.8]]);
            }
            let mut sparse = Vec::new();
            region::hatch(
                &connected,
                DepositionKind::SparseFill,
                f64::MIN_POSITIVE,
                &layer,
                &mut sparse,
                &mut 0,
                &cancel,
            )
            .unwrap();
            assert!(
                sparse
                    .iter()
                    .any(|path| { path.points_mm == [[0.0, 2.0, 0.8], [10.0, 2.0, 0.8]] })
            );
            assert!(sparse.iter().all(|path| {
                path.points_mm.iter().all(|point| {
                    (0.0..=10.0).contains(&point[0]) && point[1] == 2.0 && point[2] == 0.8
                })
            }));
        }
    }
}
