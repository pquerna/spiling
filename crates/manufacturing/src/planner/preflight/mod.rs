// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::surface;
use crate::{CompilerInput, check_cancel, error};
use spiling_contracts::geometry::{
    AabbMm, GeometryErrorCode, MAX_DEFINITIONS, MAX_OCCURRENCES, OccurrenceId, OccurrenceRecord,
};
use spiling_contracts::manufacturing::*;
use spiling_geometry::tolerant::{TolerantModel, TolerantPolicy};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

pub(super) struct Admission<'work> {
    pub bounds: Option<AabbMm>,
    pub horizontal_events: Vec<f64>,
    pub native_bounds: BTreeMap<OccurrenceId, AabbMm>,
    pub native_event_bands: Vec<[f64; 2]>,
    /// Literal native candidates delimit queries only, never physical planes.
    pub native_capture_proposal_bands: Vec<[f64; 2]>,
    pub material_domains: BTreeMap<OccurrenceId, [[f64; 2]; 2]>,
    pub surface_fields: BTreeMap<OccurrenceId, Vec<surface::PreparedFinish>>,
    pub report: PreflightReport,
    pub geometry_work: &'work mut spiling_geometry::GeometryWorkBudget,
    /// ADR012 tolerant mode: each occurrence's tessellation placed in world
    /// coordinates. None runs the exact native path.
    pub tolerant: Option<BTreeMap<OccurrenceId, std::sync::Arc<TolerantModel>>>,
}

impl Admission<'_> {
    pub(super) fn native_capture_proposal_bytes(&self) -> usize {
        self.native_capture_proposal_bands
            .capacity()
            .saturating_mul(std::mem::size_of::<[f64; 2]>())
            .saturating_add(self.tolerant.as_ref().map_or(0, |models| {
                models
                    .values()
                    .map(|model| model.retained_bytes())
                    .sum::<usize>()
            }))
    }
}

#[derive(Clone, Copy)]
enum NativeEventRole {
    Critical,
    LowerSupport,
    UpperSupport,
}

struct LowerTerminalRouting<'a> {
    analysis: &'a spiling_geometry::HeightAnalysis,
    lower_support: [f64; 2],
}

/// The selected printer's motion step on one world axis.
fn axis_resolution(input: &CompilerInput<'_>, axis: usize) -> f64 {
    let resolution = input.intent.printer.motion_resolution;
    if axis == 2 {
        resolution.z_mm
    } else {
        resolution.xy_mm
    }
}

/// A terminal touches the bed when it is within one Z step of it and inside
/// the shape budget.
fn terminal_inside_bed_envelope(
    bracket: [f64; 2],
    bed: f64,
    z_step: f64,
    shape_budget: f64,
) -> bool {
    bed.is_finite()
        && shape_budget.is_finite()
        && shape_budget > 0.0
        && bracket.iter().all(|height| height.is_finite())
        && bracket[0] <= bracket[1]
        && bracket
            .into_iter()
            .all(|height| (height - bed).abs() <= z_step && (height - bed).abs() <= shape_budget)
}

impl<'a> LowerTerminalRouting<'a> {
    fn capture(
        analysis: &'a spiling_geometry::HeightAnalysis,
        occurrence: &OccurrenceRecord,
        bounds: AabbMm,
        domain: [[f64; 2]; 2],
        bed: f64,
        policy: spiling_geometry::AnalysisPolicy,
    ) -> Option<Self> {
        let terminal = analysis.global_lower_terminal_bracket_mm()?;
        let support = analysis.global_lower_terminal_support_bracket_mm()?;
        let captured_policy = analysis.policy();
        if analysis.definition_id() != &occurrence.definition_id
            || analysis.pose() != Some(occurrence.pose)
            || analysis.height_interval_mm() != [bounds.min[2], bounds.max[2]]
            || [domain[0][0], domain[1][1]] != analysis.height_interval_mm()
            || captured_policy.maximum_height_uncertainty_mm != policy.maximum_height_uncertainty_mm
            || captured_policy.maximum_shape_error_mm != policy.maximum_shape_error_mm
            || !bed.is_finite()
            || !domain.iter().flatten().all(|height| height.is_finite())
            || domain.iter().any(|band| band[0] > band[1])
            || domain[0][0] < bed
            || support[0] < bed
            || !terminal_inside_bed_envelope(
                domain[0],
                bed,
                policy.maximum_height_uncertainty_mm,
                policy.maximum_shape_error_mm,
            )
            || !terminal_inside_bed_envelope(
                support,
                bed,
                policy.maximum_height_uncertainty_mm,
                policy.maximum_shape_error_mm,
            )
            || !terminal_inside_bed_envelope(
                terminal,
                bed,
                policy.maximum_height_uncertainty_mm,
                policy.maximum_shape_error_mm,
            )
        {
            return None;
        }
        Some(Self {
            analysis,
            lower_support: domain[0],
        })
    }

    fn requires_plane(
        &self,
        bracket: [f64; 2],
        role: NativeEventRole,
        work: &mut spiling_geometry::GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, ManufacturingError> {
        work.charge(1, cancel)
            .map_err(|e| super::native_error(e, "native terminal event routing"))?;
        Ok(match role {
            NativeEventRole::Critical => !self
                .analysis
                .is_global_lower_terminal_event_mm(bracket, work, cancel)
                .map_err(|e| super::native_error(e, "native terminal event ownership"))?,
            NativeEventRole::LowerSupport => bracket != self.lower_support,
            NativeEventRole::UpperSupport => true,
        })
    }
}

fn native_event_storage_bytes(admission: &Admission, required_capacity: usize) -> usize {
    admission
        .native_capture_proposal_bytes()
        .saturating_add(
            admission
                .native_event_bands
                .capacity()
                .saturating_mul(std::mem::size_of::<[f64; 2]>()),
        )
        .saturating_add(required_capacity.saturating_mul(std::mem::size_of::<[f64; 2]>()))
        .saturating_add(
            admission
                .horizontal_events
                .capacity()
                .saturating_mul(std::mem::size_of::<f64>()),
        )
}

fn insert_native_event_band(
    bands: &mut Vec<[f64; 2]>,
    bracket: [f64; 2],
    resident_bytes: usize,
    maximum_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    charge_capture_proposal_work(work, bands.len().max(1).ilog2() as usize + 2, cancel)?;
    let Err(position) = bands.binary_search_by(|old| {
        old[0]
            .total_cmp(&bracket[0])
            .then(old[1].total_cmp(&bracket[1]))
    }) else {
        return Ok(());
    };
    reserve_native_event_growth(bands, resident_bytes, maximum_bytes, work, cancel)?;
    charge_capture_proposal_work(work, bands.len() - position + 1, cancel)?;
    bands.insert(position, bracket);
    Ok(())
}

fn reserve_native_event_growth<T>(
    records: &mut Vec<T>,
    resident_bytes: usize,
    maximum_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if records.len() >= MAX_PRINT_LAYERS as usize {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "native event-band coverage exceeds the layer schedule budget",
        ));
    }
    if records.len() == records.capacity() {
        let next_capacity = records
            .capacity()
            .max(1)
            .saturating_mul(2)
            .min(MAX_PRINT_LAYERS as usize);
        let replacement_bytes = next_capacity.saturating_mul(std::mem::size_of::<T>());
        if resident_bytes.saturating_add(replacement_bytes) > maximum_bytes {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "native event records exceed coexisting height-query capacity",
            ));
        }
        charge_capture_proposal_work(work, records.len(), cancel)?;
        records
            .try_reserve_exact(next_capacity - records.len())
            .map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native event-band allocation refused",
                )
            })?;
        if records.capacity() > next_capacity {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "native event-band allocator exceeds admitted capacity",
            ));
        }
    }
    Ok(())
}

