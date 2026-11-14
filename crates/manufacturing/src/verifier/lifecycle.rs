// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Independent replay of a complete job's declared firmware lifecycle. Each
//! typed step has its own expected text here, written separately from the
//! backend; opaque blocks match their declared lines and apply only their
//! declared effects, which are reported as trusted declarations. The replay
//! tracks homing, modes, E, heater and levelling state itself and refuses
//! motion the surrounding program does not make safe.

use super::*;
use std::collections::VecDeque;

/// One expected lifecycle line, or a declared step whose effects apply once
/// its lines have been consumed.
pub(super) enum Item<'a> {
    Text(String),
    PrimeTravel,
    PrimeLine,
    Lift,
    ParkXy,
    Applied(&'a LifecycleStep),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Heat {
    Off,
    Target,
    Wait,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Levelling {
    /// As the firmware leaves it after power-on or homing.
    FirmwareDefault,
    Probed,
    Restored,
    Off,
    /// Declared active by an opaque block.
    DeclaredActive,
    DeclaredOff,
}

pub(super) struct Lifecycle<'a> {
    job: &'a SingleExtruderJob,
    dialect: OutputDialect,
    /// Prusa `M555` probe area `[X, Y, W, H]` from the decoded plan's first
    /// layer, computed here independently of the backend.
    mesh_area: [f64; 4],
    nozzle_mm: f64,
    start: std::slice::Iter<'a, LifecycleStep>,
    end: std::slice::Iter<'a, LifecycleStep>,
    queue: VecDeque<Item<'a>>,
    in_start: bool,
    homed: bool,
    modes: bool,
    extrusion_known: bool,
    bed: Heat,
    nozzle: Heat,
    nozzle_print: bool,
    fan_off: bool,
    motors_off: bool,
    /// Vertical lift height, fixed when the lift is decoded.
    pub lift_z: Option<f64>,
    pub levelling: Levelling,
}

impl<'a> Lifecycle<'a> {
    pub fn new(
        job: &'a SingleExtruderJob,
        printer: &'a PrinterSpecification,
        plan: &NormalizedPrintPlan,
    ) -> Self {
        Self {
            job,
            dialect: printer.capabilities.output_dialect,
            mesh_area: mesh_area(plan, printer.build_envelope),
            nozzle_mm: printer.nozzle_diameter_mm,
            start: job.lifecycle.start.iter(),
            end: job.lifecycle.end.iter(),
            queue: VecDeque::new(),
            in_start: true,
            homed: false,
            modes: true,
            extrusion_known: true,
            bed: Heat::Off,
            nozzle: Heat::Off,
            nozzle_print: false,
            fan_off: false,
            motors_off: false,
            lift_z: None,
            levelling: Levelling::FirmwareDefault,
        }
    }

    pub fn in_start(&mut self) -> bool {
        self.fill(true);
        self.in_start
    }

    /// Expand the next step of the current phase until a line is expected.
    fn fill(&mut self, start: bool) {
        loop {
            match self.queue.front() {
                Some(Item::Applied(_)) | None => {}
                Some(_) => return,
            }
            if let Some(Item::Applied(step)) = self.queue.pop_front() {
                self.apply(step);
                continue;
            }
            let next = if start {
                self.start.next()
            } else {
                self.end.next()
            };
            let Some(step) = next else {
                if start {
                    self.in_start = false;
                }
                return;
            };
            self.expand(step);
        }
    }

