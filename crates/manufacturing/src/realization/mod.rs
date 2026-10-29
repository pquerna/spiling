// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Pure normalized-path canonicalization and streamed process realization.
use crate::{check_cancel, error};
use spiling_contracts::manufacturing::program_coordinates::{
    project_xyz6_coordinate_mm, project_xyz6_point_mm,
};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy)]
pub(crate) enum Event {
    Fan(u8),
    Travel([f64; 3], f64),
    Extrusion(f64, f64), // stationary absolute E, speed
    Deposit([f64; 3], f64, f64),
    Waste([f64; 3], f64, f64),
    Barrier,
    SelectTool(u8),
    ResetExtrusion,
    HotendTarget(u8, f64),
    HotendWait(u8, f64),
}
fn rank(path: &DepositionPath) -> u8 {
    if path.owner == Some(DepositionOwner::Wall) {
        return 2;
    }
    match path.kind {
        DepositionKind::Support => 0,
        DepositionKind::SupportInterface => 1,
        DepositionKind::Perimeter => 2,
        DepositionKind::BottomSkin => 3,
        DepositionKind::TopSkin => 4,
        DepositionKind::Bridge => 5,
        DepositionKind::SeparatorWall => 6,
        DepositionKind::SolidFill => 7,
        DepositionKind::SparseFill => 8,
    }
}
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2])
}
fn unsigned_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

fn xy_compare(a: [f64; 3], b: [f64; 3]) -> std::cmp::Ordering {
    unsigned_zero(a[0])
        .total_cmp(&unsigned_zero(b[0]))
        .then_with(|| unsigned_zero(a[1]).total_cmp(&unsigned_zero(b[1])))
}

/// Compare only existing vertices in command coordinates. Raw coordinates and
/// cyclic index break exact projected ties without merging nearby candidates.
fn wall_seam(
    points: &[[f64; 3]],
    policy: SeamPolicy,
    position: Option<[f64; 3]>,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    let mut seam = 0;
    let mut best = project_xyz6_point_mm(points[0])?;
    let score = |point: [f64; 3]| match policy {
        SeamPolicy::Nearest => 0.0,
        SeamPolicy::PreferredBuildDirection { xy } => {
            unsigned_zero(point[0] * xy[0] + point[1] * xy[1])
        }
    };
    let mut best_score = score(best);
    let mut best_distance = None;
    for index in 1..points.len() - 1 {
        check_cancel(cancel)?;
        let candidate = project_xyz6_point_mm(points[index])?;
        let candidate_score = score(candidate);
        let preference = best_score.total_cmp(&candidate_score);
        let mut candidate_distance = None;
        let order = preference
            .then_with(|| {
                position.map_or(std::cmp::Ordering::Equal, |from| {
                    let candidate =
                        *candidate_distance.get_or_insert_with(|| distance(from, candidate));
                    candidate.total_cmp(best_distance.get_or_insert_with(|| distance(from, best)))
                })
            })
            .then_with(|| xy_compare(candidate, best))
            .then_with(|| xy_compare(points[index], points[seam]))
            .then_with(|| index.cmp(&seam));
        if order == std::cmp::Ordering::Less {
            seam = index;
            best = candidate;
            best_score = candidate_score;
            best_distance = candidate_distance;
        }
    }
    Ok(seam)
}