pub fn preflight(
    input: &CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<PreflightReport, ManufacturingError> {
    let mut work = spiling_geometry::GeometryWorkBudget::compile();
    Ok(admit(input, cancel, &mut work)?.0.report)
}

/// Native/finish bounds for packing heuristics. Complete candidate compilation and
/// independent replay remain mandatory; these bounds do not certify supports.
pub fn layout_footprints(
    input: &CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<Vec<(OccurrenceId, AabbMm)>, ManufacturingError> {
    if matches!(&input.intent.printer.workflow, JobWorkflow::FixedDualMarlin { job } if !job.uniform_without_construction())
    {
        return Err(error(
            ManufacturingErrorCode::UnsupportedGeometry,
            "Fixed-dual arrangement currently requires uniform parts without material construction",
        ));
    }
    let mut work = spiling_geometry::GeometryWorkBudget::compile();
    let (admission, _) = admit(input, cancel, &mut work)?;
    for check in &admission.report.checks {
        if check.required
            && matches!(
                check.evidence,
                PreflightEvidence::Unsupported
                    | PreflightEvidence::Failed
                    | PreflightEvidence::Indeterminate
            )
            && !matches!(
                check.code.as_str(),
                "outside_envelope"
                    | "hardware_placement"
                    | "surface_component_exclusion"
                    | "surface_prime_exclusion"
                    | "surface_occurrence_exclusion"
            )
        {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                &check.message,
            ));
        }
    }
    input
        .occurrences
        .iter()
        .map(|o| {
            let b = *admission
                .native_bounds
                .get(&o.occurrence_id)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        "Layout source has no admitted native bounds",
                    )
                })?;
            Ok((
                o.occurrence_id,
                admission
                    .surface_fields
                    .get(&o.occurrence_id)
                    .map_or(b, |f| surface::expanded(b, f)),
            ))
        })
        .collect()
}

pub(super) fn admit<'a, 'work>(
    input: &'a CompilerInput<'_>,
    cancel: &AtomicBool,
    geometry_work: &'work mut spiling_geometry::GeometryWorkBudget,
) -> Result<(Admission<'work>, super::PreparedSources<'a>), ManufacturingError> {
    admit_with(input, cancel, geometry_work, false)
}

