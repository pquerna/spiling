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
    definitions: &'a BTreeMap<DefinitionId, Arc<spiling_geometry::source::SourceDefinition>>,
    working_set: spiling_manufacturing::WorkingSet,
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
        working_set,
    }
}
/// Occupied reservations ordered by their lower Y edge, so each candidate row
/// examines only the reservations that can intersect it.
struct Occupied {
    rects: Vec<Rect>,
    tallest: f64,
}
impl Occupied {
    fn new(rects: &[Rect]) -> Self {
        let mut occupied = Self {
            rects: Vec::with_capacity(rects.len()),
            tallest: 0.0,
        };
        for rect in rects {
            occupied.push(*rect);
        }
        occupied
    }
    fn push(&mut self, rect: Rect) {
        let at = self
            .rects
            .partition_point(|o| o.min[1].total_cmp(&rect.min[1]).is_le());
        self.rects.insert(at, rect);
        self.tallest = self.tallest.max(rect.max[1] - rect.min[1]);
    }
    /// Reservations whose Y interval meets the open row `(low, high)`; the
    /// same predicate as [`Rect::overlaps`], so no other one can overlap it.
    fn row(&self, low: f64, high: f64) -> impl Iterator<Item = &Rect> {
        let first = self
            .rects
            .partition_point(|o| o.min[1] < low - self.tallest - 1.0);
        self.rects[first..]
            .iter()
            .take_while(move |o| o.min[1] < high)
            .filter(move |o| low < o.max[1])
    }
}

/// Best free position for `shape` among the bed, centre and reservation-edge
/// coordinates, by the style's score with ties to the lower row, then the
/// lower X. Every position on the finite edge set is decided: a candidate row
/// is checked by an exact sweep of the reservations meeting it, so the search
/// either returns the best placement or proves none fits. `work` counts
/// examined rows for cancellation and reporting; it never stops the search.
fn candidate(
    shape: Rect,
    bed: Rect,
    occupied: &Occupied,
    style: LayoutStyle,
    lowest_row: f64,
    work: &mut u32,
    cancel: &AtomicBool,
) -> Result<Option<Rect>, JobError> {
    let size: [f64; 2] = std::array::from_fn(|a| shape.max[a] - shape.min[a]);
    let mut rows = vec![
        bed.min[1],
        bed.max[1] - size[1],
        (bed.min[1] + bed.max[1] - size[1]) * 0.5,
    ];
    for b in &occupied.rects {
        rows.extend([b.max[1], b.min[1] - size[1]]);
    }
    rows.sort_by(f64::total_cmp);
    rows.dedup();
    let score = |b: &Rect| match style {
        LayoutStyle::Dense => {
            (b.min[1] - bed.min[1]) * (bed.max[0] - bed.min[0] + 1.0) + b.min[0] - bed.min[0]
        }
        LayoutStyle::Balanced => (0..2)
            .map(|a| ((b.min[a] + b.max[a] - bed.min[a] - bed.max[a]) * 0.5).powi(2))
            .sum(),
    };
    // Dense scores order rows first, so the lowest feasible row is best.
    // Balanced rows are visited outward from the centre and stop once a row's
    // own centre distance already exceeds the best score.
    let row_bound = |y: f64| ((2.0 * y + size[1] - bed.min[1] - bed.max[1]) * 0.5).powi(2);
    match style {
        LayoutStyle::Dense => rows.retain(|y| *y >= lowest_row),
        LayoutStyle::Balanced => {
            rows.sort_by(|a, b| row_bound(*a).total_cmp(&row_bound(*b)).then(a.total_cmp(b)))
        }
    }
    let mut best: Option<(f64, Rect)> = None;
    for y in rows {
        if style == LayoutStyle::Balanced && best.as_ref().is_some_and(|(s, _)| row_bound(y) > *s) {
            break;
        }
        *work = work.saturating_add(1);
        super::check_cancel(cancel)?;
        if let Some(b) = best_in_row(size, bed, occupied, y, &score)
            && best.as_ref().is_none_or(|(s, p)| {
                let candidate = score(&b);
                candidate < *s || (candidate == *s && b.min[1] < p.min[1])
            })
        {
            best = Some((score(&b), b));
            if style == LayoutStyle::Dense {
                break;
            }
        }
    }
    Ok(best.map(|(_, b)| b))
}

