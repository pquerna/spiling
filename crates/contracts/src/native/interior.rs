// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use crate::{manufacturing::*, rpc};

impl TryFrom<rpc::InteriorToolOverride> for InteriorToolOverride {
    type Error = String;
    fn try_from(v: rpc::InteriorToolOverride) -> Result<Self, String> {
        let value = match rpc::InteriorToolPolicy::try_from(v.value).map_err(|e| e.to_string())? {
            rpc::InteriorToolPolicy::Automatic => InteriorToolPolicy::Automatic,
            rpc::InteriorToolPolicy::PrimaryOnly => InteriorToolPolicy::PrimaryOnly,
            rpc::InteriorToolPolicy::Unspecified => {
                return Err("interior tool policy is unspecified".into());
            }
        };
        Ok(Self {
            enabled: v.enabled,
            value,
        })
    }
}
impl TryFrom<InteriorToolOverride> for rpc::InteriorToolOverride {
    type Error = String;
    fn try_from(v: InteriorToolOverride) -> Result<Self, String> {
        Ok(Self {
            enabled: v.enabled,
            value: match v.value {
                InteriorToolPolicy::Automatic => rpc::InteriorToolPolicy::Automatic as i32,
                InteriorToolPolicy::PrimaryOnly => rpc::InteriorToolPolicy::PrimaryOnly as i32,
            },
        })
    }
}
impl TryFrom<rpc::InteriorSelectionGoal> for InteriorSelectionGoal {
    type Error = String;
    fn try_from(v: rpc::InteriorSelectionGoal) -> Result<Self, String> {
        let result = Self {
            minimum_modeled_saving_seconds: v.minimum_modeled_saving_seconds,
            max_extra_model_fraction: v.max_extra_model_fraction,
            max_extra_waste_mm3: v.max_extra_waste_mm3,
        };
        result.validate().map_err(|e| e.to_string())?;
        Ok(result)
    }
}
impl TryFrom<InteriorSelectionGoal> for rpc::InteriorSelectionGoal {
    type Error = String;
    fn try_from(v: InteriorSelectionGoal) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            minimum_modeled_saving_seconds: v.minimum_modeled_saving_seconds,
            max_extra_model_fraction: v.max_extra_model_fraction,
            max_extra_waste_mm3: v.max_extra_waste_mm3,
        })
    }
}

impl TryFrom<rpc::InteriorToolPolicyEdit> for InteriorToolPolicyEdit {
    type Error = String;
    fn try_from(v: rpc::InteriorToolPolicyEdit) -> Result<Self, String> {
        use rpc::interior_tool_policy_edit::{Action, ExpectedState};
        Ok(Self {
            occurrence_id: crate::geometry::OccurrenceId::new(v.occurrence_id)
                .map_err(|e| e.to_string())?,
            expected_definition_id: crate::geometry::DefinitionId::parse(v.expected_definition_id)
                .map_err(|e| e.to_string())?,
            expected: match v.expected_state {
                Some(ExpectedState::ExpectedDefault(true)) => None,
                Some(ExpectedState::ExpectedOverride(value)) => Some(value.try_into()?),
                _ => return Err("missing or false interior policy expected state".into()),
            },
            value: match v.action {
                Some(Action::UseDefaults(true)) => None,
                Some(Action::SetOverride(value)) => Some(value.try_into()?),
                _ => return Err("missing or false interior policy action".into()),
            },
        })
    }
}
impl TryFrom<InteriorToolPolicyEdit> for rpc::InteriorToolPolicyEdit {
    type Error = String;
    fn try_from(v: InteriorToolPolicyEdit) -> Result<Self, String> {
        use rpc::interior_tool_policy_edit::{Action, ExpectedState};
        Ok(Self {
            occurrence_id: v.occurrence_id.get(),
            expected_definition_id: v.expected_definition_id.as_str().into(),
            expected_state: Some(match v.expected {
                None => ExpectedState::ExpectedDefault(true),
                Some(value) => ExpectedState::ExpectedOverride(value.try_into()?),
            }),
            action: Some(match v.value {
                None => Action::UseDefaults(true),
                Some(value) => Action::SetOverride(value.try_into()?),
            }),
        })
    }
}