/// Preflight in exact or ADR012 tolerant mode. Tolerant mode takes bounds and
/// material extents from each occurrence's placed tessellation instead of
/// exact native analysis; every other check is unchanged.
pub(super) fn admit_with<'a, 'work>(
    input: &'a CompilerInput<'_>,
    cancel: &AtomicBool,
    geometry_work: &'work mut spiling_geometry::GeometryWorkBudget,
    tolerant: bool,
) -> Result<(Admission<'work>, super::PreparedSources<'a>), ManufacturingError> {
    check_cancel(cancel)?;
    let mut tolerant_models: BTreeMap<spiling_contracts::geometry::DefinitionId, TolerantModel> =
        BTreeMap::new();
    let mut result = Admission {
        bounds: None,
        horizontal_events: Vec::new(),
        native_bounds: BTreeMap::new(),
        native_event_bands: Vec::new(),
        native_capture_proposal_bands: Vec::new(),
        material_domains: BTreeMap::new(),
        surface_fields: BTreeMap::new(),
        geometry_work,
        tolerant: tolerant.then(BTreeMap::new),
        report: PreflightReport {
            input_revision: input.revision,
            disposition: PreflightDisposition::Admitted,
            software_only: true,
            checks: Vec::new(),
        },
    };
    let mut truncated = false;
    let mut pending_faces = Vec::new();
    let mut add = |code: &str, message: String, occurrence_id, face_id| {
        let evidence = match code {
            "native_analysis_indeterminate"
            | "native_conditioning_failure"
            | "unresolved_native_event" => PreflightEvidence::Indeterminate,
            "native_unsupported_geometry"
            | "native_unsupported_conversion"
            | "unsupported_surface" => PreflightEvidence::Unsupported,
            _ => PreflightEvidence::Failed,
        };
        if result.report.disposition != PreflightDisposition::Refused {
            result.report.disposition = if evidence == PreflightEvidence::Indeterminate {
                PreflightDisposition::Unresolved
            } else {
                PreflightDisposition::Refused
            };
        }
        // Truncating required failures never restores admission.
        if result.report.checks.len() < 252 {
            result.report.checks.push(PreflightCheck {
                code: code.into(),
                severity: PreflightSeverity::Error,
                evidence,
                required: true,
                message,
                occurrence_id,
                face_id,
            });
        } else {
            truncated = true;
        }
    };
    if let Err(e) = input.intent.validate() {
        add("invalid_intent", e.to_string(), None, None);
    }
    if matches!(
        input.intent.recipe.sparse_infill,
        SparseInfillPolicy::Grid | SparseInfillPolicy::WebRoof { .. }
    ) && (input
        .process_document
        .layers
        .iter()
        .any(|layer| layer.enabled && !layer.operations.is_empty())
        || input
            .surface_overlays
            .layers
            .iter()
            .any(|layer| layer.enabled))
    {
        add(
            "infill_scope",
            "Two-axis infill currently requires no enabled process or finish layers".into(),
            None,
            None,
        );
    }
    if let JobWorkflow::FixedDualMarlin { job } = &input.intent.printer.workflow {
        if let Some(construction) = &job.functional_construction {
            let source = input
                .occurrences
                .iter()
                .find(|occurrence| {
                    occurrence.occurrence_id == construction.occurrence_id
                        && occurrence.definition_id == construction.definition_id
                })
                .and_then(|occurrence| input.definitions.get(&occurrence.definition_id));
            if source.is_none_or(|native| {
                native.provenance().source_hash != construction.source_hash
                    || !native
                        .faces()
                        .iter()
                        .any(|face| face.face_id == construction.face_id)
            }) {
                add("invalid_functional_target", "Functional material construction does not bind the immutable native source and face".into(), Some(construction.occurrence_id), Some(construction.face_id.clone()));
            }
        }
        for construction in &job.surface_constructions {
            let source = input
                .occurrences
                .iter()
                .find(|o| {
                    o.occurrence_id == construction.occurrence_id
                        && o.definition_id == construction.definition_id
                })
                .and_then(|o| input.definitions.get(&o.definition_id));
            if source.is_none_or(|native| {
                native.provenance().source_hash != construction.source_hash
                    || construction.faces.iter().any(|f| {
                        !native
                            .faces()
                            .iter()
                            .any(|actual| actual.face_id == f.face_id)
                    })
            }) {
                add("invalid_surface_targets", "Surface material construction does not bind the immutable native source/face table".into(), Some(construction.occurrence_id), None);
            }
        }
        if input.occurrences.len() != job.assignments.len()
            || job.assignments.iter().any(|assignment| {
                input
                    .occurrences
                    .iter()
                    .filter(|occurrence| occurrence.occurrence_id == assignment.occurrence_id)
                    .count()
                    != 1
            })
        {
            add(
                "fixed_dual_sources",
                "Fixed-dual routing requires exactly the explicitly assigned native occurrences"
                    .into(),
                None,
                None,
            );
        }
        if !matches!(input.intent.recipe.supports, SupportPolicy::Disabled)
            || (!matches!(input.intent.recipe.sparse_infill, SparseInfillPolicy::Fine)
                && !job.has_bound_interior())
            || !matches!(
                input.intent.recipe.process.retraction,
                RetractionPolicy::Disabled
            )
            || input
                .process_document
                .layers
                .iter()
                .any(|layer| layer.enabled && !layer.operations.is_empty())
        {
            add("fixed_dual_policy", "Fixed-dual routing requires disabled body retraction/support/process constructions; combined interiors require an explicit same-formulation tool binding".into(), None, None);
        }
    }
    if input.definitions.len() > MAX_DEFINITIONS as usize
        || input.occurrences.len() > MAX_OCCURRENCES as usize
    {
        add(
            "resource_limit",
            "Native input record budget exceeded".into(),
            None,
            None,
        );
    }
    if input.definitions.is_empty() || input.occurrences.is_empty() {
        add(
            "empty_project",
            "Place at least one admitted native definition before Compile".into(),
            None,
            None,
        );
    }
    if input.stored_definitions.len() != input.definitions.len() {
        add(
            "source_identity",
            "Stored/native definition sets differ".into(),
            None,
            None,
        );
    }
    for stored in input
        .stored_definitions
        .iter()
        .take(MAX_DEFINITIONS as usize)
    {
        check_cancel(cancel)?;
        match input.definitions.get(&stored.definition_id) {
            Some(native)
                if native.id() == &stored.definition_id
                    && native.provenance().source_hash == stored.provenance.source_hash
                    && native.provenance().source_unit == stored.provenance.source_unit
                    && native.provenance().uncertainty_mm == stored.provenance.uncertainty_mm => {}
            _ => add(
                "source_identity",
                "Stored/native source identity or units mismatch".into(),
                None,
                None,
            ),
        }
    }
    let mut surface_valid = match crate::validate_surface_images(input, cancel)
        .and_then(|()| surface::validate_targets(input, cancel))
    {
        Ok(()) => true,
        Err(e) if e.code == ManufacturingErrorCode::Cancelled => return Err(e),
        Err(e) => {
            add(
                if e.code == ManufacturingErrorCode::ResourceLimit {
                    "resource_limit"
                } else {
                    "invalid_surface_targets"
                },
                e.message,
                None,
                None,
            );
            false
        }
    };
    let mut occurrence_bounds = BTreeMap::new();
    let mut surface_samples = 0usize;
    let envelope = input.intent.printer.build_envelope;
    let mut geometry_sources = BTreeMap::new();
    let mut remaining_prepared = spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES;
    'occurrences: for occurrence in input.occurrences.iter().take(MAX_OCCURRENCES as usize) {
        check_cancel(cancel)?;
        if let Err(e) = occurrence.pose.validate() {
            add(
                "invalid_pose",
                e.to_string(),
                Some(occurrence.occurrence_id),
                None,
            );
            continue;
        }
        let Some(definition) = input.definitions.get(&occurrence.definition_id) else {
            add(
                "missing_definition",
                "Occurrence definition is missing".into(),
                Some(occurrence.occurrence_id),
                None,
            );
            continue;
        };
        if let JobWorkflow::FixedDualMarlin { .. } = &input.intent.printer.workflow {
            let rotation = occurrence.pose.rotation_xyzw;
            if rotation[0] != 0.0 || rotation[1] != 0.0 {
                add(
                    "fixed_dual_upright",
                    "Fixed-dual routing requires an exactly upright source frame".into(),
                    Some(occurrence.occurrence_id),
                    None,
                );
            }
            if definition.faces().iter().any(|face| {
                input
                    .surface_overlays
                    .effective_layer(
                        &occurrence.definition_id,
                        occurrence.occurrence_id,
                        &face.face_id,
                    )
                    .is_some_and(|layer| {
                        !matches!(
                            layer.finish,
                            spiling_contracts::surface::SurfaceFinish::Smooth
                        )
                    })
            }) {
                add(
                    "fixed_dual_finish",
                    "Fixed-dual routing does not admit effective source finishes".into(),
                    Some(occurrence.occurrence_id),
                    None,
                );
            }
        }
        let mut placed = if let Some(models) = result.tolerant.as_mut() {
            if !tolerant_models.contains_key(&occurrence.definition_id) {
                match TolerantModel::build(definition, TolerantPolicy::default(), cancel) {
                    Ok(model) if model.failed_faces == 0 => {
                        tolerant_models.insert(occurrence.definition_id.clone(), model);
                    }
                    Ok(model) => {
                        add(
                            "native_unsupported_geometry",
                            format!(
                                "{} source faces could not be tessellated within the profile tolerance",
                                model.failed_faces
                            ),
                            Some(occurrence.occurrence_id),
                            None,
                        );
                        continue 'occurrences;
                    }
                    Err(e) if e.code == GeometryErrorCode::Cancelled => {
                        return Err(super::native_error(e, "tolerant tessellation"));
                    }
                    Err(e) => {
                        add(
                            native_check_code(e.code),
                            e.to_string(),
                            Some(occurrence.occurrence_id),
                            None,
                        );
                        continue 'occurrences;
                    }
                }
            }
            let model = tolerant_models[&occurrence.definition_id].placed(&occurrence.pose);
            let margin = model.policy().profile_mm;
            let (min, max) = model.bounds();
            // Tessellation vertices lie on the surface; chords can cut inside
            // curved walls by at most the profile tolerance laterally. Z stays
            // at the vertex extremes so bed contact remains exactly at the bed.
            let placed = AabbMm {
                min: [min[0] - margin, min[1] - margin, min[2]],
                max: [max[0] + margin, max[1] + margin, max[2]],
            };
            result.material_domains.insert(
                occurrence.occurrence_id,
                [[min[2], min[2]], [max[2], max[2]]],
            );
            models.insert(occurrence.occurrence_id, std::sync::Arc::new(model));
            placed
        } else {
            if !geometry_sources.contains_key(&occurrence.definition_id) {
                let source = match spiling_geometry::prepare_solid(
                    definition,
                    remaining_prepared,
                    result.geometry_work,
                    cancel,
                ) {
                    Ok(source) => source,
                    Err(e)
                        if matches!(
                            e.code,
                            GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit
                        ) =>
                    {
                        return Err(super::native_error(
                            e,
                            "native material geometry preparation",
                        ));
                    }
                    Err(e) => {
                        add(
                            native_check_code(e.code),
                            e.to_string(),
                            Some(occurrence.occurrence_id),
                            None,
                        );
                        continue 'occurrences;
                    }
                };
                remaining_prepared = remaining_prepared
                    .checked_sub(source.retained_bytes())
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "aggregate native material geometry capacity exceeded",
                        )
                    })?;
                geometry_sources.insert(occurrence.definition_id.clone(), source);
            }
            let query_bytes = super::geometry_query_limit(&geometry_sources)?;
            let mut placed = AabbMm {
                min: [0.0; 3],
                max: [0.0; 3],
            };
            for axis in 0..3 {
                let direction = std::array::from_fn(|a| if a == axis { 1.0 } else { 0.0 });
                // A defect fuse per query; a fused query is a ResourceLimit
                // refusal, which the planner retries in the tolerant lane.
                let mut fuse = spiling_geometry::GeometryWorkBudget::exact_bounds_fuse();
                let query = geometry_sources[&occurrence.definition_id].placed_material_bounds(
                    occurrence.pose,
                    direction,
                    // Extents need not be finer than the printer can move.
                    spiling_geometry::BoundsPolicy {
                        maximum_endpoint_uncertainty_mm: axis_resolution(input, axis),
                    },
                    query_bytes,
                    &mut fuse,
                    cancel,
                );
                result
                    .geometry_work
                    .charge(fuse.consumed(), cancel)
                    .map_err(|e| super::native_error(e, "material projected bounds"))?;
                let extent = match query {
                    Ok(extent) => {
                        if extent.definition_id() != &occurrence.definition_id
                            || !extent.is_compatible_with_definition(definition)
                            || extent.placed_pose() != occurrence.pose
                            || extent.world_direction() != direction
                        {
                            return Err(error(
                                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                                "material support lost its native producer/source/pose/query binding",
                            ));
                        }
                        extent.bounds()
                    }
                    Err(e)
                        if matches!(
                            e.code,
                            GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit
                        ) =>
                    {
                        return Err(super::native_error(e, "material projected bounds"));
                    }
                    Err(e) => {
                        add(
                            native_check_code(e.code),
                            e.to_string(),
                            Some(occurrence.occurrence_id),
                            None,
                        );
                        continue 'occurrences;
                    }
                };
                placed.min[axis] = extent.minimum[0];
                placed.max[axis] = extent.maximum[1];
                if axis == 2 {
                    result
                        .material_domains
                        .insert(occurrence.occurrence_id, [extent.minimum, extent.maximum]);
                }
            }
            placed
        };
        result
            .native_bounds
            .insert(occurrence.occurrence_id, placed);
        if let JobWorkflow::FixedDualMarlin { job } = &input.intent.printer.workflow {
            match job.assignments.iter().find(|assignment| assignment.occurrence_id == occurrence.occurrence_id) {
                Some(assignment) if (0..3).all(|axis| {
                    placed.min[axis] >= assignment.work_envelope_mm.min[axis]
                        && placed.max[axis] <= assignment.work_envelope_mm.max[axis]
                }) => {}
                _ => add("fixed_dual_source_reservation", "Complete conservative native source bounds must lie within its explicit routed work reservation".into(), Some(occurrence.occurrence_id), None),
            }
        }
        for face in definition
            .faces()
            .iter()
            .take(16usize.saturating_sub(pending_faces.len()))
        {
            pending_faces.push((occurrence.occurrence_id, face.face_id.clone()));
        }
        if surface_valid {
            match surface::metadata(
                input,
                occurrence,
                placed,
                (MAX_DEPOSITION_SEGMENTS as usize / 4).saturating_sub(surface_samples),
                result.geometry_work,
                cancel,
            ) {
                Ok(fields) => {
                    let additional = fields
                        .iter()
                        .map(surface::PreparedFinish::sample_points)
                        .sum::<usize>();
                    surface_samples = surface_samples.saturating_add(additional);
                    placed = surface::expanded(placed, &fields);
                    if !fields.is_empty() {
                        for component in &input.intent.printer.components {
                            if surface::intersects(placed, surface::component_bounds(component)) {
                                add(
                                    "surface_component_exclusion",
                                    format!(
                                        "Finished material conservative bounds intersect fixed component {}; this lane does not certify swept-tool clearance",
                                        component.id
                                    ),
                                    Some(occurrence.occurrence_id),
                                    None,
                                );
                            }
                        }
                        if let JobWorkflow::CompleteMarlin { job } = &input.intent.printer.workflow
                            && surface::prime_intersects(
                                placed,
                                job,
                                input.intent.recipe.bead_width_mm,
                            )
                        {
                            add("surface_prime_exclusion", "Finished material expanded XY envelope intersects the declared priming segment".into(), Some(occurrence.occurrence_id), None);
                        }
                        result
                            .surface_fields
                            .insert(occurrence.occurrence_id, fields);
                    }
                }
                Err(e) if e.code == ManufacturingErrorCode::Cancelled => return Err(e),
                Err(e) => {
                    // A resource refusal is final; do not spend the remaining
                    // occurrence budget sampling more finishes after exhaustion.
                    if e.code == ManufacturingErrorCode::ResourceLimit {
                        surface_valid = false;
                    }
                    add(
                        if e.code == ManufacturingErrorCode::ResourceLimit {
                            "resource_limit"
                        } else {
                            "unsupported_surface"
                        },
                        e.message,
                        Some(occurrence.occurrence_id),
                        None,
                    );
                }
            }
        }
        if let Some(hardware) = &input.intent.hardware
            && let Err(reason) = hardware.admits_bounds(&placed)
        {
            add(
                "hardware_placement",
                reason,
                Some(occurrence.occurrence_id),
                None,
            );
        }
        occurrence_bounds.insert(occurrence.occurrence_id, placed);
        // ADR012: extents within one motion step of the envelope are the same
        // commanded position; tessellated extents may sit that far outside.
        if (0..3).any(|a| {
            placed.min[a] < envelope.min[a] - axis_resolution(input, a)
                || placed.max[a] > envelope.max[a] + axis_resolution(input, a)
        }) {
            add("outside_envelope", "Placed native extrema plus admitted surface-finish expansion exceed the build envelope; change placement, finish or printer".into(), Some(occurrence.occurrence_id), None);
        }
        if let Some(bounds) = &mut result.bounds {
            bounds
                .include(placed.min)
                .map_err(|e| error(ManufacturingErrorCode::UnsupportedGeometry, e.to_string()))?;
            bounds
                .include(placed.max)
                .map_err(|e| error(ManufacturingErrorCode::UnsupportedGeometry, e.to_string()))?;
        } else {
            result.bounds = Some(placed);
        }
    }
    for occurrence_id in result.surface_fields.keys() {
        for (other, bounds) in &occurrence_bounds {
            check_cancel(cancel)?;
            if occurrence_id == other {
                continue;
            }
            if surface::intersects(occurrence_bounds[occurrence_id], *bounds) {
                add("surface_occurrence_exclusion", "Finished occurrence conservative expanded bounds intersect another occurrence; overlapping finishes are not admitted".into(), Some(*occurrence_id), None);
            }
        }
    }
    if let Some(bounds) = result.bounds
        && bounds.min[2] > envelope.min[2] + axis_resolution(input, 2)
        && matches!(input.intent.recipe.supports, SupportPolicy::Disabled)
    {
        add("floating_without_support", "Native material starts above the bed; enable breakaway supports or place it on the bed".into(), None, None);
    }
    drop(add);
    if let Ok(selected) = input.intent.select_layer_height()
        && selected.reason != LayerHeightReason::Requested
    {
        result.report.checks.push(PreflightCheck {
            code: "layer_height_goal_adjusted".into(),
            severity: PreflightSeverity::Warning,
            evidence: PreflightEvidence::Certified,
            required: false,
            message: format!("Preferred layer height {} mm selects {} mm under captured process limits; actual intervals and geometry remain Compile obligations", input.intent.recipe.layer_height_mm, selected.height_mm),
            occurrence_id: None, face_id: None,
        });
    }
    if truncated {
        result.report.checks.push(PreflightCheck {
            code: "additional_errors".into(),
            severity: PreflightSeverity::Error,
            evidence: if result.report.disposition == PreflightDisposition::Unresolved {
                PreflightEvidence::Indeterminate
            } else {
                PreflightEvidence::Failed
            },
            required: true,
            message:
                "Additional metadata errors are omitted; correct these errors and repeat Preflight"
                    .into(),
            occurrence_id: None,
            face_id: None,
        });
    }
    if result.report.disposition == PreflightDisposition::Admitted {
        result.report.checks.push(PreflightCheck {
            code: "native_metadata_admitted".into(),
            severity: PreflightSeverity::Info,
            evidence: PreflightEvidence::Certified,
            required: true,
            message: "Captured intent, source identity, pose and bounded native placement admitted. Complete physical height events, actual schedule and native metric/feature incidence remain Compile obligations".into(),
            occurrence_id: None, face_id: None,
        });
    }
    for (occurrence_id, fields) in &result.surface_fields {
        for field in fields {
            if result.report.checks.len() >= 254 {
                break;
            }
            result.report.checks.push(PreflightCheck { code: "surface_construction_pending".into(), severity: PreflightSeverity::Info, evidence: PreflightEvidence::NotEvaluated, required: false, message: format!("Native {} certificate and bounded feature metadata admitted; Compile must still prove native material/hole retention, realized offsets and actual deposited anchors. This is a manufacturing construction, not edited BREP or physical grip certification", field.certificate_description()),
            occurrence_id: Some(*occurrence_id), face_id: Some(field.face_id().clone()), });
        }
    }
    for (occurrence_id, face_id) in pending_faces {
        if result.report.checks.len() >= 254 {
            break;
        }
        result.report.checks.push(PreflightCheck {
            code: "oriented_native_gate_pending".into(),
            severity: PreflightSeverity::Info,
            evidence: PreflightEvidence::NotEvaluated,
            required: false,
            message: "This native face requires complete physical events, certified slab shape/feature incidence, continuous occupied envelopes and actual deposited-anchor analysis during Compile".into(),
            occurrence_id: Some(occurrence_id), face_id: Some(face_id),
        });
    }
    for (code, message) in [
        (
            "native_analysis_pending",
            "Complete physical event/branch coverage, ordered uncertainty bands and every actual closed slab are Compile-only obligations. Compile must certify metric/feature incidence and continuous occupied envelopes, then section topology, offsets, supports and actual deposited anchors. Admitted input is not authorization to manufacture.",
        ),
        (
            "physical_qualification_absent",
            "Software-only admission is not physical printability or machine qualification. No physical measurements, swept-bead collision certificate or printer qualification are available.",
        ),
    ] {
        result.report.checks.push(PreflightCheck {
            code: code.into(),
            severity: PreflightSeverity::Info,
            evidence: PreflightEvidence::NotEvaluated,
            required: false,
            message: message.into(),
            occurrence_id: None,
            face_id: None,
        });
    }
    result.report.validate()?;
    Ok((result, geometry_sources))
}

