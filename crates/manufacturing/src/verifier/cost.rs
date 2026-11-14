// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{Command, Decoded, MotionState, failed};
use sha2::{Digest, Sha256};
use spiling_contracts::{geometry::SourceHash, manufacturing::*};

/// Consumes the verifier's decoded commands and current actual state. No second
/// parser, expected-path timing stream, or retained per-move allocation.
pub(super) struct JobTimeBuilder {
    ledger: JobTimeLedger,
    context: Sha256,
    in_work: bool,
}
impl JobTimeBuilder {
    pub(super) fn new(
        job: &FixedDualJob,
        recipe: &PlanarPrintRecipe,
    ) -> Result<Option<Self>, ManufacturingError> {
        let Some(model) = &job.timing_model else {
            return Ok(None);
        };
        model.validate()?;
        let bytes = serde_json::to_vec(&(
            &job.machine_id,
            job.configuration,
            &job.firmware_revision,
            &job.configuration_hash,
            &job.calibration_record_hash,
            job.homing_contract,
            job.home_position_mm,
            job.bed_temperature_c,
            &job.tools,
            job.transition_lift_mm,
            job.park_position_mm,
            recipe.travel_speed_mm_s,
            &recipe.process,
            model,
        ))
        .map_err(|e| failed(format!("job time context: {e}")))?;
        let mut context = Sha256::new();
        context.update(b"spiling-job-time-context-1\0");
        context.update(bytes);
        Ok(Some(Self {
            ledger: JobTimeLedger {
                model: model.clone(),
                model_deposition_seconds: 0.0,
                auxiliary_deposition_seconds: 0.0,
                travel_seconds: 0.0,
                feeder_seconds: 0.0,
                activation_seconds: 0.0,
                command_overhead_seconds: 0.0,
                model_moves: 0,
                auxiliary_moves: 0,
                travel_moves: 0,
                feeder_moves: 0,
                selections_per_tool: [0; 2],
                command_count: 0,
                startup_context_hash: SourceHash::from_bytes(&[]),
            },
            context,
            in_work: false,
        }))
    }
    pub(super) fn observe(
        &mut self,
        decoded: &Decoded,
        line: &str,
        state: &MotionState,
        auxiliary: bool,
    ) -> Result<(), ManufacturingError> {
        // Every other category is a disjoint subset of the command count, so
        // its u32 width is the only count limit.
        self.ledger.command_count = self
            .ledger
            .command_count
            .checked_add(1)
            .ok_or_else(|| failed("job time command count exceeds its u32 representation"))?;
        let model = &self.ledger.model;
        match decoded.command {
            Command::Travel | Command::Deposit => {
                let start = state
                    .position
                    .ok_or_else(|| failed("job time requires an actual known movement start"))?;
                let fields = &decoded.fields;
                let end = [
                    fields[0].ok_or_else(|| failed("job time X absent"))?,
                    fields[1].ok_or_else(|| failed("job time Y absent"))?,
                    fields[2].ok_or_else(|| failed("job time Z absent"))?,
                ];
                let distance = super::distance(start, end);
                let speed = fields[4].ok_or_else(|| failed("job time feed absent"))? / 60.0;
                let mut seconds = rest_to_rest(distance, speed, model.linear_acceleration_mm_s2)?;
                if let Some(e) = fields[3] {
                    let filament = (e - state.extrusion).abs();
                    if distance > 0.0 && filament > 0.0 {
                        seconds = seconds.max(rest_to_rest(
                            filament,
                            speed * filament / distance,
                            model.filament_acceleration_mm_s2,
                        )?);
                    }
                }
                if decoded.command == Command::Travel {
                    self.ledger.travel_seconds += seconds;
                    self.ledger.travel_moves += 1;
                } else if auxiliary {
                    self.ledger.auxiliary_deposition_seconds += seconds;
                    self.ledger.auxiliary_moves += 1;
                } else {
                    self.ledger.model_deposition_seconds += seconds;
                    self.ledger.model_moves += 1;
                }
            }
            Command::Recovery => {
                let filament = (decoded.fields[3]
                    .ok_or_else(|| failed("job time recovery E absent"))?
                    - state.extrusion)
                    .abs();
                self.ledger.feeder_seconds += rest_to_rest(
                    filament,
                    decoded.fields[4].ok_or_else(|| failed("job time recovery feed absent"))?
                        / 60.0,
                    model.filament_acceleration_mm_s2,
                )?;
                self.ledger.feeder_moves += 1;
            }
            command => {
                if let Command::SelectTool(tool) = command {
                    self.in_work = true;
                    self.ledger.selections_per_tool[usize::from(tool)] += 1;
                }
                if !self.in_work {
                    let code = line.split_once(';').map_or(line, |(code, _)| code).trim();
                    self.context.update((code.len() as u64).to_le_bytes());
                    self.context.update(code.as_bytes());
                }
            }
        }
        Ok(())
    }
    pub(super) fn finish(mut self) -> Result<JobTimeLedger, ManufacturingError> {
        self.ledger.activation_seconds = self
            .ledger
            .selections_per_tool
            .iter()
            .zip(self.ledger.model.tool_activation_seconds)
            .map(|(count, seconds)| f64::from(*count) * seconds)
            .sum();
        self.ledger.command_overhead_seconds =
            f64::from(self.ledger.command_count) * self.ledger.model.command_overhead_seconds;
        self.ledger.startup_context_hash = SourceHash::from_bytes(&self.context.finalize());
        self.ledger.validate()?;
        Ok(self.ledger)
    }
}

fn rest_to_rest(distance: f64, speed: f64, acceleration: f64) -> Result<f64, ManufacturingError> {
    if !distance.is_finite()
        || distance < 0.0
        || !speed.is_finite()
        || speed <= 0.0
        || !acceleration.is_finite()
        || acceleration <= 0.0
    {
        return Err(failed("job time motion inputs are not finite/admissible"));
    }
    let ramp_distance = (speed / acceleration) * speed;
    let seconds = if distance <= ramp_distance {
        2.0 * (distance / acceleration).sqrt()
    } else {
        distance / speed + speed / acceleration
    };
    if !seconds.is_finite() {
        return Err(failed("job time arithmetic is nonfinite"));
    }
    Ok(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declared_acceleration_accounts_for_ramps_and_zero_moves() {
        assert_eq!(rest_to_rest(100.0, 10.0, 2.0).unwrap(), 15.0);
        assert!((rest_to_rest(4.0, 10.0, 2.0).unwrap() - 8.0_f64.sqrt()).abs() < 1e-14);
        assert_eq!(rest_to_rest(0.0, 10.0, 2.0).unwrap(), 0.0);
        assert!(rest_to_rest(1.0, 0.0, 2.0).is_err());
        assert!(rest_to_rest(f64::INFINITY, 10.0, 2.0).is_err());
    }
}
