// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Producer-only conservative fixed-carriage admission. Replay owns its own checks.
use crate::{check_cancel, error};
use spiling_contracts::geometry::AabbMm;
use spiling_contracts::manufacturing::program_coordinates::{
    project_xyz6_point_mm, xyz6_coordinate_error_mm,
};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

fn invalid(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::InvalidSpecification, message)
}
fn overlaps(a: AabbMm, b: AabbMm) -> bool {
    (0..3).all(|axis| a.min[axis] <= b.max[axis] && b.min[axis] <= a.max[axis])
}
fn swept(a: [f64; 3], b: [f64; 3], min: [f64; 3], max: [f64; 3]) -> AabbMm {
    AabbMm {
        min: std::array::from_fn(|axis| (a[axis].min(b[axis]) + min[axis]).next_down()),
        max: std::array::from_fn(|axis| (a[axis].max(b[axis]) + max[axis]).next_up()),
    }
}
pub(crate) fn safe_z(
    plan: &NormalizedPrintPlan,
    job: &FixedDualJob,
) -> Result<f64, ManufacturingError> {
    let top = plan
        .layers
        .last()
        .ok_or_else(|| invalid("fixed-dual route has no layers"))?
        .z_mm;
    let safe = project_xyz6_point_mm([0.0, 0.0, top + job.transition_lift_mm])?[2];
    if safe <= top.next_up() || !safe.is_finite() {
        return Err(invalid(
            "fixed-dual safe plane collapses at command precision",
        ));
    }
    Ok(safe)
}
pub(crate) fn waste_points(
    job: &FixedDualJob,
    tool: u8,
    z: f64,
) -> Result<[[f64; 3]; 2], ManufacturingError> {
    let line = job.tools[usize::from(tool)].waste_line_xy_mm;
    Ok([
        project_xyz6_point_mm([line[0][0], line[0][1], z])?,
        project_xyz6_point_mm([line[1][0], line[1][1], z])?,
    ])
}
fn waste_bounds(
    job: &FixedDualJob,
    tool: usize,
    width: f64,
    top: f64,
    bed: f64,
) -> Result<AabbMm, ManufacturingError> {
    let line = waste_points(job, tool as u8, top)?;
    let decimal_error =
        line.iter()
            .flat_map(|point| &point[..2])
            .try_fold(0.0f64, |bound, coordinate| {
                Ok::<_, ManufacturingError>(bound.max(xyz6_coordinate_error_mm(*coordinate)?))
            })?;
    let radius = (width * 0.5 + decimal_error).next_up();
    let mut bounds = swept(
        line[0],
        line[1],
        [-radius, -radius, 0.0],
        [radius, radius, 0.0],
    );
    bounds.min[2] = bed;
    bounds.max[2] = top.next_up();
    Ok(bounds)
}
pub(crate) fn validate_plan(
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    job: &FixedDualJob,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    job.validate(&intent.printer)?;
    if !matches!(intent.recipe.process.retraction, RetractionPolicy::Disabled)
        || !matches!(intent.recipe.supports, SupportPolicy::Disabled)
        || !matches!(intent.recipe.sparse_infill, SparseInfillPolicy::Fine)
    {
        return Err(invalid(
            "fixed-dual route requires disabled body retraction/support and fine layers",
        ));
    }
    let safe = safe_z(plan, job)?;
    if safe > intent.printer.build_envelope.max[2]
        || project_xyz6_point_mm(job.park_position_mm)?[2] < safe
    {
        return Err(invalid(
            "fixed-dual safe plane and final park must clear all model and waste",
        ));
    }
    let bed = intent.printer.build_envelope.min[2];
    let top = plan.layers.last().unwrap().z_mm;
    let waste = [
        waste_bounds(job, 0, intent.recipe.bead_width_mm, top, bed)?,
        waste_bounds(job, 1, intent.recipe.bead_width_mm, top, bed)?,
    ];
    if waste.iter().any(|bounds| {
        (0..2).any(|axis| {
            bounds.min[axis] < intent.printer.build_envelope.min[axis]
                || bounds.max[axis] > intent.printer.build_envelope.max[axis]
        })
    }) {
        return Err(invalid(
            "complete repeated waste bead must fit within the declared bed footprint",
        ));
    }
    if overlaps(waste[0], waste[1])
        || waste.iter().any(|w| {
            job.assignments
                .iter()
                .any(|a| overlaps(*w, a.work_envelope_mm))
        })
    {
        return Err(invalid(
            "full repeated waste beads must be disjoint from both source reservations and each other",
        ));
    }
    let mut count = 0u32;
    let mut previous = bed;
    for layer in &plan.layers {
        check_cancel(cancel)?;
        if layer.lower_z_mm != previous {
            return Err(invalid(
                "fixed-dual waste requires contiguous bed-rooted actual intervals",
            ));
        }
        previous = layer.z_mm;
        let height = layer.z_mm - layer.lower_z_mm;
        let width = intent.recipe.bead_width_mm;
        let mut present = [false; 2];
        for path in &layer.paths {
            check_cancel(cancel)?;
            if path.physical_z_mm != [layer.lower_z_mm, layer.z_mm]
                || matches!(
                    path.kind,
                    DepositionKind::Support
                        | DepositionKind::SupportInterface
                        | DepositionKind::SeparatorWall
                )
            {
                return Err(invalid(
                    "fixed-dual model paths must be unsupported fine actual-layer beads",
                ));
            }
            present[usize::from(job.tool_for_path(path)?)] = true;
            count = count
                .checked_add((path.points_mm.len() - 1) as u32)
                .ok_or_else(|| invalid("fixed-dual segment count overflow"))?;
        }
        if present == [false, false] {
            return Err(invalid(
                "every actual layer must retain model work; both loaded tools own supported waste",
            ));
        }
        for (index, tool) in job.tools.iter().enumerate() {
            let limits = &tool.deposition_limits;
            if width < height
                || width < limits.min_bead_width_mm
                || width > limits.max_bead_width_mm
                || height < limits.min_bead_height_mm
                || height > limits.max_bead_height_mm
            {
                return Err(invalid(
                    "fixed-dual waste bead exceeds a loaded tool deposition envelope",
                ));
            }
            let points = waste_points(job, index as u8, layer.z_mm)?;
            let length = (points[1][0] - points[0][0]).hypot(points[1][1] - points[0][1]);
            let flow = intent.recipe.flow_multiplier
                * if layer.index == 0 {
                    intent.recipe.process.first_layer_flow_factor
                } else {
                    1.0
                };
            let volume = length
                * ((width - height) * height + std::f64::consts::PI * (height * 0.5).powi(2))
                * flow;
            if length < program_coordinates::MIN_DEPOSITION_COMMAND_MM
                || !volume.is_finite()
                || volume < tool.minimum_reprime_volume_mm3
            {
                return Err(invalid(
                    "actual supported waste stroke cannot establish declared reprime volume",
                ));
            }
        }
        count = count
            .checked_add(2)
            .ok_or_else(|| invalid("fixed-dual segment count overflow"))?;
    }
    if count > MAX_DEPOSITION_SEGMENTS
        || plan.layers.len().saturating_mul(2) > MAX_TOOL_SELECTIONS as usize
    {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "model/waste or tool selection budget exceeded",
        ));
    }
    Ok(())
}

