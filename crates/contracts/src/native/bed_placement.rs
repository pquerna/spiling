// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! The one bed placement request and its group outcome.
use super::{request_id, required, session_parent};
use crate::{geometry::*, project::ProjectRevision, rpc};

impl TryFrom<rpc::PlaceOnBedRequest> for GeometryCommand {
    type Error = String;
    fn try_from(value: rpc::PlaceOnBedRequest) -> Result<Self, String> {
        request_id(&value.request_id)?;
        Ok(Self::PlaceOnBed {
            session_id: session_parent(&value.parent)?,
            base_revision: SceneRevision(value.base_revision),
            project_revision: ProjectRevision(value.project_revision),
            occurrence_id: value
                .occurrence_id
                .map(OccurrenceId::new)
                .transpose()
                .map_err(|error| error.to_string())?,
            center_xy: value.center_xy,
            auto_orient: value.auto_orient,
        })
    }
}

impl From<BedGroupOutcome> for rpc::BedGroupOutcome {
    fn from(value: BedGroupOutcome) -> Self {
        match value {
            BedGroupOutcome::Unchanged => Self::Unchanged,
            BedGroupOutcome::Rested => Self::Rested,
            BedGroupOutcome::Centred => Self::Centred,
            BedGroupOutcome::DoesNotFit => Self::DoesNotFit,
        }
    }
}

impl TryFrom<rpc::BedGroupPlacement> for BedGroupPlacement {
    type Error = String;
    fn try_from(value: rpc::BedGroupPlacement) -> Result<Self, String> {
        let outcome = match rpc::BedGroupOutcome::try_from(value.outcome)
            .map_err(|_| "unknown bed group outcome")?
        {
            rpc::BedGroupOutcome::Unspecified => {
                return Err("bed group outcome is unspecified".into());
            }
            rpc::BedGroupOutcome::Unchanged => BedGroupOutcome::Unchanged,
            rpc::BedGroupOutcome::Rested => BedGroupOutcome::Rested,
            rpc::BedGroupOutcome::Centred => BedGroupOutcome::Centred,
            rpc::BedGroupOutcome::DoesNotFit => BedGroupOutcome::DoesNotFit,
        };
        let result = Self {
            outcome,
            placed_bounds_mm: required(value.placed_bounds_mm, "placed_bounds_mm")?.try_into()?,
            build_envelope_mm: required(value.build_envelope_mm, "build_envelope_mm")?
                .try_into()?,
        };
        result.validate().map_err(|error| error.to_string())?;
        Ok(result)
    }
}

impl TryFrom<BedGroupPlacement> for rpc::BedGroupPlacement {
    type Error = String;
    fn try_from(value: BedGroupPlacement) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            outcome: rpc::BedGroupOutcome::from(value.outcome) as i32,
            placed_bounds_mm: Some(value.placed_bounds_mm.try_into()?),
            build_envelope_mm: Some(value.build_envelope_mm.try_into()?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bed_group_placement_roundtrips_and_refuses_unspecified_outcome() {
        let envelope = AabbMm::new([0.0; 3], [200.0; 3]).unwrap();
        for outcome in [
            BedGroupOutcome::Unchanged,
            BedGroupOutcome::Rested,
            BedGroupOutcome::Centred,
            BedGroupOutcome::DoesNotFit,
        ] {
            let placement = BedGroupPlacement {
                outcome,
                placed_bounds_mm: AabbMm::new([1.0; 3], [2.0; 3]).unwrap(),
                build_envelope_mm: envelope,
            };
            let wire = rpc::BedGroupPlacement::try_from(placement.clone()).unwrap();
            assert_eq!(BedGroupPlacement::try_from(wire).unwrap(), placement);
        }
        let mut wire = rpc::BedGroupPlacement::try_from(BedGroupPlacement {
            outcome: BedGroupOutcome::Rested,
            placed_bounds_mm: envelope,
            build_envelope_mm: envelope,
        })
        .unwrap();
        wire.outcome = rpc::BedGroupOutcome::Unspecified as i32;
        assert!(BedGroupPlacement::try_from(wire).is_err());
    }
}
