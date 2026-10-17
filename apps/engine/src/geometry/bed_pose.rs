// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Read-only, revision-captured material support queries. Core still owns Apply.
use super::{check_cancel, error};
use spiling_contracts::{geometry::*, project::ProjectRevision};
use spiling_geometry::{BoundsPolicy, Definition, GeometryWorkBudget};
use std::sync::atomic::AtomicBool;

const MAX_CONTACT_WORK: u64 = 20_000_000;

pub(super) struct Request {
    pub session_id: SessionId,
    pub scene_revision: SceneRevision,
    pub project_revision: ProjectRevision,
    pub occurrence_id: OccurrenceId,
    pub definition_id: DefinitionId,
    pub pose: RigidPoseMm,
    pub envelope_mm: AabbMm,
    pub center_xy: bool,
    pub contact_tolerance_mm: f64,
}

pub(super) fn prepare(
    definition: &Definition,
    request: Request,
    cancel: &AtomicBool,
) -> Result<BedPosePreparation, GeometryError> {
    check_cancel(cancel)?;
    request.pose.validate()?;
    request.envelope_mm.validate()?;
    if definition.id() != &request.definition_id
        || !request.contact_tolerance_mm.is_finite()
        || request.contact_tolerance_mm <= 0.0
        || request.contact_tolerance_mm > SECTION_PLANE_TOLERANCE_MM
        || (0..3).any(|axis| request.envelope_mm.min[axis] >= request.envelope_mm.max[axis])
    {
        return Err(error(
            GeometryErrorCode::InvalidPose,
            "invalid native contact request",
        ));
    }
    let mut work = GeometryWorkBudget::new(MAX_CONTACT_WORK);
    let mut minimum_lower_mm = [0.0; 3];
    let mut minimum_upper_mm = [0.0; 3];
    let mut maximum_lower_mm = [0.0; 3];
    let mut maximum_upper_mm = [0.0; 3];
    for axis in 0..3 {
        check_cancel(cancel)?;
        let mut direction = [0.0; 3];
        direction[axis] = 1.0;
        let support = spiling_geometry::placed_material_bounds(
            definition,
            request.pose,
            direction,
            BoundsPolicy {
                maximum_endpoint_uncertainty_mm: request.contact_tolerance_mm * 0.25,
            },
            &mut work,
            cancel,
        )?;
        if support.definition_id() != &request.definition_id
            || !support.is_compatible_with_definition(definition)
            || support.placed_pose() != request.pose
            || support.world_direction() != direction
        {
            return Err(error(
                GeometryErrorCode::AnalysisIndeterminate,
                "material support lost its original source/recipe/pose/query binding",
            ));
        }
        let bounds = support.bounds();
        minimum_lower_mm[axis] = bounds.minimum[0];
        minimum_upper_mm[axis] = bounds.minimum[1];
        maximum_lower_mm[axis] = bounds.maximum[0];
        maximum_upper_mm[axis] = bounds.maximum[1];
    }
    // Use the same directed arithmetic as the checked proposal contract.
    // A blanket reserve based on unrelated XY/envelope magnitudes invents a
    // positive material birth above the bed even when contact is exact.
    let contact_delta = rounded_sum_interval(request.envelope_mm.min[2], -minimum_lower_mm[2])[1];
    let mut pose = request.pose;
    pose.translation_mm[2] = rounded_sum_interval(request.pose.translation_mm[2], contact_delta)[1];
    if request.center_xy {
        for axis in 0..2 {
            let target = request.envelope_mm.min[axis]
                + (request.envelope_mm.max[axis] - request.envelope_mm.min[axis]) * 0.5;
            let center =
                minimum_lower_mm[axis] + (maximum_upper_mm[axis] - minimum_lower_mm[axis]) * 0.5;
            pose.translation_mm[axis] = request.pose.translation_mm[axis] + (target - center);
        }
    }
    pose.validate()?;
    let delta_z = rounded_sum_interval(pose.translation_mm[2], -request.pose.translation_mm[2]);
    let maximum_contact_gap_mm = rounded_sum_interval(
        rounded_sum_interval(minimum_upper_mm[2], delta_z[1])[1],
        -request.envelope_mm.min[2],
    )[1];
    let result = BedPosePreparation {
        session_id: request.session_id,
        scene_revision: request.scene_revision,
        project_revision: request.project_revision,
        occurrence_id: request.occurrence_id,
        definition_id: request.definition_id,
        original_pose: request.pose,
        pose,
        envelope_mm: request.envelope_mm,
        minimum_lower_mm,
        minimum_upper_mm,
        maximum_lower_mm,
        maximum_upper_mm,
        center_xy: request.center_xy,
        contact_tolerance_mm: request.contact_tolerance_mm,
        maximum_contact_gap_mm,
    };
    result.validate().map_err(|_| {
        error(
            GeometryErrorCode::InvalidPose,
            "prepared native pose cannot fit the declared envelope and contact tolerance",
        )
    })?;
    check_cancel(cancel)?;
    Ok(result)
}
