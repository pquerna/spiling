// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Each part's resting brackets for the one bed placement rule
//! (`bed_group`): native support where it resolves, else the tolerant model.
use super::{check_cancel, error};
use spiling_contracts::geometry::*;
use spiling_geometry::{BoundsPolicy, GeometryWorkBudget, source::SourceDefinition as Definition};
use std::sync::atomic::AtomicBool;

pub(super) struct Request {
    pub occurrence_id: OccurrenceId,
    pub definition_id: DefinitionId,
    pub pose: RigidPoseMm,
    pub envelope_mm: AabbMm,
    pub contact_tolerance_mm: f64,
}

/// Native brackets enclose source material; tolerant brackets describe only
/// the sampled model identified by `basis`, not certified native extrema.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Basis {
    NativeBounded,
    TolerantModel { join_mm: f64, profile_mm: f64 },
}

/// One part's resting evidence: its pose dropped onto `envelope_mm.min[2]`
/// with XY kept, and the support brackets in the original pose.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Preparation {
    pub(super) basis: Basis,
    pub(super) original_pose: RigidPoseMm,
    pub(super) pose: RigidPoseMm,
    pub(super) envelope_mm: AabbMm,
    pub(super) minimum_lower_mm: [f64; 3],
    pub(super) minimum_upper_mm: [f64; 3],
    pub(super) maximum_lower_mm: [f64; 3],
    pub(super) maximum_upper_mm: [f64; 3],
    pub(super) contact_tolerance_mm: f64,
    pub(super) maximum_contact_gap_mm: f64,
}

impl Preparation {
    fn validate(&self) -> Result<(), GeometryError> {
        if let Basis::TolerantModel {
            join_mm,
            profile_mm,
        } = self.basis
            && (!join_mm.is_finite()
                || join_mm <= 0.0
                || !profile_mm.is_finite()
                || profile_mm <= 0.0)
        {
            return Err(error(
                GeometryErrorCode::InvalidPose,
                "invalid tolerant bed-placement model policy",
            ));
        }
        self.original_pose.validate()?;
        self.pose.validate()?;
        self.envelope_mm.validate()?;
        if self.pose.rotation_xyzw != self.original_pose.rotation_xyzw
            || !self.contact_tolerance_mm.is_finite()
            || self.contact_tolerance_mm <= 0.0
            || self.contact_tolerance_mm > MAX_BED_CONTACT_TOLERANCE_MM
            || !self.maximum_contact_gap_mm.is_finite()
            || self.maximum_contact_gap_mm < 0.0
            || self.maximum_contact_gap_mm > self.contact_tolerance_mm
        {
            return Err(error(
                GeometryErrorCode::InvalidPose,
                "invalid native bed-contact evidence",
            ));
        }
        let xy_slack = |axis: usize| {
            if axis < 2 {
                self.contact_tolerance_mm
            } else {
                0.0
            }
        };
        for axis in 0..3 {
            let values = [
                self.minimum_lower_mm[axis],
                self.minimum_upper_mm[axis],
                self.maximum_lower_mm[axis],
                self.maximum_upper_mm[axis],
            ];
            if values.iter().any(|value| !value.is_finite())
                || values[0] > values[1]
                || values[2] > values[3]
                || values[0] > values[3]
                || self.envelope_mm.min[axis] >= self.envelope_mm.max[axis]
            {
                return Err(error(
                    GeometryErrorCode::InvalidPose,
                    "invalid native support bracket",
                ));
            }
            let delta = rounded_sum_interval(
                self.pose.translation_mm[axis],
                -self.original_pose.translation_mm[axis],
            );
            let placed_min = rounded_sum_interval(values[0], delta[0])[0];
            let placed_max = rounded_sum_interval(values[3], delta[1])[1];
            if delta.iter().any(|value| !value.is_finite())
                || !placed_min.is_finite()
                || !placed_max.is_finite()
                // ADR012: in XY, within one step (the contact tolerance) of
                // the envelope is the same commanded position, so outward-
                // rounded brackets of a part touching a side are inside. Z
                // stays exact: bed contact never admits penetration.
                || placed_min < self.envelope_mm.min[axis] - xy_slack(axis)
                || placed_max > self.envelope_mm.max[axis] + xy_slack(axis)
                || (axis < 2
                    && self.pose.translation_mm[axis] != self.original_pose.translation_mm[axis])
            {
                return Err(error(
                    GeometryErrorCode::InvalidPose,
                    "prepared native pose exceeds its envelope",
                ));
            }
        }
        let delta = rounded_sum_interval(
            self.pose.translation_mm[2],
            -self.original_pose.translation_mm[2],
        );
        let gap = rounded_sum_interval(
            rounded_sum_interval(self.minimum_upper_mm[2], delta[1])[1],
            -self.envelope_mm.min[2],
        )[1];
        if !gap.is_finite() || gap < 0.0 || gap > self.maximum_contact_gap_mm {
            return Err(error(
                GeometryErrorCode::InvalidPose,
                "prepared native contact gap is not enclosed",
            ));
        }
        Ok(())
    }
}

