// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Producer-side single-nozzle Bambu Lab AMS command and prime-tower emission.
//!
//! Sequences are re-authored cleanly from read-only observation of Bambu Studio
//! 02.09.01.70 A1 / A1 mini / P1S / X1C machine profiles (`M620`/`M621`,
//! `T<slot>`, `; FLUSH_START`..`; FLUSH_END`, series cutter/chute/wiper
//! coordinates, and `; WIPE_TOWER_START`..`; WIPE_TOWER_END`). The independent
//! verifier (`verifier::ams`) decodes and proves every emitted command on its
//! own without importing this module.

use crate::{check_cancel, error};
use spiling_contracts::manufacturing::program_coordinates::{
    PROGRAM_E_SCALE, PROGRAM_F_SCALE, project_xyz6_coordinate_mm, project_xyz6_point_mm,
};
use spiling_contracts::manufacturing::*;
use std::fmt::Write;
use std::sync::atomic::AtomicBool;

/// Maximum filament length extruded per single flush G1 command (mm).
const MAX_FLUSH_CHUNK_MM: f64 = 50.0;

/// Per-layer mutable state while emitting prime-tower passes.
#[derive(Default)]
pub(crate) struct TowerLayerState {
    pub layer: Option<u32>,
    pub frame_printed: bool,
    pub next_row: usize,
}

impl TowerLayerState {
    fn reset_for(&mut self, layer: u32) {
        if self.layer != Some(layer) {
            self.layer = Some(layer);
            self.frame_printed = false;
            self.next_row = 0;
        }
    }
}

fn bead_area(width: f64, height: f64) -> f64 {
    (width - height) * height + std::f64::consts::PI * (height * 0.5).powi(2)
}

fn dist2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn segment_intersects_rect(a: [f64; 2], b: [f64; 2], min: [f64; 2], max: [f64; 2]) -> bool {
    let mut enter = 0.0_f64;
    let mut leave = 1.0_f64;
    for axis in 0..2 {
        let delta = b[axis] - a[axis];
        if delta == 0.0 {
            if a[axis] < min[axis] || a[axis] > max[axis] {
                return false;
            }
        } else {
            let t0 = (min[axis] - a[axis]) / delta;
            let t1 = (max[axis] - a[axis]) / delta;
            enter = enter.max(t0.min(t1));
            leave = leave.min(t0.max(t1));
            if enter > leave {
                return false;
            }
        }
    }
    true
}

/// Highest layer index on which a mid-print AMS slot transition occurs,
/// starting from initial slot 0 across the scheduled plan paths.
pub(crate) fn last_change_layer(plan: &NormalizedPrintPlan, ams: &BambuAms) -> Option<u32> {
    let mut active = 0u8;
    let mut last = None;
    for layer in &plan.layers {
        for path in &layer.paths {
            let slot = ams.slot_for_kind(path.kind);
            if slot != active {
                active = slot;
                last = Some(layer.index);
            }
        }
    }
    last
}

/// Whether `layer` executes any mid-print AMS switch given incoming `active_slot`.
pub(crate) fn layer_has_switch(layer: &PrintLayer, ams: &BambuAms, active_slot: u8) -> bool {
    let mut active = active_slot;
    for path in &layer.paths {
        let slot = ams.slot_for_kind(path.kind);
        if slot != active {
            return true;
        }
        active = slot;
    }
    false
}

/// Outer perimeter loop points (5 points, closed) for a prime tower layer.
fn frame_points(tower: &AmsPrimeTower) -> Result<[[f64; 2]; 5], ManufacturingError> {
    let half = tower.bead_width_mm * 0.5;
    let x0 = project_xyz6_coordinate_mm(tower.origin_xy_mm[0] + half)?;
    let y0 = project_xyz6_coordinate_mm(tower.origin_xy_mm[1] + half)?;
    let x1 = project_xyz6_coordinate_mm(tower.origin_xy_mm[0] + tower.size_xy_mm[0] - half)?;
    let y1 = project_xyz6_coordinate_mm(tower.origin_xy_mm[1] + tower.size_xy_mm[1] - half)?;
    if x1 <= x0 || y1 <= y0 {
        return Err(error(
            ManufacturingErrorCode::InvalidSpecification,
            "AMS prime tower frame collapses at coordinate precision",
        ));
    }
    Ok([[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]])
}