/// Deep Compile-only discovery. An uncertainty endpoint remains bookkeeping
/// until every affected actual slab has metric and event-incidence evidence.
pub(super) fn resolve_native_events(
    input: &CompilerInput<'_>,
    admission: &mut Admission,
    geometry_sources: &super::PreparedSources<'_>,
    proposal_work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let bed = input.intent.printer.build_envelope.min[2];
    if let Some(models) = &admission.tolerant {
        // ADR012 feature-first heights: horizontal faces and extents of each
        // placed tessellation, merged within the height tolerance. The bed is
        // the first boundary; nothing here is a native event band.
        // ADR012: heights closer than the printer's Z step are one commanded
        // height, e.g. assembled bodies meant to sit flush.
        let merge_mm = input.intent.printer.motion_resolution.z_mm;
        let mut heights = Vec::new();
        let mut combined: Option<AabbMm> = None;
        for occurrence in input.occurrences {
            check_cancel(cancel)?;
            let model = &models[&occurrence.occurrence_id];
            heights.extend(model.feature_heights(merge_mm));
            let bounds = admission.native_bounds[&occurrence.occurrence_id];
            let placed = admission
                .surface_fields
                .get(&occurrence.occurrence_id)
                .map_or(bounds, |fields| surface::expanded(bounds, fields));
            match &mut combined {
                Some(all) => {
                    all.include(placed.min)
                        .and_then(|_| all.include(placed.max))
                        .map_err(|e| {
                            error(ManufacturingErrorCode::InvalidSpecification, e.to_string())
                        })?;
                }
                None => combined = Some(placed),
            }
        }
        heights.sort_by(f64::total_cmp);
        for z in heights {
            let z = if (z - bed).abs() <= merge_mm { bed } else { z };
            if z < bed {
                continue;
            }
            if admission
                .horizontal_events
                .last()
                .is_none_or(|last| z - last > merge_mm)
            {
                admission.horizontal_events.push(z);
            }
        }
        admission.bounds = combined;
        *proposal_work = proposal_work.saturating_add(admission.horizontal_events.len());
        return Ok(());
    }
    // Heights resolve to the printer's Z step: two events closer than one step
    // are one commanded height on this machine.
    let policy = spiling_geometry::AnalysisPolicy {
        maximum_height_uncertainty_mm: axis_resolution(input, 2),
        maximum_shape_error_mm: input.intent.recipe.native_shape_error_mm,
    };
    let mut completed_bounds: Option<AabbMm> = None;
    let mut required_bands = Vec::new();
    let query_pool_bytes = super::geometry_query_limit(geometry_sources)?;
    for occurrence in input.occurrences {
        check_cancel(cancel)?;
        let bounds = admission.native_bounds[&occurrence.occurrence_id];
        let query_bytes = query_pool_bytes
            .checked_sub(native_event_storage_bytes(
                admission,
                required_bands.capacity(),
            ))
            .filter(|bytes| *bytes > 0)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native event storage exhausts shared query capacity",
                )
            })?;
        let analysis = geometry_sources[&occurrence.definition_id]
            .analyze_placed_height_domain(
                occurrence.pose,
                [bounds.min[2], bounds.max[2]],
                policy,
                query_bytes,
                admission.geometry_work,
                cancel,
            )
            .map_err(|e| super::native_error(e, "complete native height analysis"))?;
        if analysis.definition_id() != &occurrence.definition_id
            || analysis.pose() != Some(occurrence.pose)
            || analysis.height_interval_mm() != [bounds.min[2], bounds.max[2]]
        {
            return Err(error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                format!(
                    "occurrence {} has no captured complete terminal-event coverage",
                    occurrence.occurrence_id.get()
                ),
            ));
        }
        // Covered literal F candidates remain deferred G obligations. Preserve
        // their neighborhoods as query-sizing hints without adding slicing
        // planes or treating a candidate as an actual material critical root.
        let candidates = analysis.event_brackets_mm();
        let count = admission
            .native_capture_proposal_bands
            .len()
            .checked_add(candidates.len())
            .filter(|count| *count <= MAX_PRINT_LAYERS as usize)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native capture proposal count exceeds the layer schedule budget",
                )
            })?;
        for bracket in candidates {
            charge_capture_proposal_work(proposal_work, 1, cancel)?;
            if !bracket.iter().all(|z| z.is_finite()) || bracket[0] > bracket[1] {
                return Err(error(
                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                    "native capture proposal has an invalid literal event bracket",
                ));
            }
        }
        let growing = count > admission.native_capture_proposal_bands.capacity();
        let replacement_bytes = if growing {
            count.saturating_mul(std::mem::size_of::<[f64; 2]>())
        } else {
            0
        };
        if analysis
            .retained_bytes()
            .saturating_add(native_event_storage_bytes(
                admission,
                required_bands.capacity(),
            ))
            .saturating_add(replacement_bytes)
            > query_pool_bytes
        {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "native capture proposals exceed coexisting height-query capacity",
            ));
        }
        charge_capture_proposal_work(
            proposal_work,
            candidates.len().saturating_add(if growing {
                admission.native_capture_proposal_bands.len()
            } else {
                0
            }),
            cancel,
        )?;
        admission
            .native_capture_proposal_bands
            .try_reserve_exact(candidates.len())
            .map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native capture proposal allocation refused",
                )
            })?;
        if admission.native_capture_proposal_bands.capacity() > MAX_PRINT_LAYERS as usize
            || analysis
                .retained_bytes()
                .saturating_add(native_event_storage_bytes(
                    admission,
                    required_bands.capacity(),
                ))
                > query_pool_bytes
        {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "native capture proposal allocator exceeds coexisting capacity",
            ));
        }
        admission
            .native_capture_proposal_bands
            .extend_from_slice(candidates);
        let domain = admission.material_domains[&occurrence.occurrence_id];
        let lower_terminal = if admission.report.disposition == PreflightDisposition::Admitted {
            LowerTerminalRouting::capture(&analysis, occurrence, bounds, domain, bed, policy)
        } else {
            None
        };
        charge_capture_proposal_work(proposal_work, analysis.event_brackets_mm().len(), cancel)?;
        let events = analysis
            .material_plane_event_brackets_mm()
            .map(|band| (band, NativeEventRole::Critical))
            .chain([
                (domain[0], NativeEventRole::LowerSupport),
                (domain[1], NativeEventRole::UpperSupport),
            ]);
        for (bracket, role) in events {
            charge_capture_proposal_work(proposal_work, 1, cancel)?;
            if !bracket.iter().all(|z| z.is_finite())
                || bracket[0] > bracket[1]
                || bracket[1] - bracket[0] > policy.maximum_height_uncertainty_mm
                || bracket[1] - bracket[0] > policy.maximum_shape_error_mm
            {
                return Err(error(
                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                    format!(
                        "occurrence {} has an unresolved native event band {bracket:?}",
                        occurrence.occurrence_id.get()
                    ),
                ));
            }
            let resident = analysis
                .retained_bytes()
                .saturating_add(native_event_storage_bytes(
                    admission,
                    required_bands.capacity(),
                ));
            insert_native_event_band(
                &mut admission.native_event_bands,
                bracket,
                resident,
                query_pool_bytes,
                proposal_work,
                cancel,
            )?;
            let requires_plane = match lower_terminal.as_ref() {
                Some(routing) => {
                    routing.requires_plane(bracket, role, admission.geometry_work, cancel)?
                }
                None => true,
            };
            if requires_plane {
                let resident =
                    analysis
                        .retained_bytes()
                        .saturating_add(native_event_storage_bytes(
                            admission,
                            required_bands.capacity(),
                        ));
                insert_native_event_band(
                    &mut required_bands,
                    bracket,
                    resident,
                    query_pool_bytes,
                    proposal_work,
                    cancel,
                )?;
            }
        }
        let placed = admission
            .surface_fields
            .get(&occurrence.occurrence_id)
            .map_or(bounds, |fields| surface::expanded(bounds, fields));
        if let Some(combined) = &mut completed_bounds {
            combined
                .include(placed.min)
                .and_then(|_| combined.include(placed.max))
                .map_err(|e| {
                    error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        e.to_string(),
                    )
                })?;
        } else {
            completed_bounds = Some(placed);
        }
    }
    // Union overlapping scheduling covers only. Atomic source candidates and
    // their existence/incidence obligations remain in the geometry capture.
    // Neither root identity nor an increased uncertainty allowance is implied.
    let maximum_cover_width = policy
        .maximum_height_uncertainty_mm
        .min(policy.maximum_shape_error_mm);
    charge_capture_proposal_work(proposal_work, required_bands.len(), cancel)?;
    for bracket in scheduling_event_covers(&required_bands, maximum_cover_width) {
        check_cancel(cancel)?;
        let z = if bracket[0] <= bed
            && bracket[1] >= bed
            && (bracket[0] - bed).abs().max((bracket[1] - bed).abs())
                <= policy.maximum_shape_error_mm
        {
            bed
        } else {
            bracket[1]
        };
        charge_capture_proposal_work(
            proposal_work,
            admission.horizontal_events.len().max(1).ilog2() as usize + 2,
            cancel,
        )?;
        if let Err(position) = admission
            .horizontal_events
            .binary_search_by(|old| old.total_cmp(&z))
        {
            let resident = native_event_storage_bytes(admission, required_bands.capacity());
            reserve_native_event_growth(
                &mut admission.horizontal_events,
                resident,
                query_pool_bytes,
                proposal_work,
                cancel,
            )?;
            charge_capture_proposal_work(
                proposal_work,
                admission.horizontal_events.len() - position + 1,
                cancel,
            )?;
            admission.horizontal_events.insert(position, z);
        }
    }
    admission.bounds = completed_bounds;
    Ok(())
}

