// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Declared firmware lifecycle of a complete single-extruder job: the ordered
//! start steps before the body and end steps after it, rendered in the
//! printer's output dialect. Typed steps have one fixed command form per
//! dialect. Opaque blocks carry operator/printer macro lines the verifier
//! cannot interpret; they declare their effects, which validation and the
//! verifier check the surrounding program against and report as trusted
//! declarations, never refuse. Rules: `docs/protocol/manufacturing.md`.

use super::*;

/// Whether the toolhead position is still known after a block.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PositionEffect {
    Preserved,
    Unknown,
}
/// What a block does to one heater, at the job's operating temperature.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThermalEffect {
    Unchanged,
    /// Sets the job target without waiting.
    Target,
    /// Sets the job target and waits for it.
    Wait,
    Off,
    /// The nozzle only: waits at a non-printing standby or probing
    /// temperature (for example after a purge and wipe).
    Standby,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LevellingEffect {
    Unchanged,
    /// Leaves a firmware bed mesh active (probed or restored).
    Active,
    Off,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExtrusionEffect {
    Preserved,
    /// Leaves the logical E axis at zero.
    Reset,
    Unknown,
}
/// Whether millimetre/absolute XYZ/absolute E modes survive a block.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModeEffect {
    Preserved,
    Unknown,
}

/// Declared effects of an opaque block. They are operator declarations, not
/// observed firmware behaviour; the verifier trusts and reports them.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BlockEffects {
    /// The block homes every axis (the position afterwards is unknown).
    pub homes: bool,
    pub position: PositionEffect,
    pub levelling: LevellingEffect,
    pub bed: ThermalEffect,
    pub nozzle: ThermalEffect,
    pub extrusion: ExtrusionEffect,
    pub modes: ModeEffect,
    pub fan_off: bool,
    /// Steppers are released: homing and position are lost.
    pub motors_off: bool,
}
impl BlockEffects {
    /// A block that changes nothing the program relies on.
    pub const NONE: Self = Self {
        homes: false,
        position: PositionEffect::Preserved,
        levelling: LevellingEffect::Unchanged,
        bed: ThermalEffect::Unchanged,
        nozzle: ThermalEffect::Unchanged,
        extrusion: ExtrusionEffect::Preserved,
        modes: ModeEffect::Preserved,
        fan_off: false,
        motors_off: false,
    };
}

/// Placeholders a block line may contain; each renders the first-layer
/// target (the temperature the start sequence heats to) as integral Celsius.
pub const BLOCK_PLACEHOLDERS: [&str; 2] = ["{bed_temperature_c}", "{nozzle_temperature_c}"];

/// Opaque operator/printer lines, emitted verbatim after placeholder
/// substitution, with their declared effects.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LifecycleBlock {
    pub label: String,
    pub lines: Vec<String>,
    pub effects: BlockEffects,
}
impl LifecycleBlock {
    pub fn validate(&self, dialect: OutputDialect) -> Result<(), ManufacturingError> {
        label(&self.label)?;
        if self.lines.is_empty() {
            return Err(invalid("a lifecycle block must contain at least one line"));
        }
        for line in &self.lines {
            if line.is_empty()
                || line.trim() != line
                || !line.bytes().all(|b| (0x20..0x7f).contains(&b))
            {
                return Err(invalid(
                    "lifecycle block lines must be nonempty trimmed printable ASCII",
                ));
            }
            // Every brace must open a known placeholder; a rendered
            // temperature has at most three digits.
            let mut rest = line.as_str();
            let mut rendered = line.len();
            while let Some(open) = rest.find(['{', '}']) {
                let tail = &rest[open..];
                let Some(name) = BLOCK_PLACEHOLDERS.iter().find(|p| tail.starts_with(**p)) else {
                    return Err(invalid(
                        "lifecycle block braces must be {bed_temperature_c} or {nozzle_temperature_c}",
                    ));
                };
                rendered = rendered + 3 - name.len();
                rest = &tail[name.len()..];
            }
            if dialect.max_line_bytes().is_some_and(|max| rendered > max) {
                return Err(invalid(
                    "lifecycle block line exceeds the firmware command line limit",
                ));
            }
        }
        if self.effects.bed == ThermalEffect::Standby {
            return Err(invalid("only the nozzle has a standby effect"));
        }
        if self.effects.homes && self.effects.position != PositionEffect::Unknown {
            return Err(invalid("a block that homes leaves the position unknown"));
        }
        if self.effects.motors_off && self.effects.position != PositionEffect::Unknown {
            return Err(invalid(
                "a block that releases the motors leaves the position unknown",
            ));
        }
        Ok(())
    }
}

