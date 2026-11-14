// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Pure normalized-path canonicalization and streamed process realization.
use crate::{check_cancel, error};
use spiling_contracts::manufacturing::program_coordinates::{
    project_xyz6_coordinate_mm, project_xyz6_point_mm,
};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

mod schedule;

#[derive(Clone, Copy)]
pub(crate) enum Event {
    Fan(u8),
    Travel([f64; 3], f64),
    Extrusion(f64, f64), // stationary absolute E, speed
    Deposit([f64; 3], f64, f64),
    /// One planned arc run: target, commanded center, clockwise, absolute E
    /// and speed. Only for a printer that declares arc moves.
    Arc([f64; 3], [f64; 2], bool, f64, f64),
    Waste([f64; 3], f64, f64),
    Barrier,
    SelectTool(u8),
    ResetExtrusion,
    HotendTarget(u8, f64),
    HotendWait(u8, f64),
}
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2])
}

/// Deposition order is stored in the normalized plan, never hidden in the
/// emitter. The scheduler's transient decisions are released before streaming;
/// no second motion/index stream is retained.
pub(crate) fn canonicalize(
    plan: &mut NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    canonicalize_with_tool_order(
        plan,
        intent,
        cancel,
        spiling_deposition_routing::ToolOrder::Free,
    )
}

pub(crate) fn canonicalize_with_tool_order(
    plan: &mut NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
    order: spiling_deposition_routing::ToolOrder,
) -> Result<(), ManufacturingError> {
    plan.validate(intent)?;
    let routed = match &intent.printer.workflow {
        JobWorkflow::FixedDualMarlin { job } => {
            crate::routing::validate_plan(plan, intent, job, cancel)?;
            Some(job)
        }
        _ => None,
    };
    let position = match &intent.printer.workflow {
        JobWorkflow::DiagnosticBodyOnly => None,
        JobWorkflow::CompleteMarlin { job } => Some(project_xyz6_point_mm(job.prime_end_mm)?),
        JobWorkflow::FixedDualMarlin { .. } => None,
    };
    schedule::apply(plan, intent, routed, position, cancel, order)?;
    Ok(())
}