/// Interior serpentine priming rows inside the outer perimeter loop.
fn interior_rows(tower: &AmsPrimeTower) -> Result<Vec<[[f64; 2]; 2]>, ManufacturingError> {
    let w = tower.bead_width_mm;
    let inset = w * 1.5;
    let x0 = project_xyz6_coordinate_mm(tower.origin_xy_mm[0] + inset)?;
    let x1 = project_xyz6_coordinate_mm(tower.origin_xy_mm[0] + tower.size_xy_mm[0] - inset)?;
    let y_start = tower.origin_xy_mm[1] + inset;
    let y_end = tower.origin_xy_mm[1] + tower.size_xy_mm[1] - inset;
    if x1 <= x0 || y_end < y_start {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    let mut idx = 0usize;
    loop {
        let y = y_start + idx as f64 * w;
        if y > y_end + 1e-9 {
            break;
        }
        let py = project_xyz6_coordinate_mm(y.min(y_end))?;
        let row = if idx.is_multiple_of(2) {
            [[x0, py], [x1, py]]
        } else {
            [[x1, py], [x0, py]]
        };
        rows.push(row);
        idx += 1;
    }
    Ok(rows)
}

fn max_width_mm(width: &WidthProfile) -> f64 {
    match width {
        WidthProfile::Constant { width_mm } => *width_mm,
        WidthProfile::PerEdge { widths_mm } => widths_mm.iter().copied().fold(0.0_f64, f64::max),
    }
}

/// Validate AMS plan compatibility: prime tower clearance against exclusions,
/// prime line, and every deposited path, plus tower capacity for every switch layer.
pub(crate) fn validate_ams_plan(
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    job: &SingleExtruderJob,
    ams: &BambuAms,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    ams.validate(&intent.printer, job)?;
    let Some(tower) = &ams.prime_tower else {
        return Ok(());
    };
    let Some(last_change) = last_change_layer(plan, ams) else {
        return Ok(());
    };
    let min = tower.min_xy_mm();
    let max = tower.max_xy_mm();
    // Tower must stay clear of the initial prime line.
    if segment_intersects_rect(
        [job.prime_start_mm[0], job.prime_start_mm[1]],
        [job.prime_end_mm[0], job.prime_end_mm[1]],
        min,
        max,
    ) {
        return Err(error(
            ManufacturingErrorCode::InvalidSpecification,
            "AMS prime tower overlaps the start prime line",
        ));
    }
    // Tower must stay clear of every deposited path on the plate (padded by bead radius).
    for layer in &plan.layers {
        check_cancel(cancel)?;
        for path in &layer.paths {
            let pad = max_width_mm(&path.width) * 0.5 + PROFILE_TOLERANCE_MM;
            let padded_min = [min[0] - pad, min[1] - pad];
            let padded_max = [max[0] + pad, max[1] + pad];
            for edge in path.points_mm.windows(2) {
                if segment_intersects_rect(
                    [edge[0][0], edge[0][1]],
                    [edge[1][0], edge[1][1]],
                    padded_min,
                    padded_max,
                ) {
                    return Err(error(
                        ManufacturingErrorCode::InvalidSpecification,
                        "AMS prime tower overlaps deposited part, support or adhesion geometry",
                    ));
                }
            }
        }
    }
    // Verify tower layers fit printer limits and have enough capacity for every switch layer.
    let frame = frame_points(tower)?;
    let frame_len: f64 = frame.windows(2).map(|w| dist2(w[0], w[1])).sum();
    let rows = interior_rows(tower)?;
    let mut active = 0u8;
    for layer in &plan.layers {
        if layer.index > last_change {
            break;
        }
        let h = layer.z_mm - layer.lower_z_mm;
        if !intent
            .printer
            .deposition_limits
            .admits(tower.bead_width_mm, h)
        {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "AMS prime tower bead width/height exceeds printer deposition limits",
            ));
        }
        let area = bead_area(tower.bead_width_mm, h);
        let mut switches = 0usize;
        for path in &layer.paths {
            let slot = ams.slot_for_kind(path.kind);
            if slot != active {
                active = slot;
                switches += 1;
            }
        }
        if switches > 0 {
            let mut row_cursor = 0usize;
            let mut frame_used = false;
            for _ in 0..switches {
                let mut vol = 0.0;
                if !frame_used {
                    vol += frame_len * area;
                    frame_used = true;
                }
                while vol + 1e-9 < tower.prime_volume_mm3 && row_cursor < rows.len() {
                    vol += dist2(rows[row_cursor][0], rows[row_cursor][1]) * area;
                    row_cursor += 1;
                }
                if vol + 1e-9 < tower.prime_volume_mm3 {
                    return Err(error(
                        ManufacturingErrorCode::InvalidSpecification,
                        "AMS prime tower footprint is too small for the declared prime volume on a switch layer",
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Emit a closed polyline inside the prime tower (`G0` to start, `G1` edges),
/// returning `(last_point, total_emitted_filament_mm)`.
#[allow(clippy::too_many_arguments)]
fn emit_tower_polyline(
    out: &mut impl Write,
    points: &[[f64; 2]],
    z_mm: f64,
    layer_height_mm: f64,
    tower: &AmsPrimeTower,
    slot: &AmsSlot,
    printer: &PrinterSpecification,
    travel_feed: f64,
) -> Result<([f64; 3], f64), ManufacturingError> {
    let z = project_xyz6_coordinate_mm(z_mm)?;
    let area = bead_area(tower.bead_width_mm, layer_height_mm);
    let filament_area = slot.filament_area_mm2();
    let max_speed_for_flow = printer.max_volumetric_flow_mm3_s / area;
    let speed = tower
        .print_speed_mm_s
        .min(printer.max_print_feed_mm_s)
        .min(max_speed_for_flow);
    let feed = (speed * 60.0 * PROGRAM_F_SCALE).round() / PROGRAM_F_SCALE;
    if !feed.is_finite() || feed <= 0.0 {
        return Err(error(
            ManufacturingErrorCode::InvalidSpecification,
            "AMS prime tower feed collapses",
        ));
    }
    let start = project_xyz6_point_mm([points[0][0], points[0][1], z])?;
    writeln!(
        out,
        "G0 X{:.3} Y{:.3} Z{:.3} F{:.1}",
        start[0], start[1], start[2], travel_feed
    )
    .map_err(|_| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "program write failed",
        )
    })?;
    let mut prev = start;
    let mut total_e = 0.0;
    for &pt in &points[1..] {
        let to = project_xyz6_point_mm([pt[0], pt[1], z])?;
        let length = dist2([prev[0], prev[1]], [to[0], to[1]]);
        if length < program_coordinates::MIN_DEPOSITION_COMMAND_MM {
            continue;
        }
        let raw_e = length * area / filament_area;
        let rounded_e = (raw_e * PROGRAM_E_SCALE).round() / PROGRAM_E_SCALE;
        if rounded_e <= 0.0 {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "AMS prime tower extrusion collapses at E6 precision",
            ));
        }
        writeln!(
            out,
            "G1 X{:.3} Y{:.3} Z{:.3} E{:.6} F{:.1}",
            to[0], to[1], to[2], rounded_e, feed
        )
        .map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "program write failed",
            )
        })?;
        total_e += rounded_e;
        prev = to;
    }
    Ok((prev, total_e))
}