impl LifecycleBlock {
    /// The conventional Klipper start macro with its slicer temperature
    /// parameters (<https://www.klipper3d.org/Slicers.html>). The printer's
    /// macro is expected to home, level and wait for both temperatures.
    pub fn klipper_start_print() -> Self {
        Self {
            label: "Klipper START_PRINT macro".into(),
            lines: vec![
                "START_PRINT BED_TEMP={bed_temperature_c} EXTRUDER_TEMP={nozzle_temperature_c}"
                    .into(),
            ],
            effects: BlockEffects {
                homes: true,
                position: PositionEffect::Unknown,
                levelling: LevellingEffect::Active,
                bed: ThermalEffect::Wait,
                nozzle: ThermalEffect::Wait,
                extrusion: ExtrusionEffect::Unknown,
                modes: ModeEffect::Unknown,
                fan_off: false,
                motors_off: false,
            },
        }
    }
    /// Bambu Lab printer services the reference start sequences enable:
    /// vibration suppression (`M975 S1`) and filament runout detection
    /// (`M412 S1`). Observed in Bambu Studio 02.09.01.70's A1 and A1 mini
    /// machine start G-code; no position, thermal or mode effect is declared.
    pub fn bambu_printer_services() -> Self {
        Self {
            label: "Bambu vibration suppression and runout detection".into(),
            lines: vec!["M975 S1".into(), "M412 S1".into()],
            effects: BlockEffects::NONE,
        }
    }
    /// A Bambu on-screen stage message: `M1002 gcode_claim_action : <action>`
    /// sets the stage the printer's screen and app show. Action numbers as
    /// Bambu Studio 02.09.01.70's machine start G-code uses them (0 clears,
    /// 1 bed levelling, 2 and 54 heatbed preheat, 13 homing, 14 nozzle
    /// cleaning). Display only: no effect.
    pub fn bambu_stage(action: u8, stage: &str) -> Self {
        Self {
            label: format!("Bambu screen stage: {stage}"),
            lines: vec![format!("M1002 gcode_claim_action : {action}")],
            effects: BlockEffects::NONE,
        }
    }
    /// Bambu firmware Z trim (`G29.1 Z<mm>`), a persistent offset below
    /// command space. The reference start sequences clear it first and, on a
    /// textured PEI plate, lower the nozzle by the texture the homing probe
    /// touched (-0.02 mm on A1/A1 mini, -0.04 mm on P1/X1). Command-space
    /// positions are unchanged; the physical offset is reported.
    pub fn bambu_z_trim(trim_mm: f64, label: &str) -> Self {
        Self {
            label: label.into(),
            lines: vec![if trim_mm == 0.0 {
                "G29.1 Z0".into()
            } else {
                format!("G29.1 Z{trim_mm}")
            }],
            effects: BlockEffects::NONE,
        }
    }
    /// Bambu P1/X1 purge and nozzle wipe, re-authored for this producer from
    /// the facts the Bambu Studio 02.09.01.70 P1S/X1C start sequences show:
    /// the waste chute opening behind the bed at Y 265 (outside the 256 mm
    /// printable square), the silicone wiper along X 60..100 at that Y,
    /// reachable at Z 5 after homing. Requires a homed printer. Heats to the
    /// first-layer nozzle temperature, purges 40 mm of filament into the
    /// chute, retracts 0.5 mm, cools toward the 140 °C probing temperature
    /// while wiping across the wiper, and waits there with the part fan off.
    /// Omitted from the reference: AMS loading/flush volumes, the 250 °C
    /// flush, the exposed-steel scrub with soft endstops disabled.
    pub fn bambu_p1x1_purge_and_wipe() -> Self {
        let mut lines: Vec<String> = [
            "M1002 gcode_claim_action : 14",
            "G90",
            "M83",
            "G1 Z5 F1200",
            "G1 X54 Y230 F18000",
            "G1 Y265 F3000",
            "M109 S{nozzle_temperature_c}",
            "G1 E40 F200",
            "M400",
            "M106 S255",
            "G1 E-0.5 F300",
            "M104 S140",
        ]
        .map(String::from)
        .to_vec();
        for _ in 0..2 {
            lines.extend(["G1 X100 F5000", "G1 X70 F15000"].map(String::from));
        }
        lines.push("M109 S140".into());
        for _ in 0..2 {
            lines.extend(["G1 X100 F5000", "G1 X70 F15000"].map(String::from));
        }
        lines.extend(["M106 S0", "G1 Z10 F1200", "G1 X128 Y128 F18000"].map(String::from));
        Self {
            label: "Bambu P1/X1 purge into the waste chute and nozzle wipe".into(),
            lines,
            effects: BlockEffects {
                homes: false,
                position: PositionEffect::Unknown,
                levelling: LevellingEffect::Unchanged,
                bed: ThermalEffect::Unchanged,
                nozzle: ThermalEffect::Standby,
                extrusion: ExtrusionEffect::Unknown,
                modes: ModeEffect::Unknown,
                fan_off: true,
                motors_off: false,
            },
        }
    }
    /// Bambu A1 / A1 mini purge and wiper flick, re-authored for this producer
    /// from the facts Bambu Studio 02.09.01.70's A1 (`N2S`) and A1 mini (`N1`)
    /// machine start sequences show: the left-side filament purge wiper sits at
    /// X -48.2 mm (shake to -28.5 mm) on A1 and X -13.5 mm (shake to 0 mm) on
    /// A1 mini, reachable at Y 0, Z 10 after homing. Heats to the first-layer
    /// nozzle temperature, purges 40 mm, retracts 0.5 mm, cools toward 140 °C
    /// while flicking across the wiper, waits at 140 °C, flicks again, turns
    /// off the part fan and returns to bed center. Omitted from the reference:
    /// AMS loading/flush volumes, 250 °C flush, and disabling soft endstops.
    pub fn bambu_a1_purge_and_wipe(mini: bool) -> Self {
        let (wiper_x, flick_x, center_xy, label) = if mini {
            (
                "-13.5",
                "0",
                "X90 Y90",
                "Bambu A1 mini purge over the wiper and nozzle flick",
            )
        } else {
            (
                "-48.2",
                "-28.5",
                "X128 Y128",
                "Bambu A1 purge over the wiper and nozzle flick",
            )
        };
        let mut lines: Vec<String> = vec![
            "M1002 gcode_claim_action : 14".into(),
            "G90".into(),
            "M83".into(),
            "G1 Z10 F1200".into(),
            format!("G1 X{wiper_x} Y0 F18000"),
            "M109 S{nozzle_temperature_c}".into(),
            "G1 E40 F200".into(),
            "M400".into(),
            "M106 S255".into(),
            "G1 E-0.5 F300".into(),
            "M104 S140".into(),
        ];
        for _ in 0..2 {
            lines.push(format!("G1 X{flick_x} F18000"));
            lines.push(format!("G1 X{wiper_x} F3000"));
        }
        lines.push("M109 S140".into());
        for _ in 0..2 {
            lines.push(format!("G1 X{flick_x} F12000"));
            lines.push(format!("G1 X{wiper_x} F3000"));
        }
        lines.extend([
            "M106 S0".into(),
            "G1 Z10 F1200".into(),
            format!("G1 {center_xy} F18000"),
        ]);
        Self {
            label: label.into(),
            lines,
            effects: BlockEffects {
                homes: false,
                position: PositionEffect::Unknown,
                levelling: LevellingEffect::Unchanged,
                bed: ThermalEffect::Unchanged,
                nozzle: ThermalEffect::Standby,
                extrusion: ExtrusionEffect::Unknown,
                modes: ModeEffect::Unknown,
                fan_off: true,
                motors_off: false,
            },
        }
    }
    /// The conventional Klipper end macro: heaters, fan and motors off.
    pub fn klipper_end_print() -> Self {
        Self {
            label: "Klipper END_PRINT macro".into(),
            lines: vec!["END_PRINT".into()],
            effects: BlockEffects {
                homes: false,
                position: PositionEffect::Unknown,
                levelling: LevellingEffect::Unchanged,
                bed: ThermalEffect::Off,
                nozzle: ThermalEffect::Off,
                extrusion: ExtrusionEffect::Unknown,
                modes: ModeEffect::Unknown,
                fan_off: true,
                motors_off: true,
            },
        }
    }
}

fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

/// One lifecycle step. Each typed step has one fixed command form per dialect
/// (see `OutputDialect::admits`); `Prime` and `Park` are the bounded lifecycle
/// motions owned by the job declaration.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LifecycleStep {
    /// `M149 C`; Marlin builds with `TEMPERATURE_UNITS_SUPPORT` only.
    CelsiusUnits,
    /// `M200 D0`: filament-length E.
    VolumetricOff,
    /// `M220 S100`.
    FeedScaleReset,
    /// `M221 S100`.
    FlowScaleReset,
    /// `M107` (Bambu `M106 S0`).
    FanOff,
    /// Prusa `M862.1 P<nozzle>`: the firmware checks the installed nozzle.
    NozzleCheck,
    /// Prusa `M862.3 P "<model>"`: the firmware checks the printer model.
    ModelCheck {
        model: String,
    },
    /// `M140 S<bed>` at the first-layer bed temperature.
    BedTarget,
    /// Wait for the bed at the first-layer temperature (`M190 R`, Klipper
    /// `M190 S`). An off (0 °C) bed has neither step.
    BedWait,
    /// `M104 S<t>`: the standby temperature (a material profile supplies it),
    /// capped at the first-layer nozzle temperature, or the first-layer
    /// nozzle temperature when `standby_c` is null.
    NozzleTarget {
        #[serde(deserialize_with = "required_option")]
        standby_c: Option<f64>,
    },
    /// Wait for the nozzle (`M109 R`, Klipper `M109 S`) at the same choice.
    NozzleWait {
        #[serde(deserialize_with = "required_option")]
        standby_c: Option<f64>,
    },
    /// `G28`: all axes; the position becomes the declared home position.
    /// Bambu `G28 T<t>` permits homing up to `t`: the waited standby plus 5 °C.
    Home,
    /// Probe a bed mesh and leave it active (Marlin `G29`, Prusa `M555`+`G29`
    /// sequence, Klipper `BED_MESH_CALIBRATE`, Bambu: nozzle to its 140 °C
    /// probing temperature, `G29.2 S1` then `G29 A1` over the first layer when
    /// the printer's bed-levelling print option is on, so either a fresh or the
    /// stored mesh is active; the nozzle is left at 140 °C).
    ProbeMesh,
    /// Activate the stored mesh (Marlin `M420 S1`, Klipper
    /// `BED_MESH_PROFILE LOAD=default`, Bambu `G29.2 S1`).
    RestoreMesh,
    /// Disable levelling (Marlin `M420 S0`, Klipper `BED_MESH_CLEAR`, Bambu
    /// `G29.2 S0`).
    LevellingOff,
    /// `G21`, `G90` and the dialect's extrusion mode (`M83` for every
    /// complete single-extruder dialect): re-establish modes after an opaque block.
    RestoreModes,
    /// `G92 E0`.
    ResetExtrusion,
    /// Travel to the prime start and deposit the prime line.
    Prime,
    /// `M400`.
    Barrier,
    /// Vertical lift to the highest emitted Z plus the declared clearance
    /// (capped at the envelope top), then XY park at that height.
    Park,
    /// `M104 S0`.
    NozzleOff,
    /// `M140 S0`.
    BedOff,
    /// Release steppers (`M84 X Y E` holding Z on Marlin/Prusa, Klipper `M84`,
    /// Bambu `M18 X Y Z`).
    MotorsOff,
    /// Bambu single-nozzle AMS initial load (`M620 M`, `M620 S<slot>A`,
    /// `T<slot>`, `M621 S<slot>A`). Start sequence only, when AMS is declared.
    AmsLoad {
        slot: u8,
    },
    /// Bambu single-nozzle AMS final unload (`M620 S255`, `T255`, `M621 S255`).
    /// End sequence only, after park and before nozzle heater off.
    AmsUnload,
    Block {
        block: LifecycleBlock,
    },
}