    /// The next expected end item, once the body is complete.
    pub fn next_end(&mut self) -> Option<&Item<'a>> {
        self.fill(false);
        self.queue.front()
    }
    pub fn next_start(&mut self) -> Option<&Item<'a>> {
        self.fill(true);
        self.queue.front()
    }
    pub fn pop(&mut self) {
        self.queue.pop_front();
    }
    pub fn done(&mut self) -> bool {
        self.fill(false);
        !self.in_start && self.queue.is_empty()
    }

    fn temperature(&self, standby_c: &Option<f64>) -> f64 {
        let print = self.job.first_layer_nozzle_temperature_c;
        standby_c.map_or(print, |t| t.min(print))
    }

    fn expand(&mut self, step: &'a LifecycleStep) {
        use LifecycleStep as S;
        let klipper = self.dialect == OutputDialect::KlipperSingleExtruderGcodeV1;
        let prusa = self.dialect == OutputDialect::PrusaMarlinSingleExtruderGcodeV1;
        let wait = if klipper { 'S' } else { 'R' };
        // The start sequence heats to the first-layer targets.
        let bed = self.job.first_layer_bed_temperature_c;
        let text: Vec<String> = match step {
            S::CelsiusUnits => vec!["M149 C".into()],
            S::VolumetricOff => vec!["M200 D0".into()],
            S::FeedScaleReset => vec!["M220 S100".into()],
            S::FlowScaleReset => vec!["M221 S100".into()],
            S::FanOff => vec!["M107".into()],
            S::NozzleCheck => vec![format!("M862.1 P{}", self.nozzle_mm)],
            S::ModelCheck { model } => vec![format!("M862.3 P \"{model}\"")],
            S::BedTarget => vec![format!("M140 S{bed:.0}")],
            S::BedWait => vec![format!("M190 {wait}{bed:.0}")],
            S::NozzleTarget { standby_c } => {
                vec![format!("M104 S{:.0}", self.temperature(standby_c))]
            }
            S::NozzleWait { standby_c } => {
                vec![format!("M109 {wait}{:.0}", self.temperature(standby_c))]
            }
            S::Home => vec!["G28".into()],
            S::ProbeMesh if klipper => vec!["BED_MESH_CALIBRATE".into()],
            S::ProbeMesh if prusa => vec![
                format!(
                    "M555 X{} Y{} W{} H{}",
                    self.mesh_area[0], self.mesh_area[1], self.mesh_area[2], self.mesh_area[3]
                ),
                "G29 P1".into(),
                "G29 P3.2".into(),
                "G29 P3.13".into(),
                "G29 A".into(),
            ],
            S::ProbeMesh => vec!["G29".into()],
            S::RestoreMesh if klipper => vec!["BED_MESH_PROFILE LOAD=default".into()],
            S::RestoreMesh => vec!["M420 S1".into()],
            S::LevellingOff if klipper => vec!["BED_MESH_CLEAR".into()],
            S::LevellingOff => vec!["M420 S0".into()],
            // Every complete single-extruder dialect replays relative E.
            S::RestoreModes => vec!["G21".into(), "G90".into(), "M83".into()],
            S::ResetExtrusion => vec!["G92 E0".into()],
            S::Barrier => vec!["M400".into()],
            S::NozzleOff => vec!["M104 S0".into()],
            S::BedOff => vec!["M140 S0".into()],
            S::MotorsOff if klipper => vec!["M84".into()],
            S::MotorsOff => vec!["M84 X Y E".into()],
            S::Block { block } => block
                .lines
                .iter()
                .map(|line| {
                    line.replace("{bed_temperature_c}", &format!("{bed:.0}"))
                        .replace(
                            "{nozzle_temperature_c}",
                            &format!("{:.0}", self.job.first_layer_nozzle_temperature_c),
                        )
                })
                .collect(),
            S::Prime => {
                self.queue.push_back(Item::PrimeTravel);
                self.queue.push_back(Item::PrimeLine);
                vec![]
            }
            S::Park => {
                self.queue.push_back(Item::Lift);
                self.queue.push_back(Item::ParkXy);
                vec![]
            }
        };
        self.queue.extend(text.into_iter().map(Item::Text));
        self.queue.push_back(Item::Applied(step));
    }

    /// Apply a step's effects once all its lines have been accepted.
    fn apply(&mut self, step: &LifecycleStep) {
        use LifecycleStep as S;
        match step {
            S::Home => {
                self.homed = true;
                self.motors_off = false;
                if self.levelling != Levelling::Off {
                    self.levelling = Levelling::FirmwareDefault;
                }
            }
            S::ProbeMesh => self.levelling = Levelling::Probed,
            S::RestoreMesh => self.levelling = Levelling::Restored,
            S::LevellingOff => self.levelling = Levelling::Off,
            S::BedTarget => self.bed = self.bed.max(Heat::Target),
            S::BedWait => self.bed = Heat::Wait,
            S::NozzleTarget { standby_c } | S::NozzleWait { standby_c } => {
                self.nozzle = if matches!(step, S::NozzleWait { .. }) {
                    Heat::Wait
                } else {
                    Heat::Target
                };
                self.nozzle_print =
                    self.temperature(standby_c) == self.job.first_layer_nozzle_temperature_c;
            }
            S::RestoreModes => self.modes = true,
            S::ResetExtrusion => self.extrusion_known = true,
            S::FanOff => self.fan_off = true,
            S::NozzleOff => self.nozzle = Heat::Off,
            S::BedOff => self.bed = Heat::Off,
            S::MotorsOff => {
                self.motors_off = true;
                self.homed = false;
            }
            S::Block { block } => {
                let e = block.effects;
                if e.homes {
                    self.homed = true;
                    self.motors_off = false;
                }
                if e.motors_off {
                    self.motors_off = true;
                    self.homed = false;
                }
                if e.modes == ModeEffect::Unknown {
                    self.modes = false;
                }
                if e.extrusion == ExtrusionEffect::Unknown {
                    self.extrusion_known = false;
                }
                match e.levelling {
                    LevellingEffect::Unchanged => {}
                    LevellingEffect::Active => self.levelling = Levelling::DeclaredActive,
                    LevellingEffect::Off => self.levelling = Levelling::DeclaredOff,
                }
                let heat = |effect| match effect {
                    ThermalEffect::Unchanged => None,
                    ThermalEffect::Target => Some(Heat::Target),
                    ThermalEffect::Wait => Some(Heat::Wait),
                    ThermalEffect::Off => Some(Heat::Off),
                };
                if let Some(h) = heat(e.bed) {
                    self.bed = h;
                }
                if let Some(h) = heat(e.nozzle) {
                    self.nozzle = h;
                    self.nozzle_print = h != Heat::Off;
                }
                if e.fan_off {
                    self.fan_off = true;
                }
            }
            _ => {}
        }
    }

    /// Program state an accepted text line establishes immediately.
    pub fn accepted_text(&self, state: &mut MotionState) {
        if let Some(Item::Applied(step)) = self.queue.front() {
            match step {
                LifecycleStep::Home => state.position = Some(self.job.home_position_mm),
                LifecycleStep::ResetExtrusion => state.extrusion = 0.0,
                LifecycleStep::Block { block } => {
                    if block.effects.position == PositionEffect::Unknown {
                        state.position = None;
                    }
                    if block.effects.extrusion == ExtrusionEffect::Reset {
                        state.extrusion = 0.0;
                    }
                }
                LifecycleStep::MotorsOff => state.position = None,
                _ => {}
            }
        }
    }

    /// Independent preconditions of a lifecycle motion: 0 prime travel,
    /// 1 prime line, 2 lift, 3 XY park.
    pub fn check_motion(&self, kind: u8, state: &MotionState) -> Result<(), ManufacturingError> {
        if !self.homed || !self.modes {
            return Err(failed(
                "lifecycle motion without homing or established millimetre/absolute modes",
            ));
        }
        match kind {
            0 => {
                if !self.extrusion_known || state.extrusion != 0.0 {
                    return Err(failed("priming without a known zero extrusion axis"));
                }
                // An off (0 °C) first-layer bed has nothing to wait for.
                let bed_ready = self.bed == Heat::Wait
                    || (self.job.first_layer_bed_temperature_c == 0.0 && self.bed == Heat::Off);
                if !bed_ready || self.nozzle != Heat::Wait || !self.nozzle_print {
                    return Err(failed(
                        "priming before waiting for the bed and print nozzle temperatures",
                    ));
                }
            }
            2 | 3 if state.position.is_none() => {
                return Err(failed("park from an unknown position"));
            }
            _ => {}
        }
        Ok(())
    }

    /// Heaters, fan and motors must be off once the end sequence completes.
    pub fn check_shutdown(&self) -> Result<(), ManufacturingError> {
        if self.bed != Heat::Off || self.nozzle != Heat::Off || !self.fan_off || !self.motors_off {
            return Err(failed(
                "program ends without heater, fan and motor shutdown",
            ));
        }
        Ok(())
    }

    pub fn levelling_report(&self) -> &'static str {
        match self.levelling {
            Levelling::FirmwareDefault => {
                "Bed levelling left as the firmware sets it after homing; not commanded"
            }
            Levelling::Probed => {
                "Bed mesh probed each print and active: firmware corrects commanded Z below command space"
            }
            Levelling::Restored => {
                "Stored bed mesh restored and active: firmware corrects commanded Z below command space; the stored mesh is not checked"
            }
            Levelling::Off => "Bed levelling explicitly disabled",
            Levelling::DeclaredActive => {
                "Bed levelling declared active by an opaque block: firmware corrects commanded Z below command space"
            }
            Levelling::DeclaredOff => "Bed levelling declared off by an opaque block",
        }
    }
}

