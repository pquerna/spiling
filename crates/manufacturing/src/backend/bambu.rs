// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Bambu Lab program conventions around the declared lifecycle: the header
//! block the printer and Bambu Studio read, the executable-block markers, and
//! per-layer markers (`; CHANGE_LAYER`, `M73 P/R` progress, `M73 L` layer
//! number, `M991 S0 P` layer-change notification). The marker forms were
//! observed in Bambu Studio 02.09.01.70 A1/A1 mini exports; the values here
//! are this producer's own. Progress and remaining time read the routing
//! clock; they are display data, separate from the verifier's estimate.

use crate::realization::BambuEstimate;
use spiling_contracts::manufacturing::*;
use std::fmt::Write;

pub(super) struct Survey<'a> {
    estimate: &'a BambuEstimate,
    filament_diameter_mm: f64,
    ams: Option<&'a BambuAms>,
    layers: &'a [PrintLayer],
}

impl<'a> Survey<'a> {
    pub fn new(
        plan: &'a NormalizedPrintPlan,
        intent: &'a ManufacturingIntent,
        estimate: &'a BambuEstimate,
    ) -> Self {
        let ams = match &intent.printer.workflow {
            JobWorkflow::CompleteMarlin { job } => job.bambu_ams(),
            _ => None,
        };
        Self {
            estimate,
            filament_diameter_mm: intent.recipe.filament_diameter_mm,
            ams,
            layers: &plan.layers,
        }
    }

    /// The header block. Keys follow the observed Bambu Studio header; the
    /// generator line names this producer.
    pub fn header(&self, out: &mut impl Write) -> std::fmt::Result {
        let time = duration(self.estimate.total_s);
        writeln!(out, "; HEADER_BLOCK_START")?;
        writeln!(out, "; Spiling emitter {}", crate::EMITTER_VERSION)?;
        writeln!(
            out,
            "; model printing time: {time}; total estimated time: {time}"
        )?;
        writeln!(out, "; total layer number: {}", self.layers.len())?;
        if let Some(ams) = self.ams {
            let active: Vec<usize> = (0..ams.slots.len())
                .filter(|&i| {
                    i == 0
                        || self
                            .estimate
                            .slot_filament_mm
                            .get(i)
                            .copied()
                            .unwrap_or(0.0)
                            > 0.0
                })
                .collect();
            let lengths: Vec<String> = active
                .iter()
                .map(|&i| {
                    format!(
                        "{:.2}",
                        self.estimate
                            .slot_filament_mm
                            .get(i)
                            .copied()
                            .unwrap_or(0.0)
                    )
                })
                .collect();
            let volumes: Vec<String> = active
                .iter()
                .map(|&i| {
                    let mm = self
                        .estimate
                        .slot_filament_mm
                        .get(i)
                        .copied()
                        .unwrap_or(0.0);
                    let area = ams.slots[i].filament_area_mm2();
                    format!("{:.2}", mm * area / 1000.0)
                })
                .collect();
            let diameters: Vec<String> = active
                .iter()
                .map(|&i| format!("{:.2}", ams.slots[i].filament_diameter_mm))
                .collect();
            let ids: Vec<String> = active.iter().map(|&i| (i + 1).to_string()).collect();
            writeln!(out, "; total filament length [mm] : {}", lengths.join(","))?;
            writeln!(
                out,
                "; total filament volume [cm^3] : {}",
                volumes.join(",")
            )?;
            writeln!(out, "; filament_diameter: {}", diameters.join(","))?;
            writeln!(out, "; max_z_height: {:.2}", self.estimate.max_z_mm)?;
            writeln!(out, "; filament: {}", ids.join(","))?;
        } else {
            let area = std::f64::consts::PI * (self.filament_diameter_mm * 0.5).powi(2);
            writeln!(
                out,
                "; total filament length [mm] : {:.2}",
                self.estimate.filament_mm
            )?;
            writeln!(
                out,
                "; total filament volume [cm^3] : {:.2}",
                self.estimate.filament_mm * area / 1000.0
            )?;
            writeln!(out, "; filament_diameter: {:.2}", self.filament_diameter_mm)?;
            writeln!(out, "; max_z_height: {:.2}", self.estimate.max_z_mm)?;
            writeln!(out, "; filament: 1")?;
        }
        writeln!(out, "; HEADER_BLOCK_END")
    }

