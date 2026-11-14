// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Bambu Lab program conventions around the declared lifecycle: the header
//! block the printer and Bambu Studio read, the executable-block markers, and
//! per-layer markers (`; CHANGE_LAYER`, `M73 P/R` progress, `M73 L` layer
//! number, `M991 S0 P` layer-change notification). The marker forms were
//! observed in Bambu Studio 02.09.01.70 A1/A1 mini exports; the values here
//! are this producer's own. Progress and remaining time come from a simple
//! producer estimate over the same realization stream (each move at its
//! feed, capped by the speed one acceleration reaches over its length); they
//! are display data, not the verifier's independent estimate.

use crate::{check_cancel, realization};
use spiling_contracts::manufacturing::program_coordinates::project_xyz6_point_mm;
use spiling_contracts::manufacturing::*;
use std::fmt::Write;
use std::sync::atomic::AtomicBool;

pub(super) struct Survey {
    /// Producer-estimated seconds of each scheduled layer.
    layer_seconds: Vec<f64>,
    total_seconds: f64,
    filament_mm: f64,
    filament_diameter_mm: f64,
    max_z_mm: f64,
    layers: Vec<(f64, f64)>,
}

impl Survey {
    pub fn new(
        plan: &NormalizedPrintPlan,
        intent: &ManufacturingIntent,
        job: &SingleExtruderJob,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        let acceleration = job.timing_model.map(|m| m.acceleration_mm_s2);
        let move_seconds = |length: f64, speed: f64| {
            if length <= 0.0 || speed <= 0.0 {
                return 0.0;
            }
            let reachable = acceleration.map_or(speed, |a| speed.min((a * length).sqrt()));
            length / reachable
        };
        let mut layer_seconds = vec![0.0; plan.layers.len()];
        let mut layer = None;
        let mut position = Some(project_xyz6_point_mm(job.prime_end_mm)?);
        let mut extrusion = job.prime_filament_length_mm;
        let mut max_z = job.prime_end_mm[2].max(job.prime_start_mm[2]);
        let (_, last_extrusion) =
            realization::stream(plan, intent, cancel, position, extrusion, |event| {
                use realization::Event;
                let mut seconds = 0.0;
                match event {
                    Event::Layer(index) => layer = Some(index as usize),
                    Event::Travel(to, speed)
                    | Event::Deposit(to, _, speed)
                    | Event::Waste(to, _, speed)
                    | Event::Arc(to, _, _, _, speed) => {
                        let to = project_xyz6_point_mm(to)?;
                        if let Some(from) = position {
                            let length = (to[0] - from[0])
                                .hypot(to[1] - from[1])
                                .hypot(to[2] - from[2]);
                            seconds = move_seconds(length, speed);
                        }
                        if !matches!(event, Event::Travel(..)) {
                            max_z = max_z.max(to[2]);
                        }
                        if let Event::Deposit(_, e, _)
                        | Event::Waste(_, e, _)
                        | Event::Arc(_, _, _, e, _) = event
                        {
                            extrusion = e;
                        }
                        position = Some(to);
                    }
                    Event::Extrusion(e, speed) => {
                        seconds = move_seconds((e - extrusion).abs(), speed);
                        extrusion = e;
                    }
                    _ => {}
                }
                if let Some(index) = layer {
                    layer_seconds[index] += seconds;
                }
                Ok(())
            })?;
        check_cancel(cancel)?;
        let total_seconds = layer_seconds.iter().fold(0.0, |a, b| a + b);
        Ok(Self {
            layer_seconds,
            total_seconds,
            filament_mm: last_extrusion,
            filament_diameter_mm: intent.recipe.filament_diameter_mm,
            max_z_mm: max_z,
            layers: plan
                .layers
                .iter()
                .map(|l| (l.z_mm, l.z_mm - l.lower_z_mm))
                .collect(),
        })
    }

    /// The header block. Keys follow the observed Bambu Studio header; the
    /// generator line names this producer.
    pub fn header(&self, out: &mut impl Write) -> std::fmt::Result {
        let area = std::f64::consts::PI * (self.filament_diameter_mm * 0.5).powi(2);
        let time = duration(self.total_seconds);
        writeln!(out, "; HEADER_BLOCK_START")?;
        writeln!(out, "; Spiling emitter {}", crate::EMITTER_VERSION)?;
        writeln!(
            out,
            "; model printing time: {time}; total estimated time: {time}"
        )?;
        writeln!(out, "; total layer number: {}", self.layers.len())?;
        writeln!(
            out,
            "; total filament length [mm] : {:.2}",
            self.filament_mm
        )?;
        writeln!(
            out,
            "; total filament volume [cm^3] : {:.2}",
            self.filament_mm * area / 1000.0
        )?;
        writeln!(out, "; filament_diameter: {:.2}", self.filament_diameter_mm)?;
        writeln!(out, "; max_z_height: {:.2}", self.max_z_mm)?;
        writeln!(out, "; filament: 1")?;
        writeln!(out, "; HEADER_BLOCK_END")
    }

    /// `M73 P<percent> R<minutes remaining>` before `layer` (the plan's
    /// layer count gives the finish).
    pub fn progress(&self, layer: usize, out: &mut impl Write) -> std::fmt::Result {
        if layer >= self.layer_seconds.len() {
            return writeln!(out, "M73 P100 R0");
        }
        let elapsed = self.layer_seconds[..layer].iter().fold(0.0, |a, b| a + b);
        let (percent, remaining) = if self.total_seconds > 0.0 {
            (
                (100.0 * elapsed / self.total_seconds)
                    .floor()
                    .clamp(0.0, 100.0) as u32,
                ((self.total_seconds - elapsed).max(0.0) / 60.0).ceil() as u32,
            )
        } else {
            (0, 0)
        };
        writeln!(out, "M73 P{percent} R{remaining}")
    }

    pub fn layer(&self, index: u32, out: &mut impl Write) -> std::fmt::Result {
        let i = index as usize;
        let (z, height) = self.layers[i];
        writeln!(out, "; CHANGE_LAYER")?;
        writeln!(out, "; Z_HEIGHT: {}", trimmed(z))?;
        writeln!(out, "; LAYER_HEIGHT: {}", trimmed(height))?;
        self.progress(i, out)?;
        writeln!(out, "M73 L{}", index + 1)?;
        writeln!(out, "M991 S0 P{index}")
    }
}

fn duration(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

fn trimmed(value: f64) -> String {
    let text = format!("{value:.6}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}
