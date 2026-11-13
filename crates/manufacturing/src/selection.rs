// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Bounded complete constructions over one immutable project capture. Retain
//! three small assessments, never three geometry/program/GPU alternatives.
use crate::{CompilerInput, ManufacturingResult, check_cancel, compile_exact, error};
use spiling_contracts::manufacturing::*;
use std::sync::{Arc, atomic::AtomicBool};

fn capture<'a>(input: &'a CompilerInput<'_>, intent: &'a ManufacturingIntent) -> CompilerInput<'a> {
    CompilerInput {
        project_id: input.project_id,
        revision: input.revision,
        definitions: input.definitions,
        stored_definitions: input.stored_definitions,
        occurrences: input.occurrences,
        intent,
        surface_overlays: input.surface_overlays,
        process_document: input.process_document,
        surface_images: input.surface_images,
    }
}

fn strategy_intent(input: &ManufacturingIntent, strategy: InteriorStrategy) -> ManufacturingIntent {
    let mut intent = input.clone();
    let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
        unreachable!("selection route admitted")
    };
    if strategy == InteriorStrategy::Primary {
        for assignment in &mut job.assignments {
            if let Some(binding) = &mut assignment.interior {
                binding.policy_override = Some(InteriorToolOverride {
                    enabled: true,
                    value: InteriorToolPolicy::PrimaryOnly,
                });
            }
        }
    }
    if strategy == InteriorStrategy::Synchronized {
        intent.recipe.sparse_infill = SparseInfillPolicy::Fine;
    }
    intent
}

fn assessment(
    strategy: InteriorStrategy,
    bundle: &ManufacturingBundle,
) -> Result<InteriorCandidateAssessment, ManufacturingError> {
    let JobWorkflow::FixedDualMarlin { job } = &bundle.provenance.intent.printer.workflow else {
        unreachable!("qualified route")
    };
    let mut counts = [0u32; 2];
    for path in bundle
        .plan
        .layers
        .iter()
        .flat_map(|l| &l.paths)
        .filter(|p| matches!(p.owner, Some(DepositionOwner::Regional { .. })))
    {
        counts[usize::from(job.tool_for_path(path)?)] += (path.points_mm.len() - 1) as u32;
    }

    Ok(InteriorCandidateAssessment {
        strategy,
        outcome: InteriorCandidateOutcome::Qualified {
            work: QualifiedInteriorWork {
                interior_segments_per_tool: counts,
                program_hash: bundle.program.hash.clone(),
                model_volume_mm3: bundle.verification.deposited_volume_mm3,
                waste_volume_mm3: bundle
                    .verification
                    .routing
                    .as_ref()
                    .expect("qualified dual route")
                    .tools
                    .iter()
                    .map(|t| t.waste_volume_mm3)
                    .sum(),
                waste_rounding_mm3: bundle
                    .verification
                    .routing
                    .as_ref()
                    .expect("qualified dual route")
                    .waste_rounding_mm3(),
                time: bundle.verification.job_time.clone(),
            },
        },
    })
}

pub(super) fn compile(
    input: CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<ManufacturingResult, ManufacturingError> {
    input.intent.validate()?;
    let JobWorkflow::FixedDualMarlin { job } = &input.intent.printer.workflow else {
        unreachable!("selection route admitted")
    };
    let primary_pin = job
        .assignments
        .iter()
        .filter_map(|a| a.interior.as_ref())
        .all(InteriorToolBinding::primary_only);
    let skipped = if primary_pin {
        Some(InteriorSelectionReason::ExplicitPrimary)
    } else if job.interior_goal.is_none() {
        Some(InteriorSelectionReason::NoObjective)
    } else if job.timing_model.is_none() {
        Some(InteriorSelectionReason::NoTimingModel)
    } else {
        None
    };
    let mut candidates = InteriorStrategy::ALL.map(|strategy| InteriorCandidateAssessment {
        strategy,
        outcome: InteriorCandidateOutcome::NotEvaluated {
            reason: skipped.unwrap_or(InteriorSelectionReason::GroupingDisabled),
        },
    });
    // One cumulative native guard, inherited cancellation/deadline, and at most
    // three candidate compiles plus one selected reconstruction. Failed native
    // conditioning, resource exhaustion and cancellation never become a success.
    let mut geometry_work = spiling_geometry::GeometryWorkBudget::compile();
    let last_strategy = if skipped.is_some() {
        InteriorStrategy::Primary
    } else if matches!(input.intent.recipe.sparse_infill, SparseInfillPolicy::Fine) {
        InteriorStrategy::Synchronized
    } else {
        InteriorStrategy::Grouped
    };
    let mut last_bundle = None;
    for strategy in InteriorStrategy::ALL {
        check_cancel(cancel)?;
        if strategy != InteriorStrategy::Primary && skipped.is_some() {
            continue;
        }
        if strategy == InteriorStrategy::Grouped
            && matches!(input.intent.recipe.sparse_infill, SparseInfillPolicy::Fine)
        {
            continue;
        }
        let intent = strategy_intent(input.intent, strategy);
        match compile_exact(capture(&input, &intent), cancel, &mut geometry_work) {
            Ok(bundle) => {
                candidates[strategy.index()] = assessment(strategy, &bundle)?;
                // No candidate follows this one. Reuse it if it wins, with no
                // second live geometry/program alternative during evaluation.
                if strategy == last_strategy {
                    last_bundle = Some(bundle);
                }
            }
            Err(failure)
                if strategy != InteriorStrategy::Primary
                    && matches!(
                        failure.code,
                        ManufacturingErrorCode::InvalidSpecification
                            | ManufacturingErrorCode::UnsupportedCapability
                            | ManufacturingErrorCode::UnsupportedGeometry
                    ) =>
            {
                candidates[strategy.index()].outcome =
                    InteriorCandidateOutcome::Refused { error: failure };
            }
            Err(failure) => return Err(failure),
        }
    }
    let (selected, reason) = InteriorSelection::decision(job, &candidates)?;
    let selection = InteriorSelection {
        selected,
        reason,
        candidates,
    };
    selection.validate_for(input.intent)?;
    let mut bundle = if selected == last_strategy {
        last_bundle.ok_or_else(|| {
            error(
                ManufacturingErrorCode::VerificationFailed,
                "selected construction was not qualified",
            )
        })?
    } else {
        drop(last_bundle);
        let intent = strategy_intent(input.intent, selected);
        compile_exact(capture(&input, &intent), cancel, &mut geometry_work)?
    };
    selection
        .validate_current(&bundle.bundle.program.hash, &bundle.verification)
        .map_err(|failure| {
            error(
                ManufacturingErrorCode::VerificationFailed,
                format!(
                    "selected construction differs from its assessed captured input: {}",
                    failure.message
                ),
            )
        })?;
    bundle.provenance.intent = input.intent.clone();
    bundle.provenance.input_hash = input_fingerprint(
        input.project_id,
        &bundle.provenance.definitions,
        input.occurrences,
        input.intent,
        input.surface_overlays,
        input.process_document,
    )?;
    bundle.plan.interior_selection = Some(Arc::new(selection));
    bundle.validate()?;
    // The selected captured goal/projection is exercised again by independent
    // actual-byte replay; stored candidate flags do not grant output authority.
    bundle.verification =
        crate::verifier::verify(&bundle.program, &bundle.plan, input.intent, cancel)?;
    bundle.validate()?;
    check_cancel(cancel)?;
    Ok(bundle)
}
