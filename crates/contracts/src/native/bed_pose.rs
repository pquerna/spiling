// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Checked read-only native placement requests and contact evidence.
use super::{request_id, required, session_parent};
use crate::{geometry::*, project::ProjectRevision, rpc};

pub(super) fn validate_request(
    pose: RigidPoseMm,
    envelope: AabbMm,
    contact_tolerance_mm: f64,
) -> Result<(), String> {
    pose.validate().map_err(|error| error.to_string())?;
    envelope.validate().map_err(|error| error.to_string())?;
    if (0..3).any(|axis| envelope.min[axis] >= envelope.max[axis])
        || !contact_tolerance_mm.is_finite()
        || contact_tolerance_mm <= 0.0
        || contact_tolerance_mm > SECTION_PLANE_TOLERANCE_MM
    {
        return Err("invalid native bed-preparation policy".into());
    }
    Ok(())
}

impl TryFrom<rpc::PrepareBedPoseRequest> for GeometryCommand {
    type Error = String;
    fn try_from(value: rpc::PrepareBedPoseRequest) -> Result<Self, String> {
        request_id(&value.request_id)?;
        let expected_pose = required(value.expected_pose, "expected_pose")?.try_into()?;
        let envelope_mm = required(value.envelope_mm, "envelope_mm")?.try_into()?;
        validate_request(expected_pose, envelope_mm, value.contact_tolerance_mm)?;
        Ok(Self::PrepareBedPose {
            session_id: session_parent(&value.parent)?,
            base_revision: SceneRevision(value.base_revision),
            project_revision: ProjectRevision(value.project_revision),
            occurrence_id: OccurrenceId::new(value.occurrence_id).map_err(|e| e.to_string())?,
            expected_definition_id: DefinitionId::parse(value.expected_definition_id)
                .map_err(|e| e.to_string())?,
            expected_pose,
            envelope_mm,
            center_xy: value.center_xy,
            contact_tolerance_mm: value.contact_tolerance_mm,
        })
    }
}

impl TryFrom<GeometryCommand> for rpc::PrepareBedPoseRequest {
    type Error = String;
    fn try_from(value: GeometryCommand) -> Result<Self, String> {
        match super::geometry::geometry_call(value, "")? {
            super::geometry::GeometryCall::PrepareBedPose(request) => Ok(request),
            _ => Err("expected prepare_bed_pose command".into()),
        }
    }
}

impl TryFrom<rpc::BedPosePreparation> for BedPosePreparation {
    type Error = String;
    fn try_from(value: rpc::BedPosePreparation) -> Result<Self, String> {
        let result = Self {
            session_id: SessionId::parse(value.session_id).map_err(|e| e.to_string())?,
            scene_revision: SceneRevision(value.scene_revision),
            project_revision: ProjectRevision(value.project_revision),
            occurrence_id: OccurrenceId::new(value.occurrence_id).map_err(|e| e.to_string())?,
            definition_id: DefinitionId::parse(value.definition_id).map_err(|e| e.to_string())?,
            original_pose: required(value.original_pose, "original_pose")?.try_into()?,
            pose: required(value.pose, "pose")?.try_into()?,
            envelope_mm: required(value.envelope_mm, "envelope_mm")?.try_into()?,
            minimum_lower_mm: required(value.minimum_lower_mm, "minimum_lower_mm")?.try_into()?,
            minimum_upper_mm: required(value.minimum_upper_mm, "minimum_upper_mm")?.try_into()?,
            maximum_lower_mm: required(value.maximum_lower_mm, "maximum_lower_mm")?.try_into()?,
            maximum_upper_mm: required(value.maximum_upper_mm, "maximum_upper_mm")?.try_into()?,
            center_xy: value.center_xy,
            contact_tolerance_mm: value.contact_tolerance_mm,
            maximum_contact_gap_mm: value.maximum_contact_gap_mm,
        };
        result.validate().map_err(|error| error.to_string())?;
        Ok(result)
    }
}

impl TryFrom<BedPosePreparation> for rpc::BedPosePreparation {
    type Error = String;
    fn try_from(value: BedPosePreparation) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            session_id: value.session_id.into(),
            scene_revision: value.scene_revision.get(),
            project_revision: value.project_revision.0,
            occurrence_id: value.occurrence_id.get(),
            definition_id: value.definition_id.into(),
            original_pose: Some(value.original_pose.try_into()?),
            pose: Some(value.pose.try_into()?),
            envelope_mm: Some(value.envelope_mm.try_into()?),
            minimum_lower_mm: Some(value.minimum_lower_mm.try_into()?),
            minimum_upper_mm: Some(value.minimum_upper_mm.try_into()?),
            maximum_lower_mm: Some(value.maximum_lower_mm.try_into()?),
            maximum_upper_mm: Some(value.maximum_upper_mm.try_into()?),
            center_xy: value.center_xy,
            contact_tolerance_mm: value.contact_tolerance_mm,
            maximum_contact_gap_mm: value.maximum_contact_gap_mm,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact_contact() -> rpc::BedPosePreparation {
        BedPosePreparation {
            session_id: SessionId::new(),
            scene_revision: SceneRevision(3),
            project_revision: ProjectRevision(7),
            occurrence_id: OccurrenceId::new(1).unwrap(),
            definition_id: DefinitionId::from_source_sha256(&[7; 32]),
            original_pose: RigidPoseMm {
                translation_mm: [0.0, 0.0, 1.0],
                ..RigidPoseMm::IDENTITY
            },
            pose: RigidPoseMm::IDENTITY,
            envelope_mm: AabbMm::new([0.0; 3], [200.0; 3]).unwrap(),
            minimum_lower_mm: [0.0, 0.0, 1.0],
            minimum_upper_mm: [0.0, 0.0, 1.0],
            maximum_lower_mm: [1.0, 1.0, 2.0],
            maximum_upper_mm: [1.0, 1.0, 2.0],
            center_xy: false,
            contact_tolerance_mm: SECTION_PLANE_TOLERANCE_MM,
            maximum_contact_gap_mm: 0.0,
        }
        .try_into()
        .unwrap()
    }

    #[test]
    fn contact_admission_rejects_roundoff_penetration_and_false_gap_evidence() {
        let wire = exact_contact();
        let mut penetrating = wire.clone();
        // Ordinary subtraction rounds (-2^-54)-1 to -1, hiding penetration.
        penetrating
            .pose
            .as_mut()
            .unwrap()
            .translation_mm
            .as_mut()
            .unwrap()
            .z = -f64::EPSILON * 0.25;
        assert!(BedPosePreparation::try_from(penetrating).is_err());
        let mut understated = wire.clone();
        understated.minimum_upper_mm.as_mut().unwrap().z = 1.0_f64.next_up();
        assert!(BedPosePreparation::try_from(understated).is_err());
        let mut outside = wire.clone();
        outside.maximum_upper_mm.as_mut().unwrap().x = 201.0;
        assert!(BedPosePreparation::try_from(outside).is_err());
        let mut wrong_policy = wire;
        wrong_policy
            .pose
            .as_mut()
            .unwrap()
            .translation_mm
            .as_mut()
            .unwrap()
            .x = 1.0;
        assert!(BedPosePreparation::try_from(wrong_policy).is_err());
    }
}