/// The best exactly free X position in one row, or `None` if the row is full.
fn best_in_row(
    size: [f64; 2],
    bed: Rect,
    occupied: &Occupied,
    y: f64,
    score: &dyn Fn(&Rect) -> f64,
) -> Option<Rect> {
    let place = |x: f64| Rect {
        min: [x, y],
        max: [x + size[0], y + size[1]],
    };
    if !place(bed.min[0]).inside(bed) && !place(bed.max[0] - size[0]).inside(bed) {
        return None;
    }
    let row: Vec<Rect> = occupied.row(y, y + size[1]).copied().collect();
    let mut xs = vec![
        bed.min[0],
        bed.max[0] - size[0],
        (bed.min[0] + bed.max[0] - size[0]) * 0.5,
    ];
    for o in &row {
        xs.extend([o.max[0], o.min[0] - size[0]]);
    }
    xs.sort_by(f64::total_cmp);
    xs.dedup();
    // Sweep blocked X intervals in order of their left edge; a candidate is
    // free when no reservation starting at or before it still covers it.
    let mut blocks: Vec<Rect> = row;
    blocks.sort_by(|a, b| a.min[0].total_cmp(&b.min[0]));
    let mut best: Option<(f64, Rect)> = None;
    let mut next = 0;
    let mut active: std::collections::BinaryHeap<std::cmp::Reverse<OrderedMax>> =
        std::collections::BinaryHeap::new();
    for x in xs {
        let b = place(x);
        if !b.inside(bed) {
            continue;
        }
        while next < blocks.len() && blocks[next].min[0] < b.max[0] {
            active.push(std::cmp::Reverse(OrderedMax(blocks[next])));
            next += 1;
        }
        while active
            .peek()
            .is_some_and(|std::cmp::Reverse(o)| o.0.max[0] <= b.min[0])
        {
            active.pop();
        }
        // Every remaining block starts before b ends, ends after b starts and
        // meets b's row: exactly the reservations b overlaps.
        if !active.is_empty() {
            continue;
        }
        let s = score(&b);
        if best.as_ref().is_none_or(|(best, _)| s < *best) {
            best = Some((s, b));
        }
    }
    best.map(|(_, b)| b)
}
/// Orders blocked intervals by right edge for the row sweep.
#[derive(Clone, Copy)]
struct OrderedMax(Rect);
impl PartialEq for OrderedMax {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for OrderedMax {}
impl PartialOrd for OrderedMax {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for OrderedMax {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.max[0].total_cmp(&other.0.max[0])
    }
}

fn prepare_single(
    mut r: Request,
    definitions: &BTreeMap<DefinitionId, Arc<spiling_geometry::source::SourceDefinition>>,
    cancel: &AtomicBool,
    progress: &dyn Fn(String),
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
    let bounds = spiling_manufacturing::planner::layout_footprints(
        &input(
            &r.capture,
            definitions,
            crate::resources::manufacturing_working_set()?,
        ),
        cancel,
    )?;
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
            .ok_or_else(|| fail("Requested quantity exceeds the u32 occurrence width"))?
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
    // Copies are generated as they are placed: a fill request names how many
    // copies may be added, not an allocation.
    let copies = r
        .request
        .template
        .into_iter()
        .flat_map(|id| std::iter::repeat_n((id, true), (r.request.quantity - 1) as usize));
    let mut work = 0;
    let mut occupied = Occupied::new(&occupied);
    // Identical shapes never fit lower than the previous one did: reservations
    // only grow, so its row bounds the dense search for the next copy.
    let mut previous: Option<([f64; 2], f64)> = None;
    for (index, (id, copy)) in sources.into_iter().chain(copies).enumerate() {
        progress(format!(
            "layout_placing_{}_of_{desired}",
            index + r.request.pinned.len() + 1
        ));
        let b = bounds[&id];
        let shape = Rect::from_axis_bounds(b, axis_padding);
        let size = std::array::from_fn(|a| shape.max[a] - shape.min[a]);
        let lowest_row = previous
            .filter(|(s, _)| *s == size)
            .map_or(f64::NEG_INFINITY, |(_, y)| y);
        let Some(placed) = candidate(
            shape,
            bed,
            &occupied,
            r.request.style,
            lowest_row,
            &mut work,
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
        previous = Some((size, placed.min[1]));
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
    } else {
        // The search decided every candidate position: the rest do not fit.
        LayoutLimit::BuildArea
    };
    let pins = r.request.pinned.len();
    // The first `copies` placed copies (in placement order) with every original.
    let subset = |placements: &[LayoutPlacement], footprints: &[AabbMm], copies: usize| {
        let mut kept_footprints = footprints[..pins].to_vec();
        let mut kept_placements = vec![];
        let mut seen = 0;
        for (p, b) in placements.iter().zip(&footprints[pins..]) {
            if !p.copy || seen < copies {
                kept_placements.push(p.clone());
                kept_footprints.push(*b);
            }
            if p.copy {
                seen += 1;
            }
        }
        (kept_placements, kept_footprints)
    };
    let stage = |placements: &[LayoutPlacement], r: &mut Request| -> Result<(), JobError> {
        let trial = r.draft.preview(placements)?;
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
        Ok(())
    };
    let mut manufacturing_attempts = 0u32;
    let mut compile = |placements: &[LayoutPlacement], r: &mut Request| -> Result<_, JobError> {
        manufacturing_attempts += 1;
        progress(format!(
            "layout_verifying_{}_parts_attempt_{manufacturing_attempts}",
            placements.len() + pins
        ));
        stage(placements, r)?;
        Ok(spiling_manufacturing::compile(
            input(
                &r.capture,
                definitions,
                crate::resources::manufacturing_working_set()?,
            ),
            cancel,
        )?)
    };
    let resource_refusal = |error: &JobError| {
        matches!(error, JobError::Manufacturing { error } if error.code == ManufacturingErrorCode::ResourceLimit)
            || matches!(error, JobError::Project { error } if error.code == spiling_contracts::project::ProjectErrorCode::ResourceLimit)
    };
    let mut manufacturing_refusal = None;
    let bundle = match compile(&placements, &mut r) {
        Ok(bundle) => bundle,
        Err(error) => {
            let copies = placements.iter().filter(|p| p.copy).count();
            // A plate of copies only (an additional plate) keeps at least one.
            let floor = usize::from(placements.len() == copies && pins == 0);
            if !resource_refusal(&error) || copies <= floor {
                return Err(error);
            }
            // Exact boundary, never a blind cutback: `kept` copies compile and
            // `refused` copies are refused by this machine's working set. Fewer
            // placed copies never need more manufacturing resources, so this
            // keeps the most copies a full compile admits.
            let mut refused = (copies, error);
            let (floor_placements, _) = subset(&placements, &footprints, floor);
            let mut kept = (floor, compile(&floor_placements, &mut r)?);
            while refused.0 - kept.0 > 1 {
                super::check_cancel(cancel)?;
                let trial = kept.0 + (refused.0 - kept.0) / 2;
                let (trial_placements, _) = subset(&placements, &footprints, trial);
                match compile(&trial_placements, &mut r) {
                    Ok(bundle) => kept = (trial, bundle),
                    Err(error) if resource_refusal(&error) => refused = (trial, error),
                    Err(error) => return Err(error),
                }
            }
            let (kept_copies, bundle) = kept;
            let (kept_placements, kept_footprints) = subset(&placements, &footprints, kept_copies);
            stage(&kept_placements, &mut r)?;
            let reason = match &refused.1 {
                JobError::Manufacturing { error } => error.message.clone(),
                JobError::Project { error } => error.message.clone(),
                JobError::Geometry { error } => error.message.clone(),
            };
            let mut reason = reason;
            if reason.len() > MAX_LAYOUT_REFUSAL_BYTES / 2 {
                let mut end = MAX_LAYOUT_REFUSAL_BYTES / 2;
                while !reason.is_char_boundary(end) {
                    end -= 1;
                }
                reason.truncate(end);
            }
            manufacturing_refusal = Some(format!(
                "copies {} to {copies} were dropped: {kept_copies} of {copies} placed copies compile within this machine's manufacturing working set, and {} copies are refused: {reason}",
                kept_copies + 1,
                refused.0,
            ));
            placements = kept_placements;
            footprints = kept_footprints;
            limited_by = LayoutLimit::ManufacturingBudget;
            bundle
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
        manufacturing_refusal,
        verification: bundle.bundle.verification,
    };
    proposal.validate()?;
    Ok(proposal)
}

/// The same exhaustive edge-position search and independent replay for each
/// proposed job. Nothing here changes the authoritative project.
pub(super) fn prepare(
    mut r: Request,
    definitions: &BTreeMap<DefinitionId, Arc<spiling_geometry::source::SourceDefinition>>,
    cancel: &AtomicBool,
    progress: &dyn Fn(String),
) -> Result<LayoutProposal, JobError> {
    r.request.validate()?;
    let request = r.request.clone();
    let original_count = r.draft.snapshot().occurrences.len() as u32;
    let other_count = r.draft.snapshot().all_occurrences().count() as u32 - original_count;
    let available = u32::MAX
        .saturating_sub(other_count + original_count)
        .saturating_add(1);
    if original_count == 0 {
        return Err(fail("Import a part before arranging"));
    }
    let wanted = if request.objective == LayoutObjective::Arrange {
        original_count
    } else {
        original_count
            .checked_add(request.quantity - 1)
            .ok_or_else(|| fail("Requested total exceeds the u32 occurrence width"))?
    };
    // The quantity remains explicit in the returned request; this ceiling bounds trials.
    if request.objective != LayoutObjective::Arrange {
        r.request.quantity = request.quantity.min(available);
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
    let mut result = prepare_single(r, definitions, cancel, progress)?;
    let target = wanted.min(u32::MAX - other_count);
    if let Some(draft) = template_draft {
        while result.total_placed() < target && result.additional_plates.len() < extra_slots {
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
            )?;
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
    } else if result.request.use_additional_plates && result.additional_plates.len() == extra_slots
    {
        LayoutLimit::PlateBudget
    } else {
        result.limited_by
    };
    if result.limited_by != LayoutLimit::ManufacturingBudget {
        // Every copy the first plate refused was placed elsewhere or the
        // final limit names the real shortfall.
        result.manufacturing_refusal = None;
    }
    result.id = SourceHash::from_bytes(
        &serde_json::to_vec(&(&result.id, &result.request, &result.additional_plates))
            .map_err(|_| fail("Layout identity encoding failed"))?,
    );
    result.validate()?;
    Ok(result)
}
