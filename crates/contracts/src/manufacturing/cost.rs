// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;

/// Declared estimation assumptions, never firmware configuration or calibration.
/// Each movement starts and ends at rest; coordinated XYZ/E obey both limits.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DeclaredJobTimeModel {
    pub id: String,
    pub revision: String,
    pub linear_acceleration_mm_s2: f64,
    pub filament_acceleration_mm_s2: f64,
    pub tool_activation_seconds: [f64; 2],
    pub command_overhead_seconds: f64,
}
impl DeclaredJobTimeModel {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        label(&self.id)?;
        label(&self.revision)?;
        positive(&[
            self.linear_acceleration_mm_s2,
            self.filament_acceleration_mm_s2,
        ])?;
        if self
            .tool_activation_seconds
            .iter()
            .chain(std::iter::once(&self.command_overhead_seconds))
            .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err(invalid(
                "declared job time overhead must be finite and nonnegative",
            ));
        }
        Ok(())
    }
}

/// An estimate of all commanded work, excluding unknown startup homing/heating.
/// Model, auxiliary, travel, feeder and activation terms are disjoint.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct JobTimeLedger {
    pub model: DeclaredJobTimeModel,
    pub model_deposition_seconds: f64,
    pub auxiliary_deposition_seconds: f64,
    pub travel_seconds: f64,
    pub feeder_seconds: f64,
    pub activation_seconds: f64,
    pub command_overhead_seconds: f64,
    pub model_moves: u32,
    pub auxiliary_moves: u32,
    pub travel_moves: u32,
    pub feeder_moves: u32,
    pub selections_per_tool: [u32; 2],
    pub command_count: u32,
    /// Unmodeled startup commands and complete declared startup context.
    pub startup_context_hash: SourceHash,
}
impl JobTimeLedger {
    pub fn modeled_seconds(&self) -> f64 {
        self.model_deposition_seconds
            + self.auxiliary_deposition_seconds
            + self.travel_seconds
            + self.feeder_seconds
            + self.activation_seconds
            + self.command_overhead_seconds
    }
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        self.model.validate()?;
        if [
            self.model_deposition_seconds,
            self.auxiliary_deposition_seconds,
            self.travel_seconds,
            self.feeder_seconds,
            self.activation_seconds,
            self.command_overhead_seconds,
            self.modeled_seconds(),
        ]
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0)
            || self.command_count == 0
        {
            return Err(invalid(
                "job time ledger exceeds finite command/time bounds",
            ));
        }
        let accounted = [
            self.model_moves,
            self.auxiliary_moves,
            self.travel_moves,
            self.feeder_moves,
            self.selections_per_tool[0],
            self.selections_per_tool[1],
        ]
        .into_iter()
        .map(u64::from)
        .sum::<u64>();
        if u64::from(self.command_count) < accounted {
            return Err(invalid(
                "job time command count omits accounted movement/selection work",
            ));
        }
        let activation = self
            .selections_per_tool
            .iter()
            .zip(self.model.tool_activation_seconds)
            .map(|(count, seconds)| f64::from(*count) * seconds)
            .sum::<f64>();
        if self.activation_seconds != activation
            || self.command_overhead_seconds
                != f64::from(self.command_count) * self.model.command_overhead_seconds
            || [
                (self.model_moves, self.model_deposition_seconds),
                (self.auxiliary_moves, self.auxiliary_deposition_seconds),
                (self.feeder_moves, self.feeder_seconds),
            ]
            .iter()
            .any(|(count, seconds)| (*count == 0) != (*seconds == 0.0))
        {
            return Err(invalid(
                "job time overhead/deposition does not bind its actual command counts",
            ));
        }
        Ok(())
    }
}

impl VerificationReport {
    pub fn validate_time_model(&self, workflow: &JobWorkflow) -> Result<(), ManufacturingError> {
        let expected = match workflow {
            JobWorkflow::FixedDualMarlin { job } => job.timing_model.as_ref(),
            _ => None,
        };
        if self.job_time.as_ref().map(|t| &t.model) != expected {
            return Err(invalid(
                "job time report does not bind its captured declared model",
            ));
        }
        Ok(())
    }
}