/// Emit a lightweight support frame on a non-switch layer `<= last_change_layer`.
/// Returns total emitted filament length (mm).
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_tower_frame(
    out: &mut impl Write,
    intent: &ManufacturingIntent,
    ams: &BambuAms,
    slot_idx: u8,
    layer: u32,
    z_mm: f64,
    layer_height_mm: f64,
    travel_feed: f64,
    state: &mut TowerLayerState,
) -> Result<f64, ManufacturingError> {
    let Some(tower) = &ams.prime_tower else {
        return Ok(0.0);
    };
    state.reset_for(layer);
    let slot = &ams.slots[usize::from(slot_idx)];
    let frame = frame_points(tower)?;
    writeln!(out, "; WIPE_TOWER_START").map_err(|_| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "program write failed",
        )
    })?;
    let (_, e) = emit_tower_polyline(
        out,
        &frame,
        z_mm,
        layer_height_mm,
        tower,
        slot,
        &intent.printer,
        travel_feed,
    )?;
    writeln!(out, "G92 E0\n; WIPE_TOWER_END").map_err(|_| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "program write failed",
        )
    })?;
    state.frame_printed = true;
    Ok(e)
}

/// Emit a mid-print single-nozzle Bambu AMS transition (`from -> to`) with
/// chute flush, wiper flick, and optional prime-tower priming pass.
/// Returns `(flush_filament_mm, tower_filament_mm)`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_switch(
    out: &mut impl Write,
    intent: &ManufacturingIntent,
    job: &SingleExtruderJob,
    ams: &BambuAms,
    from: u8,
    to: u8,
    layer: u32,
    z_mm: f64,
    layer_height_mm: f64,
    travel_feed: f64,
    state: &mut TowerLayerState,
) -> Result<(f64, f64), ManufacturingError> {
    let write_err = |_| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "program write failed",
        )
    };
    let series = job.bambu_series().unwrap_or(BambuSeries::A1);
    let mini = job.bambu_model_id() == Some("N1");
    let incoming = &ams.slots[usize::from(to)];
    let flush_temp = ams.flush_temperature_c(from, to);
    let target_temp = if layer == 0 {
        incoming.first_layer_nozzle_temperature_c
    } else {
        incoming.nozzle_temperature_c
    };
    let flush_feed = (incoming.flush_feed_mm_min() * PROGRAM_F_SCALE).round() / PROGRAM_F_SCALE;
    let flush_len = ams.flush_length_mm(from, to);

    // 1. Barrier, part fan off, open Bambu AMS switch block.
    writeln!(out, "M400").map_err(write_err)?;
    writeln!(out, "M106 S0").map_err(write_err)?;
    writeln!(out, "M620 S{to}A").map_err(write_err)?;

    // 2. Series-specific cut & waste chute staging (re-authored from observed
    //    Bambu Studio 02.09.01.70 A1/A1 mini/P1S/X1C profiles).
    match (series, mini) {
        (BambuSeries::A1, false) => {
            writeln!(out, "G1 X267 F18000").map_err(write_err)?;
            writeln!(out, "G1 X-48.2 F18000").map_err(write_err)?;
        }
        (BambuSeries::A1, true) => {
            writeln!(out, "G1 X180 F18000").map_err(write_err)?;
            writeln!(out, "G1 X-13.5 F18000").map_err(write_err)?;
        }
        (BambuSeries::P1X1, _) => {
            writeln!(out, "G1 X70 F21000").map_err(write_err)?;
            writeln!(out, "G1 Y245 F18000").map_err(write_err)?;
            writeln!(out, "G1 Y265 F3000").map_err(write_err)?;
        }
    }
    writeln!(out, "M400").map_err(write_err)?;
    writeln!(out, "M620.1 E F{flush_feed:.1} T{flush_temp:.0}").map_err(write_err)?;
    writeln!(out, "T{to}").map_err(write_err)?;

    // 3. Chute flush at flush temperature.
    let mut emitted_flush = 0.0;
    if flush_len > 0.0 {
        writeln!(out, "; FLUSH_START").map_err(write_err)?;
        writeln!(out, "M400").map_err(write_err)?;
        writeln!(out, "M109 S{flush_temp:.0}").map_err(write_err)?;
        writeln!(out, "G92 E0").map_err(write_err)?;
        let mut remaining = flush_len;
        while remaining > 1e-6 {
            let chunk = remaining.min(MAX_FLUSH_CHUNK_MM);
            let rounded = (chunk * PROGRAM_E_SCALE).round() / PROGRAM_E_SCALE;
            if rounded <= 0.0 {
                break;
            }
            writeln!(out, "G1 E{rounded:.6} F{flush_feed:.1}").map_err(write_err)?;
            emitted_flush += rounded;
            remaining -= chunk;
        }
        writeln!(out, "; FLUSH_END").map_err(write_err)?;
    }

    // 4. Settle to target printing temperature and wipe across series wiper.
    writeln!(out, "M400").map_err(write_err)?;
    writeln!(out, "M109 S{target_temp:.0}").map_err(write_err)?;
    match (series, mini) {
        (BambuSeries::A1, false) => {
            for _ in 0..2 {
                writeln!(out, "G1 X-38.2 F18000").map_err(write_err)?;
                writeln!(out, "G1 X-48.2 F3000").map_err(write_err)?;
            }
        }
        (BambuSeries::A1, true) => {
            for _ in 0..2 {
                writeln!(out, "G1 X-3.5 F18000").map_err(write_err)?;
                writeln!(out, "G1 X-13.5 F3000").map_err(write_err)?;
            }
        }
        (BambuSeries::P1X1, _) => {
            for _ in 0..2 {
                writeln!(out, "G1 X100 F5000").map_err(write_err)?;
                writeln!(out, "G1 X70 F15000").map_err(write_err)?;
            }
            writeln!(out, "G1 Y256 F12000").map_err(write_err)?;
        }
    }
    writeln!(out, "M400").map_err(write_err)?;
    writeln!(out, "M621 S{to}A").map_err(write_err)?;
    writeln!(out, "G21\nG90\nM83\nG92 E0").map_err(write_err)?;

    // 5. Optional prime-tower priming pass in the newly loaded slot.
    let mut emitted_tower = 0.0;
    if let Some(tower) = &ams.prime_tower {
        state.reset_for(layer);
        let area = bead_area(tower.bead_width_mm, layer_height_mm);
        let frame = frame_points(tower)?;
        let rows = interior_rows(tower)?;
        writeln!(out, "; WIPE_TOWER_START").map_err(write_err)?;
        let mut primed_vol = 0.0;
        if !state.frame_printed {
            let (_, e) = emit_tower_polyline(
                out,
                &frame,
                z_mm,
                layer_height_mm,
                tower,
                incoming,
                &intent.printer,
                travel_feed,
            )?;
            emitted_tower += e;
            primed_vol += e * incoming.filament_area_mm2();
            state.frame_printed = true;
        }
        let mut stripe_points = Vec::new();
        while primed_vol + 1e-6 < tower.prime_volume_mm3 && state.next_row < rows.len() {
            let row = rows[state.next_row];
            state.next_row += 1;
            if stripe_points.is_empty() {
                stripe_points.push(row[0]);
                stripe_points.push(row[1]);
                primed_vol += dist2(row[0], row[1]) * area;
            } else {
                let prev_end = *stripe_points.last().unwrap();
                let link_len = dist2(prev_end, row[0]);
                stripe_points.push(row[0]);
                stripe_points.push(row[1]);
                primed_vol += (link_len + dist2(row[0], row[1])) * area;
            }
        }
        if stripe_points.len() >= 2 {
            let (_, e) = emit_tower_polyline(
                out,
                &stripe_points,
                z_mm,
                layer_height_mm,
                tower,
                incoming,
                &intent.printer,
                travel_feed,
            )?;
            emitted_tower += e;
        }
        writeln!(out, "G92 E0\n; WIPE_TOWER_END").map_err(write_err)?;
    }

    Ok((emitted_flush, emitted_tower))
}