/// The declared start and end sequences around the body.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FirmwareLifecycle {
    pub start: Vec<LifecycleStep>,
    pub end: Vec<LifecycleStep>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Heat {
    Off,
    Target,
    Wait,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Nozzle {
    Off,
    Standby(Heat),
    Print(Heat),
}

/// Abstract lifecycle state; the verifier keeps its own equivalent.
struct State {
    homed: bool,
    position: bool,
    modes: bool,
    extrusion_zero: bool,
    bed: Heat,
    nozzle: Nozzle,
    fan_off: bool,
    motors_off: bool,
}

impl FirmwareLifecycle {
    /// The generic Marlin 2 table for a printer without a firmware mesh, as
    /// the software reference printers declare it.
    pub fn software_reference() -> Self {
        use LifecycleStep as S;
        Self {
            start: vec![
                S::FeedScaleReset,
                S::FlowScaleReset,
                S::VolumetricOff,
                S::FanOff,
                S::BedTarget,
                S::NozzleTarget {
                    standby_c: Some(150.0),
                },
                S::Home,
                S::BedWait,
                S::NozzleWait { standby_c: None },
                S::Prime,
            ],
            end: vec![
                S::Barrier,
                S::Park,
                S::Barrier,
                S::NozzleOff,
                S::BedOff,
                S::FanOff,
                S::MotorsOff,
            ],
        }
    }
    pub fn validate(
        &self,
        dialect: OutputDialect,
        job: &SingleExtruderJob,
    ) -> Result<(), ManufacturingError> {
        // The program preamble establishes G21/G90, the extrusion mode and G92 E0.
        let mut s = State {
            homed: false,
            position: false,
            modes: true,
            extrusion_zero: true,
            bed: Heat::Off,
            nozzle: Nozzle::Off,
            fan_off: false,
            motors_off: false,
        };
        let refuse = |m: &str| Err(invalid(m));
        let Some(LifecycleStep::Prime) = self.start.last() else {
            return refuse("the start sequence must end with the prime");
        };
        let mut ams_loads = 0usize;
        for (index, step) in self.start.iter().enumerate() {
            if !dialect.admits(step) {
                return refuse("lifecycle step is not in the output dialect's command set");
            }
            match step {
                LifecycleStep::AmsLoad { slot } => {
                    let Some(ams) = job.bambu_ams() else {
                        return refuse("AMS load requires a declared Bambu AMS configuration");
                    };
                    if usize::from(*slot) >= ams.slots.len() {
                        return refuse("AMS initial load slot exceeds declared AMS slots");
                    }
                    if !s.homed {
                        return refuse("AMS initial load requires a homed printer");
                    }
                    ams_loads += 1;
                }
                LifecycleStep::Prime if index + 1 == self.start.len() => {
                    if !s.homed {
                        return refuse("lifecycle motion before homing");
                    }
                    if !s.modes || !s.extrusion_zero {
                        return refuse("priming requires established modes and E at zero");
                    }
                    // An off (0 °C) bed has nothing to wait for.
                    let bed_ready = s.bed == Heat::Wait
                        || (job.first_layer_bed_temperature_c == 0.0 && s.bed == Heat::Off);
                    if !bed_ready || s.nozzle != Nozzle::Print(Heat::Wait) {
                        return refuse("priming requires waited bed and nozzle temperatures");
                    }
                    // The prime travel is a full absolute XYZ move.
                    s.position = true;
                }
                LifecycleStep::Prime
                | LifecycleStep::Park
                | LifecycleStep::NozzleOff
                | LifecycleStep::BedOff
                | LifecycleStep::MotorsOff
                | LifecycleStep::AmsUnload => {
                    return refuse("start sequence contains a priming, park or shutdown step");
                }
                _ => apply(step, &mut s, job, dialect)?,
            }
        }
        if job.bambu_ams().is_some() && ams_loads != 1 {
            return refuse("Bambu AMS start sequence must load the initial slot exactly once");
        }
        if !s.homed || !s.position {
            return refuse("the body requires a homed, known position");
        }
        // The body leaves a known position and nonzero E.
        s.extrusion_zero = false;
        let parks = self
            .end
            .iter()
            .filter(|step| matches!(step, LifecycleStep::Park))
            .count();
        if parks != 1 {
            return refuse("the end sequence must park exactly once");
        }
        let mut parked = false;
        let mut ams_unloads = 0usize;
        for step in &self.end {
            if !dialect.admits(step) {
                return refuse("lifecycle step is not in the output dialect's command set");
            }
            match step {
                LifecycleStep::Park => {
                    if !s.homed || !s.position || !s.modes {
                        return refuse("park requires a homed, known position and modes");
                    }
                    parked = true;
                }
                LifecycleStep::AmsUnload => {
                    if job.bambu_ams().is_none() {
                        return refuse("AMS unload requires a declared Bambu AMS configuration");
                    }
                    if !parked || s.nozzle == Nozzle::Off {
                        return refuse(
                            "AMS unload must occur after parking and while the nozzle is still heated",
                        );
                    }
                    ams_unloads += 1;
                }
                LifecycleStep::Prime
                | LifecycleStep::Home
                | LifecycleStep::ProbeMesh
                | LifecycleStep::RestoreMesh
                | LifecycleStep::BedTarget
                | LifecycleStep::BedWait
                | LifecycleStep::NozzleTarget { .. }
                | LifecycleStep::NozzleWait { .. }
                | LifecycleStep::AmsLoad { .. } => {
                    return refuse("end sequence may not home, probe, prime, load AMS or heat");
                }
                LifecycleStep::Block { block } if block.effects.homes => {
                    return refuse("end sequence may not home");
                }
                _ => apply(step, &mut s, job, dialect)?,
            }
        }
        if job.bambu_ams().is_some() && ams_unloads != 1 {
            return refuse("Bambu AMS end sequence must unload exactly once");
        }
        if s.bed != Heat::Off || s.nozzle != Nozzle::Off || !s.fan_off || !s.motors_off {
            return refuse("the end sequence must turn off heaters, fan and motors");
        }
        Ok(())
    }

    /// Opaque blocks in order, with whether each is in the start sequence.
    pub fn blocks(&self) -> impl Iterator<Item = (bool, &LifecycleBlock)> {
        fn block(start: bool, step: &LifecycleStep) -> Option<(bool, &LifecycleBlock)> {
            match step {
                LifecycleStep::Block { block } => Some((start, block)),
                _ => None,
            }
        }
        self.start
            .iter()
            .filter_map(|s| block(true, s))
            .chain(self.end.iter().filter_map(|s| block(false, s)))
    }
}

fn apply(
    step: &LifecycleStep,
    s: &mut State,
    job: &SingleExtruderJob,
    dialect: OutputDialect,
) -> Result<(), ManufacturingError> {
    let standby = |standby_c: &Option<f64>| -> Result<bool, ManufacturingError> {
        match standby_c {
            None => Ok(false),
            Some(t) if t.is_finite() && *t >= 1.0 && t.fract() == 0.0 => {
                if *t > job.maximum_nozzle_temperature_c {
                    return Err(invalid("standby nozzle temperature exceeds the maximum"));
                }
                Ok(true)
            }
            Some(_) => Err(invalid(
                "standby nozzle temperature must be integral positive Celsius",
            )),
        }
    };
    match step {
        LifecycleStep::Home => {
            s.homed = true;
            s.position = true;
            s.motors_off = false;
        }
        LifecycleStep::ProbeMesh | LifecycleStep::RestoreMesh | LifecycleStep::LevellingOff => {
            if !s.homed {
                return Err(invalid("bed levelling before homing"));
            }
            // Bambu probes with the nozzle cooled to its probing temperature.
            if dialect == OutputDialect::BambuLabGcode3mfV1 && *step == LifecycleStep::ProbeMesh {
                s.nozzle = Nozzle::Standby(Heat::Wait);
            }
        }
        LifecycleStep::ModelCheck { model } => {
            if model.is_empty()
                || model.len() > 32
                || !model
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.+".contains(&b))
            {
                return Err(invalid("printer model check must be a short identifier"));
            }
        }
        LifecycleStep::BedTarget | LifecycleStep::BedWait
            if job.first_layer_bed_temperature_c == 0.0 =>
        {
            return Err(invalid(
                "an off (0 °C) first-layer bed has no bed target or wait step",
            ));
        }
        LifecycleStep::BedTarget => s.bed = s.bed.max(Heat::Target),
        LifecycleStep::BedWait => s.bed = Heat::Wait,
        LifecycleStep::NozzleTarget { standby_c } => {
            s.nozzle = if standby(standby_c)? {
                Nozzle::Standby(Heat::Target)
            } else {
                Nozzle::Print(Heat::Target)
            };
        }
        LifecycleStep::NozzleWait { standby_c } => {
            s.nozzle = if standby(standby_c)? {
                Nozzle::Standby(Heat::Wait)
            } else {
                Nozzle::Print(Heat::Wait)
            };
        }
        LifecycleStep::RestoreModes => s.modes = true,
        LifecycleStep::ResetExtrusion => s.extrusion_zero = true,
        LifecycleStep::FanOff => s.fan_off = true,
        LifecycleStep::NozzleOff => s.nozzle = Nozzle::Off,
        LifecycleStep::BedOff => s.bed = Heat::Off,
        LifecycleStep::MotorsOff => {
            s.motors_off = true;
            s.homed = false;
            s.position = false;
        }
        LifecycleStep::Block { block } => {
            let e = block.effects;
            if e.homes {
                s.homed = true;
                s.motors_off = false;
            }
            if e.position == PositionEffect::Unknown {
                s.position = false;
            }
            if e.motors_off {
                s.motors_off = true;
                s.homed = false;
            }
            if e.modes == ModeEffect::Unknown {
                s.modes = false;
            }
            match e.extrusion {
                ExtrusionEffect::Preserved => {}
                ExtrusionEffect::Reset => s.extrusion_zero = true,
                ExtrusionEffect::Unknown => s.extrusion_zero = false,
            }
            let heat = |effect| match effect {
                ThermalEffect::Unchanged => None,
                ThermalEffect::Target => Some(Heat::Target),
                ThermalEffect::Wait | ThermalEffect::Standby => Some(Heat::Wait),
                ThermalEffect::Off => Some(Heat::Off),
            };
            if let Some(h) = heat(e.bed) {
                s.bed = h;
            }
            if let Some(h) = heat(e.nozzle) {
                s.nozzle = if h == Heat::Off {
                    Nozzle::Off
                } else if e.nozzle == ThermalEffect::Standby {
                    Nozzle::Standby(h)
                } else {
                    Nozzle::Print(h)
                };
            }
            if e.fan_off {
                s.fan_off = true;
            }
        }
        LifecycleStep::CelsiusUnits
        | LifecycleStep::VolumetricOff
        | LifecycleStep::FeedScaleReset
        | LifecycleStep::FlowScaleReset
        | LifecycleStep::NozzleCheck
        | LifecycleStep::Barrier
        | LifecycleStep::Prime
        | LifecycleStep::Park
        | LifecycleStep::AmsLoad { .. }
        | LifecycleStep::AmsUnload => {}
    }
    Ok(())
}

/// Output number precision of a dialect: decimals for coordinates, E and F.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramPrecision {
    pub xyz_decimals: usize,
    pub e_decimals: usize,
    pub f_decimals: usize,
}