/// A report line (at most 256 bytes) for each opaque block: content not
/// interpreted, declared effects trusted.
pub(super) fn block_limitations(job: &SingleExtruderJob) -> Vec<String> {
    job.lifecycle
        .blocks()
        .map(|(start, block)| {
            let e = block.effects;
            let mut effects = Vec::new();
            if e.homes {
                effects.push("homes".to_string());
            }
            if e.position == PositionEffect::Unknown {
                effects.push("position unknown".into());
            }
            if e.levelling != LevellingEffect::Unchanged {
                effects.push(format!("levelling {:?}", e.levelling).to_lowercase());
            }
            if e.bed != ThermalEffect::Unchanged {
                effects.push(format!("bed {:?}", e.bed).to_lowercase());
            }
            if e.nozzle != ThermalEffect::Unchanged {
                effects.push(format!("nozzle {:?}", e.nozzle).to_lowercase());
            }
            if e.extrusion != ExtrusionEffect::Preserved {
                effects.push(format!("E {:?}", e.extrusion).to_lowercase());
            }
            if e.modes == ModeEffect::Unknown {
                effects.push("modes unknown".into());
            }
            if e.fan_off {
                effects.push("fan off".into());
            }
            if e.motors_off {
                effects.push("motors off".into());
            }
            if effects.is_empty() {
                effects.push("none".into());
            }
            let label: String = block.label.chars().take(48).collect();
            format!(
                "Opaque {} block '{label}' ({} lines) emitted verbatim, not interpreted; declared effects trusted: {}",
                if start { "start" } else { "end" },
                block.lines.len(),
                effects.join(", "),
            )
        })
        .collect()
}

/// PrusaSlicer's MK4S probe area over the first layer's XY bounds (whole
/// millimetres, at least 32 mm wide, 4 mm below the part), within the bed.
fn mesh_area(plan: &NormalizedPrintPlan, bed: spiling_contracts::geometry::AabbMm) -> [f64; 4] {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for point in plan
        .layers
        .first()
        .into_iter()
        .flat_map(|layer| &layer.paths)
        .flat_map(|path| &path.points_mm)
    {
        for axis in 0..2 {
            lo[axis] = lo[axis].min(point[axis]);
            hi[axis] = hi[axis].max(point[axis]);
        }
    }
    let size = [bed.max[0] - bed.min[0], bed.max[1] - bed.min[1]];
    if !(lo[0] <= hi[0] && lo[1] <= hi[1]) {
        return [0.0, 0.0, size[0], size[1]];
    }
    let (lo, hi) = (lo.map(f64::floor), hi.map(f64::ceil));
    let x = ((lo[0] + 32.0).min(size[0]) - 32.0).max(0.0);
    let y = (lo[1] - 4.0).max(0.0);
    let w = (lo[0] + 32.0).max(hi[0]).min(size[0]) - x;
    let h = hi[1].min(size[1]) - y;
    [x, y, w, h]
}
