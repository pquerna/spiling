// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{check_cancel, error};
use spiling_contracts::manufacturing::*;
use std::fmt::{self, Write};
use std::sync::atomic::AtomicBool;

/// Emit the explicit diagnostic or complete typed Marlin workflow; replay is mandatory.
pub fn emit(
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
) -> Result<String, ManufacturingError> {
    check_cancel(cancel)?;
    intent.validate()?;
    plan.validate(intent)?;
    let recipe = &intent.recipe;
    let filament_area = std::f64::consts::PI * (recipe.filament_diameter_mm * 0.5).powi(2);
    let travel_feed = recipe.travel_speed_mm_s * 60.0;
    if !travel_feed.is_finite() || travel_feed < 0.000001 {
        return Err(error(
            ManufacturingErrorCode::InvalidSpecification,
            "backend feed or extrusion cannot be represented at declared precision",
        ));
    }
    let mut program = BoundedProgram(String::new());
    program.write_str("; SOFTWARE VALIDATION ONLY - NOT MACHINE READY; PHYSICAL QUALIFICATION ABSENT\nG21\nG90\nM82\nG92 E0\n").map_err(limit)?;
    let mut extrusion = 0.0f64;
    let mut emitted_extrusion = 0.0f64;
    let mut position = None;
    if let JobWorkflow::CompleteMarlin { job } = &intent.printer.workflow {
        // Reviewed Marlin 2 subset: resets scale/volumetric state, homes native
        // axes, explicitly disables leveling, and waits for heating OR cooling.
        program
            .write_str("M149 C\nM200 D0\nM220 S100\nM221 S100\nM107\nG28\nM420 S0\n")
            .map_err(limit)?;
        writeln!(
            program,
            "M190 R{:.6}\nM109 R{:.6}",
            job.bed_temperature_c, job.nozzle_temperature_c
        )
        .map_err(limit)?;
        let start = job.prime_start_mm.map(round_xyz);
        let end = job.prime_end_mm.map(round_xyz);
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
            "G1 X{:.6} Y{:.6} Z{:.6} E{:.8} F{:.6}",
            end[0],
            end[1],
            end[2],
            extrusion,
            job.prime_speed_mm_s * 60.0
        )
        .map_err(limit)?;
        position = Some(end);
    }
    let (last, _) = crate::realization::stream(
        plan,
        intent,
        cancel,
        position,
        extrusion,
        |event| {
            use crate::realization::Event;
            match event {
                Event::Fan(pwm) => writeln!(program, "M106 S{pwm}").map_err(limit)?,
                Event::Extrusion(e, speed) => {
                    let rounded = (e * 100_000_000.0).round() / 100_000_000.0;
                    let feed = (speed * 60_000_000.0).round() / 1_000_000.0;
                    if !feed.is_finite()
                        || feed <= 0.0
                        || feed / 60.0 > intent.printer.max_feed_mm_s
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
                    writeln!(program, "G1 E{e:.8} F{feed:.6}").map_err(limit)?;
                    emitted_extrusion = rounded;
                }
                Event::Travel(target, speed) | Event::Deposit(target, _, speed) => {
                    let to = target.map(round_xyz);
                    writable_position(to, intent)?;
                    let feed = (speed * 60_000_000.0).round() / 1_000_000.0;
                    if !feed.is_finite()
                        || feed <= 0.0
                        || feed / 60.0 > intent.printer.max_feed_mm_s
                    {
                        return Err(error(
                            ManufacturingErrorCode::InvalidSpecification,
                            "actual emitted feed exceeds printer limit or precision",
                        ));
                    }
                    if let Event::Deposit(_, e, _) = event {
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
                        let flow = delta * filament_area * feed / 60.0 / length;
                        let rounding_slack = (intent.printer.max_volumetric_flow_mm3_s * 0.000002
                            + 0.00000001 * filament_area * speed)
                            / length;
                        if !flow.is_finite()
                            || flow > intent.printer.max_volumetric_flow_mm3_s + rounding_slack
                        {
                            return Err(error(
                                ManufacturingErrorCode::InvalidSpecification,
                                "actual decoded deposition flow exceeds printer limit",
                            ));
                        }
                        writeln!(
                            program,
                            "G1 X{:.6} Y{:.6} Z{:.6} E{e:.8} F{feed:.6}",
                            to[0], to[1], to[2]
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
                }
            }
            Ok(())
        },
    )?;
    if let JobWorkflow::CompleteMarlin { job } = &intent.printer.workflow {
        let last = last.unwrap();
        let lift = [last[0], last[1], job.park_position_mm[2]].map(round_xyz);
        let park = job.park_position_mm.map(round_xyz);
        writable_position(lift, intent)?;
        writable_position(park, intent)?;
        if lift[2] <= round_xyz(last[2]) {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "park clearance collapses at coordinate precision",
            ));
        }
        program.write_str("M400\n").map_err(limit)?;
        writeln!(
            program,
            "G0 X{:.6} Y{:.6} Z{:.6} F{:.6}",
            lift[0], lift[1], lift[2], travel_feed
        )
        .map_err(limit)?;
        writeln!(
            program,
            "G0 X{:.6} Y{:.6} Z{:.6} F{:.6}",
            park[0], park[1], park[2], travel_feed
        )
        .map_err(limit)?;
        program
            .write_str("M400\nM104 S0\nM140 S0\nM107\nM84\n")
            .map_err(limit)?;
    }
    check_cancel(cancel)?;
    Ok(program.0)
}
fn round_xyz(v: f64) -> f64 {
    let v = (v * 1_000_000.0).round() / 1_000_000.0;
    if v == 0.0 { 0.0 } else { v }
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
struct BoundedProgram(String);
impl Write for BoundedProgram {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if s.len() > MAX_MANUFACTURING_BUNDLE_BYTES as usize - self.0.len() {
            return Err(fmt::Error);
        }
        self.0.push_str(s);
        Ok(())
    }
}
fn limit(_: fmt::Error) -> ManufacturingError {
    error(
        ManufacturingErrorCode::ResourceLimit,
        "emitted program byte limit exceeded",
    )
}