/// Compute exact per-slot chute-flush and prime-tower filament totals (mm)
/// matching `emit_tower_frame` / `emit_switch`, for Bambu header display data.
pub(crate) fn estimate_ams_additions(
    plan: &NormalizedPrintPlan,
    ams: &BambuAms,
) -> Result<Vec<f64>, ManufacturingError> {
    let mut extra = vec![0.0; ams.slots.len()];
    let Some(last_change) = last_change_layer(plan, ams) else {
        return Ok(extra);
    };
    let tower_geom = ams
        .prime_tower
        .as_ref()
        .map(|t| Ok::<_, ManufacturingError>((t, frame_points(t)?, interior_rows(t)?)))
        .transpose()?;
    let mut active = 0u8;
    for layer in &plan.layers {
        let h = layer.z_mm - layer.lower_z_mm;
        let has_switch = layer_has_switch(layer, ams, active);
        if !has_switch
            && layer.index <= last_change
            && let Some((tower, frame, _)) = &tower_geom
        {
            let area = bead_area(tower.bead_width_mm, h);
            let fil_area = ams.slots[usize::from(active)].filament_area_mm2();
            for edge in frame.windows(2) {
                let len = dist2(edge[0], edge[1]);
                if len >= program_coordinates::MIN_DEPOSITION_COMMAND_MM {
                    extra[usize::from(active)] +=
                        ((len * area / fil_area) * PROGRAM_E_SCALE).round() / PROGRAM_E_SCALE;
                }
            }
        }
        let mut frame_printed = !has_switch && layer.index <= last_change;
        let mut next_row = 0usize;
        for path in &layer.paths {
            let to = ams.slot_for_kind(path.kind);
            if to != active {
                let flush_len = ams.flush_length_mm(active, to);
                if flush_len > 0.0 {
                    let mut rem = flush_len;
                    while rem > 1e-6 {
                        let chunk = rem.min(MAX_FLUSH_CHUNK_MM);
                        let rounded = (chunk * PROGRAM_E_SCALE).round() / PROGRAM_E_SCALE;
                        if rounded <= 0.0 {
                            break;
                        }
                        extra[usize::from(to)] += rounded;
                        rem -= chunk;
                    }
                }
                if let Some((tower, frame, rows)) = &tower_geom {
                    let area = bead_area(tower.bead_width_mm, h);
                    let fil_area = ams.slots[usize::from(to)].filament_area_mm2();
                    let mut primed_vol = 0.0;
                    if !frame_printed {
                        for edge in frame.windows(2) {
                            let len = dist2(edge[0], edge[1]);
                            if len >= program_coordinates::MIN_DEPOSITION_COMMAND_MM {
                                let e = ((len * area / fil_area) * PROGRAM_E_SCALE).round()
                                    / PROGRAM_E_SCALE;
                                extra[usize::from(to)] += e;
                                primed_vol += e * fil_area;
                            }
                        }
                        frame_printed = true;
                    }
                    let mut prev_end: Option<[f64; 2]> = None;
                    while primed_vol + 1e-6 < tower.prime_volume_mm3 && next_row < rows.len() {
                        let row = rows[next_row];
                        next_row += 1;
                        if let Some(prev) = prev_end {
                            let link = dist2(prev, row[0]);
                            if link >= program_coordinates::MIN_DEPOSITION_COMMAND_MM {
                                let e = ((link * area / fil_area) * PROGRAM_E_SCALE).round()
                                    / PROGRAM_E_SCALE;
                                extra[usize::from(to)] += e;
                                primed_vol += link * area;
                            }
                        }
                        let len = dist2(row[0], row[1]);
                        if len >= program_coordinates::MIN_DEPOSITION_COMMAND_MM {
                            let e = ((len * area / fil_area) * PROGRAM_E_SCALE).round()
                                / PROGRAM_E_SCALE;
                            extra[usize::from(to)] += e;
                            primed_vol += len * area;
                        }
                        prev_end = Some(row[1]);
                    }
                }
                active = to;
            }
        }
    }
    Ok(extra)
}