pub(super) fn charge_capture_proposal_work(
    work: &mut usize,
    amount: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    *work = work.checked_add(amount).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "cumulative native capture proposal work counter overflow",
        )
    })?;
    Ok(())
}

/// Sorted, validated candidate bands become bounded scheduling-set covers.
/// A disjoint gap, however small, never supplies shared event identity.
fn scheduling_event_covers(
    bands: &[[f64; 2]],
    maximum_width: f64,
) -> impl Iterator<Item = [f64; 2]> + '_ {
    let mut bands = bands.iter().copied();
    let mut pending = None;
    std::iter::from_fn(move || {
        let mut cover = pending.take().or_else(|| bands.next())?;
        for candidate in bands.by_ref() {
            let upper = cover[1].max(candidate[1]);
            // Containment adds no uncertainty. An expanding union keeps a
            // strict margin so rounded subtraction cannot admit an overwide set.
            if candidate[0] <= cover[1] && (upper <= cover[1] || upper - cover[0] < maximum_width) {
                cover[1] = upper;
            } else {
                pending = Some(candidate);
                break;
            }
        }
        Some(cover)
    })
}

fn native_check_code(code: GeometryErrorCode) -> &'static str {
    match code {
        GeometryErrorCode::ConditioningFailure => "native_conditioning_failure",
        GeometryErrorCode::AnalysisIndeterminate
        | GeometryErrorCode::ParameterCorrespondenceUnavailable => "native_analysis_indeterminate",
        GeometryErrorCode::UnsupportedGeometry => "native_unsupported_geometry",
        GeometryErrorCode::UnsupportedConversion => "native_unsupported_conversion",
        GeometryErrorCode::KernelFailure => "native_kernel_failure",
        GeometryErrorCode::SingularGeometry => "native_singular_geometry",
        _ => "native_invalid_geometry",
    }
}

