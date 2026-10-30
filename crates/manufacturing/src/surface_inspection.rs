// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Called only after the actual emitted-program/path bijection and full-bead
//! material verification. Counts physical layer intervals once per face; stroke
//! volume and waste never stand in for exposed area. Dimensions remain nominal.
use crate::{
    check_cancel, error,
    planner::{forward, rotate},
};
use spiling_contracts::{manufacturing::*, optical::OpticalPatch};
use std::sync::atomic::AtomicBool;

pub(crate) fn inspect(
    bundle: &ManufacturingBundle,
    cancel: &AtomicBool,
) -> Result<Vec<SurfaceColorInspection>, ManufacturingError> {
    let JobWorkflow::FixedDualMarlin { job } = &bundle.provenance.intent.printer.workflow else {
        return Ok(Vec::new());
    };
    let failed = |message: &str| error(ManufacturingErrorCode::VerificationFailed, message);
    let mut records = Vec::with_capacity(bundle.plan.material_frames.len());
    for frame in &bundle.plan.material_frames {
        check_cancel(cancel)?;
        let construction = &job.surface_constructions[usize::from(frame.construction_index)];
        let occurrence = bundle
            .provenance
            .occurrences
            .iter()
            .find(|o| o.occurrence_id == construction.occurrence_id)
            .ok_or_else(|| failed("surface inspection source occurrence is absent"))?;
        let normal = rotate(&occurrence.pose, frame.outward_normal);
        let horizontal = rotate(&occurrence.pose, frame.horizontal_direction);
        let tangent = [-normal[1], normal[0]];
        let u = if horizontal[0] * tangent[0] + horizontal[1] * tangent[1] > 0.0 {
            frame.selected_u_mm[0]
        } else {
            frame.selected_u_mm[1]
        };
        let mut source = std::array::from_fn(|axis| {
            frame.origin_mm[axis] + frame.horizontal_direction[axis] * u
        });
        source[2] = frame.source_z_mm[0];
        let patch = OpticalPatch {
            origin_mm: forward(&occurrence.pose, source),
            normal_xy: [normal[0], normal[1]],
            width_mm: frame.selected_u_mm[1] - frame.selected_u_mm[0],
            height_mm: frame.source_z_mm[1] - frame.source_z_mm[0],
        };
        patch.validate().map_err(|e| failed(&e.message))?;
        let mut cursor = patch.origin_mm[2];
        let top = cursor + patch.height_mm;
        let mut exposed = [0.0; 2];
        let mut count = 0;
        for layer in &bundle.plan.layers {
            check_cancel(cancel)?;
            let mut interval = None;
            for path in &layer.paths {
                let Some(owner) = path.material_domain else {
                    continue;
                };
                if owner.construction_index != frame.construction_index
                    || owner.role != MaterialConstructionRole::Shell
                {
                    continue;
                }
                let tool = job.tool_for_path(path)?;
                let current = (path.physical_z_mm, tool);
                if interval.is_some_and(|previous| previous != current) {
                    return Err(failed(
                        "surface layer has inconsistent shell material or physical intervals",
                    ));
                }
                interval = Some(current);
            }
            let Some(([lo, hi], tool)) = interval else {
                continue;
            };
            let lo = lo.max(patch.origin_mm[2]);
            let hi = hi.min(top);
            if hi <= lo {
                continue;
            }
            // Only scale-aware arithmetic identity; no geometric tolerance can
            // authorize a missing/overlapping material interval.
            let epsilon = 16.0 * f64::EPSILON * lo.abs().max(cursor.abs()).max(1.0);
            if (lo - cursor).abs() > epsilon {
                return Err(failed(
                    "surface exposure has a missing or overlapping physical interval",
                ));
            }
            exposed[usize::from(tool)] += hi - lo;
            cursor = hi;
            count += 1;
        }
        if count == 0 || (cursor - top).abs() > 16.0 * f64::EPSILON * top.abs().max(1.0) {
            return Err(failed(
                "surface exposure does not cover the complete realized patch",
            ));
        }
        records.push(SurfaceColorInspection {
            construction_index: frame.construction_index,
            face_id: frame.face_id.clone(),
            realized_patch: patch,
            interval_count: count,
            exposed_height_by_tool_mm: exposed,
            fractions_by_tool: exposed.map(|height| height / patch.height_mm),
            reference: None,
        });
    }
    Ok(records)
}
