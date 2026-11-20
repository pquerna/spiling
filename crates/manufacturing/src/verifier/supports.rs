// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{cancelled, failed};
use crate::clearance_screen::{BLOCK_PATHS, ObjectClearanceIndex, bounded_height};
use crate::planner::support::{
    AnchorCandidate, AnchorWorkspace, GapKind, ProjectedAnchorPaths, StrokeRuns, gap_kind,
};
use rayon::prelude::*;
use spiling_contracts::manufacturing::{program_coordinates::*, *};
use std::sync::atomic::AtomicBool;

fn charge(work: &mut usize, cancel: &AtomicBool) -> Result<(), ManufacturingError> {
    cancelled(cancel)?;
    *work = work.checked_add(1).ok_or_else(|| {
        ManufacturingError::new(
            ManufacturingErrorCode::ResourceLimit,
            "support replay work overflow",
        )
    })?;
    Ok(())
}

fn build_object_clearance_index(
    layers: &[PrintLayer],
    live: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<ObjectClearanceIndex>, ManufacturingError> {
    ObjectClearanceIndex::build(
        layers,
        crate::working_set::bytes().saturating_sub(live),
        DepositionKind::is_model,
        || charge(work, cancel),
    )
}

fn support(path: &DepositionPath) -> bool {
    matches!(
        path.kind,
        DepositionKind::Support | DepositionKind::SupportInterface
    )
}

fn height(path: &DepositionPath) -> Result<[f64; 2], ManufacturingError> {
    project_xyz6_bead_height_mm(path.physical_z_mm[1], path.height_mm())
}

#[expect(
    clippy::too_many_arguments,
    reason = "Optional scope-local metadata preserves the supplied previous and contact iterators"
)]
fn anchored<'a>(
    path: &DepositionPath,
    previous: impl Iterator<Item = &'a DepositionPath> + Clone,
    contacts: impl Iterator<Item = &'a DepositionPath> + Clone,
    prepared: Option<(&ProjectedAnchorPaths<'a>, bool)>,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    for (first, last, width) in StrokeRuns::for_path(path)? {
        charge(work, cancel)?;
        let endpoints = [path.points_mm[first], path.points_mm[last]];
        let projected = [
            project_xyz6_point_mm(endpoints[0])?,
            project_xyz6_point_mm(endpoints[1])?,
        ];
        let length = (projected[1][0] - projected[0][0]).hypot(projected[1][1] - projected[0][1]);
        let candidate = AnchorCandidate {
            endpoints_mm: projected,
            bead_width_mm: width,
        };
        let gaps = if let Some((prepared, with_contacts)) = prepared {
            anchors.gaps_reused_projected(
                candidate,
                path.physical_z_mm[0],
                with_contacts,
                prepared,
                work,
                cancel,
            )?
        } else {
            anchors.gaps_projected(
                candidate,
                previous
                    .clone()
                    .filter(|lower| lower.physical_z_mm[1] == path.physical_z_mm[0]),
                contacts.clone(),
                work,
                cancel,
            )?
        };
        for gap in gaps {
            let kind = gap_kind(
                gap,
                (gap.end - gap.start) * length,
                width,
                recipe.process.max_bridge_span_mm,
            );
            // Sparse infill and wall connectors keep the one short-span rule:
            // they may span lower strokes within the bridge limit, or run
            // unanchored for at most three bead widths, and still root what
            // rests on them. A road joined by a connector is judged as a road.
            let sparse = path.kind == DepositionKind::SparseFill
                || (first..last).all(|edge| path.is_connector_edge(edge));
            let span = (gap.end - gap.start) * length;
            if matches!(kind, GapKind::Demand)
                && !(sparse && span <= MAX_SPARSE_UNSUPPORTED_RUN_WIDTHS * width)
                || (matches!(kind, GapKind::Bridge)
                    && path.kind.is_model()
                    && path.kind != DepositionKind::Bridge
                    && !sparse)
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Consecutive object edges screened together against one support edge.
const EDGE_BLOCK: usize = 32;

struct LayerBounds {
    index: usize,
    interval: [f64; 2],
    prefix_upper: f64,
}

fn edge_bounds(points: &[[f64; 3]]) -> [[f64; 2]; 2] {
    [
        [
            points[0][0].min(points[1][0]),
            points[0][1].min(points[1][1]),
        ],
        [
            points[0][0].max(points[1][0]),
            points[0][1].max(points[1][1]),
        ],
    ]
}

fn path_bounds(
    path: &DepositionPath,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<([[f64; 2]; 2], f64), ManufacturingError> {
    let width = match &path.width {
        WidthProfile::Constant { width_mm } => *width_mm,
        WidthProfile::PerEdge { widths_mm } => widths_mm.iter().copied().fold(0.0, f64::max),
    };
    let mut bounds = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
    for point in &path.points_mm {
        charge(work, cancel)?;
        for axis in 0..2 {
            bounds[0][axis] = bounds[0][axis].min(point[axis]);
            bounds[1][axis] = bounds[1][axis].max(point[axis]);
        }
    }
    Ok((bounds, width))
}

fn reserve<T>(
    values: &mut Vec<T>,
    additional: usize,
    live: usize,
) -> Result<(), ManufacturingError> {
    let needed = values.len().checked_add(additional).ok_or_else(|| {
        ManufacturingError::new(
            ManufacturingErrorCode::ResourceLimit,
            "support replay capacity overflow",
        )
    })?;
    if needed <= values.capacity() {
        return Ok(());
    }
    let capacity = needed.max(values.capacity().saturating_mul(2));
    capacity
        .checked_add(values.capacity())
        .and_then(|count| count.checked_mul(std::mem::size_of::<T>()))
        .and_then(|bytes| live.checked_add(bytes))
        .filter(|bytes| *bytes <= crate::working_set::bytes())
        .ok_or_else(|| {
            ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "support replay working set exceeds live byte allowance",
            )
        })?;
    values
        .try_reserve_exact(capacity - values.len())
        .map_err(|_| {
            ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "support replay allocation refused",
            )
        })
}

fn first_unseparated(
    layers: &[PrintLayer],
    floor: f64,
    gap: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    let (mut start, mut end) = (0, layers.len());
    while start < end {
        charge(work, cancel)?;
        let middle = start + (end - start) / 2;
        let layer = &layers[middle];
        let top = project_xyz6_bead_height_mm(layer.z_mm, layer.height_mm)?[1];
        if enclosed_vertical_gap_mm(floor, top)? >= gap {
            start = middle + 1;
        } else {
            end = middle;
        }
    }
    Ok(start)
}

fn capsule(
    edge: &[[f64; 3]],
    width: f64,
) -> Result<spiling_geometry::PlanarCapsuleMm, ManufacturingError> {
    let points = [
        project_xyz6_point_mm(edge[0])?,
        project_xyz6_point_mm(edge[1])?,
    ];
    let mut endpoint_error = 0.0_f64;
    for point in points {
        let error = xyz6_coordinate_error_mm(point[0])?.hypot(xyz6_coordinate_error_mm(point[1])?);
        endpoint_error = endpoint_error.max(if error == 0.0 { 0.0 } else { error.next_up() });
    }
    Ok(spiling_geometry::PlanarCapsuleMm {
        segment_xy_mm: points.map(|point| [point[0], point[1]]),
        radius_mm: width * 0.5,
        endpoint_error_mm: endpoint_error,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "On-demand replay ancestry admits masks beside retained contacts and shared anchor scratch"
)]
fn establish_until(
    layers: &[PrintLayer],
    established: &mut Vec<Vec<bool>>,
    recipe: &PlanarPrintRecipe,
    gap: f64,
    live: &mut usize,
    contact_bytes: usize,
    work: &mut usize,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    while established.len() < layers.len() {
        let index = established.len();
        let layer = &layers[index];
        let mut contact_top = None;
        for (lower_index, lower) in layers[..index].iter().enumerate().rev() {
            charge(work, cancel)?;
            let top = project_xyz6_bead_height_mm(lower.z_mm, lower.height_mm)?[1];
            let floor = project_xyz6_bead_height_mm(layer.z_mm, layer.height_mm)?[0];
            if enclosed_vertical_gap_mm(floor, top)? >= gap {
                contact_top = Some(lower_index);
                break;
            }
        }
        let mut valid = Vec::new();
        reserve(
            &mut valid,
            layer.paths.len(),
            *live + contact_bytes + anchors.retained_bytes(),
        )?;
        anchors.admit_beside(*live + valid.capacity() + contact_bytes)?;
        let owned_bytes = *live + valid.capacity() + contact_bytes;
        let prepared = if index == 0 {
            None
        } else {
            ProjectedAnchorPaths::build(
                layers[index - 1]
                    .paths
                    .iter()
                    .zip(&established[index - 1])
                    .filter_map(|(path, valid)| (support(path) || *valid).then_some(path)),
                contact_top.into_iter().flat_map(|top| {
                    layers[top]
                        .paths
                        .iter()
                        .filter(|path| path.kind == DepositionKind::SupportInterface)
                }),
                anchors,
                owned_bytes,
                work,
                cancel,
            )?
        };
        anchors.admit_beside(
            owned_bytes
                + prepared
                    .as_ref()
                    .map_or(0, ProjectedAnchorPaths::retained_bytes),
        )?;
        for path in &layer.paths {
            charge(work, cancel)?;
            let rooted = path.kind.is_model()
                && (index == 0
                    || anchored(
                        path,
                        layers[index - 1]
                            .paths
                            .iter()
                            .zip(&established[index - 1])
                            .filter_map(|(path, valid)| (support(path) || *valid).then_some(path)),
                        contact_top.into_iter().flat_map(|top| {
                            layers[top]
                                .paths
                                .iter()
                                .filter(|path| path.kind == DepositionKind::SupportInterface)
                        }),
                        prepared.as_ref().map(|prepared| (prepared, true)),
                        recipe,
                        work,
                        anchors,
                        cancel,
                    )?);
            valid.push(rooted);
        }
        drop(prepared);
        *live += valid.capacity();
        established.push(valid);
    }
    Ok(())
}

pub(super) fn verify(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<AnchorWorkspace, ManufacturingError> {
    verify_with_waves(plan, recipe, work, cancel, rayon::current_num_threads())
}

fn verify_with_waves(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    cancel: &AtomicBool,
    workers: usize,
) -> Result<AnchorWorkspace, ManufacturingError> {
    if workers > 1 && plan.layers.len() > 1 {
        let mut trial_work = *work;
        let mut anchors = AnchorWorkspace::default();
        if verify_inner(
            plan,
            recipe,
            &mut trial_work,
            &mut spiling_geometry::GeometryWorkBudget::monitored(),
            &mut anchors,
            cancel,
            true,
            &mut false,
            Some(workers),
        )
        .is_ok()
        {
            *work = trial_work;
            return Ok(anchors);
        }
    }
    verify_with_screen(plan, recipe, work, cancel, true)
}

fn verify_with_screen(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    cancel: &AtomicBool,
    enabled: bool,
) -> Result<AnchorWorkspace, ManufacturingError> {
    let mut anchors = AnchorWorkspace::default();
    let mut native_work = spiling_geometry::GeometryWorkBudget::monitored();
    let mut admitted = false;
    let mut result = verify_inner(
        plan,
        recipe,
        work,
        &mut native_work,
        &mut anchors,
        cancel,
        enabled,
        &mut admitted,
        None,
    );
    if admitted && result.as_ref().is_err_and(retry_for_clearance_pressure) {
        anchors = AnchorWorkspace::default();
        result = verify_inner(
            plan,
            recipe,
            work,
            &mut native_work,
            &mut anchors,
            cancel,
            false,
            &mut false,
            None,
        );
    }
    result?;
    Ok(anchors)
}

fn retry_for_clearance_pressure(error: &ManufacturingError) -> bool {
    error.code == ManufacturingErrorCode::ResourceLimit
        && matches!(
            error.message.as_str(),
            "support replay working set exceeds live byte allowance"
                | "support replay allocation refused"
                | "support replay contacts exceed live byte allowance"
                | "deposited-anchor owners exceed live byte allowance"
                | "retained deposited-anchor interval budget exceeded"
                | "combined retained anchor interval budget exceeded"
                | "component anchor witness retention exceeded"
                | "component anchor witness allocation refused"
        )
}

#[expect(
    clippy::too_many_arguments,
    reason = "Serial retry retains both work counters; the speculative pass uses private counters"
)]
fn verify_inner(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
    enabled: bool,
    admitted: &mut bool,
    wave_workers: Option<usize>,
) -> Result<(), ManufacturingError> {
    if !plan
        .layers
        .iter()
        .flat_map(|layer| &layer.paths)
        .any(support)
    {
        return Ok(());
    }
    let SupportPolicy::Breakaway {
        z_gap_mm,
        xy_clearance_mm,
        ..
    } = recipe.supports
    else {
        return Err(failed("actual support requires breakaway policy"));
    };
    let last_support = plan
        .layers
        .iter()
        .rposition(|layer| layer.paths.iter().any(support))
        .unwrap_or(0);
    let mut bounds = Vec::new();
    reserve(&mut bounds, plan.layers.len(), anchors.retained_bytes())?;
    for (index, layer) in plan.layers.iter().enumerate() {
        let mut interval = [f64::INFINITY, f64::NEG_INFINITY];
        for path in layer.paths.iter().filter(|path| path.kind.is_model()) {
            charge(work, cancel)?;
            let actual = height(path)?;
            interval[0] = interval[0].min(actual[0]);
            interval[1] = interval[1].max(actual[1]);
        }
        if interval[0].is_finite() {
            bounds.push(LayerBounds {
                index,
                interval,
                prefix_upper: interval[1],
            });
        }
    }
    bounds.sort_unstable_by(|a, b| {
        a.interval[0]
            .total_cmp(&b.interval[0])
            .then(a.index.cmp(&b.index))
    });
    let mut upper = f64::NEG_INFINITY;
    for bound in &mut bounds {
        upper = upper.max(bound.interval[1]);
        bound.prefix_upper = upper;
    }
    let mut established: Vec<Vec<bool>> = Vec::new();
    let mut live = bounds.capacity() * std::mem::size_of::<LayerBounds>();
    reserve(
        &mut established,
        last_support + 1,
        live + anchors.retained_bytes(),
    )?;
    live += established.capacity() * std::mem::size_of::<Vec<bool>>();
    let screen = if enabled {
        build_object_clearance_index(&plan.layers, live + anchors.retained_bytes(), work, cancel)?
    } else {
        None
    };
    let index_bytes = screen
        .as_ref()
        .map_or(0, ObjectClearanceIndex::retained_bytes);
    *admitted = screen.is_some();
    live += index_bytes;
    let clearance = ObjectClearance {
        plan,
        bounds: &bounds,
        screen: screen.as_ref(),
        z_gap_mm,
        xy_clearance_mm,
    };
    let mut contacts = Vec::new();
    let mut max_clearance_live = 0;
    for (index, layer) in plan.layers.iter().enumerate() {
        let mut contact_floor = None;
        let mut prepared = None;
        let mut prepared_attempted = false;
        for path in layer.paths.iter().filter(|path| support(path)) {
            charge(work, cancel)?;
            let support_height = height(path)?;
            let candidate = wave_workers
                .is_none()
                .then(|| ClearancePath::new(path, support_height, work, cancel))
                .transpose()?;
            anchors.admit_beside(
                live + contacts.capacity() * std::mem::size_of::<&DepositionPath>(),
            )?;
            if index != 0 && !prepared_attempted {
                prepared = ProjectedAnchorPaths::build(
                    plan.layers[index - 1]
                        .paths
                        .iter()
                        .filter(|path| support(path)),
                    contacts.iter().copied().filter(|_| contact_floor.is_some()),
                    anchors,
                    live + contacts.capacity() * std::mem::size_of::<&DepositionPath>(),
                    work,
                    cancel,
                )?;
                prepared_attempted = true;
            }
            anchors.admit_beside(
                live + contacts.capacity() * std::mem::size_of::<&DepositionPath>()
                    + prepared
                        .as_ref()
                        .map_or(0, ProjectedAnchorPaths::retained_bytes),
            )?;
            let continuing = index == 0
                || anchored(
                    path,
                    plan.layers[index - 1]
                        .paths
                        .iter()
                        .filter(|path| support(path)),
                    std::iter::empty(),
                    prepared.as_ref().map(|prepared| (prepared, false)),
                    recipe,
                    work,
                    anchors,
                    cancel,
                )?;
            if !continuing
                && path.kind == DepositionKind::SupportInterface
                && contact_floor != Some(support_height[0].to_bits())
            {
                drop(prepared.take());
                prepared_attempted = false;
                establish_until(
                    &plan.layers[..index],
                    &mut established,
                    recipe,
                    z_gap_mm,
                    &mut live,
                    contacts.capacity() * std::mem::size_of::<&DepositionPath>(),
                    work,
                    anchors,
                    cancel,
                )?;
                contacts.clear();
                contact_floor = Some(support_height[0].to_bits());
                let end = first_unseparated(
                    &plan.layers[..index],
                    support_height[0],
                    z_gap_mm,
                    work,
                    cancel,
                )?;
                let start = if index > 0 {
                    let previous = &plan.layers[index - 1];
                    let floor = project_xyz6_bead_height_mm(previous.z_mm, previous.height_mm)?[0];
                    first_unseparated(
                        &plan.layers[..end.min(index.saturating_sub(1))],
                        floor,
                        z_gap_mm,
                        work,
                        cancel,
                    )?
                } else {
                    0
                };
                for (offset, lower) in plan.layers[start..end].iter().enumerate() {
                    let lower_index = start + offset;
                    for object in lower
                        .paths
                        .iter()
                        .zip(&established[lower_index])
                        .filter_map(|(path, valid)| {
                            (path.kind.is_model() && *valid).then_some(path)
                        })
                    {
                        charge(work, cancel)?;
                        let top = height(object)?[1];
                        if enclosed_vertical_gap_mm(support_height[0], top)? < z_gap_mm {
                            continue;
                        }
                        if contacts.len() == contacts.capacity() {
                            let needed = contacts
                                .len()
                                .checked_add(1)
                                .filter(|count| {
                                    count
                                        .saturating_mul(std::mem::size_of::<&DepositionPath>())
                                        .saturating_add(anchors.retained_bytes())
                                        <= crate::working_set::bytes()
                                })
                                .ok_or_else(|| {
                                    ManufacturingError::new(
                                        ManufacturingErrorCode::ResourceLimit,
                                        "support replay contacts exceed live byte allowance",
                                    )
                                })?;
                            let additional = needed - contacts.len();
                            reserve(&mut contacts, additional, live + anchors.retained_bytes())?;
                        }
                        contacts.push(object);
                    }
                }
            }
            anchors.admit_beside(
                live + contacts.capacity() * std::mem::size_of::<&DepositionPath>(),
            )?;
            if index != 0 && !prepared_attempted {
                prepared = ProjectedAnchorPaths::build(
                    plan.layers[index - 1]
                        .paths
                        .iter()
                        .filter(|path| support(path)),
                    contacts.iter().copied().filter(|_| contact_floor.is_some()),
                    anchors,
                    live + contacts.capacity() * std::mem::size_of::<&DepositionPath>(),
                    work,
                    cancel,
                )?;
                prepared_attempted = true;
            }
            anchors.admit_beside(
                live + contacts.capacity() * std::mem::size_of::<&DepositionPath>()
                    + prepared
                        .as_ref()
                        .map_or(0, ProjectedAnchorPaths::retained_bytes),
            )?;
            if !continuing
                && !anchored(
                    path,
                    plan.layers[index - 1]
                        .paths
                        .iter()
                        .filter(|path| support(path)),
                    contacts
                        .iter()
                        .copied()
                        .filter(|_| path.kind == DepositionKind::SupportInterface),
                    prepared
                        .as_ref()
                        .map(|prepared| (prepared, path.kind == DepositionKind::SupportInterface)),
                    recipe,
                    work,
                    anchors,
                    cancel,
                )?
            {
                return Err(failed(format!(
                    "actual support layer {index} has no established bed, continuing support or earlier-part contact"
                )));
            }
            let clearance_live = live
                + contacts.capacity() * std::mem::size_of::<&DepositionPath>()
                + anchors.retained_bytes();
            if let Some(candidate) = candidate {
                clearance.check(index, candidate, clearance_live, work, native_work, cancel)?;
            } else {
                max_clearance_live = max_clearance_live.max(clearance_live);
            }
        }
        drop(prepared);
        if index > last_support {
            break;
        }
    }
    drop((established, contacts));
    if let Some(workers) = wave_workers {
        let shared = bounds.capacity() * std::mem::size_of::<LayerBounds>()
            + index_bytes
            + anchors.retained_bytes();
        let Some(totals) =
            clearance.waves(last_support, workers, shared, max_clearance_live, cancel)
        else {
            return Err(wave_unavailable());
        };
        *work = work
            .checked_add(totals.replay)
            .ok_or_else(wave_unavailable)?;
        native_work
            .charge(totals.native, cancel)
            .map_err(|_| wave_unavailable())?;
    }
    drop(screen);
    anchors.set_budget(crate::working_set::units());
    Ok(())
}

struct ClearancePath<'a> {
    height: [f64; 2],
    edges: DepositionEdges<'a>,
    bounds: [[f64; 2]; 2],
    width: f64,
}

