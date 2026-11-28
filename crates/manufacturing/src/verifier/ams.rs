// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Independent verifier for single-nozzle Bambu Lab AMS tool changes
//! (`M620 S<slot>A .. M621 S<slot>A`), chute flushing (`; FLUSH_START`..
//! `; FLUSH_END`), and bed-rooted prime towers (`; WIPE_TOWER_START`..
//! `; WIPE_TOWER_END`).
//!
//! Never imports `backend::ams` or shares producer tower/switch layout code.

use super::*;
use spiling_contracts::manufacturing::program_coordinates::{
    MIN_DEPOSITION_COMMAND_MM, project_xyz6_coordinate_mm,
};

const VOLUME_SLACK_MM3: f64 = 1e-2;
const XY_SLACK_MM: f64 = 0.001;

fn max_width_mm(width: &WidthProfile) -> f64 {
    match width {
        WidthProfile::Constant { width_mm } => *width_mm,
        WidthProfile::PerEdge { widths_mm } => widths_mm.iter().copied().fold(0.0_f64, f64::max),
    }
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

/// Independently compute the highest layer index containing a mid-print AMS
/// switch, starting from slot 0 across `plan.layers`.
pub(super) fn last_change_layer(plan: &NormalizedPrintPlan, ams: &BambuAms) -> Option<u32> {
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

/// Independently test whether `layer` switches slots from `active_slot`.
pub(super) fn layer_has_switch(layer: &PrintLayer, ams: &BambuAms, active_slot: u8) -> bool {
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

enum SwitchPhase {
    PreBarrier,
    PreFanOff,
    OpenM620,
    Stage(usize),
    StageBarrier,
    FlushHint,
    SelectTool,
    FlushStart,
    FlushBarrier,
    FlushHeat,
    FlushResetE,
    FlushExtrude { decoded_mm: f64 },
    SettleBarrier,
    SettleHeat,
    Wipe(usize),
    CloseBarrier,
    CloseM621,
    Restore(usize),
}

struct TowerState {
    layer: u32,
    z_mm: f64,
    require_prime_volume: bool,
    started: bool,
    saw_reset_e: bool,
    position: Option<[f64; 3]>,
    segments: u32,
    volume_mm3: f64,
    filament_mm: f64,
}

enum Pending {
    None,
    Switch {
        from: u8,
        to: u8,
        layer: u32,
        z_mm: f64,
        phase: SwitchPhase,
    },
    Tower(TowerState),
}

pub(super) struct AmsVerifier<'a> {
    printer: &'a PrinterSpecification,
    job: &'a SingleExtruderJob,
    ams: &'a BambuAms,
    series: BambuSeries,
    mini: bool,
    active_slot: u8,
    slots: Vec<AmsSlotAccounting>,
    change_count: u32,
    sequence: Vec<u8>,
    tower_deposition_segments: u32,
    tower_travel_segments: u32,
    tower_layers: u32,
    last_tower_layer: Option<u32>,
    pending: Pending,
}

impl<'a> AmsVerifier<'a> {
    pub(super) fn new(
        plan: &NormalizedPrintPlan,
        printer: &'a PrinterSpecification,
        job: &'a SingleExtruderJob,
        ams: &'a BambuAms,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        ams.validate(printer, job)?;
        let series = job.bambu_series().unwrap_or(BambuSeries::A1);
        let mini = job.bambu_model_id() == Some("N1");
        // Independently verify prime tower clearance against exclusions, prime line and paths.
        if let Some(tower) = &ams.prime_tower
            && last_change_layer(plan, ams).is_some()
        {
            tower.validate(printer)?;
            let min = tower.min_xy_mm();
            let max = tower.max_xy_mm();
            if segment_intersects_rect(
                [job.prime_start_mm[0], job.prime_start_mm[1]],
                [job.prime_end_mm[0], job.prime_end_mm[1]],
                min,
                max,
            ) {
                return Err(failed("AMS prime tower intersects the start prime line"));
            }
            for layer in &plan.layers {
                cancelled(cancel)?;
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
                            return Err(failed(
                                "AMS prime tower collides with deposited part, support or adhesion geometry",
                            ));
                        }
                    }
                }
            }
        }
        let slots = (0..ams.slots.len())
            .map(|i| AmsSlotAccounting {
                slot: i as u8,
                model_filament_length_mm: 0.0,
                support_filament_length_mm: 0.0,
                adhesion_filament_length_mm: 0.0,
                priming_filament_length_mm: 0.0,
                flush_filament_length_mm: 0.0,
                tower_filament_length_mm: 0.0,
                used_for_object: false,
                used_for_support: false,
            })
            .collect();
        Ok(Self {
            printer,
            job,
            ams,
            series,
            mini,
            active_slot: 0,
            slots,
            change_count: 0,
            sequence: vec![0],
            tower_deposition_segments: 0,
            tower_travel_segments: 0,
            tower_layers: 0,
            last_tower_layer: None,
            pending: Pending::None,
        })
    }

    pub(super) fn pending(&self) -> bool {
        !matches!(self.pending, Pending::None)
    }

    pub(super) fn active_slot(&self) -> u8 {
        self.active_slot
    }

    pub(super) fn active_filament_area(&self) -> f64 {
        self.ams.slots[usize::from(self.active_slot)].filament_area_mm2()
    }

    pub(super) fn record_prime(&mut self, length_mm: f64) {
        self.slots[0].priming_filament_length_mm += length_mm;
    }

    pub(super) fn record_deposition(&mut self, kind: DepositionKind, delta_e: f64) {
        let acct = &mut self.slots[usize::from(self.active_slot)];
        if kind.is_support() {
            acct.support_filament_length_mm += delta_e;
            acct.used_for_support = true;
        } else if kind.is_adhesion() {
            acct.adhesion_filament_length_mm += delta_e;
        } else {
            acct.model_filament_length_mm += delta_e;
            acct.used_for_object = true;
        }
    }

    pub(super) fn begin_tower_frame(
        &mut self,
        layer: u32,
        slot: u8,
        z_mm: f64,
    ) -> Result<(), ManufacturingError> {
        if self.pending() {
            return Err(failed(
                "nested AMS tower frame while another AMS sequence is active",
            ));
        }
        if slot != self.active_slot || self.ams.prime_tower.is_none() {
            return Err(failed(
                "unexpected AMS tower frame for inactive slot or absent tower",
            ));
        }
        self.pending = Pending::Tower(TowerState {
            layer,
            z_mm: project_xyz6_coordinate_mm(z_mm)?,
            require_prime_volume: false,
            started: false,
            saw_reset_e: false,
            position: None,
            segments: 0,
            volume_mm3: 0.0,
            filament_mm: 0.0,
        });
        Ok(())
    }

    pub(super) fn begin_switch(
        &mut self,
        from: u8,
        to: u8,
        layer: u32,
        z_mm: f64,
    ) -> Result<(), ManufacturingError> {
        if self.pending() {
            return Err(failed(
                "nested AMS switch while another AMS sequence is active",
            ));
        }
        if from != self.active_slot || from == to || usize::from(to) >= self.ams.slots.len() {
            return Err(failed("invalid AMS slot transition"));
        }
        self.pending = Pending::Switch {
            from,
            to,
            layer,
            z_mm: project_xyz6_coordinate_mm(z_mm)?,
            phase: SwitchPhase::PreBarrier,
        };
        Ok(())
    }

    fn staging_lines(&self) -> &'static [&'static str] {
        match (self.series, self.mini) {
            (BambuSeries::A1, false) => &["G1 X267 F18000", "G1 X-48.2 F18000"],
            (BambuSeries::A1, true) => &["G1 X180 F18000", "G1 X-13.5 F18000"],
            (BambuSeries::P1X1, _) => &["G1 X70 F21000", "G1 Y245 F18000", "G1 Y265 F3000"],
        }
    }

    fn wipe_lines(&self) -> &'static [&'static str] {
        match (self.series, self.mini) {
            (BambuSeries::A1, false) => &[
                "G1 X-38.2 F18000",
                "G1 X-48.2 F3000",
                "G1 X-38.2 F18000",
                "G1 X-48.2 F3000",
            ],
            (BambuSeries::A1, true) => &[
                "G1 X-3.5 F18000",
                "G1 X-13.5 F3000",
                "G1 X-3.5 F18000",
                "G1 X-13.5 F3000",
            ],
            (BambuSeries::P1X1, _) => &[
                "G1 X100 F5000",
                "G1 X70 F15000",
                "G1 X100 F5000",
                "G1 X70 F15000",
                "G1 Y256 F12000",
            ],
        }
    }

    /// Consume one program line belonging to an active AMS switch or prime-tower pass.
    pub(super) fn consume_line(
        &mut self,
        line: &str,
        line_number: usize,
        state: &mut MotionState,
        fan_state: &mut Option<u8>,
        max_z: &mut f64,
    ) -> Result<(), ManufacturingError> {
        let trimmed = line.trim();
        let err = |msg: &str| Err(failed(format!("program line {line_number}: AMS: {msg}")));
        let staging = self.staging_lines();
        let wipes = self.wipe_lines();
        const RESTORE: [&str; 4] = ["G21", "G90", "M83", "G92 E0"];

        match &mut self.pending {
            Pending::None => err("consume_line called without pending AMS state"),
            Pending::Switch {
                from,
                to,
                layer,
                z_mm,
                phase,
            } => {
                let (from, to, layer, z_mm) = (*from, *to, *layer, *z_mm);
                let incoming = &self.ams.slots[usize::from(to)];
                let flush_temp = self.ams.flush_temperature_c(from, to);
                let target_temp = if layer == 0 {
                    incoming.first_layer_nozzle_temperature_c
                } else {
                    incoming.nozzle_temperature_c
                };
                let flush_vol = self.ams.flush_volume_mm3(from, to);
                let max_flush_feed = incoming.flush_feed_mm_min() + F_HALF_STEP;
                match phase {
                    SwitchPhase::PreBarrier => {
                        if trimmed != "M400" {
                            return err("expected M400 barrier before AMS switch");
                        }
                        *phase = SwitchPhase::PreFanOff;
                    }
                    SwitchPhase::PreFanOff => {
                        if trimmed != "M106 S0" {
                            return err("expected M106 S0 before AMS switch");
                        }
                        *fan_state = Some(0);
                        *phase = SwitchPhase::OpenM620;
                    }
                    SwitchPhase::OpenM620 => {
                        if trimmed != format!("M620 S{to}A") {
                            return err("expected M620 S<slot>A opening AMS switch");
                        }
                        *phase = SwitchPhase::Stage(0);
                    }
                    SwitchPhase::Stage(idx) => {
                        if trimmed != staging[*idx] {
                            return err("AMS cutter/chute staging differs from series geometry");
                        }
                        *idx += 1;
                        if *idx == staging.len() {
                            *phase = SwitchPhase::StageBarrier;
                        }
                    }
                    SwitchPhase::StageBarrier => {
                        if trimmed != "M400" {
                            return err("expected M400 after AMS staging");
                        }
                        *phase = SwitchPhase::FlushHint;
                    }
                    SwitchPhase::FlushHint => {
                        let (feed, temp) = parse_m620_1(trimmed).ok_or_else(|| {
                            failed(format!(
                                "program line {line_number}: malformed M620.1 flush hint"
                            ))
                        })?;
                        if temp != flush_temp || feed <= 0.0 || feed > max_flush_feed {
                            return err(
                                "M620.1 flush speed or temperature disagrees with AMS slot",
                            );
                        }
                        *phase = SwitchPhase::SelectTool;
                    }
                    SwitchPhase::SelectTool => {
                        if trimmed != format!("T{to}") {
                            return err("expected T<slot> matching M620 target");
                        }
                        if flush_vol > 0.0 {
                            *phase = SwitchPhase::FlushStart;
                        } else {
                            *phase = SwitchPhase::SettleBarrier;
                        }
                    }
                    SwitchPhase::FlushStart => {
                        if trimmed != "; FLUSH_START" {
                            return err("expected ; FLUSH_START marker");
                        }
                        *phase = SwitchPhase::FlushBarrier;
                    }
                    SwitchPhase::FlushBarrier => {
                        if trimmed != "M400" {
                            return err("expected M400 before flush heat");
                        }
                        *phase = SwitchPhase::FlushHeat;
                    }
                    SwitchPhase::FlushHeat => {
                        if trimmed != format!("M109 S{flush_temp:.0}") {
                            return err("expected M109 at declared flush temperature");
                        }
                        *phase = SwitchPhase::FlushResetE;
                    }
                    SwitchPhase::FlushResetE => {
                        if trimmed != "G92 E0" {
                            return err("expected G92 E0 before chute flush");
                        }
                        *phase = SwitchPhase::FlushExtrude { decoded_mm: 0.0 };
                    }
                    SwitchPhase::FlushExtrude { decoded_mm } => {
                        if trimmed == "; FLUSH_END" {
                            let flushed_vol = *decoded_mm * incoming.filament_area_mm2();
                            if flushed_vol + VOLUME_SLACK_MM3 < flush_vol {
                                return err(
                                    "decoded chute flush volume is below declared flush matrix",
                                );
                            }
                            self.slots[usize::from(to)].flush_filament_length_mm += *decoded_mm;
                            *phase = SwitchPhase::SettleBarrier;
                        } else {
                            let (e, f) = parse_flush_move(trimmed).ok_or_else(|| {
                                failed(format!(
                                    "program line {line_number}: malformed flush G1 E F move"
                                ))
                            })?;
                            if e <= 0.0 || f <= 0.0 || f > max_flush_feed {
                                return err(
                                    "chute flush extrusion delta or feed violates AMS slot limit",
                                );
                            }
                            *decoded_mm += e;
                        }
                    }
                    SwitchPhase::SettleBarrier => {
                        if trimmed != "M400" {
                            return err("expected M400 before post-flush temperature settle");
                        }
                        *phase = SwitchPhase::SettleHeat;
                    }
                    SwitchPhase::SettleHeat => {
                        if trimmed != format!("M109 S{target_temp:.0}") {
                            return err(
                                "expected M109 settling to incoming slot print temperature",
                            );
                        }
                        *phase = SwitchPhase::Wipe(0);
                    }
                    SwitchPhase::Wipe(idx) => {
                        if trimmed != wipes[*idx] {
                            return err("AMS wiper sequence differs from series geometry");
                        }
                        *idx += 1;
                        if *idx == wipes.len() {
                            *phase = SwitchPhase::CloseBarrier;
                        }
                    }
                    SwitchPhase::CloseBarrier => {
                        if trimmed != "M400" {
                            return err("expected M400 before closing M621");
                        }
                        *phase = SwitchPhase::CloseM621;
                    }
                    SwitchPhase::CloseM621 => {
                        if trimmed != format!("M621 S{to}A") {
                            return err("expected M621 S<slot>A closing AMS switch");
                        }
                        *phase = SwitchPhase::Restore(0);
                    }
                    SwitchPhase::Restore(idx) => {
                        if trimmed != RESTORE[*idx] {
                            return err("expected G21/G90/M83/G92 E0 mode restore after M621");
                        }
                        *idx += 1;
                        if *idx == RESTORE.len() {
                            self.active_slot = to;
                            self.change_count = self
                                .change_count
                                .checked_add(1)
                                .ok_or_else(|| failed("AMS change count exceeds u32"))?;
                            self.sequence.push(to);
                            state.position = None;
                            state.extrusion = 0.0;
                            if self.ams.prime_tower.is_some() {
                                self.pending = Pending::Tower(TowerState {
                                    layer,
                                    z_mm,
                                    require_prime_volume: true,
                                    started: false,
                                    saw_reset_e: false,
                                    position: None,
                                    segments: 0,
                                    volume_mm3: 0.0,
                                    filament_mm: 0.0,
                                });
                            } else {
                                self.pending = Pending::None;
                            }
                        }
                    }
                }
                Ok(())
            }
            Pending::Tower(tower_state) => {
                let Some(tower) = &self.ams.prime_tower else {
                    return err("prime tower block without declared prime tower");
                };
                if !tower_state.started {
                    if trimmed != "; WIPE_TOWER_START" {
                        return err("expected ; WIPE_TOWER_START");
                    }
                    tower_state.started = true;
                    return Ok(());
                }
                if tower_state.saw_reset_e {
                    if trimmed != "; WIPE_TOWER_END" {
                        return err("expected ; WIPE_TOWER_END immediately after G92 E0");
                    }
                    if tower_state.require_prime_volume {
                        if tower_state.volume_mm3 + VOLUME_SLACK_MM3 < tower.prime_volume_mm3 {
                            return err(
                                "prime tower deposited volume is below declared prime_volume_mm3",
                            );
                        }
                    } else if tower_state.segments < 4 {
                        return err("prime tower support frame must deposit a closed 4-edge loop");
                    }
                    self.slots[usize::from(self.active_slot)].tower_filament_length_mm +=
                        tower_state.filament_mm;
                    if self.last_tower_layer != Some(tower_state.layer) {
                        self.tower_layers += 1;
                        self.last_tower_layer = Some(tower_state.layer);
                    }
                    *max_z = max_z.max(tower_state.z_mm);
                    state.position = None;
                    state.extrusion = 0.0;
                    self.pending = Pending::None;
                    return Ok(());
                }
                if trimmed == "G92 E0" {
                    tower_state.saw_reset_e = true;
                    return Ok(());
                }
                let mv = parse_tower_move(trimmed).ok_or_else(|| {
                    failed(format!(
                        "program line {line_number}: malformed prime-tower move"
                    ))
                })?;
                if mv.z != tower_state.z_mm {
                    return err("prime tower move Z differs from layer Z");
                }
                let min = tower.min_xy_mm();
                let max = tower.max_xy_mm();
                if mv.x < min[0] - XY_SLACK_MM
                    || mv.x > max[0] + XY_SLACK_MM
                    || mv.y < min[1] - XY_SLACK_MM
                    || mv.y > max[1] + XY_SLACK_MM
                {
                    return err("prime tower move leaves declared tower footprint");
                }
                let to = [mv.x, mv.y, mv.z];
                if let Some(delta_e) = mv.e {
                    let from = tower_state.position.ok_or_else(|| {
                        failed(format!(
                            "program line {line_number}: tower extrusion before travel"
                        ))
                    })?;
                    let len = (to[0] - from[0]).hypot(to[1] - from[1]);
                    if len < MIN_DEPOSITION_COMMAND_MM || delta_e <= 0.0 {
                        return err("prime tower extrusion segment collapses or is nonpositive");
                    }
                    if mv.f <= 0.0
                        || mv.f / 60.0 > self.printer.max_print_feed_mm_s + F_HALF_STEP / 60.0
                    {
                        return err("prime tower extrusion feed exceeds printer limit");
                    }
                    let fil_area =
                        self.ams.slots[usize::from(self.active_slot)].filament_area_mm2();
                    let vol = delta_e * fil_area;
                    let flow = vol * (mv.f / 60.0) / len;
                    if !flow.is_finite()
                        || flow > self.printer.max_volumetric_flow_mm3_s * 1.01 + 1e-4
                    {
                        return err("prime tower volumetric flow exceeds printer limit");
                    }
                    tower_state.segments += 1;
                    tower_state.volume_mm3 += vol;
                    tower_state.filament_mm += delta_e;
                    self.tower_deposition_segments += 1;
                } else {
                    if mv.f <= 0.0
                        || mv.f / 60.0 > self.printer.max_travel_feed_mm_s + F_HALF_STEP / 60.0
                    {
                        return err("prime tower travel feed exceeds printer limit");
                    }
                    self.tower_travel_segments += 1;
                }
                tower_state.position = Some(to);
                Ok(())
            }
        }
    }

    pub(super) fn finish(self) -> Result<AmsVerification, ManufacturingError> {
        if self.pending() {
            return Err(failed(
                "program ended inside an incomplete AMS switch or prime tower block",
            ));
        }
        let total_flush_filament_length_mm: f64 =
            self.slots.iter().map(|s| s.flush_filament_length_mm).sum();
        let total_tower_filament_length_mm: f64 =
            self.slots.iter().map(|s| s.tower_filament_length_mm).sum();
        let report = AmsVerification {
            slots: self.slots,
            change_count: self.change_count,
            sequence: self.sequence,
            tower_deposition_segments: self.tower_deposition_segments,
            tower_travel_segments: self.tower_travel_segments,
            tower_layers: self.tower_layers,
            total_flush_filament_length_mm,
            total_tower_filament_length_mm,
        };
        report.validate()?;
        let _ = self.job;
        Ok(report)
    }
}