/// Streamed counts are u32 on the wire; their width is the only count limit.
fn count_event(counter: &mut u32) -> Result<(), ManufacturingError> {
    *counter = counter.checked_add(1).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "realized event count exceeds its u32 representation",
        )
    })?;
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
    let mut debt = [0.0f64; 2];
    let mut fan = None;
    let mut travels = 0u32;
    let mut process_events = 0u32;
    for layer in &plan.layers {
        check_cancel(cancel)?;
        let order: &[u8] = if routed.is_some() {
            plan.routing_tool_orders
                .get(layer.index as usize)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::InvalidSpecification,
                        "missing scheduled tool order",
                    )
                })?
        } else {
            &[0]
        };
        for &tool in order {
            if let Some(job) = routed {
                let [start, end] = crate::routing::waste_points(job, tool, layer.z_mm)?;
                let switched = active_tool != tool;
                if switched {
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
                    let incoming = job.tools[usize::from(tool)].offset_xy_mm;
                    // Stage the same carriage at the incoming waste lane
                    // while coordinates still name the outgoing hotend.
                    emit(Event::Travel(
                        [
                            start[0] + old.offset_xy_mm[0] - incoming[0],
                            start[1] + old.offset_xy_mm[1] - incoming[1],
                            safe_z.unwrap(),
                        ],
                        recipe.travel_speed_mm_s,
                    ))?;
                    emit(Event::Barrier)?;
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
                if switched {
                    emit(Event::HotendWait(tool, loaded.nozzle_temperature_c))?;
                }
                let owed = debt[usize::from(tool)];
                if owed > 0.0 {
                    extrusion += owed;
                    emit(Event::Extrusion(extrusion, loaded.retract_speed_mm_s))?;
                    debt[usize::from(tool)] = 0.0;
                }
                if !switched {
                    emit(Event::Travel(
                        [start[0], start[1], safe_z.unwrap()],
                        recipe.travel_speed_mm_s,
                    ))?;
                }
                emit(Event::Travel(start, recipe.travel_speed_mm_s))?;
                let process = intent.process_for_tool(Some(tool))?;
                let height = layer.z_mm - layer.lower_z_mm;
                let flow = process.flow_multiplier
                    * if layer.index == 0 {
                        process.policy.first_layer_flow_factor
                    } else {
                        1.0
                    };
                let speed = process.speed_mm_s
                    * if layer.index == 0 {
                        process.policy.first_layer_speed_factor
                    } else {
                        1.0
                    };
                let length = distance(start, end);
                let area = ((loaded.waste_bead_width_mm - height) * height
                    + std::f64::consts::PI * (height * 0.5).powi(2))
                    * flow;
                let loaded_area =
                    std::f64::consts::PI * (loaded.filament_diameter_mm * 0.5).powi(2);
                extrusion += length * area / loaded_area;
                emit(Event::Waste(end, extrusion, speed))?;
                position = Some(end);
            }
            for path in &layer.paths {
                check_cancel(cancel)?;
                if let Some(job) = routed
                    && job.tool_for_path(path)? != tool
                {
                    continue;
                }
                let process = intent.process_for_path(path)?;
                let target = path.points_mm[0];
                let pwm = if path.kind == DepositionKind::Bridge {
                    process.policy.bridge_fan_pwm
                } else if layer.index == 0 {
                    process.policy.fan_first_layers
                } else {
                    process.policy.fan_after_first_layers
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
                        count_event(&mut travels)?;
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
                    count_event(&mut travels)?;
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
                    count_event(&mut process_events)?;
                    count_event(&mut process_events)?;
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
                        count_event(&mut travels)?;
                        emit(Event::Travel(p, recipe.travel_speed_mm_s))?;
                    }
                    emit(Event::Extrusion(extrusion, speed_mm_s))?;
                }
                let mut speed = process.speed_mm_s;
                let mut flow = process.flow_multiplier;
                if layer.index == 0 {
                    speed *= process.policy.first_layer_speed_factor;
                    flow *= process.policy.first_layer_flow_factor;
                }
                if path.kind == DepositionKind::Bridge {
                    speed *= process.policy.bridge_speed_factor;
                    flow *= process.policy.bridge_flow_factor;
                }
                let height = path.height_mm();
                let mut runs = if intent.printer.capabilities.arc_moves && routed.is_none() {
                    path.arcs.as_slice()
                } else {
                    &[]
                }
                .iter()
                .peekable();
                let mut resume = 0usize;
                for (index, edge) in path.checked_edges()?.enumerate() {
                    check_cancel(cancel)?;
                    if index < resume {
                        continue;
                    }
                    if let Some(run) = runs.next_if(|run| run.first as usize == index) {
                        let (first, last) = (run.first as usize, run.last as usize);
                        let from = project_xyz6_point_mm(path.points_mm[first])?;
                        let to = project_xyz6_point_mm(path.points_mm[last])?;
                        let center = [
                            program_coordinates::project_xyz6_coordinate_mm(run.center_mm[0])?,
                            program_coordinates::project_xyz6_coordinate_mm(run.center_mm[1])?,
                        ];
                        let arc = program_coordinates::CommandedArc::new(
                            from,
                            to,
                            center,
                            run.clockwise,
                        )?;
                        let area = ((edge.width_mm - height) * height
                            + std::f64::consts::PI * (height * 0.5).powi(2))
                            * flow;
                        // The bead follows the commanded arc, so its length
                        // owns the extrusion.
                        extrusion += arc.length_mm * area / filament_area;
                        emit(Event::Arc(
                            path.points_mm[last],
                            center,
                            run.clockwise,
                            extrusion,
                            speed,
                        ))?;
                        resume = last;
                        continue;
                    }
                    let from = project_xyz6_point_mm(edge.points_mm[0])?;
                    let to = project_xyz6_point_mm(edge.points_mm[1])?;
                    let command_length = distance(from, to);
                    if command_length < program_coordinates::MIN_DEPOSITION_COMMAND_MM {
                        return Err(error(
                            ManufacturingErrorCode::UnsupportedGeometry,
                            format!(
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
                            * (job.tools[usize::from(active_tool)].filament_diameter_mm * 0.5)
                                .powi(2)
                    });
                    extrusion += command_length * area / filament_area;
                    emit(Event::Deposit(edge.points_mm[1], extrusion, speed))?;
                }
                position = path.points_mm.last().copied();
            }
        }
    }
    Ok((position, extrusion))
}