#[cfg(test)]
mod tests {
    use super::super::LAYER_QUANTIZATION_MM;
    use super::*;

    #[test]
    fn terminal_bed_envelope_checks_each_bracket_and_shape_budget_independently() {
        let bed = 2.0;
        let shape = 0.25 * LAYER_QUANTIZATION_MM;
        assert!(terminal_inside_bed_envelope(
            [bed, bed],
            bed,
            LAYER_QUANTIZATION_MM,
            shape
        ));
        assert!(terminal_inside_bed_envelope(
            [bed - shape * 0.5, bed + shape * 0.5],
            bed,
            LAYER_QUANTIZATION_MM,
            shape,
        ));
        assert!(!terminal_inside_bed_envelope(
            [bed, bed + shape * 2.0],
            bed,
            LAYER_QUANTIZATION_MM,
            shape,
        ));
        assert!(!terminal_inside_bed_envelope(
            [bed, bed + LAYER_QUANTIZATION_MM * 2.0],
            bed,
            LAYER_QUANTIZATION_MM,
            0.05,
        ));
        assert!(!terminal_inside_bed_envelope(
            [bed + shape, bed],
            bed,
            LAYER_QUANTIZATION_MM,
            shape
        ));
        assert!(!terminal_inside_bed_envelope(
            [bed, f64::NAN],
            bed,
            LAYER_QUANTIZATION_MM,
            shape
        ));
    }