impl OutputDialect {
    /// Complete single-extruder dialects with a declared lifecycle.
    pub fn single_extruder_lifecycle(self) -> bool {
        matches!(
            self,
            Self::Marlin2SingleExtruderGcodeV1
                | Self::PrusaMarlinSingleExtruderGcodeV1
                | Self::KlipperSingleExtruderGcodeV1
                | Self::BambuLabGcode3mfV1
        )
    }
    /// Longest accepted command line, excluding the newline: Marlin and Prusa
    /// Buddy read commands into a `MAX_CMD_SIZE` (96 byte, NUL-terminated)
    /// buffer; Klipper has no such fixed buffer.
    pub fn max_line_bytes(self) -> Option<usize> {
        match self {
            Self::Marlin2SingleExtruderGcodeV1 | Self::PrusaMarlinSingleExtruderGcodeV1 => Some(95),
            Self::CartesianAbsoluteGcodeV1
            | Self::Marlin2FixedDualGcodeV1
            | Self::KlipperSingleExtruderGcodeV1
            | Self::BambuLabGcode3mfV1 => None,
        }
    }
    /// Complete single-extruder dialects command relative extrusion (`M83`):
    /// each E word is that move's filament delta, so firmware that keeps
    /// positions in 32-bit floats never accumulates a long absolute E. The
    /// diagnostic and fixed-dual dialects keep absolute E (`M82`).
    pub fn relative_extrusion(self) -> bool {
        self.single_extruder_lifecycle()
    }
    /// Every current dialect uses the `program_coordinates` lattice, which is
    /// also the planner's coordinate contract. A dialect whose firmware needs
    /// fewer decimals declares them here.
    pub fn precision(self) -> ProgramPrecision {
        use super::program_coordinates as lattice;
        ProgramPrecision {
            xyz_decimals: lattice::PROGRAM_XYZ_DECIMALS,
            e_decimals: lattice::PROGRAM_E_DECIMALS,
            f_decimals: lattice::PROGRAM_F_DECIMALS,
        }
    }
    /// Thermal waits use `R` (wait for heating or cooling) except on Klipper,
    /// whose `M109`/`M190` read only `S` and always settle at the target, and
    /// Bambu Lab, whose start sequences wait with `S` in both directions
    /// (Bambu Studio 02.09.01.70 A1/A1 mini machine start G-code).
    pub fn waits_with_r(self) -> bool {
        !matches!(
            self,
            Self::KlipperSingleExtruderGcodeV1 | Self::BambuLabGcode3mfV1
        )
    }
    /// The dialect's command set for typed lifecycle steps.
    pub fn admits(self, step: &LifecycleStep) -> bool {
        use LifecycleStep as S;
        let marlin = self == Self::Marlin2SingleExtruderGcodeV1;
        let prusa = self == Self::PrusaMarlinSingleExtruderGcodeV1;
        let klipper = self == Self::KlipperSingleExtruderGcodeV1;
        let bambu = self == Self::BambuLabGcode3mfV1;
        if !(marlin || prusa || klipper || bambu) {
            return false;
        }
        match step {
            S::CelsiusUnits | S::VolumetricOff => marlin,
            S::NozzleCheck | S::ModelCheck { .. } => prusa,
            S::RestoreMesh | S::LevellingOff => marlin || klipper || bambu,
            S::AmsLoad { .. } | S::AmsUnload => bambu,
            _ => true,
        }
    }
}
