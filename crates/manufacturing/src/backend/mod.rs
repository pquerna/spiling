// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{check_cancel, error};
use spiling_contracts::manufacturing::*;
use std::fmt::{self, Write};
use std::sync::atomic::AtomicBool;

/// Emit only the software-validation dialect. Independent replay is mandatory.
pub fn emit(
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
) -> Result<String, ManufacturingError> {
    check_cancel(cancel)?;
    intent.validate()?;
    plan.validate(intent)?;
    let recipe = &intent.recipe;
    let bead_area = (recipe.bead_width_mm - recipe.layer_height_mm) * recipe.layer_height_mm
        + std::f64::consts::PI * (recipe.layer_height_mm * 0.5).powi(2);
    let filament_area = std::f64::consts::PI * (recipe.filament_diameter_mm * 0.5).powi(2);
    let extrusion_per_mm = bead_area * recipe.flow_multiplier / filament_area;
    let print_feed = recipe.print_speed_mm_s * 60.0;
    let travel_feed = recipe.travel_speed_mm_s * 60.0;
    if !extrusion_per_mm.is_finite()
        || !print_feed.is_finite()
        || !travel_feed.is_finite()
        || print_feed < 0.000001
        || travel_feed < 0.000001
    {
        return Err(error(
            ManufacturingErrorCode::InvalidSpecification,
            "backend feed or extrusion cannot be represented at declared precision",
        ));
    }
    let mut program = BoundedProgram(String::new());
    program.write_str("; SOFTWARE VALIDATION ONLY - NOT MACHINE READY\n; No homing, heating, startup, collision or physical printability certification\nG21\nG90\nM82\nG92 E0\n").map_err(limit)?;
    let mut extrusion = 0.0f64;
    let mut emitted_extrusion = 0.0f64;
    for layer in &plan.layers {
        check_cancel(cancel)?;
        for path in &layer.paths {
            check_cancel(cancel)?;
            let first = path.points_mm[0].map(round_xyz);
            writable_position(first, intent)?;
            writeln!(
                program,
                "G0 X{:.6} Y{:.6} Z{:.6} F{:.6}",
                first[0], first[1], first[2], travel_feed
            )
            .map_err(limit)?;
            let mut last = first;
            for edge in path.points_mm.windows(2) {
                check_cancel(cancel)?;
                let destination = edge[1].map(round_xyz);
                writable_position(destination, intent)?;
                if destination == last {
                    return Err(error(
                        ManufacturingErrorCode::UnsupportedGeometry,
                        "deposition collapses at emitted coordinate precision",
                    ));
                }
                let length = (edge[1][0] - edge[0][0])
                    .hypot(edge[1][1] - edge[0][1])
                    .hypot(edge[1][2] - edge[0][2]);
                extrusion += length * extrusion_per_mm;
                let rounded = (extrusion * 100_000_000.0).round() / 100_000_000.0;
                if !rounded.is_finite() || rounded <= emitted_extrusion {
                    return Err(error(
                        ManufacturingErrorCode::InvalidSpecification,
                        "deposition extrusion collapses or overflows at emitted precision",
                    ));
                }
                writeln!(
                    program,
                    "G1 X{:.6} Y{:.6} Z{:.6} E{:.8} F{:.6}",
                    destination[0], destination[1], destination[2], extrusion, print_feed
                )
                .map_err(limit)?;
                emitted_extrusion = rounded;
                last = destination;
            }
        }
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
