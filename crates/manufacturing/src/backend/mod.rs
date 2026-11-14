// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{check_cancel, error};
use spiling_chunks::{ChunkSource, ChunkWriter};
use spiling_contracts::manufacturing::program_coordinates::{
    self, project_xyz6_coordinate_mm, project_xyz6_point_mm,
};
use spiling_contracts::manufacturing::*;
use std::fmt::{self, Write};
use std::sync::atomic::AtomicBool;

mod lifecycle;

/// Emit the explicit diagnostic, complete single-extruder or fixed-dual
/// workflow in the printer's dialect; replay is mandatory.
pub fn emit(
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
) -> Result<ChunkSource, ManufacturingError> {
    check_cancel(cancel)?;
    intent.validate()?;
    plan.validate(intent)?;
    let execution = plan.execution_intent(intent)?;
    let intent = execution.as_ref();
    let routed = match &intent.printer.workflow {
        JobWorkflow::FixedDualMarlin { job } => {
            crate::routing::validate_plan(plan, intent, job, cancel)?;
            Some(job)
        }
        _ => None,
    };
    let dialect = intent.printer.capabilities.output_dialect;
    if dialect == OutputDialect::BambuLabGcode3mfV1 {
        return Err(error(
            ManufacturingErrorCode::UnsupportedCapability,
            "Bambu Lab firmware takes a .gcode.3mf delivery package, which is not yet supported; the plan is valid but no program is emitted",
        ));
    }
    let recipe = &intent.recipe;
    let filament_area = std::f64::consts::PI * (recipe.filament_diameter_mm * 0.5).powi(2);
    let travel_feed = recipe.travel_speed_mm_s * 60.0;
    if !travel_feed.is_finite() || travel_feed < 0.000001 {
        return Err(error(
            ManufacturingErrorCode::InvalidSpecification,
            "backend feed or extrusion cannot be represented at declared precision",
        ));
    }
    let mut program = BoundedProgram {
        writer: ChunkWriter::new(u64::from(MAX_MANUFACTURING_PROGRAM_BYTES)),
        max_line: dialect.max_line_bytes(),
        line: 0,
        long_line: false,
    };
    // Complete single-extruder dialects take relative E: every E word is the
    // move's exact E8 delta of the tracked absolute target.
    let relative = dialect.relative_extrusion();
    program
        .write_str(if relative {
            "; SOFTWARE VALIDATION ONLY - NOT MACHINE READY; PHYSICAL QUALIFICATION ABSENT\nG21\nG90\nM83\n"
        } else {
            "; SOFTWARE VALIDATION ONLY - NOT MACHINE READY; PHYSICAL QUALIFICATION ABSENT\nG21\nG90\nM82\n"
        })
        .map_err(limit)?;
    if routed.is_none() {
        program.write_str("G92 E0\n").map_err(limit)?;
    }
    let mut extrusion = 0.0f64;
    let mut emitted_extrusion = 0.0f64;
    let mut position = None;
    let mut active_tool = 0u8;
    let mut built_top = intent.printer.build_envelope.min[2];
    let mut travels = 0u32;
    let mut process_events = 0u32;
    let mut deposits = 0u32;
    if let Some(job) = routed {
        program.write_str("M149 C\nM200 D0\nM220 S100\nM221 T0 S100\nM221 T1 S100\nM107\nM218 T0 X0.000000 Y0.000000 Z0.000000\n").map_err(limit)?;
        let offset = job.tools[1].offset_xy_mm;
        if offset
            .iter()
            .any(|coordinate| project_xyz6_coordinate_mm(*coordinate).is_err())
            || project_xyz6_coordinate_mm(offset[0])? != offset[0]
            || project_xyz6_coordinate_mm(offset[1])? != offset[1]
        {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "hotend offsets must be exactly representable in the frozen XYZ6 dialect",
            ));
        }
        writeln!(program, "M218 T1 X{:.6} Y{:.6} Z0.000000\nT0 S1\nG28\nM420 S0\nG92 E0\nM190 R{:.6}\nM104 T0 S{:.6}\nM104 T1 S{:.6}",
            offset[0], offset[1], job.bed_temperature_c, job.tools[0].standby_temperature_c, job.tools[1].standby_temperature_c).map_err(limit)?;
        let home = project_xyz6_point_mm(job.home_position_mm)?;
        crate::routing::check_motion(home, home, 0, built_top, intent, job)?;
        position = Some(home);
    }
    // Highest emitted Z of a complete job: the park lifts above it.
    let mut top_z = f64::NEG_INFINITY;
    if let JobWorkflow::CompleteMarlin { job } = &intent.printer.workflow {
        for step in &job.lifecycle.start {
            match lifecycle::render(step, &intent.printer, job) {
                lifecycle::Emit::Lines(lines) => {
                    for line in lines {
                        writeln!(program, "{line}").map_err(limit)?;
                    }
                }
                lifecycle::Emit::Park => {
                    return Err(error(
                        ManufacturingErrorCode::InvalidSpecification,
                        "park in the start sequence",
                    ));
                }
                lifecycle::Emit::Prime => {
                    let start = project_xyz6_point_mm(job.prime_start_mm)?;
                    let end = project_xyz6_point_mm(job.prime_end_mm)?;
                    writable_position(start, intent)?;
                    writable_position(end, intent)?;
                    if start == end {
                        return Err(error(
                            ManufacturingErrorCode::InvalidSpecification,
                            "priming collapses at coordinate precision",
                        ));
                    }
                    writeln!(
                        program,
                        "G0 X{:.6} Y{:.6} Z{:.6} F{:.6}",
                        start[0], start[1], start[2], travel_feed
                    )
                    .map_err(limit)?;
                    extrusion = job.prime_filament_length_mm;
                    emitted_extrusion = (extrusion * 100_000_000.0).round() / 100_000_000.0;
                    if emitted_extrusion <= 0.0 {
                        return Err(error(
                            ManufacturingErrorCode::InvalidSpecification,
                            "priming extrusion collapses",
                        ));
                    }
                    writeln!(
                        program,
                        "G1 X{:.6} Y{:.6} Z{:.6} E{} F{:.6}",
                        end[0],
                        end[1],
                        end[2],
                        e_word(extrusion, emitted_extrusion, 0.0, relative),
                        job.prime_speed_mm_s * 60.0
                    )
                    .map_err(limit)?;
                    position = Some(end);
                    top_z = start[2].max(end[2]);
                }
            }
        }
    }
    let (last, last_extrusion) = crate::realization::stream(
        plan,
        intent,
        cancel,
        position,
        extrusion,
        |event| {
            use crate::realization::Event;
            match event {
                Event::Barrier => program.write_str("M400\n").map_err(limit)?,
                Event::HotendTarget(tool, temperature) => {
                    writeln!(program, "M104 T{tool} S{temperature:.6}").map_err(limit)?
                }
                Event::HotendWait(tool, temperature) => {
                    writeln!(program, "M109 T{tool} R{temperature:.6}").map_err(limit)?
                }
                Event::ResetExtrusion => {
                    program.write_str("G92 E0\n").map_err(limit)?;
                    emitted_extrusion = 0.0;
                }
                Event::SelectTool(tool) => {
                    let job = routed.ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::InvalidSpecification,
                            "tool selection outside routed workflow",
                        )
                    })?;
                    let from = position.unwrap();
                    let old = job.tools[usize::from(active_tool)].offset_xy_mm;
                    let new = job.tools[usize::from(tool)].offset_xy_mm;
                    let shifted = [
                        from[0] + new[0] - old[0],
                        from[1] + new[1] - old[1],
                        from[2],
                    ];
                    crate::routing::check_motion(shifted, shifted, tool, built_top, intent, job)?;
                    writeln!(program, "T{tool} S1").map_err(limit)?;
                    position = Some(shifted);
                    active_tool = tool;
                }
                Event::Fan(pwm) => writeln!(program, "M106 S{pwm}").map_err(limit)?,
                Event::Extrusion(e, speed) => {
                    count_event(&mut process_events)?;
                    let rounded = (e * 100_000_000.0).round() / 100_000_000.0;
                    let feed = (speed * 60_000_000.0).round() / 1_000_000.0;
                    if !feed.is_finite()
                        || feed <= 0.0
                        || feed / 60.0 > intent.printer.max_travel_feed_mm_s
                    {
                        return Err(error(
                            ManufacturingErrorCode::InvalidSpecification,
                            "actual stationary extrusion feed exceeds printer limit",
                        ));
                    }
                    if position.is_none() || !rounded.is_finite() || rounded == emitted_extrusion {
                        return Err(error(
                            ManufacturingErrorCode::InvalidSpecification,
                            "stationary extrusion collapses or lacks known position",
                        ));
                    }
                    let e = if routed.is_some() { rounded } else { e };
                    writeln!(
                        program,
                        "G1 E{} F{feed:.6}",
                        e_word(e, rounded, emitted_extrusion, relative)
                    )
                    .map_err(limit)?;
                    emitted_extrusion = rounded;
                }
                Event::Arc(target, center, clockwise, e, speed) => {
                    // Arcs come only from a printer declaring arc moves and
                    // never from routed jobs (rejected by validation).
                    let from = position.ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::InvalidSpecification,
                            "deposition before positioning",
                        )
                    })?;
                    let to = project_xyz6_point_mm(target)?;
                    writable_position(to, intent)?;
                    let arc = program_coordinates::CommandedArc::new(from, to, center, clockwise)?;
                    let [lo, hi] = arc.xy_bounds();
                    writable_position([lo[0], lo[1], to[2]], intent)?;
                    writable_position([hi[0], hi[1], to[2]], intent)?;
                    count_event(&mut deposits)?;
                    built_top = built_top.max(to[2]);
                    let feed = (speed * 60_000_000.0).round() / 1_000_000.0;
                    if !feed.is_finite()
                        || feed <= 0.0
                        || feed / 60.0 > intent.printer.max_print_feed_mm_s
                    {
                        return Err(error(
                            ManufacturingErrorCode::InvalidSpecification,
                            "actual emitted feed exceeds printer limit or precision",
                        ));
                    }
                    let rounded = (e * 100_000_000.0).round() / 100_000_000.0;
                    let delta = rounded - emitted_extrusion;
                    if !rounded.is_finite()
                        || arc.length_mm < program_coordinates::MIN_DEPOSITION_COMMAND_MM
                        || delta <= 0.0
                    {
                        return Err(error(
                            ManufacturingErrorCode::UnsupportedGeometry,
                            "arc deposition collapses at serialized precision",
                        ));
                    }
                    let flow_limit = intent.process_for_tool(None)?.max_volumetric_flow_mm3_s;
                    let flow = delta * filament_area * feed / 60.0 / arc.length_mm;
                    let rounding_slack = (flow_limit * 0.000002
                        + 0.00000001 * filament_area * speed)
                        / arc.length_mm;
                    if !flow.is_finite() || flow > flow_limit + rounding_slack {
                        return Err(error(
                            ManufacturingErrorCode::InvalidSpecification,
                            "actual decoded deposition flow exceeds printer limit",
                        ));
                    }
                    writeln!(
                        program,
                        "{} X{:.6} Y{:.6} Z{:.6} I{:.6} J{:.6} E{} F{feed:.6}",
                        if clockwise { "G2" } else { "G3" },
                        to[0],
                        to[1],
                        to[2],
                        center[0] - from[0],
                        center[1] - from[1],
                        e_word(e, rounded, emitted_extrusion, relative),
                    )
                    .map_err(limit)?;
                    emitted_extrusion = rounded;
                    position = Some(to);
                    top_z = top_z.max(to[2]);
                }
                Event::Travel(target, speed)
                | Event::Deposit(target, _, speed)
                | Event::Waste(target, _, speed) => {
                    let to = project_xyz6_point_mm(target)?;
                    writable_position(to, intent)?;
                    if matches!(event, Event::Deposit(..) | Event::Waste(..)) {
                        count_event(&mut deposits)?;
                        built_top = built_top.max(to[2]);
                    } else {
                        count_event(&mut travels)?;
                    }
                    if let Some(job) = routed {
                        crate::routing::check_motion(
                            position.unwrap(),
                            to,
                            active_tool,
                            built_top,
                            intent,
                            job,
                        )?;
                    }
                    let feed = (speed * 60_000_000.0).round() / 1_000_000.0;
                    let feed_limit = if matches!(event, Event::Travel(..)) {
                        intent.printer.max_travel_feed_mm_s
                    } else {
                        intent.printer.max_print_feed_mm_s
                    };
                    if !feed.is_finite() || feed <= 0.0 || feed / 60.0 > feed_limit {
                        return Err(error(
                            ManufacturingErrorCode::InvalidSpecification,
                            "actual emitted feed exceeds printer limit or precision",
                        ));
                    }
                    if let Event::Deposit(_, e, _) | Event::Waste(_, e, _) = event {
                        let from = position.ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::InvalidSpecification,
                                "deposition before positioning",
                            )
                        })?;
                        let length = (to[0] - from[0])
                            .hypot(to[1] - from[1])
                            .hypot(to[2] - from[2]);
                        let rounded = (e * 100_000_000.0).round() / 100_000_000.0;
                        let delta = rounded - emitted_extrusion;
                        if !rounded.is_finite() || length <= 0.0 || delta <= 0.0 {
                            return Err(error(
                                ManufacturingErrorCode::UnsupportedGeometry,
                                format!(
                                    "deposition collapses at serialized precision: from={from:?}, target={target:?}, rounded={to:?}, filament_delta={delta:.12}, filament={e:.12}"
                                ),
                            ));
                        }
                        let filament_area = routed.map_or(filament_area, |job| {
                            std::f64::consts::PI
                                * (job.tools[usize::from(active_tool)].filament_diameter_mm * 0.5)
                                    .powi(2)
                        });
                        if let (Some(job), Event::Waste(..)) = (routed, event)
                            && delta * filament_area
                                < job.tools[usize::from(active_tool)].minimum_reprime_volume_mm3
                        {
                            return Err(error(
                                ManufacturingErrorCode::InvalidSpecification,
                                "actual E8 waste volume is below declared reprime minimum",
                            ));
                        }
                        let flow_limit = intent
                            .process_for_tool(routed.map(|_| active_tool))?
                            .max_volumetric_flow_mm3_s;
                        let flow = delta * filament_area * feed / 60.0 / length;
                        let rounding_slack =
                            (flow_limit * 0.000002 + 0.00000001 * filament_area * speed) / length;
                        if !flow.is_finite() || flow > flow_limit + rounding_slack {
                            return Err(error(
                                ManufacturingErrorCode::InvalidSpecification,
                                "actual decoded deposition flow exceeds printer limit",
                            ));
                        }
                        let e = if routed.is_some() { rounded } else { e };
                        writeln!(
                            program,
                            "G1 X{:.6} Y{:.6} Z{:.6} E{} F{feed:.6}",
                            to[0],
                            to[1],
                            to[2],
                            e_word(e, rounded, emitted_extrusion, relative)
                        )
                        .map_err(limit)?;
                        emitted_extrusion = rounded;
                    } else {
                        writeln!(
                            program,
                            "G0 X{:.6} Y{:.6} Z{:.6} F{feed:.6}",
                            to[0], to[1], to[2]
                        )
                        .map_err(limit)?;
                    }
                    position = Some(to);
                    top_z = top_z.max(to[2]);
                }
            }
            Ok(())
        },
    )?;
    if let JobWorkflow::CompleteMarlin { job } = &intent.printer.workflow {
        let mut last = last.ok_or_else(|| {
            error(
                ManufacturingErrorCode::InvalidSpecification,
                "park without a known position",
            )
        })?;
        for step in &job.lifecycle.end {
            match lifecycle::render(step, &intent.printer, job) {
                lifecycle::Emit::Lines(lines) => {
                    for line in lines {
                        writeln!(program, "{line}").map_err(limit)?;
                    }
                }
                lifecycle::Emit::Prime => {
                    return Err(error(
                        ManufacturingErrorCode::InvalidSpecification,
                        "prime in the end sequence",
                    ));
                }
                lifecycle::Emit::Park => {
                    // Lift above everything emitted, never to a fixed height.
                    let height = (top_z.max(last[2]) + job.park_clearance_mm)
                        .min(intent.printer.build_envelope.max[2]);
                    let lift = project_xyz6_point_mm([last[0], last[1], height])?;
                    let park =
                        project_xyz6_point_mm([job.park_xy_mm[0], job.park_xy_mm[1], height])?;
                    writable_position(lift, intent)?;
                    writable_position(park, intent)?;
                    if lift[2] <= project_xyz6_coordinate_mm(last[2])? || lift[2] <= top_z {
                        return Err(error(
                            ManufacturingErrorCode::InvalidSpecification,
                            "park clearance above the highest emitted Z collapses within the envelope",
                        ));
                    }
                    for target in [lift, park] {
                        writeln!(
                            program,
                            "G0 X{:.6} Y{:.6} Z{:.6} F{:.6}",
                            target[0], target[1], target[2], travel_feed
                        )
                        .map_err(limit)?;
                    }
                    last = park;
                }
            }
        }
    }
    if let Some(job) = routed {
        let last = position.unwrap();
        let tool = &job.tools[usize::from(active_tool)];
        let retract = last_extrusion - tool.retract_filament_mm;
        let rounded = (retract * 100_000_000.0).round() / 100_000_000.0;
        if !rounded.is_finite() || rounded >= emitted_extrusion {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "final feeder retract collapses at E8 precision",
            ));
        }
        let park = project_xyz6_point_mm(job.park_position_mm)?;
        let lift = [last[0], last[1], park[2]];
        let retract_feed = (tool.retract_speed_mm_s * 60_000_000.0).round() / 1_000_000.0;
        if !retract_feed.is_finite()
            || retract_feed <= 0.0
            || retract_feed / 60.0 > intent.printer.max_travel_feed_mm_s
        {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "final feeder retract feed exceeds actual command envelope",
            ));
        }
        crate::routing::check_motion(last, lift, active_tool, built_top, intent, job)?;
        crate::routing::check_motion(lift, park, active_tool, built_top, intent, job)?;
        for _ in 0..2 {
            count_event(&mut travels)?;
        }
        count_event(&mut process_events)?;
        writeln!(program, "M400\nG1 E{rounded:.8} F{:.6}\nG0 X{:.6} Y{:.6} Z{:.6} F{travel_feed:.6}\nG0 X{:.6} Y{:.6} Z{:.6} F{travel_feed:.6}\nM400\nM104 T0 S0\nM104 T1 S0\nM140 S0\nM107\nM84",
            retract_feed, lift[0], lift[1], lift[2], park[0], park[1], park[2]).map_err(limit)?;
    }
    check_cancel(cancel)?;
    if program.long_line {
        return Err(error(
            ManufacturingErrorCode::UnsupportedCapability,
            "a command line exceeds the firmware's command buffer (Marlin/Prusa MAX_CMD_SIZE, 95 bytes)",
        ));
    }
    program.writer.finish().map_err(|_| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "emitted program byte limit exceeded",
        )
    })
}
/// The E word of an extruding command. Absolute dialects write the target;
/// relative dialects write the exact E8 delta between the rounded target and
/// the previously emitted rounded position, so the firmware's running sum of
/// deltas equals the absolute E8 lattice position without float drift.
fn e_word(target: f64, rounded: f64, emitted: f64, relative: bool) -> String {
    if !relative {
        return format!("{target:.8}");
    }
    let units = (rounded * 100_000_000.0).round() as i64 - (emitted * 100_000_000.0).round() as i64;
    let sign = if units < 0 { "-" } else { "" };
    let magnitude = units.unsigned_abs();
    format!(
        "{sign}{}.{:08}",
        magnitude / 100_000_000,
        magnitude % 100_000_000
    )
}
fn writable_position(
    point: [f64; 3],
    intent: &ManufacturingIntent,
) -> Result<(), ManufacturingError> {
    let b = intent.printer.build_envelope;
    if (0..3).any(|a| !point[a].is_finite() || point[a] < b.min[a] || point[a] > b.max[a]) {
        return Err(error(
            ManufacturingErrorCode::InvalidSpecification,
            "rounded emitted coordinate exceeds build envelope",
        ));
    }
    Ok(())
}
/// Program text compressed frame by frame as it is emitted, refusing output
/// beyond the program byte limit and recording any line longer than the
/// dialect's firmware command buffer.
struct BoundedProgram {
    writer: ChunkWriter,
    max_line: Option<usize>,
    line: usize,
    long_line: bool,
}
impl Write for BoundedProgram {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if let Some(max) = self.max_line {
            for byte in s.bytes() {
                if byte == b'\n' {
                    self.line = 0;
                } else {
                    self.line += 1;
                    self.long_line |= self.line > max;
                }
            }
        }
        std::io::Write::write_all(&mut self.writer, s.as_bytes()).map_err(|_| fmt::Error)
    }
}
fn limit(_: fmt::Error) -> ManufacturingError {
    error(
        ManufacturingErrorCode::ResourceLimit,
        "emitted program byte limit exceeded",
    )
}

/// Program counts are u32 on the wire; their width is the only count limit.
/// Memory is bounded by the streamed program's byte format.
fn count_event(counter: &mut u32) -> Result<(), ManufacturingError> {
    *counter = counter.checked_add(1).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "program event count exceeds its u32 representation",
        )
    })?;
    Ok(())
}