fn parse_m620_1(line: &str) -> Option<(f64, f64)> {
    let words: Vec<&str> = line.split_ascii_whitespace().collect();
    if words.len() != 4 || words[0] != "M620.1" || words[1] != "E" {
        return None;
    }
    let f = words[2].strip_prefix('F')?.parse::<f64>().ok()?;
    let t = words[3].strip_prefix('T')?.parse::<f64>().ok()?;
    if f.is_finite() && t.is_finite() {
        Some((f, t))
    } else {
        None
    }
}

fn parse_flush_move(line: &str) -> Option<(f64, f64)> {
    let words: Vec<&str> = line.split_ascii_whitespace().collect();
    if words.len() != 3 || words[0] != "G1" {
        return None;
    }
    let e = words[1].strip_prefix('E')?.parse::<f64>().ok()?;
    let f = words[2].strip_prefix('F')?.parse::<f64>().ok()?;
    if e.is_finite() && f.is_finite() {
        Some((e, f))
    } else {
        None
    }
}

struct TowerMove {
    x: f64,
    y: f64,
    z: f64,
    e: Option<f64>,
    f: f64,
}

fn parse_tower_move(line: &str) -> Option<TowerMove> {
    let words: Vec<&str> = line.split_ascii_whitespace().collect();
    match words.as_slice() {
        ["G0", x, y, z, f] => {
            let x = x.strip_prefix('X')?.parse::<f64>().ok()?;
            let y = y.strip_prefix('Y')?.parse::<f64>().ok()?;
            let z = z.strip_prefix('Z')?.parse::<f64>().ok()?;
            let f = f.strip_prefix('F')?.parse::<f64>().ok()?;
            (x.is_finite() && y.is_finite() && z.is_finite() && f.is_finite()).then_some(
                TowerMove {
                    x,
                    y,
                    z,
                    e: None,
                    f,
                },
            )
        }
        ["G1", x, y, z, e, f] => {
            let x = x.strip_prefix('X')?.parse::<f64>().ok()?;
            let y = y.strip_prefix('Y')?.parse::<f64>().ok()?;
            let z = z.strip_prefix('Z')?.parse::<f64>().ok()?;
            let e = e.strip_prefix('E')?.parse::<f64>().ok()?;
            let f = f.strip_prefix('F')?.parse::<f64>().ok()?;
            (x.is_finite() && y.is_finite() && z.is_finite() && e.is_finite() && f.is_finite())
                .then_some(TowerMove {
                    x,
                    y,
                    z,
                    e: Some(e),
                    f,
                })
        }
        _ => None,
    }
}