impl<'a> ClearancePath<'a> {
    fn new(
        path: &'a DepositionPath,
        height: [f64; 2],
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        let edges = path.checked_edges()?;
        let (bounds, width) = path_bounds(path, work, cancel)?;
        Ok(Self {
            height,
            edges,
            bounds,
            width,
        })
    }
}

struct ObjectClearance<'a> {
    plan: &'a NormalizedPrintPlan,
    bounds: &'a [LayerBounds],
    screen: Option<&'a ObjectClearanceIndex>,
    z_gap_mm: f64,
    xy_clearance_mm: f64,
}

#[derive(Clone, Copy, Default)]
struct ClearanceWork {
    replay: usize,
    native: u64,
}

fn wave_unavailable() -> ManufacturingError {
    ManufacturingError::new(
        ManufacturingErrorCode::ResourceLimit,
        "speculative support clearance must replay in order",
    )
}

impl ObjectClearance<'_> {
    fn waves(
        &self,
        last_support: usize,
        workers: usize,
        shared: usize,
        max_clearance_live: usize,
        cancel: &AtomicBool,
    ) -> Option<ClearanceWork> {
        let layers = &self.plan.layers[..=last_support];
        let wave_width = layers.len().min(workers.saturating_mul(4));
        let spare = crate::working_set::bytes().checked_sub(shared)?;
        let receipt_bytes = wave_width.checked_mul(std::mem::size_of::<Option<ClearanceWork>>())?;
        if receipt_bytes >= spare {
            return None;
        }
        let mut receipts = Vec::new();
        receipts.try_reserve_exact(wave_width).ok()?;
        let receipt_bytes = receipts
            .capacity()
            .checked_mul(std::mem::size_of::<Option<ClearanceWork>>())?;
        let per_worker = (spare.checked_sub(receipt_bytes)? / workers)
            .min(crate::working_set::bytes().checked_sub(max_clearance_live)?);
        let allowance = crate::working_set::WorkingSet::new(per_worker).ok()?;
        let mut totals = ClearanceWork::default();
        for (wave, layers) in layers.chunks(wave_width).enumerate() {
            cancelled(cancel).ok()?;
            layers
                .par_iter()
                .enumerate()
                .map(|(offset, layer)| {
                    crate::working_set::scope(allowance, || {
                        self.check_layer(wave * wave_width + offset, layer, cancel)
                            .ok()
                    })
                })
                .collect_into_vec(&mut receipts);
            for receipt in &receipts {
                let receipt = (*receipt)?;
                totals.replay = totals.replay.checked_add(receipt.replay)?;
                totals.native = totals.native.saturating_add(receipt.native);
            }
        }
        Some(totals)
    }

    fn check_layer(
        &self,
        index: usize,
        layer: &PrintLayer,
        cancel: &AtomicBool,
    ) -> Result<ClearanceWork, ManufacturingError> {
        let mut work = 0;
        let mut native_work = spiling_geometry::GeometryWorkBudget::monitored();
        for path in layer.paths.iter().filter(|path| support(path)) {
            let candidate = ClearancePath::new(path, height(path)?, &mut work, cancel)?;
            self.check(index, candidate, 0, &mut work, &mut native_work, cancel)?;
        }
        Ok(ClearanceWork {
            replay: work,
            native: native_work.consumed(),
        })
    }

    fn check(
        &self,
        index: usize,
        candidate: ClearancePath<'_>,
        live: usize,
        work: &mut usize,
        native_work: &mut spiling_geometry::GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        let ClearancePath {
            height: support_height,
            edges: support_edges,
            bounds: support_bounds,
            width: support_width,
        } = candidate;
        let Self {
            plan,
            bounds,
            screen,
            z_gap_mm,
            xy_clearance_mm,
        } = *self;
        let end = bounds
            .partition_point(|bound| bound.interval[0] <= (support_height[1] + z_gap_mm).next_up());
        let start = bounds[..end].partition_point(|bound| {
            bound.prefix_upper < (support_height[0] - z_gap_mm).next_down()
        });
        for bound in &bounds[start..end] {
            charge(work, cancel)?;
            if enclosed_vertical_gap_mm(bound.interval[0], support_height[1])? >= z_gap_mm
                || enclosed_vertical_gap_mm(support_height[0], bound.interval[1])? >= z_gap_mm
            {
                continue;
            }
            let object_layer = &plan.layers[bound.index];
            let screen_blocks = screen
                .filter(|_| bounded_height(bound.interval) && bounded_height(support_height))
                .map(|screen| screen.layer(bound.index));
            let span = if screen_blocks.is_some() {
                BLOCK_PATHS
            } else {
                object_layer.paths.len().max(1)
            };
            for (block_ordinal, paths) in object_layer.paths.chunks(span).enumerate() {
                if let Some(blocks) = screen_blocks {
                    charge(work, cancel)?;
                    let block = &blocks[block_ordinal];
                    if block.clear(support_bounds, support_width, xy_clearance_mm) {
                        continue;
                    }
                }
                for object in paths.iter().filter(|path| path.kind.is_model()) {
                    charge(work, cancel)?;
                    let object_height = height(object)?;
                    if enclosed_vertical_gap_mm(object_height[0], support_height[1])? >= z_gap_mm
                        || enclosed_vertical_gap_mm(support_height[0], object_height[1])?
                            >= z_gap_mm
                    {
                        continue;
                    }
                    let object_edges = object.checked_edges()?;
                    let (object_bounds, object_width) = path_bounds(object, work, cancel)?;
                    if spiling_geometry::planar_bounds_clear(
                        support_bounds,
                        object_bounds,
                        (support_width + object_width) * 0.5 + 2.0 * XYZ6_PROJECTION_SCREEN_MM,
                        xy_clearance_mm,
                    ) {
                        continue;
                    }
                    // Consecutive object edges are screened in blocks: a
                    // block whose joint bounds and widest bead clear this
                    // support edge clears each of its edges by the same
                    // test, so only the per-edge charge remains.
                    let edge_count = object.points_mm.len().saturating_sub(1);
                    let mut blocks: Vec<([[f64; 2]; 2], f64)> = Vec::new();
                    reserve(&mut blocks, edge_count.div_ceil(EDGE_BLOCK), live)?;
                    for start in (0..edge_count).step_by(EDGE_BLOCK) {
                        let mut bounds = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
                        let mut width = 0.0f64;
                        for index in start..(start + EDGE_BLOCK).min(edge_count) {
                            let edge = object_edges.edge(index).expect("object edge in range");
                            let edge_box = edge_bounds(edge.points_mm);
                            for axis in 0..2 {
                                bounds[0][axis] = bounds[0][axis].min(edge_box[0][axis]);
                                bounds[1][axis] = bounds[1][axis].max(edge_box[1][axis]);
                            }
                            width = width.max(edge.width_mm);
                        }
                        blocks.push((bounds, width));
                    }
                    for support_edge in support_edges {
                        let support_capsule =
                            capsule(support_edge.points_mm, support_edge.width_mm)?;
                        let support_box = edge_bounds(support_edge.points_mm);
                        for (block, &(block_bounds, block_width)) in blocks.iter().enumerate() {
                            let start = block * EDGE_BLOCK;
                            let end = (start + EDGE_BLOCK).min(edge_count);
                            if spiling_geometry::planar_bounds_clear(
                                support_box,
                                block_bounds,
                                (support_edge.width_mm + block_width) * 0.5
                                    + 2.0 * XYZ6_PROJECTION_SCREEN_MM,
                                xy_clearance_mm,
                            ) {
                                for _ in start..end {
                                    charge(work, cancel)?;
                                }
                                continue;
                            }
                            for object_edge in (start..end).map(|index| {
                                object_edges.edge(index).expect("object edge in range")
                            }) {
                                charge(work, cancel)?;
                                if spiling_geometry::planar_bounds_clear(
                                    edge_bounds(support_edge.points_mm),
                                    edge_bounds(object_edge.points_mm),
                                    (support_edge.width_mm + object_edge.width_mm) * 0.5
                                        + 2.0 * XYZ6_PROJECTION_SCREEN_MM,
                                    xy_clearance_mm,
                                ) {
                                    continue;
                                }
                                let object_capsule =
                                    capsule(object_edge.points_mm, object_edge.width_mm)?;
                                let bounds = |segment: [[f64; 2]; 2]| {
                                    [
                                        [
                                            segment[0][0].min(segment[1][0]),
                                            segment[0][1].min(segment[1][1]),
                                        ],
                                        [
                                            segment[0][0].max(segment[1][0]),
                                            segment[0][1].max(segment[1][1]),
                                        ],
                                    ]
                                };
                                let reach = ((support_capsule.radius_mm
                                    + object_capsule.radius_mm)
                                    .next_up()
                                    + support_capsule.endpoint_error_mm)
                                    .next_up();
                                let reach = (reach + object_capsule.endpoint_error_mm).next_up();
                                if spiling_geometry::planar_bounds_clear(
                                    bounds(support_capsule.segment_xy_mm),
                                    bounds(object_capsule.segment_xy_mm),
                                    reach,
                                    xy_clearance_mm,
                                ) {
                                    continue;
                                }
                                spiling_geometry::certify_planar_capsule_pair_clearance(
                                    [support_capsule, object_capsule],
                                    xy_clearance_mm,
                                    native_work,
                                    cancel,
                                )
                                .map_err(|failure| {
                                    failed(format!(
                                        "actual support layer {index} violates object XY/Z clearance: {failure:?}"
                                    ))
                                })?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (ManufacturingIntent, NormalizedPrintPlan) {
        let (mut intent, mut plan) = crate::verifier::tests::fixture();
        intent.printer.capabilities.generated_supports = true;
        intent.recipe.supports = SupportPolicy::Breakaway {
            density: 0.5,
            xy_clearance_mm: 0.25,
            z_gap_mm: 0.5,
            interface_thickness_mm: 1.0,
        };
        let template = plan.layers[0].clone();
        plan.layers = (0..5)
            .map(|index| {
                let mut layer = template.clone();
                layer.index = index;
                layer.hatch_course = index;
                layer.lower_z_mm = index as f64;
                layer.z_mm = index as f64 + 1.0;
                layer.height_mm = 1.0;
                layer.section_z_mm = index as f64 + 0.5;
                layer.paths.clear();
                layer.region_coverage.section_area_mm2 = 0.0;
                layer.region_coverage.source_section_area_mm2 = 0.0;
                layer.region_coverage.wall_area_mm2 = 0.0;
                layer.region_coverage.nominal_deposition_volume_mm3 = 0.0;
                if index.is_multiple_of(2) {
                    layer.paths.push(DepositionPath {
                        material_domain: None,
                        kind: if index == 2 {
                            DepositionKind::SupportInterface
                        } else {
                            DepositionKind::BottomSkin
                        },
                        width: WidthProfile::Constant { width_mm: 1.0 },
                        physical_z_mm: [layer.lower_z_mm, layer.z_mm],
                        owner: (index != 2).then_some(DepositionOwner::MandatorySkin {
                            skin: ProcessSkinKind::BottomSkin,
                        }),
                        points_mm: vec![[1.0, 1.0, layer.z_mm], [5.0, 1.0, layer.z_mm]],
                        arcs: Vec::new(),
                        exposed: false,
                        connectors: Vec::new(),
                    });
                    if index == 2 {
                        layer.region_coverage.support_area_mm2 = 4.0;
                    } else {
                        layer.region_coverage.section_area_mm2 = 4.0;
                        layer.region_coverage.source_section_area_mm2 = 4.0;
                        layer.region_coverage.bottom_skin_area_mm2 = 4.0;
                    }
                }
                layer
            })
            .collect();
        crate::verifier::tests::refresh_volumes(&intent, &mut plan);
        (intent, plan)
    }

    fn check(
        intent: &ManufacturingIntent,
        plan: &NormalizedPrintPlan,
    ) -> Result<(), ManufacturingError> {
        verify(plan, &intent.recipe, &mut 0, &AtomicBool::new(false)).map(|_| ())
    }

    #[test]
    fn clearance_waves_keep_work_and_legacy_first_refusal() {
        let (intent, mut plan) = fixture();
        let cancel = AtomicBool::new(false);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();
        let support_path = plan.layers[2].paths[0].clone();
        let mut continuing = support_path.clone();
        continuing.physical_z_mm = [3.0, 4.0];
        for point in &mut continuing.points_mm {
            point[2] = 4.0;
        }
        plan.layers[3].paths.push(continuing);
        let object = &mut plan.layers[4].paths[0];
        object.physical_z_mm = support_path.physical_z_mm;
        object.points_mm = vec![[5.9, 1.9, 3.0], [6.9, 2.9, 3.0]];

        let inner = |plan: &NormalizedPrintPlan, wave_workers, initial| {
            pool.install(|| {
                let mut work = initial;
                let mut native_work = spiling_geometry::GeometryWorkBudget::monitored();
                let mut anchors = AnchorWorkspace::default();
                let mut admitted = false;
                let result = verify_inner(
                    plan,
                    &intent.recipe,
                    &mut work,
                    &mut native_work,
                    &mut anchors,
                    &cancel,
                    true,
                    &mut admitted,
                    wave_workers,
                );
                assert!(admitted);
                (
                    result,
                    work,
                    native_work.consumed(),
                    anchors.retained_bytes(),
                )
            })
        };
        let run = |plan: &NormalizedPrintPlan, waves, initial| {
            pool.install(|| {
                let mut work = initial;
                let result = if waves {
                    verify(plan, &intent.recipe, &mut work, &cancel)
                } else {
                    verify_with_screen(plan, &intent.recipe, &mut work, &cancel, true)
                };
                (result.map(|_| ()), work)
            })
        };
        let serial = inner(&plan, None, 0);
        assert_eq!(serial.0, Ok(()));
        assert!(serial.2 > 0, "fixture must reach the native capsule proof");
        assert_eq!(inner(&plan, Some(2), 0), serial);

        let initial = usize::MAX - serial.1 + 1;
        let overflow = run(&plan, false, initial);
        assert_eq!(
            overflow.0.as_ref().unwrap_err().code,
            ManufacturingErrorCode::ResourceLimit
        );
        assert_eq!(run(&plan, true, initial), overflow);

        let mut invalid = plan;
        let mut collision = invalid.layers[4].paths[0].clone();
        collision.points_mm = support_path.points_mm;
        invalid.layers[4].paths.push(collision);
        assert_eq!(inner(&invalid, Some(2), 19).0, Err(wave_unavailable()));
        let serial = run(&invalid, false, 19);
        assert!(
            serial
                .0
                .as_ref()
                .unwrap_err()
                .message
                .contains("layer 2 violates object XY/Z clearance")
        );
        assert_eq!(run(&invalid, true, 19), serial);

        invalid.layers[3].paths[0].kind = DepositionKind::Support;
        for point in &mut invalid.layers[3].paths[0].points_mm {
            point[1] += 30.0;
        }
        let serial = run(&invalid, false, 19);
        assert!(
            serial
                .0
                .as_ref()
                .unwrap_err()
                .message
                .contains("layer 2 violates object XY/Z clearance")
        );
        let speculative = inner(&invalid, Some(2), 19);
        assert!(
            speculative
                .0
                .unwrap_err()
                .message
                .contains("layer 3 has no established")
        );
        assert_eq!(run(&invalid, true, 19), serial);

        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        let cancelled = run(&invalid, false, 19);
        assert_eq!(
            cancelled.0.as_ref().unwrap_err().code,
            ManufacturingErrorCode::Cancelled
        );
        assert_eq!(run(&invalid, true, 19), cancelled);
    }

    #[test]
    fn clearance_waves_admit_receipts_and_simultaneous_scratch() {
        let (_, mut plan) = fixture();
        let object = &mut plan.layers[4].paths[0];
        object.physical_z_mm = [2.0, 3.0];
        object.points_mm = vec![[5.9, 1.9, 3.0], [6.9, 2.9, 3.0]];
        let bounds = [LayerBounds {
            index: 4,
            interval: height(object).unwrap(),
            prefix_upper: height(object).unwrap()[1],
        }];
        let shared = std::mem::size_of_val(&bounds);
        let clearance = ObjectClearance {
            plan: &plan,
            bounds: &bounds,
            screen: None,
            z_gap_mm: 0.5,
            xy_clearance_mm: 0.25,
        };
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();
        let cancel = AtomicBool::new(false);
        let scratch = std::mem::size_of::<([[f64; 2]; 2], f64)>();
        let receipts = 3 * std::mem::size_of::<Option<ClearanceWork>>();
        let exact = shared + receipts + 2 * scratch;
        let run = |bytes, legacy_live| {
            pool.install(|| {
                crate::working_set::scope(crate::WorkingSet::new(bytes).unwrap(), || {
                    clearance.waves(2, 2, shared, legacy_live, &cancel)
                })
            })
        };
        assert!(run(exact - 1, shared).is_none());
        assert!(run(exact, exact - scratch + 1).is_none());
        assert!(run(exact, exact - scratch).unwrap().native > 0);
    }

    #[test]
    fn ordered_object_screen_preserves_first_refusal() {
        let (intent, plan) = fixture();
        let support = plan.layers[2].paths[0].clone();
        let mut collision = plan.layers[4].paths[0].clone();
        collision.physical_z_mm = support.physical_z_mm;
        collision.points_mm = support.points_mm.clone();
        let mut clear = collision.clone();
        for point in &mut clear.points_mm {
            point[1] += 20.0;
        }
        let mut malformed = clear.clone();
        malformed.width = WidthProfile::PerEdge { widths_mm: vec![] };
        let run = |plan: &NormalizedPrintPlan, enabled| {
            verify_with_screen(
                plan,
                &intent.recipe,
                &mut 0,
                &AtomicBool::new(false),
                enabled,
            )
            .map(|_| ())
        };
        let mut changed = plan;
        changed.layers[4].paths = vec![clear; BLOCK_PATHS * 2];
        assert_eq!(run(&changed, true), Ok(()));
        assert_eq!(run(&changed, false), Ok(()));
        changed.layers[4]
            .paths
            .extend([collision.clone(), malformed.clone()]);
        let first_collision = run(&changed, false).unwrap_err();
        assert!(
            first_collision
                .message
                .contains("violates object XY/Z clearance")
        );
        assert_eq!(run(&changed, true), Err(first_collision.clone()));
        changed.layers[4]
            .paths
            .swap(BLOCK_PATHS * 2, BLOCK_PATHS * 2 + 1);
        let first_malformed = run(&changed, false).unwrap_err();
        assert_ne!(first_malformed, first_collision);
        assert_eq!(run(&changed, true), Err(first_malformed));
    }

    #[test]
    fn admitted_index_pressure_restarts_exhaustive_replay_from_the_first_check() {
        use std::mem::size_of;

        let (intent, mut plan) = fixture();
        let cancel = AtomicBool::new(false);
        let root_count = 128;
        let mut distant = plan.layers[0].paths[0].clone();
        for point in &mut distant.points_mm {
            point[1] += 20.0;
        }
        plan.layers[0].paths.resize(root_count, distant);
        let support = &plan.layers[2].paths[0];
        let mut collision = plan.layers[4].paths[0].clone();
        collision.physical_z_mm = support.physical_z_mm;
        collision.points_mm = support.points_mm.clone();
        let mut clear = collision.clone();
        for point in &mut clear.points_mm {
            point[1] += 20.0;
        }
        let mut malformed = clear.clone();
        malformed.width = WidthProfile::PerEdge { widths_mm: vec![] };
        let mut layer_bounds: Vec<LayerBounds> = Vec::new();
        reserve(&mut layer_bounds, plan.layers.len(), 0).unwrap();
        let mut ancestry: Vec<Vec<bool>> = Vec::new();
        reserve(&mut ancestry, 3, 0).unwrap();
        let mandatory = layer_bounds.capacity() * size_of::<LayerBounds>()
            + ancestry.capacity() * size_of::<Vec<bool>>();
        drop((layer_bounds, ancestry));
        for (faults, expected_message) in [
            (vec![], None),
            (
                vec![collision.clone(), malformed.clone()],
                Some("violates object XY/Z clearance"),
            ),
            (
                vec![malformed.clone(), collision.clone()],
                Some("exactly one width"),
            ),
        ] {
            plan.layers[4].paths = vec![clear.clone(); BLOCK_PATHS * 16];
            plan.layers[4].paths.extend(faults);
            let index_bytes =
                build_object_clearance_index(&plan.layers, mandatory, &mut 0, &cancel)
                    .unwrap()
                    .unwrap()
                    .retained_bytes();
            let mut mask = Vec::<bool>::new();
            reserve(&mut mask, root_count, 0).unwrap();
            let mut contact_capacity = Vec::<&DepositionPath>::new();
            reserve(&mut contact_capacity, root_count / 2, 0).unwrap();
            let old_capacity = contact_capacity.capacity();
            contact_capacity.resize(old_capacity, &plan.layers[0].paths[0]);
            reserve(&mut contact_capacity, 1, 0).unwrap();
            let contact_peak =
                (old_capacity + contact_capacity.capacity()) * size_of::<&DepositionPath>();
            let tight = crate::WorkingSet::new(
                mandatory + index_bytes + mask.capacity() + contact_peak - 1,
            )
            .unwrap();
            drop((mask, contact_capacity));
            crate::working_set::scope(tight, || {
                let mut admitted = false;
                let mut attempt_work = 73;
                let attempted = verify_inner(
                    &plan,
                    &intent.recipe,
                    &mut attempt_work,
                    &mut spiling_geometry::GeometryWorkBudget::monitored(),
                    &mut AnchorWorkspace::default(),
                    &cancel,
                    true,
                    &mut admitted,
                    None,
                );
                assert!(admitted);
                let error = attempted.unwrap_err();
                assert!(retry_for_clearance_pressure(&error));
                assert_eq!(
                    error.message,
                    "support replay working set exceeds live byte allowance"
                );
                assert!(attempt_work > 73 + root_count);
                let run = |enabled| {
                    let mut work = 73;
                    let result =
                        verify_with_screen(&plan, &intent.recipe, &mut work, &cancel, enabled)
                            .map(|_| ());
                    (result, work)
                };
                let exhaustive = run(false);
                if let Some(message) = expected_message {
                    assert!(exhaustive.0.as_ref().unwrap_err().message.contains(message));
                } else {
                    assert_eq!(exhaustive.0, Ok(()));
                }
                let screened = run(true);
                assert_eq!(screened.0, exhaustive.0);
                assert_eq!(screened.1, attempt_work + exhaustive.1 - 73);
                let mut work = 73;
                let waves =
                    verify_with_waves(&plan, &intent.recipe, &mut work, &cancel, 2).map(|_| ());
                assert_eq!((waves, work), screened);
            });
        }
    }

    #[test]
    fn clearance_retry_does_not_mask_counter_overflow_or_cancellation() {
        for (code, message) in [
            (
                ManufacturingErrorCode::ResourceLimit,
                "support replay work overflow",
            ),
            (
                ManufacturingErrorCode::ResourceLimit,
                "cumulative deposited-stroke anchor work counter overflow",
            ),
            (
                ManufacturingErrorCode::ResourceLimit,
                "deposition width allocation limit exceeded",
            ),
            (
                ManufacturingErrorCode::Cancelled,
                "support replay allocation refused",
            ),
        ] {
            assert!(!retry_for_clearance_pressure(&ManufacturingError::new(
                code, message
            )));
        }
    }

    #[test]
    fn ordered_object_screen_preserves_both_closed_vertical_gaps() {
        let (intent, mut plan) = fixture();
        let support = plan.layers[2].paths[0].clone();
        for (interval, clears) in [
            ([3.5, 4.5], true),
            ([3.5f64.next_down(), 4.5], false),
            ([0.5, 1.5], true),
            ([0.5, 1.5 + XYZ6_QUANTUM_MM], false),
        ] {
            let object = &mut plan.layers[4].paths[0];
            object.physical_z_mm = interval;
            object.width = WidthProfile::Constant { width_mm: 1.25 };
            for (point, support_point) in object.points_mm.iter_mut().zip(&support.points_mm) {
                *point = [support_point[0], support_point[1], interval[1]];
            }
            let run = |enabled| {
                verify_with_screen(
                    &plan,
                    &intent.recipe,
                    &mut 0,
                    &AtomicBool::new(false),
                    enabled,
                )
                .map(|_| ())
            };
            let exhaustive = run(false);
            assert_eq!(exhaustive.is_ok(), clears, "{interval:?}");
            if let Err(error) = &exhaustive {
                assert!(error.message.contains("violates object XY/Z clearance"));
            }
            assert_eq!(run(true), exhaustive);
        }
    }

    #[test]
    fn ordered_object_screen_declines_under_pressure_and_uses_widest_edge() {
        let (_, mut plan) = fixture();
        let path = &mut plan.layers[4].paths[0];
        path.points_mm = vec![[0.0, 2.0, 5.0], [1.0, 2.0, 5.0], [2.0, 2.0, 5.0]];
        path.width = WidthProfile::PerEdge {
            widths_mm: vec![1.0, 2.0],
        };
        let cancel = AtomicBool::new(false);
        let index = build_object_clearance_index(&plan.layers, 0, &mut 0, &cancel)
            .unwrap()
            .unwrap();
        let block = &index.layer(4)[0];
        assert!(!block.clear([[0.0, 0.0], [2.0, 0.0]], 1.0, 0.5));
        assert!(block.clear([[0.0, -0.001], [2.0, -0.001]], 1.0, 0.5));
        let tight = crate::WorkingSet::new(index.retained_bytes() - 1).unwrap();
        crate::working_set::scope(tight, || {
            assert!(
                build_object_clearance_index(&plan.layers, 0, &mut 0, &cancel)
                    .unwrap()
                    .is_none()
            );
        });
        assert!(
            build_object_clearance_index(&plan.layers, 0, &mut 0, &AtomicBool::new(true)).is_err()
        );
    }

    #[test]
    fn projected_scope_matches_ordered_anchored_roles_and_malformed_errors() {
        let (intent, plan) = fixture();
        let cancel = AtomicBool::new(false);
        for kind in [
            DepositionKind::BottomSkin,
            DepositionKind::SparseFill,
            DepositionKind::Bridge,
            DepositionKind::SupportInterface,
        ] {
            for malformed in [false, true] {
                let mut candidate = plan.layers[0].paths[0].clone();
                candidate.kind = kind;
                candidate.physical_z_mm = [1.0, 2.0];
                candidate.points_mm = vec![[1.0, 1.0, 2.0], [5.0, 1.0, 2.0]];
                let mut left = plan.layers[0].paths[0].clone();
                left.points_mm = vec![[1.0, 1.0, 1.0], [2.0, 1.0, 1.0]];
                let mut right = left.clone();
                right.points_mm = vec![[4.0, 1.0, 1.0], [5.0, 1.0, 1.0]];
                let mut contact = right.clone();
                contact.physical_z_mm = [2.0, 3.0];
                contact.points_mm = vec![[4.0, 1.0, 3.0], [5.0, 1.0, 3.0]];
                if malformed {
                    right.width = WidthProfile::PerEdge { widths_mm: vec![] };
                }
                let previous = [left, right];
                let contacts = [contact];
                let mut scalar_workspace = AnchorWorkspace::default();
                let mut cached_workspace = AnchorWorkspace::default();
                let prepared = ProjectedAnchorPaths::build(
                    previous.iter(),
                    contacts.iter(),
                    &cached_workspace,
                    0,
                    &mut 0,
                    &cancel,
                )
                .unwrap()
                .unwrap();
                cached_workspace
                    .admit_beside(prepared.retained_bytes())
                    .unwrap();
                let expected = anchored(
                    &candidate,
                    previous.iter(),
                    contacts.iter(),
                    None,
                    &intent.recipe,
                    &mut 0,
                    &mut scalar_workspace,
                    &cancel,
                );
                let actual = anchored(
                    &candidate,
                    previous.iter(),
                    contacts.iter(),
                    Some((&prepared, true)),
                    &intent.recipe,
                    &mut 0,
                    &mut cached_workspace,
                    &cancel,
                );
                match (expected, actual) {
                    (Ok(expected), Ok(actual)) => assert_eq!(expected, actual),
                    (Err(expected), Err(actual)) => {
                        assert_eq!(expected.code, actual.code);
                        assert_eq!(expected.message, actual.message);
                    }
                    _ => panic!("cached replay changed anchored acceptance"),
                }
            }
        }
    }

    #[test]
    fn on_part_contacts_replay_both_gaps_without_bed_support() {
        let (intent, plan) = fixture();
        check(&intent, &plan).unwrap();
        plan.validate(&intent).unwrap();
        let cancel = AtomicBool::new(false);
        let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
        assert!(
            crate::verifier::verify(&program, &plan, &intent, &cancel)
                .unwrap()
                .verified
        );
        let text = String::from_utf8(program.to_vec().unwrap()).unwrap();
        let mut missing = false;
        let missing_root = text
            .lines()
            .filter(|line| {
                if !missing && line.starts_with("G1 ") {
                    missing = true;
                    false
                } else {
                    true
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(missing);
        let program = crate::ChunkSource::from_slice(missing_root.as_bytes(), u64::MAX).unwrap();
        assert!(crate::verifier::verify(&program, &plan, &intent, &cancel).is_err());
    }

    #[test]
    fn retained_contacts_reject_missing_moved_and_tangent_roots() {
        let (intent, plan) = fixture();
        for offset in [1.0, 20.0] {
            let mut moved = plan.clone();
            for point in &mut moved.layers[0].paths[0].points_mm {
                point[1] += offset;
            }
            assert!(check(&intent, &moved).is_err());
            let cancel = AtomicBool::new(false);
            let program = crate::backend::emit(&moved, &intent, &cancel).unwrap();
            assert!(crate::verifier::verify(&program, &moved, &intent, &cancel).is_err());
        }
        let mut missing = plan;
        missing.layers[0].paths.clear();
        assert!(check(&intent, &missing).is_err());
    }

    #[test]
    fn retained_contacts_reject_future_same_layer_and_unsupported_roots() {
        let (intent, plan) = fixture();
        for destination in [1, 2, 4] {
            let mut changed = plan.clone();
            let mut root = changed.layers[0].paths.remove(0);
            root.physical_z_mm = [destination as f64, destination as f64 + 1.0];
            for point in &mut root.points_mm {
                point[2] = destination as f64 + 1.0;
            }
            changed.layers[destination].paths.push(root);
            assert!(check(&intent, &changed).is_err());
        }
    }

    #[test]
    fn retained_contacts_reject_insufficient_lower_and_upper_gap() {
        let (mut intent, plan) = fixture();
        if let SupportPolicy::Breakaway { z_gap_mm, .. } = &mut intent.recipe.supports {
            *z_gap_mm = 1.1;
        }
        assert!(check(&intent, &plan).is_err());
        let (intent, mut changed) = fixture();
        changed.layers[4].paths[0].physical_z_mm = [3.25, 4.25];
        for point in &mut changed.layers[4].paths[0].points_mm {
            point[2] = 4.25;
        }
        assert!(check(&intent, &changed).is_err());
    }

    #[test]
    fn ordinary_model_gap_cannot_supply_a_hypothetical_bridge_root() {
        let (intent, plan) = fixture();
        let mut path = plan.layers[0].paths[0].clone();
        path.physical_z_mm = [1.0, 2.0];
        path.points_mm = vec![[1.0, 1.0, 2.0], [5.0, 1.0, 2.0]];
        let mut left = plan.layers[0].paths[0].clone();
        left.points_mm = vec![[1.0, 1.0, 1.0], [2.0, 1.0, 1.0]];
        let mut right = left.clone();
        right.points_mm = vec![[4.0, 1.0, 1.0], [5.0, 1.0, 1.0]];
        let lower = [left, right];
        let mut anchors = AnchorWorkspace::default();
        let mut work = 0;
        let cancel = AtomicBool::new(false);
        assert!(
            !anchored(
                &path,
                lower.iter(),
                std::iter::empty(),
                None,
                &intent.recipe,
                &mut work,
                &mut anchors,
                &cancel
            )
            .unwrap()
        );
        // Sparse infill spanning the rows beneath it roots what rests on it.
        path.kind = DepositionKind::SparseFill;
        assert!(
            anchored(
                &path,
                lower.iter(),
                std::iter::empty(),
                None,
                &intent.recipe,
                &mut work,
                &mut anchors,
                &cancel
            )
            .unwrap()
        );
        // So does a wall connector, judged by the same short-span rule.
        let mut connected = path.clone();
        connected.kind = DepositionKind::TopSkin;
        connected.points_mm = vec![
            [1.0, 0.0, 2.0],
            [1.0, 1.0, 2.0],
            [5.0, 1.0, 2.0],
            [5.0, 0.0, 2.0],
        ];
        connected.connectors = vec![ConnectorRun { first: 1, last: 2 }];
        let mut under = lower.to_vec();
        for x in [1.0, 5.0] {
            let mut post = lower[0].clone();
            post.points_mm = vec![[x, -0.5, 1.0], [x, 1.5, 1.0]];
            under.push(post);
        }
        assert!(
            anchored(
                &connected,
                under.iter(),
                std::iter::empty(),
                None,
                &intent.recipe,
                &mut work,
                &mut anchors,
                &cancel
            )
            .unwrap()
        );
        connected.connectors.clear();
        assert!(
            !anchored(
                &connected,
                under.iter(),
                std::iter::empty(),
                None,
                &intent.recipe,
                &mut work,
                &mut anchors,
                &cancel
            )
            .unwrap()
        );
        path.kind = DepositionKind::Bridge;
        assert!(
            anchored(
                &path,
                lower.iter(),
                std::iter::empty(),
                None,
                &intent.recipe,
                &mut work,
                &mut anchors,
                &cancel
            )
            .unwrap()
        );
    }
}
