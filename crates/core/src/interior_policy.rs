// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! One scoped authoring edit and its detached reassessment. No planner logic.
use crate::{Snapshot, context_hash, invalid, manufacturing_options, validate_pins};
use spiling_contracts::{geometry::SourceHash, manufacturing::*, project::ProjectError};
use std::sync::Arc;

fn hashes(snapshot: &Snapshot) -> Result<(SourceHash, SourceHash), ProjectError> {
    let mut intent = snapshot
        .manufacturing_intent
        .as_deref()
        .cloned()
        .ok_or_else(|| invalid("interior policy requires captured tool bindings"))?;
    let source = context_hash(snapshot)?;
    let input = serde_json::to_vec(&("spiling.interior-policy-input.v1", &source, &intent))
        .map_err(|_| invalid("policy input cannot be encoded"))?;
    if let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow {
        for assignment in &mut job.assignments {
            if let Some(binding) = &mut assignment.interior {
                binding.policy_override = None;
                binding.policy_context = None;
            }
        }
    }
    let context = serde_json::to_vec(&("spiling.interior-policy-context.v1", source, &intent))
        .map_err(|_| invalid("policy context cannot be encoded"))?;
    Ok((
        SourceHash::from_bytes(&context),
        SourceHash::from_bytes(&input),
    ))
}

pub(crate) fn apply(
    snapshot: &mut Snapshot,
    edit: InteriorToolPolicyEdit,
) -> Result<(), ProjectError> {
    let occurrence = snapshot
        .occurrences
        .get(&edit.occurrence_id)
        .ok_or_else(|| invalid("interior policy part is not on the addressed plate"))?;
    if occurrence.definition_id != edit.expected_definition_id {
        return Err(invalid("interior policy source identity changed"));
    }
    let context = hashes(snapshot)?.0;
    let mut intent = snapshot
        .manufacturing_intent
        .as_deref()
        .cloned()
        .ok_or_else(|| invalid("interior policy requires captured tool bindings"))?;
    let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
        return Err(invalid(
            "interior policy requires the captured dual-tool route",
        ));
    };
    let binding = job
        .assignments
        .iter_mut()
        .find(|a| a.occurrence_id == edit.occurrence_id)
        .and_then(|a| a.interior.as_mut())
        .ok_or_else(|| invalid("part has no captured interior tool binding"))?;
    if binding.policy_override != edit.expected {
        return Err(invalid(
            "interior policy changed since this edit was captured",
        ));
    }
    binding.policy_context = match edit.value {
        None => None,
        Some(value) if value.enabled => Some(context),
        Some(_) => binding.policy_context.clone(),
    };
    binding.policy_override = edit.value;
    manufacturing_options::set_intent(snapshot, Arc::new(intent))
}

/// Metadata eligibility only. A changed context is never evidence of benefit.
pub fn interior_policy_opportunity(
    snapshot: &Snapshot,
) -> Result<Option<InteriorPolicyOpportunity>, ProjectError> {
    let Some(intent) = snapshot.manufacturing_intent.as_deref() else {
        return Ok(None);
    };
    let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow else {
        return Ok(None);
    };
    if !job.has_bound_interior()
        || job.interior_goal.is_none()
        || job.timing_model.is_none()
        || job
            .assignments
            .iter()
            .any(|a| a.active_interior().is_some())
    {
        return Ok(None);
    }
    let candidates = job.assignments.iter().filter(|a| {
        a.interior
            .as_ref()
            .is_some_and(|b| b.primary_only() && b.policy_context.is_some())
    });
    if candidates.clone().next().is_none() {
        return Ok(None);
    }
    let (context_hash, input_hash) = hashes(snapshot)?;
    for assignment in candidates {
        let binding = assignment.interior.as_ref().expect("filtered binding");
        if binding.policy_context.as_ref() == Some(&context_hash) {
            continue;
        }
        let Some(occurrence) = snapshot.occurrences.get(&assignment.occurrence_id) else {
            continue;
        };
        return Ok(Some(InteriorPolicyOpportunity {
            edit: InteriorToolPolicyEdit {
                occurrence_id: assignment.occurrence_id,
                expected_definition_id: occurrence.definition_id.clone(),
                expected: binding.policy_override,
                value: None,
            },
            context_hash,
            input_hash,
        }));
    }
    Ok(None)
}

/// Apply the exact owning edit to a checked immutable trial, never mutate/Undo Current.
pub fn interior_policy_trial(
    snapshot: &Snapshot,
    opportunity: &InteriorPolicyOpportunity,
) -> Result<Arc<Snapshot>, ProjectError> {
    if interior_policy_opportunity(snapshot)?.as_ref() != Some(opportunity) {
        return Err(invalid(
            "interior policy proposal context is stale or no longer eligible",
        ));
    }
    let mut trial = snapshot.clone();
    trial.manufacturing_artifact = None;
    apply(&mut trial, opportunity.edit.clone())?;
    validate_pins(std::iter::once(&trial))?;
    Ok(Arc::new(trial))
}
