// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Final plan check of the printer's bed exclusion regions: every deposited
//! bead (model, support, skirt, brim) stays out of each region above its
//! ceiling. Measured on the emitted XYZ6 coordinates with each edge's own
//! width; an edge the printer receives as part of a circular move is widened
//! by its arc's sagitta over the chord. Layout keeps parts away and the
//! adhesion planner leaves out skirt/brim loops that would enter a region;
//! anything still inside refuses with the region named.

use crate::{check_cancel, error};
use spiling_contracts::manufacturing::program_coordinates::project_xyz6_point_mm;
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

pub(super) fn check(
    plan: &NormalizedPrintPlan,
    printer: &PrinterSpecification,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if printer.bed_exclusions.is_empty() {
        return Ok(());
    }
    for layer in &plan.layers {
        check_cancel(cancel)?;
        for path in &layer.paths {
            let top = path.physical_z_mm[1];
            if printer
                .bed_exclusions
                .iter()
                .all(|x| top <= x.max_material_z_mm)
            {
                continue;
            }
            for (index, edge) in path.checked_edges()?.enumerate() {
                let a = project_xyz6_point_mm(edge.points_mm[0])?;
                let b = project_xyz6_point_mm(edge.points_mm[1])?;
                let bulge = if printer.capabilities.arc_moves {
                    path.arcs
                        .iter()
                        .find(|run| (run.first as usize..run.last as usize).contains(&index))
                        .map_or(0.0, |run| {
                            let step = run.step_radians(edge.points_mm[0], edge.points_mm[1]);
                            run.radius_mm(&path.points_mm) * (1.0 - (0.5 * step).cos())
                        })
                } else {
                    0.0
                };
                let radius = 0.5 * edge.width_mm + bulge;
                let z = a[2].max(b[2]);
                if let Some(x) = printer
                    .bed_exclusions
                    .iter()
                    .find(|x| !x.admits_bead([a[0], a[1]], [b[0], b[1]], radius, z))
                {
                    return Err(error(
                        ManufacturingErrorCode::UnsupportedGeometry,
                        format!(
                            "{:?} material at Z {z} mm enters the printer's bed exclusion region {} (material allowed up to Z {} mm); move the part away from it",
                            path.kind, x.id, x.max_material_z_mm
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}
