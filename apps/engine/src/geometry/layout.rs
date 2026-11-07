// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Finite rectangular placement search, followed by complete ordinary compilation.
//! Footprints are search reservations, never replacements for native material or replay.
use super::{JobError, worker::CompileRequest};
use spiling_contracts::{geometry::*, manufacturing::*};
use spiling_core::PlacementDraft;
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};

pub(super) struct Request {
    pub session: SessionId,
    pub scene: SceneRevision,
    pub request: LayoutRequest,
    pub capture: CompileRequest,
    pub draft: PlacementDraft,
}
fn fail(message: impl AsRef<str>) -> JobError {
    ManufacturingError::new(ManufacturingErrorCode::UnsupportedGeometry, message).into()
}
#[derive(Clone, Copy, Debug)]
struct Rect {
    min: [f64; 2],
    max: [f64; 2],
}
impl Rect {
    fn from_bounds(b: AabbMm, padding: f64) -> Self {
        Self {
            min: [b.min[0] - padding, b.min[1] - padding],
            max: [b.max[0] + padding, b.max[1] + padding],
        }
    }
    fn from_axis_bounds(b: AabbMm, padding: [f64; 2]) -> Self {
        Self {
            min: std::array::from_fn(|a| b.min[a] - padding[a]),
            max: std::array::from_fn(|a| b.max[a] + padding[a]),
        }
    }
    fn overlaps(self, b: Self) -> bool {
        (0..2).all(|a| self.min[a] < b.max[a] && b.min[a] < self.max[a])
    }
    fn inside(self, b: Self) -> bool {
        (0..2).all(|a| self.min[a] >= b.min[a] && self.max[a] <= b.max[a])
    }
    fn translated(self, d: [f64; 2]) -> Self {
        Self {
            min: std::array::from_fn(|a| self.min[a] + d[a]),
            max: std::array::from_fn(|a| self.max[a] + d[a]),
        }
    }
}
fn input<'a>(
    capture: &'a CompileRequest,
    definitions: &'a BTreeMap<DefinitionId, Arc<spiling_geometry::Definition>>,
) -> spiling_manufacturing::CompilerInput<'a> {
    spiling_manufacturing::CompilerInput {
        project_id: &capture.project_id,
        revision: capture.revision,
        definitions,
        stored_definitions: &capture.stored_definitions,
        occurrences: &capture.occurrences,
        intent: &capture.intent,
        surface_overlays: &capture.surface_overlays,
        process_document: &capture.process_document,
        surface_images: &capture.surface_images,
    }
}
fn candidate(
    shape: Rect,
    bed: Rect,
    occupied: &[Rect],
    style: LayoutStyle,
    work: &mut u32,
    budget: u32,
    cancel: &AtomicBool,
) -> Result<Option<Rect>, JobError> {
    let size: [f64; 2] = std::array::from_fn(|a| shape.max[a] - shape.min[a]);
    let mut axes: [Vec<f64>; 2] = std::array::from_fn(|a| {
        vec![
            bed.min[a],
            bed.max[a] - size[a],
            (bed.min[a] + bed.max[a] - size[a]) * 0.5,
        ]
    });
    for b in occupied {
        for a in 0..2 {
            axes[a].extend([b.max[a], b.min[a] - size[a]]);
        }
    }
    for axis in &mut axes {
        axis.sort_by(f64::total_cmp);
        axis.dedup();
    }
    let mut best: Option<(f64, Rect)> = None;
    for y in &axes[1] {
        for x in &axes[0] {
            if *work >= budget {
                return Ok(None);
            }
            *work += 1;
            super::check_cancel(cancel)?;
            let b = Rect {
                min: [*x, *y],
                max: [x + size[0], y + size[1]],
            };
            if !b.inside(bed) || occupied.iter().any(|o| b.overlaps(*o)) {
                continue;
            }
            let score = match style {
                LayoutStyle::Dense => {
                    (b.min[1] - bed.min[1]) * (bed.max[0] - bed.min[0] + 1.0) + b.min[0]
                        - bed.min[0]
                }
                LayoutStyle::Balanced => (0..2)
                    .map(|a| ((b.min[a] + b.max[a] - bed.min[a] - bed.max[a]) * 0.5).powi(2))
                    .sum(),
            };
            if best.as_ref().is_none_or(|(s, _)| score < *s) {
                best = Some((score, b));
            }
        }
    }
    Ok(best.map(|(_, b)| b))
}