/// Check actual selected-tip segment, both nozzle centers and carriage body.
/// Low coplanar inactive tips are excluded from full source/waste reservations;
/// higher horizontal motion uses the one declared all-layer clearance plane.
pub(crate) fn check_motion(
    from: [f64; 3],
    to: [f64; 3],
    tool: u8,
    built_top: f64,
    intent: &ManufacturingIntent,
    job: &FixedDualJob,
) -> Result<(), ManufacturingError> {
    let offset = job.tools[usize::from(tool)].offset_xy_mm;
    let carriage = |point: [f64; 3]| [point[0] - offset[0], point[1] - offset[1], point[2]];
    let a = carriage(from);
    let b = carriage(to);
    let rounding = from
        .into_iter()
        .chain(to)
        .try_fold(0.0f64, |bound, coordinate| {
            Ok::<_, ManufacturingError>(bound.max(xyz6_coordinate_error_mm(coordinate)?))
        })?;
    let pad = (rounding
        + f64::EPSILON * a.into_iter().chain(b).fold(1.0f64, |m, x| m.max(x.abs())) * 16.0)
        .next_up();
    for point in [from, to] {
        if (0..3).any(|axis| {
            point[axis] < intent.printer.build_envelope.min[axis]
                || point[axis] > intent.printer.build_envelope.max[axis]
        }) {
            return Err(invalid("selected nozzle exceeds actual command envelope"));
        }
    }
    for point in [a, b] {
        if (0..3).any(|axis| {
            point[axis] < job.carriage_travel_bounds_mm.min[axis]
                || point[axis] > job.carriage_travel_bounds_mm.max[axis]
        }) {
            return Err(invalid("fixed carriage exceeds declared travel envelope"));
        }
    }
    let body = swept(
        a,
        b,
        job.carriage_body_bounds_mm.min.map(|x| x - pad),
        job.carriage_body_bounds_mm.max.map(|x| x + pad),
    );
    let inactive = &job.tools[1 - usize::from(tool)];
    let radius = job.inactive_nozzle_radius_mm + pad;
    let tip = swept(
        a,
        b,
        [
            inactive.offset_xy_mm[0] - radius,
            inactive.offset_xy_mm[1] - radius,
            -pad,
        ],
        [
            inactive.offset_xy_mm[0] + radius,
            inactive.offset_xy_mm[1] + radius,
            pad,
        ],
    );
    let bed = intent.printer.build_envelope.min[2];
    for mut obstacle in job.assignments.iter().map(|a| a.work_envelope_mm).chain([
        waste_bounds(job, 0, intent.recipe.bead_width_mm, built_top, bed)?,
        waste_bounds(job, 1, intent.recipe.bead_width_mm, built_top, bed)?,
    ]) {
        obstacle.max[2] = obstacle.max[2].min(built_top.next_up());
        if obstacle.min[2] <= obstacle.max[2]
            && (overlaps(tip, obstacle) || overlaps(body, obstacle))
        {
            return Err(invalid(
                "inactive coplanar nozzle or carriage body cannot clear deposited source/waste reservation",
            ));
        }
    }
    // Fixtures have no admitted route-specific swept-body contract yet.
    if intent
        .printer
        .components
        .iter()
        .any(|component| component.role == MachineComponentRole::Fixture)
    {
        return Err(invalid(
            "fixed-dual route does not admit fixture components",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn swept_bounds_include_offset_body_and_touch() {
        let sweep = swept(
            [1.0, 2.0, 3.0],
            [5.0, 4.0, 3.0],
            [-2.0, -1.0, 0.2],
            [2.0, 1.0, 1.0],
        );
        assert!(sweep.min[0] < -1.0 && sweep.max[0] > 7.0);
        assert!(overlaps(
            sweep,
            AabbMm {
                min: [7.0, 2.0, 3.5],
                max: [8.0, 3.0, 4.0]
            }
        ));
        assert!(!overlaps(
            sweep,
            AabbMm {
                min: [8.0, 2.0, 3.5],
                max: [9.0, 3.0, 4.0]
            }
        ));
    }
    #[test]
    fn inactive_nozzle_sweep_catches_obstacle_between_clear_endpoints() {
        let obstacle = AabbMm {
            min: [14.0, 0.0, 0.0],
            max: [16.0, 2.0, 4.0],
        };
        let offset_min = [9.8, -0.2, 0.0];
        let offset_max = [10.2, 0.2, 0.0];
        let start = [0.0, 1.0, 4.0];
        let end = [10.0, 1.0, 4.0];
        assert!(!overlaps(
            swept(start, start, offset_min, offset_max),
            obstacle
        ));
        assert!(!overlaps(swept(end, end, offset_min, offset_max), obstacle));
        assert!(overlaps(
            swept(start, end, offset_min, offset_max),
            obstacle
        ));
    }
    #[test]
    fn whole_layer_safe_plane_clears_coplanar_tip_but_low_pass_does_not() {
        let obstacle = AabbMm {
            min: [14.0, 0.0, 0.0],
            max: [16.0, 2.0, 4.0],
        };
        let offset_min = [9.8, -0.2, -0.000001];
        let offset_max = [10.2, 0.2, 0.000001];
        assert!(!overlaps(
            swept([0.0, 1.0, 5.0], [10.0, 1.0, 5.0], offset_min, offset_max),
            obstacle
        ));
        assert!(overlaps(
            swept([0.0, 1.0, 3.0], [10.0, 1.0, 3.0], offset_min, offset_max),
            obstacle
        ));
    }
}
