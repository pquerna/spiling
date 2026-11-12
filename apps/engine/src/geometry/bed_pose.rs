// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Read-only, revision-captured material support queries. Core still owns Apply.
use super::{check_cancel, error};
use spiling_contracts::{geometry::*, project::ProjectRevision};
use spiling_geometry::{
    BoundsPolicy, GeometryWorkBudget, source::SourceDefinition as Definition,
    tolerant::TolerantPolicy,
};
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
    let [
        minimum_lower_mm,
        minimum_upper_mm,
        maximum_lower_mm,
        maximum_upper_mm,
    ] = match exact_brackets(definition, &request, cancel) {
        Ok(brackets) => brackets,
        Err(exact) if tolerant_retry(&exact) => tolerant_brackets(definition, &request, cancel)
            .map_err(|tolerant| {
                error(
                    tolerant.code,
                    format!(
                        "{} (exact native bounds also refused: {})",
                        tolerant.message, exact.message
                    ),
                )
            })?,
        Err(exact) => return Err(exact),
    };
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

/// Exact native placement brackets: minimum lower/upper and maximum lower/upper.
fn exact_brackets(
    definition: &Definition,
    request: &Request,
    cancel: &AtomicBool,
) -> Result<[[f64; 3]; 4], GeometryError> {
    let mut work = GeometryWorkBudget::query();
    let source = spiling_geometry::prepare_solid(
        definition.native()?,
        spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES,
        &mut work,
        cancel,
    )?;
    // Share immutable native trim domains; admit each query's scratch beside
    // their retained capacities in the same geometry pool.
    let query_bytes = spiling_geometry::MAX_PREPARED_SECTION_BYTES
        .checked_sub(source.retained_bytes())
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "bed preparation source and query geometry exceed simultaneous storage",
            )
        })?;
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
        // Each axis runs under its own defect fuse; a fused query falls back
        // to tolerant brackets like any other exact refusal.
        let mut fuse = GeometryWorkBudget::exact_bounds_fuse();
        let support = source.placed_material_bounds(
            query_pose,
            direction,
            BoundsPolicy {
                maximum_endpoint_uncertainty_mm: if axis == 2 {
                    request.contact_tolerance_mm * 0.5
                } else {
                    request.contact_tolerance_mm
                },
            },
            query_bytes,
            &mut fuse,
            cancel,
        )?;
        work.charge(fuse.consumed(), cancel)?;
        if support.definition_id() != &request.definition_id
            || !support.is_compatible_with_definition(definition.native()?)
            || support.placed_pose() != query_pose
            || support.world_direction() != direction
        {
            return Err(error(
                GeometryErrorCode::AnalysisIndeterminate,
                "material support lost its native producer/source/pose/query binding",
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
    Ok([
        minimum_lower_mm,
        minimum_upper_mm,
        maximum_lower_mm,
        maximum_upper_mm,
    ])
}

/// Exact bounds that refused for a geometric reason, not cancellation or a
/// bad request, fall back to ADR012 tolerant bounds.
fn tolerant_retry(error: &GeometryError) -> bool {
    !matches!(
        error.code,
        GeometryErrorCode::Cancelled
            | GeometryErrorCode::InvalidPose
            | GeometryErrorCode::StaleRevision
            | GeometryErrorCode::UnknownHandle
            | GeometryErrorCode::Busy
            | GeometryErrorCode::SourceIo
            | GeometryErrorCode::SourceChanged
    )
}

/// ADR012 tolerant placement brackets from the placed tessellation. Its
/// vertices lie on the surface, so its extremes bracket the solid's within the
/// profile tolerance.
fn tolerant_brackets(
    definition: &Definition,
    request: &Request,
    cancel: &AtomicBool,
) -> Result<[[f64; 3]; 4], GeometryError> {
    let model = definition.prepare(TolerantPolicy::default(), cancel)?;
    if model.failed_faces > 0 {
        return Err(error(
            GeometryErrorCode::UnsupportedGeometry,
            format!(
                "{} source faces could not be tessellated",
                model.failed_faces
            ),
        ));
    }
    let (minimum, maximum) = model.placed(&request.pose).bounds();
    check_cancel(cancel)?;
    Ok([minimum, minimum, maximum, maximum])
}