impl TryFrom<rpc::InteriorPolicyOpportunity> for InteriorPolicyOpportunity {
    type Error = String;
    fn try_from(v: rpc::InteriorPolicyOpportunity) -> Result<Self, String> {
        Ok(Self {
            edit: v
                .edit
                .ok_or("missing interior policy opportunity edit")?
                .try_into()?,
            context_hash: crate::geometry::SourceHash::parse(v.context_hash)
                .map_err(|e| e.to_string())?,
            input_hash: crate::geometry::SourceHash::parse(v.input_hash)
                .map_err(|e| e.to_string())?,
        })
    }
}
impl TryFrom<InteriorPolicyOpportunity> for rpc::InteriorPolicyOpportunity {
    type Error = String;
    fn try_from(v: InteriorPolicyOpportunity) -> Result<Self, String> {
        Ok(Self {
            edit: Some(v.edit.try_into()?),
            context_hash: v.context_hash.as_str().into(),
            input_hash: v.input_hash.as_str().into(),
        })
    }
}
impl TryFrom<rpc::InteriorPolicyAssessment> for InteriorPolicyAssessment {
    type Error = String;
    fn try_from(v: rpc::InteriorPolicyAssessment) -> Result<Self, String> {
        Ok(Self {
            opportunity: v
                .opportunity
                .ok_or("missing policy assessment opportunity")?
                .try_into()?,
            primary: v
                .primary
                .ok_or("missing policy primary reference")?
                .try_into()?,
            automatic: v
                .automatic
                .ok_or("missing policy automatic result")?
                .try_into()?,
            strategy: v.strategy.try_into()?,
            reason: v.reason.try_into()?,
        })
    }
}
impl TryFrom<InteriorPolicyAssessment> for rpc::InteriorPolicyAssessment {
    type Error = String;
    fn try_from(v: InteriorPolicyAssessment) -> Result<Self, String> {
        Ok(Self {
            opportunity: Some(v.opportunity.try_into()?),
            primary: Some(v.primary.try_into()?),
            automatic: Some(v.automatic.try_into()?),
            strategy: v.strategy.into(),
            reason: v.reason.into(),
        })
    }
}
impl TryFrom<rpc::QualifiedInteriorWork> for QualifiedInteriorWork {
    type Error = String;
    fn try_from(v: rpc::QualifiedInteriorWork) -> Result<Self, String> {
        Ok(Self {
            interior_segments_per_tool: v
                .interior_segments_per_tool
                .try_into()
                .map_err(|_| "policy work needs two tool counts")?,
            program_hash: crate::geometry::SourceHash::parse(v.program_hash)
                .map_err(|e| e.to_string())?,
            model_volume_mm3: v.model_volume_mm3,
            waste_volume_mm3: v.waste_volume_mm3,
            waste_rounding_mm3: v.waste_rounding_mm3,
            time: v.time.map(TryInto::try_into).transpose()?,
        })
    }
}
impl TryFrom<QualifiedInteriorWork> for rpc::QualifiedInteriorWork {
    type Error = String;
    fn try_from(v: QualifiedInteriorWork) -> Result<Self, String> {
        Ok(Self {
            interior_segments_per_tool: v.interior_segments_per_tool.to_vec(),
            program_hash: v.program_hash.as_str().into(),
            model_volume_mm3: v.model_volume_mm3,
            waste_volume_mm3: v.waste_volume_mm3,
            waste_rounding_mm3: v.waste_rounding_mm3,
            time: v.time.map(TryInto::try_into).transpose()?,
        })
    }
}
impl TryFrom<i32> for InteriorStrategy {
    type Error = String;
    fn try_from(value: i32) -> Result<Self, String> {
        match value {
            1 => Ok(Self::Primary),
            2 => Ok(Self::Synchronized),
            3 => Ok(Self::Grouped),
            _ => Err("unknown InteriorStrategy".into()),
        }
    }
}
impl From<InteriorStrategy> for i32 {
    fn from(value: InteriorStrategy) -> Self {
        match value {
            InteriorStrategy::Primary => 1,
            InteriorStrategy::Synchronized => 2,
            InteriorStrategy::Grouped => 3,
        }
    }
}
impl TryFrom<i32> for InteriorSelectionReason {
    type Error = String;
    fn try_from(value: i32) -> Result<Self, String> {
        match value {
            1 => Ok(Self::ExplicitPrimary),
            2 => Ok(Self::NoTimingModel),
            3 => Ok(Self::NoObjective),
            4 => Ok(Self::GroupingDisabled),
            5 => Ok(Self::NoQualifiedAlternative),
            6 => Ok(Self::IncomparableTiming),
            7 => Ok(Self::ObjectiveNotMet),
            8 => Ok(Self::ImprovedModeledWork),
            _ => Err("unknown InteriorSelectionReason".into()),
        }
    }
}
impl From<InteriorSelectionReason> for i32 {
    fn from(value: InteriorSelectionReason) -> Self {
        match value {
            InteriorSelectionReason::ExplicitPrimary => 1,
            InteriorSelectionReason::NoTimingModel => 2,
            InteriorSelectionReason::NoObjective => 3,
            InteriorSelectionReason::GroupingDisabled => 4,
            InteriorSelectionReason::NoQualifiedAlternative => 5,
            InteriorSelectionReason::IncomparableTiming => 6,
            InteriorSelectionReason::ObjectiveNotMet => 7,
            InteriorSelectionReason::ImprovedModeledWork => 8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detached_policy_request_is_typed_and_rejects_competing_context() {
        use crate::{
            geometry::{DefinitionId, OccurrenceId, SessionId, SourceHash},
            project::{PrintLayoutId, ProjectRevision},
        };
        let command = ManufacturingCommand::EvaluateInteriorPolicy {
            layout_id: PrintLayoutId::parse("1ccf1bc1-590b-4fe2-88b5-7e8de3da1fcb").unwrap(),
            session_id: SessionId::new(),
            base_revision: ProjectRevision(2),
            opportunity: InteriorPolicyOpportunity {
                edit: InteriorToolPolicyEdit {
                    occurrence_id: OccurrenceId::new(1).unwrap(),
                    expected_definition_id: DefinitionId::from_source_sha256(&[1; 32]),
                    expected: Some(InteriorToolOverride {
                        enabled: true,
                        value: InteriorToolPolicy::PrimaryOnly,
                    }),
                    value: None,
                },
                context_hash: SourceHash::from_bytes(b"context"),
                input_hash: SourceHash::from_bytes(b"input"),
            },
        };
        let wire: rpc::EvaluateManufacturingAlternativeRequest =
            command.clone().try_into().unwrap();
        assert_eq!(
            ManufacturingCommand::try_from(wire.clone()).unwrap(),
            command
        );
        let mut wrong = wire.clone();
        wrong.expected_context_hash = SourceHash::from_bytes(b"competing").as_str().into();
        assert!(ManufacturingCommand::try_from(wrong).is_err());
        let mut missing = wire;
        missing.construction = None;
        assert!(ManufacturingCommand::try_from(missing).is_err());
        assert!(InteriorStrategy::try_from(0).is_err());
        assert!(InteriorSelectionReason::try_from(999).is_err());
    }
    #[test]
    fn policy_edit_requires_explicit_expected_state_and_action() {
        use rpc::interior_tool_policy_edit::{Action, ExpectedState};
        let edit = InteriorToolPolicyEdit {
            occurrence_id: crate::geometry::OccurrenceId::new(1).unwrap(),
            expected_definition_id: crate::geometry::DefinitionId::from_source_sha256(&[1; 32]),
            expected: None,
            value: Some(InteriorToolOverride {
                enabled: true,
                value: InteriorToolPolicy::PrimaryOnly,
            }),
        };
        let wire: rpc::InteriorToolPolicyEdit = edit.clone().try_into().unwrap();
        assert_eq!(
            InteriorToolPolicyEdit::try_from(wire.clone()).unwrap(),
            edit
        );
        for expected in [None, Some(ExpectedState::ExpectedDefault(false))] {
            assert!(
                InteriorToolPolicyEdit::try_from(rpc::InteriorToolPolicyEdit {
                    expected_state: expected,
                    ..wire.clone()
                })
                .is_err()
            );
        }
        for action in [None, Some(Action::UseDefaults(false))] {
            assert!(
                InteriorToolPolicyEdit::try_from(rpc::InteriorToolPolicyEdit {
                    action,
                    ..wire.clone()
                })
                .is_err()
            );
        }
        let mut json = serde_json::to_value(&edit).unwrap();
        json.as_object_mut().unwrap().remove("expected");
        assert!(serde_json::from_value::<InteriorToolPolicyEdit>(json).is_err());
    }
}
