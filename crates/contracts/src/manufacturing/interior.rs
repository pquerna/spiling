// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use std::borrow::Cow;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InteriorToolPolicy {
    Automatic,
    PrimaryOnly,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InteriorToolOverride {
    pub enabled: bool,
    pub value: InteriorToolPolicy,
}

/// Compare-and-set edit of one occurrence's captured interior policy.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InteriorToolPolicyEdit {
    pub occurrence_id: OccurrenceId,
    pub expected_definition_id: DefinitionId,
    #[serde(deserialize_with = "required_option")]
    pub expected: Option<InteriorToolOverride>,
    #[serde(deserialize_with = "required_option")]
    pub value: Option<InteriorToolOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InteriorSelectionGoal {
    pub minimum_modeled_saving_seconds: f64,
    pub max_extra_model_fraction: f64,
    pub max_extra_waste_mm3: f64,
}
impl InteriorSelectionGoal {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if [
            self.minimum_modeled_saving_seconds,
            self.max_extra_model_fraction,
            self.max_extra_waste_mm3,
        ]
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0)
            || self.max_extra_model_fraction > 1.0
        {
            return Err(invalid(
                "interior selection goals require finite nonnegative bounds",
            ));
        }
        Ok(())
    }
}

impl InteriorToolBinding {
    pub fn primary_only(&self) -> bool {
        matches!(
            self.policy_override,
            Some(InteriorToolOverride {
                enabled: true,
                value: InteriorToolPolicy::PrimaryOnly
            })
        )
    }
}
impl OccurrenceMaterialAssignment {
    pub fn active_interior(&self) -> Option<&InteriorToolBinding> {
        self.interior
            .as_ref()
            .filter(|binding| !binding.primary_only())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InteriorStrategy {
    Primary,
    Synchronized,
    Grouped,
}
impl InteriorStrategy {
    pub const ALL: [Self; 3] = [Self::Primary, Self::Synchronized, Self::Grouped];
    pub fn index(self) -> usize {
        match self {
            Self::Primary => 0,
            Self::Synchronized => 1,
            Self::Grouped => 2,
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InteriorSelectionReason {
    ExplicitPrimary,
    NoTimingModel,
    NoObjective,
    GroupingDisabled,
    NoQualifiedAlternative,
    IncomparableTiming,
    ObjectiveNotMet,
    ImprovedModeledWork,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct QualifiedInteriorWork {
    pub interior_segments_per_tool: [u32; 2],
    pub program_hash: SourceHash,
    pub model_volume_mm3: f64,
    pub waste_volume_mm3: f64,
    #[serde(deserialize_with = "required_option")]
    pub time: Option<JobTimeLedger>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[expect(
    clippy::large_enum_variant,
    reason = "At most three bounded construction assessments stay inline without one allocation per outcome"
)]
pub enum InteriorCandidateOutcome {
    Qualified { work: QualifiedInteriorWork },
    Refused { error: ManufacturingError },
    NotEvaluated { reason: InteriorSelectionReason },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InteriorCandidateAssessment {
    pub strategy: InteriorStrategy,
    pub outcome: InteriorCandidateOutcome,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InteriorSelection {
    pub selected: InteriorStrategy,
    pub reason: InteriorSelectionReason,
    pub candidates: [InteriorCandidateAssessment; 3],
}
impl InteriorSelection {
    pub fn decision(
        job: &FixedDualJob,
        candidates: &[InteriorCandidateAssessment; 3],
    ) -> Result<(InteriorStrategy, InteriorSelectionReason), ManufacturingError> {
        let InteriorCandidateOutcome::Qualified { work: baseline } = &candidates[0].outcome else {
            return Err(invalid(
                "automatic construction requires a qualified primary reference",
            ));
        };
        if job
            .assignments
            .iter()
            .filter_map(|a| a.interior.as_ref())
            .all(InteriorToolBinding::primary_only)
        {
            return Ok((
                InteriorStrategy::Primary,
                InteriorSelectionReason::ExplicitPrimary,
            ));
        }
        let Some(goal) = &job.interior_goal else {
            return Ok((
                InteriorStrategy::Primary,
                InteriorSelectionReason::NoObjective,
            ));
        };
        let Some(reference_time) = &baseline.time else {
            return Ok((
                InteriorStrategy::Primary,
                InteriorSelectionReason::NoTimingModel,
            ));
        };
        let model_bound = baseline.model_volume_mm3 * (1.0 + goal.max_extra_model_fraction);
        let waste_bound = baseline.waste_volume_mm3 + goal.max_extra_waste_mm3;
        if !model_bound.is_finite() || !waste_bound.is_finite() {
            return Err(invalid("interior objective arithmetic overflow"));
        }
        let mut selected = InteriorStrategy::Primary;
        let mut best = reference_time.modeled_seconds();
        let mut reason = InteriorSelectionReason::NoQualifiedAlternative;
        for candidate in &candidates[1..] {
            let InteriorCandidateOutcome::Qualified { work } = &candidate.outcome else {
                continue;
            };
            let Some(time) = &work.time else {
                reason = InteriorSelectionReason::IncomparableTiming;
                continue;
            };
            if time.model != reference_time.model
                || time.startup_context_hash != reference_time.startup_context_hash
            {
                reason = InteriorSelectionReason::IncomparableTiming;
                continue;
            }
            reason = InteriorSelectionReason::ObjectiveNotMet;
            let seconds = time.modeled_seconds();
            if work.model_volume_mm3 <= model_bound
                && work.waste_volume_mm3 <= waste_bound
                && seconds < best
                && reference_time.modeled_seconds() - seconds >= goal.minimum_modeled_saving_seconds
            {
                selected = candidate.strategy;
                best = seconds;
            }
        }
        Ok((
            selected,
            if selected == InteriorStrategy::Primary {
                reason
            } else {
                InteriorSelectionReason::ImprovedModeledWork
            },
        ))
    }
    pub fn validate_current(
        &self,
        program: &str,
        report: &VerificationReport,
    ) -> Result<(), ManufacturingError> {
        let InteriorCandidateOutcome::Qualified { work } =
            &self.candidates[self.selected.index()].outcome
        else {
            return Err(invalid(
                "selected interior strategy has no qualified actual program",
            ));
        };
        let waste = report
            .routing
            .as_ref()
            .map_or(0.0, |r| r.tools.iter().map(|t| t.waste_volume_mm3).sum());
        if work.program_hash != SourceHash::from_bytes(program.as_bytes())
            || work.time != report.job_time
            || work.model_volume_mm3 != report.deposited_volume_mm3
            || work.waste_volume_mm3 != waste
            || work.interior_segments_per_tool[0]
                .checked_add(work.interior_segments_per_tool[1])
                .is_none_or(|count| count > report.deposition_segments)
        {
            return Err(invalid(
                "selected interior evidence does not bind the current actual program/report",
            ));
        }
        Ok(())
    }
    pub fn validate_for(&self, intent: &ManufacturingIntent) -> Result<(), ManufacturingError> {
        let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow else {
            return Err(invalid(
                "interior selection requires the captured dual route",
            ));
        };
        if !job.has_bound_interior() {
            return Err(invalid(
                "interior selection has no captured eligible tool binding",
            ));
        }
        for (candidate, strategy) in self.candidates.iter().zip(InteriorStrategy::ALL) {
            if candidate.strategy != strategy {
                return Err(invalid(
                    "interior candidate table must contain each strategy once in canonical order",
                ));
            }
            if let InteriorCandidateOutcome::Qualified { work } = &candidate.outcome {
                positive(&[work.model_volume_mm3])?;
                if work
                    .interior_segments_per_tool
                    .iter()
                    .any(|n| *n > MAX_DEPOSITION_SEGMENTS)
                    || work.interior_segments_per_tool.iter().sum::<u32>() > MAX_DEPOSITION_SEGMENTS
                {
                    return Err(resource("interior candidate segment budget exceeded"));
                }
                if !work.waste_volume_mm3.is_finite() || work.waste_volume_mm3 < 0.0 {
                    return Err(invalid("invalid interior candidate waste quantity"));
                }
                if let Some(time) = &work.time {
                    time.validate()?;
                }
                if work.time.as_ref().map(|t| &t.model) != job.timing_model.as_ref() {
                    return Err(invalid("interior candidate lost its captured timing model"));
                }
            }
        }
        if (self.selected, self.reason) != Self::decision(job, &self.candidates)? {
            return Err(invalid(
                "interior selection disagrees with its complete-job evidence/objective",
            ));
        }
        Ok(())
    }
}

impl NormalizedPrintPlan {
    /// Projection of a checked construction choice, retaining all installed
    /// tools, profiles, frames and auxiliaries. Captured authoring stays intact.
    pub fn execution_intent<'a>(
        &self,
        intent: &'a ManufacturingIntent,
    ) -> Result<Cow<'a, ManufacturingIntent>, ManufacturingError> {
        let Some(selection) = &self.interior_selection else {
            return Ok(Cow::Borrowed(intent));
        };
        let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow else {
            return Err(invalid("interior choice lost its workflow"));
        };
        if selection.selected == InteriorStrategy::Grouped
            || selection.selected == InteriorStrategy::Synchronized
                && matches!(intent.recipe.sparse_infill, SparseInfillPolicy::Fine)
            || selection.selected == InteriorStrategy::Primary
                && job
                    .assignments
                    .iter()
                    .all(|a| a.active_interior().is_none())
        {
            return Ok(Cow::Borrowed(intent));
        }
        let mut resolved = intent.clone();
        let JobWorkflow::FixedDualMarlin { job } = &mut resolved.printer.workflow else {
            return Err(invalid("interior choice lost its workflow"));
        };
        if selection.selected == InteriorStrategy::Primary {
            for assignment in &mut job.assignments {
                if let Some(binding) = &mut assignment.interior {
                    binding.policy_override = Some(InteriorToolOverride {
                        enabled: true,
                        value: InteriorToolPolicy::PrimaryOnly,
                    });
                }
            }
        }
        if selection.selected == InteriorStrategy::Synchronized {
            resolved.recipe.sparse_infill = SparseInfillPolicy::Fine;
        }
        Ok(Cow::Owned(resolved))
    }
}