    /// `M73 P<percent> R<minutes remaining>` before `layer` (the plan's
    /// layer count gives the finish).
    pub fn progress(&self, layer: usize, out: &mut impl Write) -> std::fmt::Result {
        if layer >= self.estimate.starts_s.len() {
            return writeln!(out, "M73 P100 R0");
        }
        let elapsed = self.estimate.starts_s[layer];
        let (percent, remaining) = if self.estimate.total_s > 0.0 {
            (
                (100.0 * elapsed / self.estimate.total_s)
                    .floor()
                    .clamp(0.0, 100.0) as u32,
                ((self.estimate.total_s - elapsed).max(0.0) / 60.0).ceil() as u32,
            )
        } else {
            (0, 0)
        };
        writeln!(out, "M73 P{percent} R{remaining}")
    }

    pub fn layer(&self, index: u32, out: &mut impl Write) -> std::fmt::Result {
        let i = index as usize;
        let layer = &self.layers[i];
        let (z, height) = (layer.z_mm, layer.z_mm - layer.lower_z_mm);
        writeln!(out, "; CHANGE_LAYER")?;
        writeln!(out, "; Z_HEIGHT: {}", trimmed(z))?;
        writeln!(out, "; LAYER_HEIGHT: {}", trimmed(height))?;
        self.progress(i, out)?;
        writeln!(out, "M73 L{}", index + 1)?;
        writeln!(out, "M991 S0 P{index}")
    }
}

/// Metadata for emitter/verifier fixtures that begin with a hand-authored
/// normalized plan, outside the production scheduling boundary.
#[cfg(test)]
pub(super) fn fixture_estimate(
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    job: &SingleExtruderJob,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<BambuEstimate, ManufacturingError> {
    use crate::realization::{Event, stream};
    let ams = job.bambu_ams();
    let slot_count = ams.map_or(1, |a| a.slots.len());
    let mut slot_filament_mm = vec![0.0; slot_count];
    slot_filament_mm[0] = job.prime_filament_length_mm;
    let mut estimate = BambuEstimate {
        starts_s: Vec::with_capacity(plan.layers.len()),
        total_s: 0.0,
        filament_mm: job.prime_filament_length_mm,
        max_z_mm: job.prime_start_mm[2].max(job.prime_end_mm[2]),
        slot_filament_mm,
    };
    let mut position = job.prime_end_mm;
    let mut active_slot = 0usize;
    let mut last_e = job.prime_filament_length_mm;
    let (_, extrusion) = stream(
        plan,
        intent,
        cancel,
        Some(position),
        estimate.filament_mm,
        |event| {
            match event {
                Event::Layer(_) => estimate.starts_s.push(estimate.total_s),
                Event::AmsSwitch { to, .. } => {
                    active_slot = usize::from(to);
                    last_e = 0.0;
                }
                Event::AmsTowerFrame { .. } => {
                    last_e = 0.0;
                }
                Event::Travel(to, speed) => {
                    estimate.total_s += (to[0] - position[0])
                        .hypot(to[1] - position[1])
                        .hypot(to[2] - position[2])
                        / speed;
                    position = to;
                }
                Event::Deposit(to, e, speed) | Event::Arc(to, _, _, e, speed) => {
                    estimate.total_s += (to[0] - position[0])
                        .hypot(to[1] - position[1])
                        .hypot(to[2] - position[2])
                        / speed;
                    let delta = (e - last_e).max(0.0);
                    estimate.slot_filament_mm[active_slot] += delta;
                    last_e = e;
                    position = to;
                    estimate.max_z_mm = estimate.max_z_mm.max(to[2]);
                }
                _ => {}
            }
            Ok(())
        },
    )?;
    if let Some(ams) = ams {
        let extra = super::ams::estimate_ams_additions(plan, ams)?;
        for (slot, add) in estimate.slot_filament_mm.iter_mut().zip(extra) {
            *slot += add;
        }
        estimate.filament_mm = estimate.slot_filament_mm.iter().sum();
    } else {
        estimate.filament_mm = extrusion;
        estimate.slot_filament_mm[0] = extrusion;
    }
    Ok(estimate)
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