pub(super) fn prepare_with_budget(
    definition: &Definition,
    request: Request,
    additional_bytes: usize,
    cancel: &AtomicBool,
) -> Result<Preparation, GeometryError> {
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
            Ok(Some(brackets)) => (brackets, Basis::NativeBounded),
            Ok(None) => tolerant_model_brackets(definition, &request, additional_bytes, cancel)?,
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
                tolerant_model_brackets(definition, &request, additional_bytes, cancel)?
            }
            Err(failure) => return Err(failure),
        },
        Definition::Hybrid(_) => {
            tolerant_model_brackets(definition, &request, additional_bytes, cancel)?
        }
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
    pose.validate()?;
    let delta_z = rounded_sum_interval(pose.translation_mm[2], -request.pose.translation_mm[2]);
    let maximum_contact_gap_mm = rounded_sum_interval(
        rounded_sum_interval(minimum_upper_mm[2], delta_z[1])[1],
        -request.envelope_mm.min[2],
    )[1];
    let result = Preparation {
        basis,
        original_pose: request.pose,
        pose,
        envelope_mm: request.envelope_mm,
        minimum_lower_mm,
        minimum_upper_mm,
        maximum_lower_mm,
        maximum_upper_mm,
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
    additional_bytes: usize,
    cancel: &AtomicBool,
) -> Result<([[f64; 3]; 4], Basis), GeometryError> {
    let policy = spiling_geometry::tolerant::TolerantPolicy::default();
    let (model, outcome) = definition.prepare_with_budget(policy, additional_bytes, cancel)?;
    if model.failed_faces != 0 {
        return Err(error(
            GeometryErrorCode::UnsupportedGeometry,
            "bed model omitted source faces",
        ));
    }
    let (minimum, maximum) = if request.pose == RigidPoseMm::IDENTITY {
        model.bounds()
    } else {
        model
            .placed_with_budget(
                &request.pose,
                additional_bytes.saturating_sub(outcome.allocated_bytes()),
                cancel,
            )?
            .bounds()
    };
    check_cancel(cancel)?;
    Ok((
        [minimum, minimum, maximum, maximum],
        Basis::TolerantModel {
            join_mm: policy.join_mm,
            profile_mm: policy.profile_mm,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact_contact() -> Preparation {
        Preparation {
            basis: Basis::NativeBounded,
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
            contact_tolerance_mm: SECTION_PLANE_TOLERANCE_MM,
            maximum_contact_gap_mm: 0.0,
        }
    }

    #[test]
    fn contact_admission_rejects_roundoff_penetration_and_false_gap_evidence() {
        assert!(exact_contact().validate().is_ok());
        let mut penetrating = exact_contact();
        // Ordinary subtraction rounds (-2^-54)-1 to -1, hiding penetration.
        penetrating.pose.translation_mm[2] = -f64::EPSILON * 0.25;
        assert!(penetrating.validate().is_err());
        let mut understated = exact_contact();
        understated.minimum_upper_mm[2] = 1.0_f64.next_up();
        assert!(understated.validate().is_err());
        let mut outside = exact_contact();
        outside.maximum_upper_mm[0] = 201.0;
        assert!(outside.validate().is_err());
        let mut moved_xy = exact_contact();
        moved_xy.pose.translation_mm[0] = 1.0;
        assert!(moved_xy.validate().is_err());
        let mut sampled = exact_contact();
        sampled.basis = Basis::TolerantModel {
            join_mm: 0.01,
            profile_mm: f64::NAN,
        };
        assert!(sampled.validate().is_err());
    }
}