    #[test]
    fn terminal_routing_retains_nonmembers_thin_events_and_foreign_capture_refusals() {
        let cancel = AtomicBool::new(false);
        let definition = spiling_geometry::from_step_bytes_with_cancel(
            include_bytes!("../../../../../fixtures/geometry/cylinder.step"),
            &cancel,
        )
        .unwrap();
        let angle = 1.0_f64.to_radians();
        let component = (angle * 0.5).sin() / 2.0_f64.sqrt();
        let mut pose = spiling_contracts::geometry::RigidPoseMm {
            translation_mm: [0.0; 3],
            rotation_xyzw: [component, component, 0.0, (angle * 0.5).cos()],
        };
        let mut work = spiling_geometry::GeometryWorkBudget::query();
        let bounds_policy = spiling_geometry::BoundsPolicy {
            maximum_endpoint_uncertainty_mm: LAYER_QUANTIZATION_MM,
        };
        let initial = spiling_geometry::placed_material_bounds(
            &definition,
            pose,
            [0.0, 0.0, 1.0],
            bounds_policy,
            &mut work,
            &cancel,
        )
        .unwrap()
        .bounds();
        pose.translation_mm[2] = -initial.minimum[0];
        let support = spiling_geometry::placed_material_bounds(
            &definition,
            pose,
            [0.0, 0.0, 1.0],
            bounds_policy,
            &mut work,
            &cancel,
        )
        .unwrap()
        .bounds();
        let bounds = AabbMm {
            min: [0.0, 0.0, support.minimum[0]],
            max: [0.0, 0.0, support.maximum[1]],
        };
        let domain = [support.minimum, support.maximum];
        let policy = spiling_geometry::AnalysisPolicy {
            maximum_height_uncertainty_mm: LAYER_QUANTIZATION_MM,
            maximum_shape_error_mm: 0.05,
        };
        let analysis = spiling_geometry::analyze_placed_height_domain(
            &definition,
            pose,
            [bounds.min[2], bounds.max[2]],
            policy,
            &mut work,
            &cancel,
        )
        .unwrap();
        let occurrence = OccurrenceRecord {
            occurrence_id: OccurrenceId::new(1).unwrap(),
            definition_id: definition.id().clone(),
            pose,
        };
        let routing =
            LowerTerminalRouting::capture(&analysis, &occurrence, bounds, domain, 0.0, policy)
                .expect("same-source qualified terminal inside the existing placement envelope");
        let original = analysis.event_brackets_mm().to_vec();
        let terminal = analysis.global_lower_terminal_bracket_mm().unwrap();
        let mut members = 0;
        for band in analysis.material_plane_event_brackets_mm() {
            let member = analysis
                .is_global_lower_terminal_event_mm(band, &mut work, &cancel)
                .unwrap();
            assert_eq!(
                routing
                    .requires_plane(band, NativeEventRole::Critical, &mut work, &cancel)
                    .unwrap(),
                !member
            );
            members += usize::from(member);
        }
        assert!(members > 0);
        let owned_band = original
            .iter()
            .copied()
            .find(|band| {
                analysis
                    .is_global_lower_terminal_event_mm(*band, &mut work, &cancel)
                    .unwrap()
            })
            .unwrap();
        assert!(
            routing
                .requires_plane(
                    owned_band,
                    NativeEventRole::UpperSupport,
                    &mut work,
                    &cancel,
                )
                .unwrap(),
            "identical numbers do not erase another event role"
        );
        assert!(
            !routing
                .requires_plane(domain[0], NativeEventRole::LowerSupport, &mut work, &cancel)
                .unwrap()
        );
        assert_eq!(
            routing
                .requires_plane(domain[0], NativeEventRole::Critical, &mut work, &cancel)
                .unwrap(),
            !analysis
                .is_global_lower_terminal_event_mm(domain[0], &mut work, &cancel)
                .unwrap()
        );
        assert!(
            routing
                .requires_plane(domain[1], NativeEventRole::UpperSupport, &mut work, &cancel)
                .unwrap()
        );
        let nonmember = [terminal[1].next_up(); 2];
        assert!(
            routing
                .requires_plane(nonmember, NativeEventRole::Critical, &mut work, &cancel)
                .unwrap()
        );
        assert!(
            routing
                .requires_plane(nonmember, NativeEventRole::LowerSupport, &mut work, &cancel)
                .unwrap()
        );
        let thin = [[0.5, 0.5], [0.5000001, 0.5000001]];
        for band in thin {
            assert!(
                routing
                    .requires_plane(band, NativeEventRole::Critical, &mut work, &cancel)
                    .unwrap()
            );
        }
        assert_eq!(
            scheduling_event_covers(&thin, LAYER_QUANTIZATION_MM).collect::<Vec<_>>(),
            thin
        );
        for events in [
            vec![nonmember[1]],
            thin.iter().map(|band| band[1]).collect(),
        ] {
            // Planes closer than one Z step are one commanded height: the
            // schedule never contains a layer thinner than that step.
            let schedule = crate::planner::layer_schedule(0.0, 1.0, 0.2, &events).unwrap();
            assert!(
                schedule
                    .iter()
                    .all(|layer| layer.height > crate::planner::REFERENCE_Z_STEP_MM)
            );
        }
        let mut foreign = occurrence.clone();
        foreign.pose.translation_mm[0] += 1.0;
        assert!(
            LowerTerminalRouting::capture(&analysis, &foreign, bounds, domain, 0.0, policy)
                .is_none()
        );
        let mut all_bands = Vec::new();
        let mut required_bands = Vec::new();
        let mut proposal_work = 0;
        insert_native_event_band(
            &mut all_bands,
            owned_band,
            0,
            4096,
            &mut proposal_work,
            &cancel,
        )
        .unwrap();
        insert_native_event_band(
            &mut all_bands,
            owned_band,
            4096,
            4096,
            &mut proposal_work,
            &cancel,
        )
        .unwrap();
        insert_native_event_band(
            &mut required_bands,
            owned_band,
            0,
            4096,
            &mut proposal_work,
            &cancel,
        )
        .unwrap();
        assert_eq!(required_bands, all_bands);
        let other = spiling_geometry::from_step_bytes_with_cancel(
            include_bytes!("../../../../../fixtures/geometry/box-mm.step"),
            &cancel,
        )
        .unwrap();
        foreign = occurrence.clone();
        foreign.definition_id = other.id().clone();
        assert!(
            LowerTerminalRouting::capture(&analysis, &foreign, bounds, domain, 0.0, policy)
                .is_none()
        );
        let mut incomplete = domain;
        incomplete[0][0] = incomplete[0][0].next_up();
        assert!(
            LowerTerminalRouting::capture(&analysis, &occurrence, bounds, incomplete, 0.0, policy)
                .is_none()
        );
        assert!(
            LowerTerminalRouting::capture(&analysis, &occurrence, bounds, domain, -1.0, policy)
                .is_none()
        );
        assert_eq!(analysis.event_brackets_mm(), original);
        for (mut budget, cancellation, expected) in [
            (
                spiling_geometry::GeometryWorkBudget::new(0),
                AtomicBool::new(false),
                ManufacturingErrorCode::ResourceLimit,
            ),
            (
                spiling_geometry::GeometryWorkBudget::query(),
                AtomicBool::new(true),
                ManufacturingErrorCode::Cancelled,
            ),
        ] {
            assert_eq!(
                routing
                    .requires_plane(
                        owned_band,
                        NativeEventRole::Critical,
                        &mut budget,
                        &cancellation,
                    )
                    .unwrap_err()
                    .code,
                expected
            );
        }
    }

