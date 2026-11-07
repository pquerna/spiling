// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Transport explicit occurrence reservations, never infer material from bounds.
//! Native preflight and complete-bead replay independently admit the new layout.
use crate::{Snapshot, invalid, manufacturing_options};
use spiling_contracts::project::ProjectError;
use spiling_contracts::{
    geometry::{OccurrenceId, RigidPoseMm},
    manufacturing::*,
};
use std::sync::Arc;

pub(crate) fn apply(
    snapshot: &mut Snapshot,
    original: &Snapshot,
    placements: &[(OccurrenceId, OccurrenceId, RigidPoseMm)],
) -> Result<(), ProjectError> {
    let Some(intent) = original.manufacturing_intent.as_deref() else {
        return Ok(());
    };
    let JobWorkflow::FixedDualMarlin { job: source_job } = &intent.printer.workflow else {
        return Ok(());
    };
    if !source_job.uniform_without_construction() {
        if placements
            .iter()
            .any(|(source, target, _)| source != target)
        {
            return Err(invalid(
                "routed copies currently require uniform parts without material constructions",
            ));
        }
        return Ok(());
    }
    let mut intent = snapshot
        .manufacturing_intent
        .as_deref()
        .cloned()
        .ok_or_else(|| invalid("routed placement lost captured setup"))?;
    let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
        return Err(invalid("routed placement changed its setup"));
    };
    for (source, target, pose) in placements {
        let old = original
            .occurrences
            .get(source)
            .ok_or_else(|| invalid("routed placement source is absent"))?;
        if pose.rotation_xyzw != old.pose.rotation_xyzw
            || pose.translation_mm[2] != old.pose.translation_mm[2]
        {
            return Err(invalid(
                "routed placement retains orientation and bed height; recapture a changed setup explicitly",
            ));
        }
        let mut assignment = source_job
            .assignments
            .iter()
            .find(|a| a.occurrence_id == *source)
            .cloned()
            .ok_or_else(|| invalid("routed placement source has no captured tool binding"))?;
        assignment.occurrence_id = *target;
        for axis in 0..2 {
            if pose.translation_mm[axis] == old.pose.translation_mm[axis] {
                continue;
            }
            let delta = pose.translation_mm[axis] - old.pose.translation_mm[axis];
            assignment.work_envelope_mm.min[axis] =
                (assignment.work_envelope_mm.min[axis] + delta.next_down()).next_down();
            assignment.work_envelope_mm.max[axis] =
                (assignment.work_envelope_mm.max[axis] + delta.next_up()).next_up();
        }
        if source == target {
            let row = job
                .assignments
                .iter_mut()
                .find(|a| a.occurrence_id == *target)
                .ok_or_else(|| invalid("moved part has no routed assignment"))?;
            *row = assignment;
        } else {
            job.assignments.push(assignment);
        }
    }
    manufacturing_options::set_intent(snapshot, Arc::new(intent))?;
    let copied = placements
        .iter()
        .filter_map(|(source, target, _)| (source != target).then_some(*target))
        .collect::<Vec<_>>();
    crate::interior_policy::stamp_copied_pins(snapshot, &copied)
}
