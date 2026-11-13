// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Read-only, revision-captured material support queries. Core still owns Apply.
use super::{check_cancel, error};
use spiling_contracts::{geometry::*, project::ProjectRevision};
use spiling_geometry::{BoundsPolicy, GeometryWorkBudget, source::SourceDefinition as Definition};
use std::sync::atomic::AtomicBool;

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
        || request.contact_tolerance_mm > MAX_BED_CONTACT_TOLERANCE_MM
        || (0..3).any(|axis| request.envelope_mm.min[axis] >= request.envelope_mm.max[axis])
    {
        return Err(error(
            GeometryErrorCode::InvalidPose,
            "invalid native contact request",
        ));
    }
    let (brackets, basis) = match definition {
        Definition::Native(_) => match native_brackets(definition, &request, cancel) {
            Ok(Some(brackets)) => (brackets, BedPoseBasis::NativeBounded),
            Ok(None) => tolerant_model_brackets(definition, &request, cancel)?,
            Err(failure)
                if matches!(
                    failure.code,
                    GeometryErrorCode::ParameterCorrespondenceUnavailable
                        | GeometryErrorCode::AnalysisIndeterminate
                        | GeometryErrorCode::ConditioningFailure
                        | GeometryErrorCode::UnsupportedGeometry
                        | GeometryErrorCode::UnsupportedOperation
                ) =>
            {
                tolerant_model_brackets(definition, &request, cancel)?
            }
            Err(failure) => return Err(failure),
        },
        Definition::Hybrid(_) => tolerant_model_brackets(definition, &request, cancel)?,
    };
    let [
        minimum_lower_mm,
        minimum_upper_mm,
        maximum_lower_mm,
        maximum_upper_mm,
    ] = brackets;
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
        basis,
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

/// Native placement brackets: minimum lower/upper and maximum lower/upper.
/// Raw tolerant-mesh extrema cannot certify contact between sampled vertices;
/// unresolved native support refuses rather than publishing singleton evidence.
fn native_brackets(
    definition: &Definition,
    request: &Request,
    cancel: &AtomicBool,
) -> Result<Option<[[f64; 3]; 4]>, GeometryError> {
    let mut work = GeometryWorkBudget::query();
    let native = definition.native()?;
    let mut minimum_lower_mm = [0.0; 3];
    let mut minimum_upper_mm = [0.0; 3];
    let mut maximum_lower_mm = [0.0; 3];
    let mut maximum_upper_mm = [0.0; 3];
    // Conservative placement uses the minimum's lower enclosure, so the
    // contact gap is at most the Z bracket width; half the contact tolerance
    // leaves room for the transported translation. XY only centers the part
    // and is rechecked against the printer's envelope at compile, so it needs
    // no finer than the contact tolerance. Heights within one printer Z step
    // are one layer boundary, so a looser Z bracket cannot invent a layer.
    // The old Z translation is removed by placement. Query without it so its
    // binary64 spacing cannot make native-scale support unrepresentable, then
    // transport closed brackets back for the original-pose contact evidence.
    for axis in 0..3 {
        check_cancel(cancel)?;
        let mut direction = [0.0; 3];
        direction[axis] = 1.0;
        let mut query_pose = request.pose;
        if axis == 2 {
            query_pose.translation_mm[2] = 0.0;
        }
        let policy = BoundsPolicy {
            maximum_endpoint_uncertainty_mm: if axis == 2 {
                request.contact_tolerance_mm * 0.5
            } else {
                request.contact_tolerance_mm
            },
        };
        let Some(support) = spiling_geometry::placed_native_support(
            native,
            query_pose,
            direction,
            policy,
            spiling_geometry::MAX_DIRECTIONAL_SCRATCH_BYTES,
            &mut work,
            cancel,
        )?
        else {
            return Ok(None);
        };
        if support.definition_id() != &request.definition_id
            || !support.is_compatible_with_definition(native)
            || support.placed_pose() != query_pose
            || support.world_direction() != direction
            || support.policy().maximum_endpoint_uncertainty_mm
                != policy.maximum_endpoint_uncertainty_mm
        {
            return Err(error(
                GeometryErrorCode::AnalysisIndeterminate,
                "source support lost its native source/pose/query binding",
            ));
        }
        let bounds = support.bounds();
        let mut minimum = bounds.minimum;
        let mut maximum = bounds.maximum;
        if axis == 2 {
            let shift = request.pose.translation_mm[2];
            minimum = [
                rounded_sum_interval(minimum[0], shift)[0],
                rounded_sum_interval(minimum[1], shift)[1],
            ];
            maximum = [
                rounded_sum_interval(maximum[0], shift)[0],
                rounded_sum_interval(maximum[1], shift)[1],
            ];
        }
        minimum_lower_mm[axis] = minimum[0];
        minimum_upper_mm[axis] = minimum[1];
        maximum_lower_mm[axis] = maximum[0];
        maximum_upper_mm[axis] = maximum[1];
    }
    Ok(Some([
        minimum_lower_mm,
        minimum_upper_mm,
        maximum_lower_mm,
        maximum_upper_mm,
    ]))
}

fn tolerant_model_brackets(
    definition: &Definition,
    request: &Request,
    cancel: &AtomicBool,
) -> Result<([[f64; 3]; 4], BedPoseBasis), GeometryError> {
    let policy = spiling_geometry::tolerant::TolerantPolicy::default();
    let model = definition.prepare(policy, cancel)?;
    if model.failed_faces != 0 {
        return Err(error(
            GeometryErrorCode::UnsupportedGeometry,
            "bed model omitted source faces",
        ));
    }
    let (minimum, maximum) = model.placed(&request.pose).bounds();
    check_cancel(cancel)?;
    Ok((
        [minimum, minimum, maximum, maximum],
        BedPoseBasis::TolerantModel {
            join_mm: policy.join_mm,
            profile_mm: policy.profile_mm,
        },
    ))
}
