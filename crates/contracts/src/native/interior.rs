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

#[cfg(test)]
mod tests {
    use super::*;
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
