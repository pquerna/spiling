// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Pure normalized-path canonicalization and streamed process realization.
use crate::{check_cancel, error};
use spiling_contracts::manufacturing::program_coordinates::{
    project_xyz6_coordinate_mm, project_xyz6_point_mm,
};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

mod connect;
mod lower;
mod schedule;
pub(crate) mod stage;

#[derive(Clone, Copy)]
pub(crate) enum Event {
    /// A scheduled layer begins (its index); dialects with layer markers
    /// write them here, others ignore it.
    Layer(u32),
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
    /// Single-extruder nozzle/bed targets set without waiting (the
    /// first-layer to later-layer change).
    NozzleTarget(f64),
    BedTarget(f64),
    /// Single-nozzle Bambu AMS support frame on a non-switch layer <= last change.
    AmsTowerFrame {
        layer: u32,
        slot: u8,
        z_mm: f64,
        height_mm: f64,
    },
    /// Single-nozzle Bambu AMS mid-print filament change + flush + optional tower prime.
    AmsSwitch {
        from: u8,
        to: u8,
        layer: u32,
        z_mm: f64,
        height_mm: f64,
    },
}
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2])
}

/// Transient Bambu display data from the normalized order. It is not part of
/// the serialized plan or used by independent replay.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct BambuEstimate {
    pub starts_s: Vec<f64>,
    pub total_s: f64,
    pub filament_mm: f64,
    pub max_z_mm: f64,
    #[serde(default)]
    pub slot_filament_mm: Vec<f64>,
}

#[cfg(test)]
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

#[cfg(test)]
pub(crate) fn canonicalize_with_tool_order(
    plan: &mut NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
    order: spiling_deposition_routing::ToolOrder,
) -> Result<(), ManufacturingError> {
    let hatch_keys = hatch_alternation_keys(&plan.layers);
    prepare_order(plan, intent, &hatch_keys, cancel, order).map(drop)
}

#[derive(serde::Serialize)]
pub(crate) struct StageWork {
    anchor: usize,
    routing: u64,
    arc_runs: usize,
    lowered_edges: usize,
}

pub(crate) struct StageOutput {
    pub(crate) estimate: Option<BambuEstimate>,
    work: StageWork,
}

/// Deposition order is stored in the normalized plan. The emitter receives
/// only the display estimates in addition to the plan, never a motion buffer.
pub(crate) fn prepare(
    plan: &mut NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    hatch_keys: &[u32],
    cancel: &AtomicBool,
) -> Result<StageOutput, ManufacturingError> {
    prepare_order(
        plan,
        intent,
        hatch_keys,
        cancel,
        spiling_deposition_routing::ToolOrder::Free,
    )
}

fn prepare_order(
    plan: &mut NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    hatch_keys: &[u32],
    cancel: &AtomicBool,
    order: spiling_deposition_routing::ToolOrder,
) -> Result<StageOutput, ManufacturingError> {
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
    let anchor = connect::connect_roads(plan, intent, routed.is_some(), hatch_keys, cancel)?;
    // Hidden walls, skins, bridges and interface are the structure class:
    // their lowering budget is the class tolerance less the section's own
    // profile tolerance (RFC 11). Exposed walls and top skin are the surface
    // class and keep every point.
    let lowered = lower::structure_roads(
        plan,
        intent,
        intent.recipe.road_tolerances.structure_mm - PROFILE_TOLERANCE_MM,
        cancel,
    )?;
    let (mut estimate, routing) = schedule::apply(plan, intent, routed, position, cancel, order)?;
    let arc_runs = lower_arcs(plan, intent, estimate.as_mut(), cancel)?;
    Ok(StageOutput {
        estimate,
        work: StageWork {
            anchor,
            routing,
            arc_runs,
            lowered_edges: lowered,
        },
    })
}

fn filament_area(diameter_mm: f64) -> f64 {
    std::f64::consts::PI * (diameter_mm * 0.5).powi(2)
}

fn bead_area(width: f64, height: f64, flow: f64) -> f64 {
    ((width - height) * height + std::f64::consts::PI * (height * 0.5).powi(2)) * flow
}

fn commanded_arc(
    points: &[[f64; 3]],
    run: &ArcRun,
) -> Result<([f64; 2], program_coordinates::CommandedArc), ManufacturingError> {
    let from = project_xyz6_point_mm(points[run.first as usize])?;
    let to = project_xyz6_point_mm(points[run.last as usize])?;
    let center = [
        project_xyz6_coordinate_mm(run.center_mm[0])?,
        project_xyz6_coordinate_mm(run.center_mm[1])?,
    ];
    Ok((
        center,
        program_coordinates::CommandedArc::new(from, to, center, run.clockwise)?,
    ))
}

pub(crate) fn arc_firmware(intent: &ManufacturingIntent) -> Option<ArcFirmware> {
    let capabilities = &intent.printer.capabilities;
    ArcFirmware::for_dialect(capabilities.output_dialect).filter(|_| {
        capabilities.arc_moves
            && !matches!(intent.printer.workflow, JobWorkflow::FixedDualMarlin { .. })
    })
}