    #[test]
    fn event_band_storage_preserves_capacity_work_and_cancellation_refusals() {
        let mut bands = Vec::new();
        for (limit, cancel) in [(0, AtomicBool::new(false)), (1024, AtomicBool::new(true))] {
            let result =
                insert_native_event_band(&mut bands, [0.0, 0.0], 0, limit, &mut 0, &cancel)
                    .unwrap_err();
            assert_eq!(
                result.code,
                if limit == 0 {
                    ManufacturingErrorCode::ResourceLimit
                } else {
                    ManufacturingErrorCode::Cancelled
                }
            );
            assert!(bands.is_empty());
        }
        let mut exhausted_work = usize::MAX;
        assert_eq!(
            insert_native_event_band(
                &mut bands,
                [0.0, 0.0],
                0,
                1024,
                &mut exhausted_work,
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            ManufacturingErrorCode::ResourceLimit
        );
        assert!(bands.is_empty());
    }

    #[test]
    fn scheduling_covers_merge_overlap_without_erasing_a_disjoint_ulp_gap() {
        let next = 1.0_f64.next_up();
        let bands = [[0.0, 0.6], [0.2, 0.4], [0.4, 1.0], [next, 1.2]];
        let covers: Vec<_> = scheduling_event_covers(&bands, 1.1).collect();
        assert_eq!(covers, [[0.0, 1.0], [next, 1.2]]);
    }

    #[test]
    fn scheduling_covers_keep_the_policy_ceiling_and_every_candidate() {
        let bands = [[0.0, 0.75], [0.5, 1.25], [1.0, 1.375]];
        let covers: Vec<_> = scheduling_event_covers(&bands, 1.0).collect();
        assert_eq!(covers, [[0.0, 0.75], [0.5, 1.375]]);
        for band in bands {
            assert!(
                covers
                    .iter()
                    .any(|cover| cover[0] <= band[0] && cover[1] >= band[1])
            );
        }
        assert!(covers.iter().all(|cover| cover[1] - cover[0] <= 1.0));
    }
}
