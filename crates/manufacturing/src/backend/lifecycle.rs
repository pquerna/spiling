// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Per-dialect command forms of the declared lifecycle steps. References:
//! Marlin <https://marlinfw.org/meta/gcode/>, Prusa Buddy
//! <https://help.prusa3d.com/article/buddy-firmware-specific-g-code-commands_633112>
//! and PrusaSlicer's MK4S start G-code, Klipper
//! <https://www.klipper3d.org/G-Codes.html>.

use spiling_contracts::manufacturing::{
    BambuSeries, DeliveryPackage, LifecycleStep, NormalizedPrintPlan, OutputDialect,
    PrinterSpecification, SingleExtruderJob,
};

pub(super) enum Emit {
    Lines(Vec<String>),
    Prime,
    Park,
}

/// The Prusa `M555` probe area `[X, Y, W, H]`: PrusaSlicer's MK4S formula
/// over the first layer's XY bounds, in whole millimetres, at least 32 mm
/// wide and starting 4 mm below the part, within the bed.
pub(super) fn first_layer_mesh_area(
    plan: &NormalizedPrintPlan,
    printer: &PrinterSpecification,
) -> [f64; 4] {
    let e = printer.build_envelope;
    let size = [e.max[0] - e.min[0], e.max[1] - e.min[1]];
    let Some(layer) = plan.layers.first() else {
        return [0.0, 0.0, size[0], size[1]];
    };
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in layer.paths.iter().flat_map(|path| path.points_mm.iter()) {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    if lo[0] > hi[0] {
        return [0.0, 0.0, size[0], size[1]];
    }
    let lo = [lo[0].floor(), lo[1].floor()];
    let hi = [hi[0].ceil(), hi[1].ceil()];
    let x = (size[0].min(lo[0] + 32.0) - 32.0).max(0.0);
    let y = (lo[1] - 4.0).max(0.0);
    let w = size[0].min(hi[0].max(lo[0] + 32.0)) - x;
    let h = size[1].min(hi[1]) - y;
    [x, y, w, h]
}

/// The Bambu `G29 A1` probe area `[X, Y, I, J]`: the first layer's XY bounds
/// in whole millimetres (floor of the minimum, ceiling of the maximum) within
/// the bed, as Bambu Studio passes its first-layer print bounds.
pub(super) fn first_layer_bambu_area(
    plan: &NormalizedPrintPlan,
    printer: &PrinterSpecification,
) -> [f64; 4] {
    let e = printer.build_envelope;
    let size = [e.max[0] - e.min[0], e.max[1] - e.min[1]];
    let Some(layer) = plan.layers.first() else {
        return [0.0, 0.0, size[0], size[1]];
    };
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in layer.paths.iter().flat_map(|path| path.points_mm.iter()) {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    if lo[0] > hi[0] {
        return [0.0, 0.0, size[0], size[1]];
    }
    let x = lo[0].floor().max(0.0);
    let y = lo[1].floor().max(0.0);
    [
        x,
        y,
        hi[0].ceil().min(size[0]) - x,
        hi[1].ceil().min(size[1]) - y,
    ]
}

pub(super) fn render(
    step: &LifecycleStep,
    printer: &PrinterSpecification,
    job: &SingleExtruderJob,
    mesh_area: [f64; 4],
    bambu_area: [f64; 4],
) -> Emit {
    use LifecycleStep as S;
    let dialect = printer.capabilities.output_dialect;
    let klipper = dialect == OutputDialect::KlipperSingleExtruderGcodeV1;
    let prusa = dialect == OutputDialect::PrusaMarlinSingleExtruderGcodeV1;
    let bambu = dialect == OutputDialect::BambuLabGcode3mfV1;
    // Klipper's M109/M190 read only S and always settle at the target.
    let wait = if dialect.waits_with_r() { 'R' } else { 'S' };
    // The start sequence heats to the first-layer targets.
    let bed = job.first_layer_bed_temperature_c;
    let print = job.first_layer_nozzle_temperature_c;
    let nozzle = |standby_c: &Option<f64>| standby_c.map_or(print, |t| t.min(print));
    let one = |line: &str| Emit::Lines(vec![line.into()]);
    match step {
        S::CelsiusUnits => one("M149 C"),
        S::VolumetricOff => one("M200 D0"),
        S::FeedScaleReset => one("M220 S100"),
        S::FlowScaleReset => one("M221 S100"),
        // Bambu firmware turns the part fan off with PWM zero.
        S::FanOff if bambu => one("M106 S0"),
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
        // Bambu G28 permits homing up to its T temperature (A1 mini reference
        // `G28 T145` at 140 °C): the waited standby plus 5 °C.
        S::Home if bambu => {
            let standby = job
                .lifecycle
                .start
                .iter()
                .rev()
                .find_map(|s| match s {
                    S::NozzleWait { standby_c: Some(t) } => Some(t.min(print)),
                    _ => None,
                })
                .unwrap_or(print);
            Emit::Lines(vec![format!("G28 T{:.0}", standby + 5.0)])
        }
        S::Home => one("G28"),
        S::ProbeMesh if klipper => one("BED_MESH_CALIBRATE"),
        // PrusaSlicer's MK4S sequence over the first layer's area.
        S::ProbeMesh if prusa => {
            let [x, y, w, h] = mesh_area;
            Emit::Lines(vec![
                format!("M555 X{x} Y{y} W{w} H{h}"),
                "G29 P1".into(),
                "G29 P3.2".into(),
                "G29 P3.13".into(),
                "G29 A".into(),
            ])
        }
        // Bambu: enable mesh compensation, then probe the first layer's
        // area when the printer's bed-levelling print option is on.
        S::ProbeMesh if bambu => {
            let [x, y, w, h] = bambu_area;
            // A1 firmware probes with `G29 A1`, P1/X1 with `G29 A`.
            let probe = match &job.delivery {
                Some(DeliveryPackage::BambuGcode3mf {
                    series: BambuSeries::P1X1,
                    ..
                }) => "G29 A",
                _ => "G29 A1",
            };
            Emit::Lines(vec![
                // Bambu's reference probes at 140 °C on every plate.
                "M104 S140".into(),
                "M109 S140".into(),
                "G29.2 S1".into(),
                "M1002 judge_flag g29_before_print_flag".into(),
                "M622 J1".into(),
                "M1002 gcode_claim_action : 1".into(),
                format!("{probe} X{x} Y{y} I{w} J{h}"),
                "M400".into(),
                "M623".into(),
            ])
        }
        S::ProbeMesh => one("G29"),
        S::RestoreMesh if klipper => one("BED_MESH_PROFILE LOAD=default"),
        S::RestoreMesh if bambu => one("G29.2 S1"),
        S::RestoreMesh => one("M420 S1"),
        S::LevellingOff if klipper => one("BED_MESH_CLEAR"),
        S::LevellingOff if bambu => one("G29.2 S0"),
        S::LevellingOff => one("M420 S0"),
        S::RestoreModes => Emit::Lines(vec![
            "G21".into(),
            "G90".into(),
            if dialect.relative_extrusion() {
                "M83"
            } else {
                "M82"
            }
            .into(),
        ]),
        S::ResetExtrusion => one("G92 E0"),
        S::Barrier => one("M400"),
        S::NozzleOff => one("M104 S0"),
        S::BedOff => one("M140 S0"),
        S::MotorsOff if klipper => one("M84"),
        S::MotorsOff if bambu => one("M18 X Y Z"),
        // Holding Z keeps the gantry from settling onto the part.
        S::MotorsOff => one("M84 X Y E"),
        // Single-nozzle Bambu AMS initial load and final unload, matching the
        // command forms observed in Bambu Studio 02.09.01.70 A1/A1M/P1S/X1C profiles.
        S::AmsLoad { slot } => Emit::Lines(vec![
            "M620 M".into(),
            format!("M620 S{slot}A"),
            format!("T{slot}"),
            format!("M621 S{slot}A"),
        ]),
        S::AmsUnload => Emit::Lines(vec!["M620 S255".into(), "T255".into(), "M621 S255".into()]),
        S::Block { block } => Emit::Lines(
            block
                .lines
                .iter()
                .map(|line| {
                    line.replace("{bed_temperature_c}", &format!("{bed:.0}"))
                        .replace("{nozzle_temperature_c}", &format!("{print:.0}"))
                })
                .collect(),
        ),
        S::Prime => Emit::Prime,
        S::Park => Emit::Park,
    }
}