/// Keep only the arc runs this printer will execute faithfully, after seams
/// have split them. A run its firmware would segment beyond the recorded
/// profile tolerance, or too short to name its direction safely, stays the
/// chords its points already are. The stored plan is exactly what the
/// emitter writes. Bambu also accounts for the retained projected arcs here.
fn lower_arcs(
    plan: &mut NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    mut estimate: Option<&mut BambuEstimate>,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    let mut runs = 0;
    let firmware = arc_firmware(intent);
    for layer in &mut plan.layers {
        check_cancel(cancel)?;
        for path in &mut layer.paths {
            if path.arcs.is_empty() {
                continue;
            }
            runs += path.arcs.len();
            let Some(firmware) = firmware else {
                path.arcs.clear();
                continue;
            };
            let points = &path.points_mm;
            path.arcs.retain(|run| {
                let (first, last) = (run.first as usize, run.last as usize);
                let radius = run.radius_mm(points);
                let sweep: f64 = (first..last)
                    .map(|i| run.step_radians(points[i], points[i + 1]))
                    .sum();
                let (a, b) = (points[first], points[last]);
                // Segment counts are whole numbers: decide on the worst count
                // that output rounding of the ends and center could produce.
                let worst = [1.0 - 1e-5, 1.0 + 1e-5]
                    .into_iter()
                    .flat_map(|r| [1.0 - 1e-5, 1.0 + 1e-5].map(|t| (r, t)))
                    .map(|(r, t)| firmware.deviation_mm(radius * r, sweep * t))
                    .fold(0.0, f64::max);
                (a[0] - b[0]).hypot(a[1] - b[1]) >= MIN_ARC_CHORD_MM
                    && worst <= MAX_FIRMWARE_ARC_DEVIATION_MM
            });
            if let Some(estimate) = estimate.as_deref_mut()
                && !path.arcs.is_empty()
            {
                // The scheduler counted XYZ6 chords. Replace just the
                // chords covered by retained commanded arcs, after seams.
                let edges = path.checked_edges()?;
                let process = intent.process_for_path(path)?;
                let flow = speeds::flow_factor(&process, path.kind, layer.index);
                let bambu_ams = match &intent.printer.workflow {
                    JobWorkflow::CompleteMarlin { job } => job.bambu_ams(),
                    _ => None,
                };
                let slot = bambu_ams.map_or(0u8, |ams| ams.slot_for_kind(path.kind));
                let area = bambu_ams.map_or_else(
                    || filament_area(intent.recipe.filament_diameter_mm),
                    |ams| ams.slots[usize::from(slot)].filament_area_mm2(),
                );
                for run in &path.arcs {
                    check_cancel(cancel)?;
                    let (_, arc) = commanded_arc(points, run)?;
                    let first = run.first as usize;
                    let width = edges.edge(first).unwrap().width_mm;
                    let mut chord_length = 0.0;
                    for edge in edges.skip(first).take((run.last - run.first) as usize) {
                        chord_length += distance(
                            project_xyz6_point_mm(edge.points_mm[0])?,
                            project_xyz6_point_mm(edge.points_mm[1])?,
                        );
                    }
                    let delta = (arc.length_mm - chord_length)
                        * bead_area(width, path.height_mm(), flow)
                        / area;
                    estimate.filament_mm += delta;
                    if let Some(entry) = estimate.slot_filament_mm.get_mut(usize::from(slot)) {
                        *entry += delta;
                    }
                }
            }
        }
    }
    Ok(runs)
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
    let filament_area = filament_area(recipe.filament_diameter_mm);
    let routed = match &intent.printer.workflow {
        JobWorkflow::FixedDualMarlin { job } => Some(job),
        _ => None,
    };
    let bambu_ams = match &intent.printer.workflow {
        JobWorkflow::CompleteMarlin { job } => job.bambu_ams(),
        _ => None,
    };
    let last_ams_change =
        bambu_ams.and_then(|ams| crate::backend::ams::last_change_layer(plan, ams));
    let safe_z = routed
        .map(|job| crate::routing::safe_z(plan, job))
        .transpose()?;
    let router = routed.map(|job| job.router()).transpose()?;
    let mut active_tool = 0u8;
    let mut debt = [0.0f64; 2];
    let mut fan = None;
    let mut travels = 0u32;
    let mut process_events = 0u32;
    // Later-layer temperature targets are set before the first path above
    // the first layer.
    let single_job = match &intent.printer.workflow {
        JobWorkflow::CompleteMarlin { job } => Some(job),
        _ => None,
    };
    let mut later_nozzle_done = false;
    let mut later_bed_done = false;
    for layer in &plan.layers {
        check_cancel(cancel)?;
        emit(Event::Layer(layer.index))?;
        if let Some(ams) = bambu_ams
            && ams.prime_tower.is_some()
            && last_ams_change.is_some_and(|lc| layer.index <= lc)
            && !crate::backend::ams::layer_has_switch(layer, ams, active_tool)
        {
            emit(Event::AmsTowerFrame {
                layer: layer.index,
                slot: active_tool,
                z_mm: layer.z_mm,
                height_mm: layer.z_mm - layer.lower_z_mm,
            })?;
            position = None;
            extrusion = 0.0;
        }
        // The declared speed rule's cooling decision for this layer.
        let cooling = speeds::layer_cooling(intent, layer, layer.index)?;
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
                    let before = (extrusion * program_coordinates::PROGRAM_E_SCALE).round();
                    extrusion -= old.retract_filament_mm;
                    let after = (extrusion * program_coordinates::PROGRAM_E_SCALE).round();
                    debt[usize::from(active_tool)] +=
                        (before - after) / program_coordinates::PROGRAM_E_SCALE;
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
                if let Some(job) = &router
                    && job.tool_for_path(path)? != tool
                {
                    continue;
                }
                if let Some(ams) = bambu_ams {
                    let needed = ams.slot_for_kind(path.kind);
                    if needed != active_tool {
                        emit(Event::AmsSwitch {
                            from: active_tool,
                            to: needed,
                            layer: layer.index,
                            z_mm: layer.z_mm,
                            height_mm: layer.z_mm - layer.lower_z_mm,
                        })?;
                        active_tool = needed;
                        fan = Some(0);
                        position = None;
                        extrusion = 0.0;
                        if layer.index > 0 {
                            // Switch already settled M109 to later-layer target for `needed`.
                            later_nozzle_done = true;
                        }
                    }
                }
                if layer.index > 0
                    && let Some(job) = single_job
                {
                    if !later_nozzle_done {
                        later_nozzle_done = true;
                        let (nozzle_c, first_nozzle_c) = match bambu_ams {
                            Some(ams) => {
                                let s = &ams.slots[usize::from(active_tool)];
                                (s.nozzle_temperature_c, s.first_layer_nozzle_temperature_c)
                            }
                            None => (
                                job.nozzle_temperature_c,
                                job.first_layer_nozzle_temperature_c,
                            ),
                        };
                        if nozzle_c != first_nozzle_c {
                            emit(Event::NozzleTarget(nozzle_c))?;
                        }
                    }
                    if !later_bed_done {
                        later_bed_done = true;
                        if job.bed_temperature_c != job.first_layer_bed_temperature_c {
                            emit(Event::BedTarget(job.bed_temperature_c))?;
                        }
                    }
                }
                let process = intent.process_for_path(path)?;
                let target = path.points_mm[0];
                let role = speeds::speed_role(recipe, layer, path.kind, path.owner);
                let pwm = speeds::path_fan_pwm(process.policy, role, layer.index, cooling.short);
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
                // Role speed, then the flow ceiling per edge width, then the
                // layer's cooling scale ([`speeds`]).
                let base =
                    speeds::base_speed_mm_s(recipe, &process, role, path.exposed, layer.index)
                        .min(intent.printer.max_print_feed_mm_s);
                let flow = speeds::flow_factor(&process, path.kind, layer.index);
                let height = path.height_mm();
                let edge_speed = |width: f64| {
                    let (unslowed, _) = speeds::flow_limited_speed_mm_s(
                        base,
                        width,
                        height,
                        flow,
                        process.max_volumetric_flow_mm3_s,
                        routed.is_some(),
                    );
                    cooling.speed_mm_s(unslowed, recipe.process.minimum_print_speed_mm_s)
                };
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
                        let last = run.last as usize;
                        let (center, arc) = commanded_arc(&path.points_mm, run)?;
                        let area = bead_area(edge.width_mm, height, flow);
                        let active_filament_area = bambu_ams.map_or(filament_area, |ams| {
                            ams.slots[usize::from(active_tool)].filament_area_mm2()
                        });
                        // The bead follows the commanded arc, so its length
                        // owns the extrusion.
                        extrusion += arc.length_mm * area / active_filament_area;
                        emit(Event::Arc(
                            path.points_mm[last],
                            center,
                            run.clockwise,
                            extrusion,
                            edge_speed(edge.width_mm),
                        ))?;
                        resume = last;
                        continue;
                    }
                    let from = project_xyz6_point_mm(edge.points_mm[0])?;
                    let to = project_xyz6_point_mm(edge.points_mm[1])?;
                    let command_length = distance(from, to);
                    if !program_coordinates::meets_minimum_command(command_length) {
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
                    let area = bead_area(edge.width_mm, height, flow);
                    // Constant-section G1 deposits along the actual XYZ6 chord,
                    // not its unrounded source proposal. Otherwise a circular bead
                    // can become wholly sub-circular merely through XY rounding.
                    let filament_area = routed.map_or_else(
                        || {
                            bambu_ams.map_or(filament_area, |ams| {
                                ams.slots[usize::from(active_tool)].filament_area_mm2()
                            })
                        },
                        |job| {
                            std::f64::consts::PI
                                * (job.tools[usize::from(active_tool)].filament_diameter_mm * 0.5)
                                    .powi(2)
                        },
                    );
                    extrusion += command_length * area / filament_area;
                    emit(Event::Deposit(
                        edge.points_mm[1],
                        extrusion,
                        edge_speed(edge.width_mm),
                    ))?;
                }
                position = path.points_mm.last().copied();
            }
        }
    }
    Ok((position, extrusion))
}