fn prepare_single(
    mut r: Request,
    definitions: &BTreeMap<DefinitionId, Arc<spiling_geometry::Definition>>,
    cancel: &AtomicBool,
    progress: &dyn Fn(String),
    budget: u32,
) -> Result<LayoutProposal, JobError> {
    r.request.validate()?;
    // Worker residency includes other plates and history. Every independent trial
    // must use the same captured source closure as ordinary compilation.
    let scoped = r
        .capture
        .definition_ids
        .iter()
        .map(|id| {
            definitions
                .get(id)
                .map(|definition| (id.clone(), definition.clone()))
                .ok_or_else(|| {
                    ManufacturingError::new(
                        ManufacturingErrorCode::StaleRevision,
                        "layout definition was reclaimed",
                    )
                })
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let definitions = &scoped;
    let current = r.draft.snapshot();
    if current.occurrences.is_empty() {
        return Err(fail("Import a part before arranging"));
    }
    if r.request
        .pinned
        .iter()
        .any(|id| !current.occurrences.contains_key(id))
        || r.request
            .template
            .is_some_and(|id| !current.occurrences.contains_key(&id))
    {
        return Err(fail("Layout contains an unknown template or pinned part"));
    }
    progress("layout_preparing_native_footprints".into());
    let bounds =
        spiling_manufacturing::planner::layout_footprints(&input(&r.capture, definitions), cancel)?;
    let mut bounds: BTreeMap<_, _> = bounds.into_iter().collect();
    let intent = &r.capture.intent;
    let mut bed_bounds = intent.printer.build_envelope;
    if let Some(h) = &intent.hardware {
        let usable = h.selected_face().map_err(fail)?.usable_mm;
        for a in 0..2 {
            bed_bounds.min[a] = bed_bounds.min[a].max(usable.min[a]);
            bed_bounds.max[a] = bed_bounds.max[a].min(usable.max[a]);
        }
    }
    let bed = Rect::from_bounds(bed_bounds, 0.0);
    // Reserve room beyond the full native/finish envelope. The final complete
    // deposition check below independently refuses constructions escaping it.
    let construction_margin = intent.recipe.bead_width_mm
        + match intent.recipe.supports {
            SupportPolicy::Disabled => 0.0,
            SupportPolicy::Breakaway {
                xy_clearance_mm, ..
            } => xy_clearance_mm,
        };
    let padding = construction_margin + r.request.spacing_mm * 0.5;
    let mut axis_padding = [padding; 2];
    let mut obstacles = vec![];
    if let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow {
        if r.request.use_additional_plates {
            return Err(fail("Fixed-dual copies currently stay on the active plate"));
        }
        // Pairwise packing reserves half the maximum captured relative reach
        // on each part. Their separation therefore covers the complete reach.
        // These rectangles only propose a layout; actual route replay remains mandatory.
        for (axis, margin) in axis_padding.iter_mut().enumerate() {
            let mut reach = 0.0_f64;
            for tool in &job.tools {
                reach = reach
                    .max((job.carriage_body_bounds_mm.min[axis] - tool.offset_xy_mm[axis]).abs())
                    .max((job.carriage_body_bounds_mm.max[axis] - tool.offset_xy_mm[axis]).abs());
                for other in &job.tools {
                    reach = reach.max(
                        (other.offset_xy_mm[axis] - tool.offset_xy_mm[axis]).abs()
                            + job.inactive_nozzle_radius_mm,
                    );
                }
            }
            *margin += reach.next_up() * 0.5;
        }
        for assignment in &job.assignments {
            if let Some(b) = bounds.get_mut(&assignment.occurrence_id) {
                for axis in 0..2 {
                    b.min[axis] = b.min[axis].min(assignment.work_envelope_mm.min[axis]);
                    b.max[axis] = b.max[axis].max(assignment.work_envelope_mm.max[axis]);
                }
            }
        }
        for tool in &job.tools {
            obstacles.push(Rect {
                min: std::array::from_fn(|a| {
                    tool.waste_line_xy_mm[0][a].min(tool.waste_line_xy_mm[1][a])
                        - tool.waste_bead_width_mm * 0.5
                        - axis_padding[a]
                }),
                max: std::array::from_fn(|a| {
                    tool.waste_line_xy_mm[0][a].max(tool.waste_line_xy_mm[1][a])
                        + tool.waste_bead_width_mm * 0.5
                        + axis_padding[a]
                }),
            });
        }
    }
    if let Some(h) = &intent.hardware {
        obstacles.extend(
            h.selected_face()
                .map_err(fail)?
                .keepouts_mm
                .iter()
                .map(|b| Rect::from_bounds(*b, 0.0)),
        );
        for accessory in &h.accessories {
            if let spiling_contracts::hardware::AccessoryEffect::Fixture { bounds_mm } =
                accessory.effect
            {
                obstacles.push(Rect::from_bounds(bounds_mm, 0.0));
            }
        }
    }
    for c in &intent.printer.components {
        if c.role == MachineComponentRole::Fixture {
            let local = match c.shape {
                MachineComponentShape::Box { bounds_mm } => bounds_mm,
                MachineComponentShape::Cylinder {
                    radius_mm,
                    height_mm,
                } => AabbMm {
                    min: [-radius_mm, -radius_mm, 0.0],
                    max: [radius_mm, radius_mm, height_mm],
                },
            };
            obstacles.push(Rect::from_bounds(
                super::placement::scene_bounds(c.pose, local)?,
                0.0,
            ));
        }
    }
    if let JobWorkflow::CompleteMarlin { job } = &intent.printer.workflow {
        obstacles.push(Rect {
            min: std::array::from_fn(|a| {
                job.prime_start_mm[a].min(job.prime_end_mm[a]) - intent.recipe.bead_width_mm
            }),
            max: std::array::from_fn(|a| {
                job.prime_start_mm[a].max(job.prime_end_mm[a]) + intent.recipe.bead_width_mm
            }),
        });
    }
    let mut footprints = vec![];
    let mut placements = vec![];
    let mut occupied = obstacles.clone();
    for id in &r.request.pinned {
        let b = bounds[id];
        let rect = Rect::from_axis_bounds(b, axis_padding);
        if !rect.inside(bed) || occupied.iter().any(|o| rect.overlaps(*o)) {
            return Err(fail(
                "A pinned part conflicts with the build area, another pin or a reserved print structure",
            ));
        }
        occupied.push(rect);
        footprints.push(b);
    }
    let desired = if r.request.objective == LayoutObjective::Arrange {
        current.occurrences.len() as u32
    } else {
        (current.occurrences.len() as u32)
            .checked_add(r.request.quantity - 1)
            .filter(|n| *n <= MAX_OCCURRENCES)
            .ok_or_else(|| fail("Requested quantity exceeds the project occurrence budget"))?
    };
    let mut sources: Vec<_> = current
        .occurrences
        .keys()
        .filter(|id| !r.request.pinned.contains(id))
        .map(|id| (*id, false))
        .collect();
    sources.sort_by(|(a, _), (b, _)| {
        let area = |id| {
            let b: AabbMm = bounds[id];
            (b.max[0] - b.min[0]) * (b.max[1] - b.min[1])
        };
        area(b).total_cmp(&area(a)).then(a.cmp(b))
    });
    let originals = sources.len();
    if let Some(id) = r.request.template {
        sources.extend(std::iter::repeat_n(
            (id, true),
            (r.request.quantity - 1) as usize,
        ));
    }
    let route_count_limit =
        if matches!(intent.printer.workflow, JobWorkflow::FixedDualMarlin { .. }) {
            MAX_FIXED_DUAL_OCCURRENCES
        } else {
            MAX_OCCURRENCES as usize
        };
    let mut work = 0;
    for (index, (id, copy)) in sources.into_iter().enumerate() {
        if footprints.len() >= route_count_limit {
            break;
        }
        progress(format!(
            "layout_placing_{}_of_{desired}",
            index + r.request.pinned.len() + 1
        ));
        let b = bounds[&id];
        let shape = Rect::from_axis_bounds(b, axis_padding);
        let Some(placed) = candidate(
            shape,
            bed,
            &occupied,
            r.request.style,
            &mut work,
            budget,
            cancel,
        )?
        else {
            if index < originals {
                return Err(fail(
                    "Existing parts cannot fit with the pinned parts and reservations; Current was preserved",
                ));
            }
            break;
        };
        let shift = std::array::from_fn::<_, 2, _>(|a| placed.min[a] - shape.min[a]);
        let mut pose = current.occurrences[&id].pose;
        for (a, d) in shift.iter().enumerate() {
            pose.translation_mm[a] += d;
        }
        let mut footprint = b;
        for (a, d) in shift.iter().enumerate() {
            footprint.min[a] += d;
            footprint.max[a] += d;
        }
        occupied.push(shape.translated(shift));
        footprints.push(footprint);
        placements.push(LayoutPlacement {
            source: id,
            copy,
            pose,
        });
    }
    // Center the whole group for the balanced objective, preserving reservations.
    // This makes two copies balanced about the plate rather than the first copy.
    if r.request.style == LayoutStyle::Balanced && r.request.pinned.is_empty() {
        let group = footprints.iter().fold(
            Rect {
                min: [f64::INFINITY; 2],
                max: [f64::NEG_INFINITY; 2],
            },
            |mut g, b| {
                let b = Rect::from_axis_bounds(*b, axis_padding);
                for a in 0..2 {
                    g.min[a] = g.min[a].min(b.min[a]);
                    g.max[a] = g.max[a].max(b.max[a]);
                }
                g
            },
        );
        let shift = std::array::from_fn::<_, 2, _>(|a| {
            (bed.min[a] + bed.max[a] - group.min[a] - group.max[a]) * 0.5
        });
        if footprints.iter().all(|b| {
            let p = Rect::from_axis_bounds(*b, axis_padding).translated(shift);
            p.inside(bed) && !obstacles.iter().any(|o| p.overlaps(*o))
        }) {
            for p in &mut placements {
                for (a, d) in shift.iter().enumerate() {
                    p.pose.translation_mm[a] += d;
                }
            }
            for b in &mut footprints {
                for (a, d) in shift.iter().enumerate() {
                    b.min[a] += d;
                    b.max[a] += d;
                }
            }
        }
    }
    let mut limited_by = if footprints.len() as u32 == desired {
        LayoutLimit::RequestedQuantity
    } else if footprints.len() >= route_count_limit {
        LayoutLimit::OccurrenceBudget
    } else if work >= budget {
        LayoutLimit::SearchBudget
    } else {
        LayoutLimit::BuildArea
    };
    let mut manufacturing_attempts = 0;
    let bundle = loop {
        manufacturing_attempts += 1;
        progress(format!(
            "layout_verifying_{}_parts_attempt_{manufacturing_attempts}",
            footprints.len()
        ));
        let attempt = (|| -> Result<_, JobError> {
            let trial = r.draft.preview(&placements)?;
            r.capture.occurrences = trial.occurrences.values().cloned().collect();
            r.capture.intent = trial
                .manufacturing_intent
                .clone()
                .ok_or_else(|| fail("Layout trial lost captured manufacturing intent"))?;
            let (surface, process) = trial.scoped_documents();
            r.capture
                .surface_images
                .retain(|hash, _| surface.images.iter().any(|r| &r.image_hash == hash));
            r.capture.surface_overlays = Arc::new(surface);
            r.capture.process_document = Arc::new(process);
            Ok(spiling_manufacturing::compile(
                input(&r.capture, definitions),
                cancel,
            )?)
        })();
        match attempt {
            Ok(bundle) => break bundle,
            Err(error) => {
                let resource = matches!(&error,JobError::Manufacturing { error } if error.code == ManufacturingErrorCode::ResourceLimit)
                    || matches!(&error,JobError::Project { error } if error.code == spiling_contracts::project::ProjectErrorCode::ResourceLimit);
                let copies = placements.iter().filter(|p| p.copy).count();
                if !resource || copies == 0 || manufacturing_attempts == 4 {
                    return Err(error);
                }
                // Preserve every existing occurrence. A bounded best-found result may
                // reduce only requested new copies when full manufacturing exhausts work.
                let keep = if manufacturing_attempts == 3 {
                    0
                } else {
                    copies / 2
                };
                let mut seen = 0;
                let pins = r.request.pinned.len();
                let mut kept_footprints = footprints[..pins].to_vec();
                let mut kept_placements = vec![];
                for (p, b) in placements
                    .into_iter()
                    .zip(footprints.into_iter().skip(pins))
                {
                    if !p.copy || seen < keep {
                        kept_placements.push(p.clone());
                        kept_footprints.push(b);
                    }
                    if p.copy {
                        seen += 1;
                    }
                }
                placements = kept_placements;
                footprints = kept_footprints;
                limited_by = LayoutLimit::ManufacturingBudget;
            }
        }
    };
    // Every whole actual nominal bead must remain in one disjoint construction
    // reservation. This catches support/skin/bridge growth omitted by a heuristic.
    for layer in &bundle.plan.layers {
        for path in &layer.paths {
            let edges = path.checked_edges()?;
            for edge in edges {
                let radius = edge.width_mm * 0.5;
                let fits = footprints.iter().any(|b| {
                    (0..2).all(|a| {
                        edge.points_mm[0][a].min(edge.points_mm[1][a]) - radius
                            >= b.min[a] - construction_margin
                            && edge.points_mm[0][a].max(edge.points_mm[1][a]) + radius
                                <= b.max[a] + construction_margin
                    })
                });
                if !fits {
                    return Err(fail(
                        "Generated construction exceeds its packing reservation; increase spacing or arrange fewer parts",
                    ));
                }
            }
        }
    }
    let placed_count = r.capture.occurrences.len() as u32;
    let id = SourceHash::from_bytes(
        &serde_json::to_vec(&(
            r.capture.project_id.clone(),
            r.capture.revision,
            r.scene,
            &r.request,
            &placements,
        ))
        .map_err(|_| fail("Layout identity encoding failed"))?,
    );
    let proposal = LayoutProposal {
        id,
        session_id: r.session,
        project_id: r.capture.project_id,
        project_revision: r.capture.revision,
        scene_revision: r.scene,
        request: r.request,
        placements,
        additional_plates: vec![],
        footprints_mm: footprints,
        requested_count: desired,
        placed_count,
        candidates_examined: work,
        manufacturing_attempts,
        limited_by,
        verification: bundle.verification,
    };
    proposal.validate()?;
    Ok(proposal)
}

/// The same bounded search and independent replay for each proposed job, under
/// one aggregate candidate budget. Nothing here changes the authoritative project.
pub(super) fn prepare(
    mut r: Request,
    definitions: &BTreeMap<DefinitionId, Arc<spiling_geometry::Definition>>,
    cancel: &AtomicBool,
    progress: &dyn Fn(String),
) -> Result<LayoutProposal, JobError> {
    r.request.validate()?;
    let request = r.request.clone();
    let original_count = r.draft.snapshot().occurrences.len() as u32;
    let other_count = r.draft.snapshot().all_occurrences().count() as u32 - original_count;
    let available = MAX_OCCURRENCES
        .saturating_sub(other_count + original_count)
        .saturating_add(1);
    if original_count == 0 {
        return Err(fail("Import a part before arranging"));
    }
    let wanted = if request.objective == LayoutObjective::Arrange {
        original_count
    } else {
        original_count + request.quantity - 1
    };
    // The quantity remains explicit in the returned request; this ceiling bounds trials.
    if request.objective != LayoutObjective::Arrange {
        r.request.quantity = request.quantity.min(available);
    }
    if wanted > MAX_OCCURRENCES {
        return Err(fail(
            "Requested total exceeds the project occurrence budget",
        ));
    }
    let extra_slots =
        spiling_contracts::project::MAX_PRINT_LAYOUTS - r.draft.snapshot().layouts.len();
    let template_draft = if request.use_additional_plates {
        Some(
            r.draft.template_only(
                request
                    .template
                    .ok_or_else(|| fail("Choose a copy template"))?,
            )?,
        )
    } else {
        None
    };
    let capture = r.capture.clone();
    let session = r.session.clone();
    let scene = r.scene;
    let mut result = prepare_single(r, definitions, cancel, progress, 2_000_000)?;
    let mut work = result.candidates_examined;
    let target = wanted.min(MAX_OCCURRENCES - other_count);
    if let Some(draft) = template_draft {
        while result.total_placed() < target
            && result.additional_plates.len() < extra_slots
            && work < 2_000_000
        {
            super::check_cancel(cancel)?;
            progress(format!(
                "layout_additional_plate_{}",
                result.additional_plates.len() + 1
            ));
            let remaining = target - result.total_placed();
            let mut extra_request = request.clone();
            extra_request.quantity = remaining;
            extra_request.use_additional_plates = false;
            extra_request.pinned.clear();
            let mut extra_capture = capture.clone();
            let snapshot = draft.snapshot();
            extra_capture.occurrences = snapshot.occurrences.values().cloned().collect();
            let (surface, process) = snapshot.scoped_documents();
            let used: std::collections::BTreeSet<_> = extra_capture
                .occurrences
                .iter()
                .map(|o| &o.definition_id)
                .collect();
            extra_capture
                .stored_definitions
                .retain(|d| used.contains(&d.definition_id));
            extra_capture.definition_ids.retain(|id| used.contains(id));
            extra_capture
                .surface_images
                .retain(|hash, _| surface.images.iter().any(|r| &r.image_hash == hash));
            extra_capture.surface_overlays = Arc::new(surface);
            extra_capture.process_document = Arc::new(process);
            let extra = prepare_single(
                Request {
                    session: session.clone(),
                    scene,
                    request: extra_request,
                    capture: extra_capture,
                    draft: draft.clone(),
                },
                definitions,
                cancel,
                progress,
                2_000_000 - work,
            )?;
            work += extra.candidates_examined;
            result.additional_plates.push(AdditionalPlateProposal {
                placements: extra
                    .placements
                    .into_iter()
                    .map(|mut p| {
                        p.copy = true;
                        p
                    })
                    .collect(),
                footprints_mm: extra.footprints_mm,
                candidates_examined: extra.candidates_examined,
                manufacturing_attempts: extra.manufacturing_attempts,
                verification: extra.verification,
            });
        }
    }
    result.request = request;
    result.requested_count = wanted;
    result.limited_by = if result.total_placed() == wanted {
        LayoutLimit::RequestedQuantity
    } else if result.total_placed() == target && wanted > target {
        LayoutLimit::OccurrenceBudget
    } else if work >= 2_000_000 {
        LayoutLimit::SearchBudget
    } else if result.request.use_additional_plates && result.additional_plates.len() == extra_slots
    {
        LayoutLimit::PlateBudget
    } else {
        result.limited_by
    };
    result.id = SourceHash::from_bytes(
        &serde_json::to_vec(&(&result.id, &result.request, &result.additional_plates))
            .map_err(|_| fail("Layout identity encoding failed"))?,
    );
    result.validate()?;
    Ok(result)
}
