// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! The one bed placement rule both clients use: rest the scene's parts on the
//! bed as one rigid group, keep their XY when the printer admits them there
//! (ADR012: within one motion step), otherwise centre them on the selected
//! plate's usable region. Decisions only; Core applies the moves.
use super::error;
use spiling_contracts::{
    geometry::*, hardware::ProjectHardwareCapture, manufacturing::PrinterSpecification,
};

/// One member's native support brackets from `bed_pose::prepare_with_budget`
/// against a wide XY envelope at the bed height.
pub(super) struct Member {
    pub occurrence_id: OccurrenceId,
    pub pose: RigidPoseMm,
    /// Lowest material (minimum lower bracket) in the current pose.
    pub lower_mm: [f64; 3],
    /// Highest material (maximum upper bracket) in the current pose.
    pub upper_mm: [f64; 3],
    /// Z translation that rests this member on the bed.
    pub drop_mm: f64,
    /// True when auto-orient changed this member's rotation.
    pub rotated: bool,
}

pub(super) struct Decision {
    pub placement: BedGroupPlacement,
    pub moves: Vec<LayoutPlacement>,
}

pub(super) fn decide(
    members: &[Member],
    printer: &PrinterSpecification,
    hardware: Option<&ProjectHardwareCapture>,
    center_xy: bool,
) -> Result<Decision, GeometryError> {
    if members.is_empty() {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "Add a part before placing parts on the bed",
        ));
    }
    let mut lower = [f64::INFINITY; 3];
    let mut upper = [f64::NEG_INFINITY; 3];
    // A rigid group: every member moves by the drop that rests the lowest one.
    let mut lift = f64::NEG_INFINITY;
    for member in members {
        for axis in 0..3 {
            lower[axis] = lower[axis].min(member.lower_mm[axis]);
            upper[axis] = upper[axis].max(member.upper_mm[axis]);
        }
        lift = lift.max(member.drop_mm);
    }
    let placed = |delta: [f64; 3]| AabbMm {
        min: std::array::from_fn(|axis| lower[axis] + delta[axis]),
        max: std::array::from_fn(|axis| upper[axis] + delta[axis]),
    };
    let on_bed = |bounds: &AabbMm| {
        printer.envelope_admits(bounds)
            && hardware.is_none_or(|capture| capture.admits_bounds(bounds).is_ok())
    };
    let envelope = printer.build_envelope;
    let mut delta = [0.0, 0.0, lift];
    let centre = center_xy || !on_bed(&placed(delta));
    if centre {
        let target = match hardware {
            Some(capture) => capture
                .usable_center_xy()
                .map_err(|message| error(GeometryErrorCode::InvalidGeometry, message))?,
            None => [
                (envelope.min[0] + envelope.max[0]) * 0.5,
                (envelope.min[1] + envelope.max[1]) * 0.5,
            ],
        };
        for axis in 0..2 {
            delta[axis] = target[axis] - (lower[axis] + upper[axis]) * 0.5;
        }
    }
    let placed_bounds_mm = placed(delta);
    let result = |outcome, placed_bounds_mm, moves| {
        let placement = BedGroupPlacement {
            outcome,
            placed_bounds_mm,
            build_envelope_mm: envelope,
        };
        placement.validate()?;
        Ok(Decision { placement, moves })
    };
    if !printer.envelope_admits(&placed_bounds_mm) {
        return result(BedGroupOutcome::DoesNotFit, placed_bounds_mm, vec![]);
    }
    let any_rotated = members.iter().any(|member| member.rotated);
    if !centre && !any_rotated && lift.abs() <= printer.motion_resolution.z_mm {
        return result(BedGroupOutcome::Unchanged, placed([0.0; 3]), vec![]);
    }
    let moves = members
        .iter()
        .map(|member| {
            let mut pose = member.pose;
            for (translation, step) in pose.translation_mm.iter_mut().zip(delta) {
                *translation += step;
            }
            LayoutPlacement {
                source: member.occurrence_id,
                copy: false,
                pose,
            }
        })
        .collect();
    result(
        if centre {
            BedGroupOutcome::Centred
        } else {
            BedGroupOutcome::Rested
        },
        placed_bounds_mm,
        moves,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::manufacturing::MotionResolution;

    fn printer() -> PrinterSpecification {
        let mut printer: PrinterSpecification = serde_json::from_value(
            serde_json::from_str::<serde_json::Value>(include_str!(
                "../../../../fixtures/manufacturing/solid-fill.intent.json"
            ))
            .unwrap()["printer"]
                .clone(),
        )
        .unwrap();
        printer.build_envelope = AabbMm::new([0.0; 3], [220.0; 3]).unwrap();
        printer.motion_resolution = MotionResolution {
            xy_mm: 0.0125,
            z_mm: 0.0025,
        };
        printer
    }
    fn member(id: u32, x: f64, y: f64, z: f64, size: [f64; 3]) -> Member {
        Member {
            occurrence_id: OccurrenceId::new(id).unwrap(),
            pose: RigidPoseMm {
                translation_mm: [x, y, z],
                rotation_xyzw: [0.0, 0.0, 0.0, 1.0],
            },
            lower_mm: [x, y, z],
            upper_mm: [x + size[0], y + size[1], z + size[2]],
            drop_mm: -z,
            rotated: false,
        }
    }

    #[test]
    fn part_touching_the_envelope_edge_is_left_unchanged() {
        let mut edge = member(1, 0.0, 0.0, 0.0, [20.0, 10.0, 8.0]);
        // Outward-rounded native brackets sit a few ulps outside the edge.
        edge.lower_mm[0] = -3.6e-15;
        let decision = decide(&[edge], &printer(), None, false).unwrap();
        assert_eq!(decision.placement.outcome, BedGroupOutcome::Unchanged);
        assert!(decision.moves.is_empty());
    }

    #[test]
    fn floating_part_on_the_bed_is_rested_in_place() {
        let decision = decide(
            &[member(1, 50.0, 60.0, 25.0, [20.0, 10.0, 8.0])],
            &printer(),
            None,
            false,
        )
        .unwrap();
        assert_eq!(decision.placement.outcome, BedGroupOutcome::Rested);
        assert_eq!(
            decision.moves[0].pose.translation_mm,
            [50.0, 60.0, 0.0],
            "XY kept, Z rested"
        );
    }

    #[test]
    fn off_bed_group_is_centred_as_one_rigid_group() {
        let decision = decide(
            &[
                member(1, -400.0, 700.0, 25.0, [20.0, 10.0, 8.0]),
                member(2, -370.0, 700.0, 30.0, [10.0, 10.0, 8.0]),
            ],
            &printer(),
            None,
            false,
        )
        .unwrap();
        assert_eq!(decision.placement.outcome, BedGroupOutcome::Centred);
        // Group spans x -400..-360, y 700..710, z from 25: centred on 110,110.
        assert_eq!(decision.moves[0].pose.translation_mm, [90.0, 105.0, 0.0]);
        assert_eq!(decision.moves[1].pose.translation_mm, [120.0, 105.0, 5.0]);
        assert!(printer().envelope_admits(&decision.placement.placed_bounds_mm));
    }

    #[test]
    fn center_request_centres_a_part_already_on_the_bed() {
        let decision = decide(
            &[member(1, 0.0, 0.0, 0.0, [20.0, 10.0, 8.0])],
            &printer(),
            None,
            true,
        )
        .unwrap();
        assert_eq!(decision.placement.outcome, BedGroupOutcome::Centred);
        assert_eq!(decision.moves[0].pose.translation_mm, [100.0, 105.0, 0.0]);
    }

    #[test]
    fn oversized_group_does_not_fit_and_moves_nothing() {
        let decision = decide(
            &[member(1, 0.0, 0.0, 5.0, [300.0, 10.0, 8.0])],
            &printer(),
            None,
            false,
        )
        .unwrap();
        assert_eq!(decision.placement.outcome, BedGroupOutcome::DoesNotFit);
        assert!(decision.moves.is_empty());
        assert_eq!(decision.placement.placed_bounds_mm.min[0], -40.0);
    }
}
