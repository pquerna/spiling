// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::required;
use crate::{geometry::SourceHash, manufacturing::*, rpc};

impl TryFrom<rpc::DeclaredJobTimeModel> for DeclaredJobTimeModel {
    type Error = String;
    fn try_from(v: rpc::DeclaredJobTimeModel) -> Result<Self, String> {
        let result = Self {
            id: v.id,
            revision: v.revision,
            linear_acceleration_mm_s2: v.linear_acceleration_mm_s2,
            filament_acceleration_mm_s2: v.filament_acceleration_mm_s2,
            tool_activation_seconds: v
                .tool_activation_seconds
                .try_into()
                .map_err(|_| "time model requires exactly two activation estimates")?,
            command_overhead_seconds: v.command_overhead_seconds,
        };
        result.validate().map_err(|e| e.to_string())?;
        Ok(result)
    }
}
impl TryFrom<DeclaredJobTimeModel> for rpc::DeclaredJobTimeModel {
    type Error = String;
    fn try_from(v: DeclaredJobTimeModel) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            id: v.id,
            revision: v.revision,
            linear_acceleration_mm_s2: v.linear_acceleration_mm_s2,
            filament_acceleration_mm_s2: v.filament_acceleration_mm_s2,
            tool_activation_seconds: v.tool_activation_seconds.to_vec(),
            command_overhead_seconds: v.command_overhead_seconds,
        })
    }
}
impl TryFrom<rpc::JobTimeLedger> for JobTimeLedger {
    type Error = String;
    fn try_from(v: rpc::JobTimeLedger) -> Result<Self, String> {
        let result = Self {
            model: required(v.model, "job time model")?.try_into()?,
            model_deposition_seconds: v.model_deposition_seconds,
            auxiliary_deposition_seconds: v.auxiliary_deposition_seconds,
            travel_seconds: v.travel_seconds,
            feeder_seconds: v.feeder_seconds,
            activation_seconds: v.activation_seconds,
            command_overhead_seconds: v.command_overhead_seconds,
            model_moves: v.model_moves,
            auxiliary_moves: v.auxiliary_moves,
            travel_moves: v.travel_moves,
            feeder_moves: v.feeder_moves,
            selections_per_tool: v
                .selections_per_tool
                .try_into()
                .map_err(|_| "job time requires exactly two selection counts")?,
            command_count: v.command_count,
            startup_context_hash: SourceHash::parse(v.startup_context_hash)
                .map_err(|e| e.to_string())?,
        };
        result.validate().map_err(|e| e.to_string())?;
        Ok(result)
    }
}
impl TryFrom<JobTimeLedger> for rpc::JobTimeLedger {
    type Error = String;
    fn try_from(v: JobTimeLedger) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            model: Some(v.model.try_into()?),
            model_deposition_seconds: v.model_deposition_seconds,
            auxiliary_deposition_seconds: v.auxiliary_deposition_seconds,
            travel_seconds: v.travel_seconds,
            feeder_seconds: v.feeder_seconds,
            activation_seconds: v.activation_seconds,
            command_overhead_seconds: v.command_overhead_seconds,
            model_moves: v.model_moves,
            auxiliary_moves: v.auxiliary_moves,
            travel_moves: v.travel_moves,
            feeder_moves: v.feeder_moves,
            selections_per_tool: v.selections_per_tool.to_vec(),
            command_count: v.command_count,
            startup_context_hash: v.startup_context_hash.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_capture_roundtrips_and_refuses_unknown_cardinality_and_process_limits() {
        let model = DeclaredJobTimeModel {
            id: "declared-test".into(),
            revision: "1".into(),
            linear_acceleration_mm_s2: 1000.0,
            filament_acceleration_mm_s2: 500.0,
            tool_activation_seconds: [0.5, 0.75],
            command_overhead_seconds: 0.001,
        };
        let wire = rpc::DeclaredJobTimeModel::try_from(model.clone()).unwrap();
        assert_eq!(DeclaredJobTimeModel::try_from(wire.clone()).unwrap(), model);
        for count in [0, 1, 3] {
            let mut wrong = wire.clone();
            wrong.tool_activation_seconds = vec![0.5; count];
            assert!(DeclaredJobTimeModel::try_from(wrong).is_err());
        }
        for acceleration in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut wrong = wire.clone();
            wrong.linear_acceleration_mm_s2 = acceleration;
            assert!(DeclaredJobTimeModel::try_from(wrong).is_err());
        }
    }
}
