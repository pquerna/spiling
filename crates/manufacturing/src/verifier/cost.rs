// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{Command, Decoded, MotionState, failed};
use sha2::{Digest, Sha256};
use spiling_contracts::{
    geometry::SourceHash,
    manufacturing::{program_coordinates, *},
};

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

/// Independent print-time estimate for a complete single-extruder job from
/// the decoded commands: one declared acceleration, Marlin-style junction
/// deviation between consecutive moves, and a bounded lookahead (like the
/// firmware's planner buffer) that assumes a stop after the last buffered
/// move. E-only retraction/recovery is a full stop at its own feed. Heating,
/// homing and opaque blocks take no modelled time. A software estimate, not
/// a measurement.
pub(super) struct PrintTimeEstimate {
    acceleration: f64,
    junction_deviation: f64,
    moves: std::collections::VecDeque<PlannedMove>,
    direction: Option<[f64; 3]>,
    seconds: f64,
}
#[derive(Clone, Copy)]
struct PlannedMove {
    length: f64,
    cruise: f64,
    /// Largest entry speed the junction with the previous move admits.
    max_entry: f64,
    entry: f64,
}
/// Planner buffer: moves held for lookahead before their timing is fixed.
const LOOKAHEAD_MOVES: usize = 64;
impl PrintTimeEstimate {
    pub(super) fn new(model: &PrintTimeModel) -> Result<Self, ManufacturingError> {
        model.validate()?;
        Ok(Self {
            acceleration: model.acceleration_mm_s2,
            junction_deviation: model.junction_deviation_mm,
            moves: std::collections::VecDeque::with_capacity(LOOKAHEAD_MOVES + 1),
            direction: None,
            seconds: 0.0,
        })
    }
    /// Observe one decoded command at the actual state before it executes.
    pub(super) fn observe(
        &mut self,
        decoded: &Decoded,
        state: &MotionState,
    ) -> Result<(), ManufacturingError> {
        let fields = &decoded.fields;
        match decoded.command {
            Command::Travel | Command::Deposit | Command::Arc(_) => {
                let Some(start) = state.position else {
                    return Ok(());
                };
                let end = [
                    fields[0].ok_or_else(|| failed("print time X absent"))?,
                    fields[1].ok_or_else(|| failed("print time Y absent"))?,
                    fields[2].ok_or_else(|| failed("print time Z absent"))?,
                ];
                let speed = fields[4].ok_or_else(|| failed("print time feed absent"))? / 60.0;
                let length = match decoded.command {
                    Command::Arc(clockwise) => {
                        let center = [
                            start[0] + fields[9].ok_or_else(|| failed("arc I absent"))?,
                            start[1] + fields[10].ok_or_else(|| failed("arc J absent"))?,
                        ];
                        program_coordinates::CommandedArc::new(start, end, center, clockwise)
                            .map_err(|e| failed(e.message))?
                            .length_mm
                    }
                    _ => super::distance(start, end),
                };
                self.motion(start, end, length, speed)
            }
            Command::Recovery => {
                let e = fields[3].ok_or_else(|| failed("print time E absent"))?;
                let speed = fields[4].ok_or_else(|| failed("print time feed absent"))? / 60.0;
                self.stop();
                let seconds = (e - state.extrusion).abs() / speed;
                if !seconds.is_finite() {
                    return Err(failed("print time arithmetic is nonfinite"));
                }
                self.seconds += seconds;
                Ok(())
            }
            _ => Ok(()),
        }
    }
    fn motion(
        &mut self,
        start: [f64; 3],
        end: [f64; 3],
        length: f64,
        speed: f64,
    ) -> Result<(), ManufacturingError> {
        if !length.is_finite() || !speed.is_finite() || speed <= 0.0 {
            return Err(failed("print time motion inputs are not finite/admissible"));
        }
        if length <= 0.0 {
            return Ok(());
        }
        let chord = super::distance(start, end);
        let direction = if chord > 0.0 {
            Some([
                (end[0] - start[0]) / chord,
                (end[1] - start[1]) / chord,
                (end[2] - start[2]) / chord,
            ])
        } else {
            None
        };
        let max_entry = match (self.direction, direction, self.moves.back()) {
            (Some(u), Some(v), Some(previous)) => {
                // Marlin's junction deviation: the speed whose centripetal
                // acceleration on a circle of deviation JD is the limit.
                let cos = -(u[0] * v[0] + u[1] * v[1] + u[2] * v[2]);
                let junction = if cos <= -0.999_999 {
                    f64::INFINITY
                } else if cos >= 0.999_999 {
                    0.0
                } else {
                    let half = (0.5 * (1.0 - cos)).sqrt();
                    (self.acceleration * self.junction_deviation * half / (1.0 - half)).sqrt()
                };
                junction.min(previous.cruise).min(speed)
            }
            _ => 0.0,
        };
        self.direction = direction;
        self.moves.push_back(PlannedMove {
            length,
            cruise: speed,
            max_entry,
            entry: max_entry,
        });
        if self.moves.len() > LOOKAHEAD_MOVES {
            self.plan();
            for _ in 0..LOOKAHEAD_MOVES / 2 {
                self.retire();
            }
        }
        Ok(())
    }
    /// Backward then forward pass over the buffer, stopping after its end.
    fn plan(&mut self) {
        let a = self.acceleration;
        let mut exit = 0.0f64;
        for m in self.moves.iter_mut().rev() {
            m.entry = m.max_entry.min((exit * exit + 2.0 * a * m.length).sqrt());
            exit = m.entry;
        }
        let mut previous: Option<PlannedMove> = None;
        for m in self.moves.iter_mut() {
            if let Some(p) = previous {
                m.entry = m.entry.min((p.entry * p.entry + 2.0 * a * p.length).sqrt());
            }
            previous = Some(*m);
        }
    }
    /// Fix the timing of the oldest buffered move.
    fn retire(&mut self) {
        let Some(m) = self.moves.pop_front() else {
            return;
        };
        let exit = self.moves.front().map_or(0.0, |next| next.entry);
        let a = self.acceleration;
        let (v0, v1, vc) = (m.entry, exit.min(m.cruise), m.cruise);
        let up = (vc * vc - v0 * v0).max(0.0) / (2.0 * a);
        let down = (vc * vc - v1 * v1).max(0.0) / (2.0 * a);
        self.seconds += if up + down <= m.length {
            (vc - v0) / a + (vc - v1) / a + (m.length - up - down) / vc
        } else {
            let peak = ((2.0 * a * m.length + v0 * v0 + v1 * v1) * 0.5)
                .sqrt()
                .min(vc);
            (peak - v0).max(0.0) / a + (peak - v1).max(0.0) / a
        };
    }
    /// A full stop: every buffered move decelerates to rest.
    fn stop(&mut self) {
        self.plan();
        while !self.moves.is_empty() {
            self.retire();
        }
        self.direction = None;
    }
    pub(super) fn finish(mut self) -> Result<f64, ManufacturingError> {
        self.stop();
        if !self.seconds.is_finite() || self.seconds < 0.0 {
            return Err(failed("print time estimate is not finite"));
        }
        Ok(self.seconds)
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
    fn lookahead_trapezoids_join_collinear_moves_and_slow_at_corners() {
        let model = PrintTimeModel {
            acceleration_mm_s2: 1000.0,
            junction_deviation_mm: 0.013,
        };
        let run = |points: &[[f64; 3]]| {
            let mut estimate = PrintTimeEstimate::new(&model).unwrap();
            for pair in points.windows(2) {
                estimate
                    .motion(
                        pair[0],
                        pair[1],
                        super::super::distance(pair[0], pair[1]),
                        50.0,
                    )
                    .unwrap();
            }
            estimate.finish().unwrap()
        };
        // One 100 mm move from rest to rest: L/v + v/a.
        let single = run(&[[0.0; 3], [100.0, 0.0, 0.0]]);
        assert!((single - 2.05).abs() < 1e-12, "{single}");
        // Collinear pieces keep their speed through the junction.
        let split = run(&[[0.0; 3], [50.0, 0.0, 0.0], [100.0, 0.0, 0.0]]);
        assert!((split - single).abs() < 1e-9, "{split}");
        // A right-angle corner slows to the junction speed: slower overall.
        let corner = run(&[[0.0; 3], [50.0, 0.0, 0.0], [50.0, 50.0, 0.0]]);
        assert!(corner > single + 0.01, "{corner}");
        // A reversal stops completely: two separate rest-to-rest moves.
        let reversal = run(&[[0.0; 3], [50.0, 0.0, 0.0], [0.0, 0.0, 0.0]]);
        assert!((reversal - 2.0 * (1.0 + 0.05)).abs() < 1e-9, "{reversal}");
    }
    #[test]
    fn declared_acceleration_accounts_for_ramps_and_zero_moves() {
        assert_eq!(rest_to_rest(100.0, 10.0, 2.0).unwrap(), 15.0);
        assert!((rest_to_rest(4.0, 10.0, 2.0).unwrap() - 8.0_f64.sqrt()).abs() < 1e-14);
        assert_eq!(rest_to_rest(0.0, 10.0, 2.0).unwrap(), 0.0);
        assert!(rest_to_rest(1.0, 0.0, 2.0).is_err());
        assert!(rest_to_rest(f64::INFINITY, 10.0, 2.0).is_err());
    }
}
