// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Per-dialect command forms of the declared lifecycle steps. References:
//! Marlin <https://marlinfw.org/meta/gcode/>, Prusa Buddy
//! <https://help.prusa3d.com/article/buddy-firmware-specific-g-code-commands_633112>
//! and PrusaSlicer's MK4S start G-code, Klipper
//! <https://www.klipper3d.org/G-Codes.html>.

use spiling_contracts::manufacturing::{
    LifecycleStep, OutputDialect, PrinterSpecification, SingleExtruderJob,
};

pub(super) enum Emit {
    Lines(Vec<String>),
    Prime,
    Park,
}

pub(super) fn render(
    step: &LifecycleStep,
    printer: &PrinterSpecification,
    job: &SingleExtruderJob,
) -> Emit {
    use LifecycleStep as S;
    let dialect = printer.capabilities.output_dialect;
    let klipper = dialect == OutputDialect::KlipperSingleExtruderGcodeV1;
    let prusa = dialect == OutputDialect::PrusaMarlinSingleExtruderGcodeV1;
    // Klipper's M109/M190 read only S and always settle at the target.
    let wait = if dialect.waits_with_r() { 'R' } else { 'S' };
    let bed = job.bed_temperature_c;
    let nozzle = |standby_c: &Option<f64>| {
        standby_c.map_or(job.nozzle_temperature_c, |t| {
            t.min(job.nozzle_temperature_c)
        })
    };
    let one = |line: &str| Emit::Lines(vec![line.into()]);
    match step {
        S::CelsiusUnits => one("M149 C"),
        S::VolumetricOff => one("M200 D0"),
        S::FeedScaleReset => one("M220 S100"),
        S::FlowScaleReset => one("M221 S100"),
        S::FanOff => one("M107"),
        S::NozzleCheck => Emit::Lines(vec![format!("M862.1 P{}", printer.nozzle_diameter_mm)]),
        S::ModelCheck { model } => Emit::Lines(vec![format!("M862.3 P \"{model}\"")]),
        S::BedTarget => Emit::Lines(vec![format!("M140 S{bed:.0}")]),
        S::BedWait => Emit::Lines(vec![format!("M190 {wait}{bed:.0}")]),
        S::NozzleTarget { standby_c } => {
            Emit::Lines(vec![format!("M104 S{:.0}", nozzle(standby_c))])
        }
        S::NozzleWait { standby_c } => {
            Emit::Lines(vec![format!("M109 {wait}{:.0}", nozzle(standby_c))])
        }
        S::Home => one("G28"),
        S::ProbeMesh if klipper => one("BED_MESH_CALIBRATE"),
        // PrusaSlicer's MK4S sequence over the whole bed as the print area.
        S::ProbeMesh if prusa => {
            let e = printer.build_envelope;
            Emit::Lines(vec![
                format!(
                    "M555 X0 Y0 W{} H{}",
                    e.max[0] - e.min[0],
                    e.max[1] - e.min[1]
                ),
                "G29 P1".into(),
                "G29 P3.2".into(),
                "G29 P3.13".into(),
                "G29 A".into(),
            ])
        }
        S::ProbeMesh => one("G29"),
        S::RestoreMesh if klipper => one("BED_MESH_PROFILE LOAD=default"),
        S::RestoreMesh => one("M420 S1"),
        S::LevellingOff if klipper => one("BED_MESH_CLEAR"),
        S::LevellingOff => one("M420 S0"),
        S::RestoreModes => Emit::Lines(vec!["G21".into(), "G90".into(), "M82".into()]),
        S::ResetExtrusion => one("G92 E0"),
        S::Barrier => one("M400"),
        S::NozzleOff => one("M104 S0"),
        S::BedOff => one("M140 S0"),
        S::MotorsOff if klipper => one("M84"),
        // Holding Z keeps the gantry from settling onto the part.
        S::MotorsOff => one("M84 X Y E"),
        S::Block { block } => Emit::Lines(
            block
                .lines
                .iter()
                .map(|line| {
                    line.replace("{bed_temperature_c}", &format!("{bed:.0}"))
                        .replace(
                            "{nozzle_temperature_c}",
                            &format!("{:.0}", job.nozzle_temperature_c),
                        )
                })
                .collect(),
        ),
        S::Prime => Emit::Prime,
        S::Park => Emit::Park,
    }
}