/// Canonical geometry traversal is stored in the normalized plan, never hidden
/// in the emitter. In-place sort/rotation/reversal requires no motion/index vector.
pub(crate) fn canonicalize(
    plan: &mut NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    plan.validate(intent)?;
    let routed = match &intent.printer.workflow {
        JobWorkflow::FixedDualMarlin { job } => {
            crate::routing::validate_plan(plan, intent, job, cancel)?;
            Some(job)
        }
        _ => None,
    };
    let mut position = match &intent.printer.workflow {
        JobWorkflow::DiagnosticBodyOnly => None,
        JobWorkflow::CompleteMarlin { job } => Some(project_xyz6_point_mm(job.prime_end_mm)?),
        JobWorkflow::FixedDualMarlin { .. } => None,
    };
    for layer in &mut plan.layers {
        check_cancel(cancel)?;
        let mut boundary = layer.paths.len();
        if let Some(job) = routed {
            let mut index = 0;
            while index < boundary {
                check_cancel(cancel)?;
                if job.tool_for_path(&layer.paths[index])? == 0 {
                    index += 1;
                } else {
                    boundary -= 1;
                    layer.paths.swap(index, boundary);
                }
            }
        }
        let order = |a: &DepositionPath, b: &DepositionPath| {
            rank(a)
                .cmp(&rank(b))
                .then_with(|| a.points_mm[0][0].total_cmp(&b.points_mm[0][0]))
                .then_with(|| a.points_mm[0][1].total_cmp(&b.points_mm[0][1]))
                .then_with(|| a.points_mm.len().cmp(&b.points_mm.len()))
                .then_with(|| {
                    a.points_mm
                        .iter()
                        .flatten()
                        .zip(b.points_mm.iter().flatten())
                        .find_map(|(a, b)| {
                            let c = a.total_cmp(b);
                            (c != std::cmp::Ordering::Equal).then_some(c)
                        })
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        };
        let (zero, one) = layer.paths.split_at_mut(boundary);
        zero.sort_unstable_by(order);
        one.sort_unstable_by(order);
        let mut previous_tool = None;
        for path in &mut layer.paths {
            check_cancel(cancel)?;
            if let Some(job) = routed {
                let tool = job.tool_for_path(path)?;
                if previous_tool != Some(tool) {
                    position = Some(project_xyz6_point_mm(job.waste_end_mm(tool, layer.z_mm))?);
                    previous_tool = Some(tool);
                }
            }
            let n = path.points_mm.len();
            if path.owner == Some(DepositionOwner::Wall)
                && path.points_mm[0] == path.points_mm[n - 1]
            {
                let seam = wall_seam(
                    &path.points_mm,
                    intent.recipe.seam_policy,
                    position.map(project_xyz6_point_mm).transpose()?,
                    cancel,
                )?;
                path.rotate_closed(seam)?;
            } else if let Some(from) = position {
                if path.points_mm[0] == path.points_mm[n - 1] {
                    let mut seam = 0;
                    for i in 1..n - 1 {
                        if distance(from, path.points_mm[i]) < distance(from, path.points_mm[seam])
                        {
                            seam = i;
                        }
                    }
                    path.rotate_closed(seam)?;
                } else if distance(from, path.points_mm[n - 1]) < distance(from, path.points_mm[0])
                {
                    path.reverse();
                }
            }
            position = path.points_mm.last().copied();
        }
    }
    Ok(())
}

pub(crate) fn stream(
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
    mut position: Option<[f64; 3]>,
    mut extrusion: f64,
    mut emit: impl FnMut(Event) -> Result<(), ManufacturingError>,
) -> Result<(Option<[f64; 3]>, f64), ManufacturingError> {
    let recipe = &intent.recipe;
    let filament_area = std::f64::consts::PI * (recipe.filament_diameter_mm * 0.5).powi(2);
    let routed = match &intent.printer.workflow {
        JobWorkflow::FixedDualMarlin { job } => Some(job),
        _ => None,
    };
    let safe_z = routed
        .map(|job| crate::routing::safe_z(plan, job))
        .transpose()?;
    let mut active_tool = 0u8;
    let mut group_started = false;
    let mut debt = [0.0f64; 2];
    let mut fan = None;
    let mut travels = 0u32;
    let mut process = 0u32;
    for layer in &plan.layers {
        let mut previous_tool = None;
        check_cancel(cancel)?;
        for path in &layer.paths {
            check_cancel(cancel)?;
            if let Some(job) = routed {
                let tool = job.tool_for_path(path)?;
                if (previous_tool.is_none() && tool != 0)
                    || previous_tool.is_some_and(|previous| previous > tool)
                {
                    return Err(error(
                        ManufacturingErrorCode::InvalidSpecification,
                        "fixed-dual paths must be canonically grouped tool zero then one on every layer",
                    ));
                }
                if previous_tool != Some(tool) {
                    if group_started {
                        let old = &job.tools[usize::from(active_tool)];
                        emit(Event::Barrier)?;
                        let before = (extrusion * 100_000_000.0).round();
                        extrusion -= old.retract_filament_mm;
                        let after = (extrusion * 100_000_000.0).round();
                        debt[usize::from(active_tool)] += (before - after) / 100_000_000.0;
                        emit(Event::Extrusion(extrusion, old.retract_speed_mm_s))?;
                        let from = position.unwrap();
                        let lifted = [from[0], from[1], safe_z.unwrap()];
                        emit(Event::Travel(lifted, recipe.travel_speed_mm_s))?;
                        emit(Event::HotendTarget(active_tool, old.standby_temperature_c))?;
                        emit(Event::SelectTool(tool))?;
                        emit(Event::ResetExtrusion)?;
                        extrusion = 0.0;
                    } else {
                        let from = position.unwrap();
                        emit(Event::HotendWait(
                            tool,
                            job.tools[usize::from(tool)].nozzle_temperature_c,
                        ))?;
                        emit(Event::Travel(
                            [from[0], from[1], safe_z.unwrap()],
                            recipe.travel_speed_mm_s,
                        ))?;
                    }
                    active_tool = tool;
                    let loaded = &job.tools[usize::from(tool)];
                    if group_started {
                        emit(Event::HotendWait(tool, loaded.nozzle_temperature_c))?;
                    }
                    let owed = debt[usize::from(tool)];
                    if owed > 0.0 {
                        extrusion += owed;
                        emit(Event::Extrusion(extrusion, loaded.retract_speed_mm_s))?;
                        debt[usize::from(tool)] = 0.0;
                    }
                    let [start, end] = crate::routing::waste_points(job, tool, layer.z_mm)?;
                    emit(Event::Travel(
                        [start[0], start[1], safe_z.unwrap()],
                        recipe.travel_speed_mm_s,
                    ))?;
                    emit(Event::Travel(start, recipe.travel_speed_mm_s))?;
                    let height = layer.z_mm - layer.lower_z_mm;
                    let flow = recipe.flow_multiplier
                        * if layer.index == 0 {
                            recipe.process.first_layer_flow_factor
                        } else {
                            1.0
                        };
                    let speed = recipe.print_speed_mm_s
                        * if layer.index == 0 {
                            recipe.process.first_layer_speed_factor
                        } else {
                            1.0
                        };
                    let length = distance(start, end);
                    let area = ((recipe.bead_width_mm - height) * height
                        + std::f64::consts::PI * (height * 0.5).powi(2))
                        * flow;
                    let loaded_area =
                        std::f64::consts::PI * (loaded.filament_diameter_mm * 0.5).powi(2);
                    extrusion += length * area / loaded_area;
                    emit(Event::Waste(end, extrusion, speed))?;
                    position = Some(end);
                    previous_tool = Some(tool);
                    group_started = true;
                }
            }
            let target = path.points_mm[0];
            let pwm = if path.kind == DepositionKind::Bridge {
                recipe.process.bridge_fan_pwm
            } else if layer.index == 0 {
                recipe.process.fan_first_layers
            } else {
                recipe.process.fan_after_first_layers
            };
            if fan != Some(pwm) {
                emit(Event::Fan(pwm))?;
                fan = Some(pwm);
            }
            if let Some(safe) = safe_z {
                let from = position.unwrap();
                for point in [
                    [from[0], from[1], safe],
                    [target[0], target[1], safe],
                    target,
                ] {
                    travels = travels.checked_add(1).ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "routed travel count overflow",
                        )
                    })?;
                    if travels > MAX_DEPOSITION_SEGMENTS {
                        return Err(error(
                            ManufacturingErrorCode::ResourceLimit,
                            "routed travel budget exceeded",
                        ));
                    }
                    emit(Event::Travel(point, recipe.travel_speed_mm_s))?;
                }
            }
            let retract = match (position, &recipe.process.retraction) {
                (
                    Some(from),
                    RetractionPolicy::Enabled {
                        minimum_travel_mm, ..
                    },
                ) => distance(from, target) >= *minimum_travel_mm,
                _ => false,
            };
            if routed.is_some() {
                // The route above owns the lifted entry; body retraction is
                // explicitly disabled and feeder debt belongs to selections.
            } else if !retract {
                if travels == MAX_DEPOSITION_SEGMENTS {
                    return Err(error(
                        ManufacturingErrorCode::ResourceLimit,
                        "realized travel budget exceeded",
                    ));
                }
                travels += 1;
                emit(Event::Travel(target, recipe.travel_speed_mm_s))?;
            } else {
                let RetractionPolicy::Enabled {
                    distance_mm,
                    speed_mm_s,
                    z_hop_mm,
                    ..
                } = recipe.process.retraction
                else {
                    unreachable!()
                };
                if process > 100_000 - 2 {
                    return Err(error(
                        ManufacturingErrorCode::ResourceLimit,
                        "process event budget exceeded",
                    ));
                }
                process += 2;
                emit(Event::Extrusion(extrusion - distance_mm, speed_mm_s))?;
                let from = position.unwrap();
                let base_z = from[2].max(target[2]);
                let hop_z = base_z + z_hop_mm;
                if z_hop_mm > 0.0
                    && (!hop_z.is_finite()
                        || project_xyz6_coordinate_mm(hop_z)?
                            <= project_xyz6_coordinate_mm(base_z)?)
                {
                    return Err(error(
                        ManufacturingErrorCode::InvalidSpecification,
                        "Z hop collapses at emitted precision",
                    ));
                }
                let targets = if z_hop_mm > 0.0 {
                    [
                        Some([from[0], from[1], hop_z]),
                        Some([target[0], target[1], hop_z]),
                        Some(target),
                    ]
                } else {
                    [Some(target), None, None]
                };
                for p in targets.into_iter().flatten() {
                    if travels == MAX_DEPOSITION_SEGMENTS {
                        return Err(error(
                            ManufacturingErrorCode::ResourceLimit,
                            "realized travel budget exceeded",
                        ));
                    }
                    travels += 1;
                    emit(Event::Travel(p, recipe.travel_speed_mm_s))?;
                }
                emit(Event::Extrusion(extrusion, speed_mm_s))?;
            }
            let mut speed = recipe.print_speed_mm_s;
            let mut flow = recipe.flow_multiplier;
            if layer.index == 0 {
                speed *= recipe.process.first_layer_speed_factor;
                flow *= recipe.process.first_layer_flow_factor;
            }
            if path.kind == DepositionKind::Bridge {
                speed *= recipe.process.bridge_speed_factor;
                flow *= recipe.process.bridge_flow_factor;
            }
            let height = path.height_mm();
            for edge in path.checked_edges()? {
                check_cancel(cancel)?;
                let from = project_xyz6_point_mm(edge.points_mm[0])?;
                let to = project_xyz6_point_mm(edge.points_mm[1])?;
                let command_length = distance(from, to);
                if command_length < program_coordinates::MIN_DEPOSITION_COMMAND_MM {
                    return Err(error(
                        ManufacturingErrorCode::UnsupportedGeometry,
                        &format!(
                            "deposition edge is below the declared software minimum command length: layer={} kind={:?} source={:?} xyz6={from:?}->{to:?} length_mm={command_length:.17e} minimum_mm={:.17e}",
                            layer.index,
                            path.kind,
                            edge.points_mm,
                            program_coordinates::MIN_DEPOSITION_COMMAND_MM,
                        ),
                    ));
                }
                let area = ((edge.width_mm - height) * height
                    + std::f64::consts::PI * (height * 0.5).powi(2))
                    * flow;
                // Constant-section G1 deposits along the actual XYZ6 chord,
                // not its unrounded source proposal. Otherwise a circular bead
                // can become wholly sub-circular merely through XY rounding.
                let filament_area = routed.map_or(filament_area, |job| {
                    std::f64::consts::PI
                        * (job.tools[usize::from(active_tool)].filament_diameter_mm * 0.5).powi(2)
                });
                extrusion += command_length * area / filament_area;
                emit(Event::Deposit(edge.points_mm[1], extrusion, speed))?;
            }
            position = path.points_mm.last().copied();
        }
    }
    Ok((position, extrusion))
}
