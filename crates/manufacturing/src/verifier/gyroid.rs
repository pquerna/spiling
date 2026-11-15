// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Independent implicit-field and continuous-road checks after the decoded
//! program/path bijection. Does not call the analytic wave generator.

use super::{cancelled, constant_width, failed, project_xyz6_point_mm};
use crate::planner::support::{AnchorCandidate, AnchorWorkspace, PreparedAnchorPaths};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

pub(super) fn verify(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if !matches!(recipe.sparse_infill, SparseInfillPolicy::Gyroid) {
        return Ok(());
    }
    for (index, layer) in plan.layers.iter().enumerate() {
        // Borrow and bound the previous roads once per layer. Conservative path
        // bounds only skip disjoint capsules; the anchor math and work accounting
        // remain the same for every chord, including skipped edges.
        let mut prepared = None;
        for path in layer
            .paths
            .iter()
            .filter(|p| p.kind == DepositionKind::SparseFill)
        {
            cancelled(cancel)?;
            if path.physical_z_mm != [layer.lower_z_mm, layer.z_mm] || !path.arcs.is_empty() {
                return Err(failed(
                    "Gyroid requires planar fine-interval polyline deposition",
                ));
            }
            let Some(DepositionOwner::Regional { region_index }) = path.owner else {
                return Err(failed("Gyroid lacks its classified interior owner"));
            };
            let region = layer
                .region_coverage
                .process_regions
                .get(usize::from(region_index))
                .ok_or_else(|| failed("Gyroid owner is outside classified regions"))?;
            let width = constant_width(path)?;
            let equivalent_width =
                width - recipe.layer_height_mm * (1.0 - std::f64::consts::PI / 4.0);
            let frequency = 2.55577 * region.fraction / equivalent_width;
            if !frequency.is_finite() || frequency <= 0.0 {
                return Err(failed("Gyroid has no positive nominal frequency"));
            }
            let previous = index.checked_sub(1).map(|i| &plan.layers[i]);
            if previous.is_some_and(|lower| lower.z_mm != layer.lower_z_mm)
                || previous.is_none() && layer.lower_z_mm != 0.0
            {
                return Err(failed("Gyroid crosses an unreported physical layer gap"));
            }
            let mut unsupported_run = 0.0_f64;
            for edge in path.points_mm.windows(2) {
                cancelled(cancel)?;
                *work = work
                    .checked_add(1)
                    .ok_or_else(|| failed("Gyroid verification work overflow"))?;
                let a = project_xyz6_point_mm(edge[0])?;
                let b = project_xyz6_point_mm(edge[1])?;
                let length = (b[0] - a[0]).hypot(b[1] - a[1]);
                if length * frequency > 2.0 {
                    return Err(failed("Gyroid chord skips its local periodic profile"));
                }
                // Independent equation in physical build coordinates. The
                // generator uses roots/atan and clipping, not this residual.
                // |grad F| <= 2 sqrt(3); include XYZ6 projection in the same
                // recorded sparse road tolerance, not a new site epsilon.
                let allowance = 2.0 * 3.0_f64.sqrt() * frequency * SPARSE_ROAD_TOLERANCE_MM;
                for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
                    let x = (a[0] + t * (b[0] - a[0])) * frequency;
                    let y = (a[1] + t * (b[1] - a[1])) * frequency;
                    let z = layer.z_mm * frequency;
                    let residual = x.sin() * y.cos() + y.sin() * z.cos() + z.sin() * x.cos();
                    if !residual.is_finite() || residual.abs() > allowance {
                        return Err(failed(
                            "Gyroid deposition violates its physical-height field or density",
                        ));
                    }
                }
                let Some(previous) = previous else {
                    continue;
                };
                if prepared.is_none() {
                    prepared = Some(PreparedAnchorPaths::new(previous.paths.iter(), cancel)?);
                }
                let gaps = anchors.gaps_prepared(
                    AnchorCandidate {
                        endpoints_mm: [a, b],
                        bead_width_mm: width,
                    },
                    prepared.as_ref().expect("earlier paths prepared"),
                    work,
                    cancel,
                )?;
                let mut cursor = 0.0;
                for gap in gaps {
                    if gap.start > cursor {
                        unsupported_run = 0.0;
                    }
                    unsupported_run += (gap.end - gap.start) * length;
                    if unsupported_run > MAX_SPARSE_UNSUPPORTED_RUN_WIDTHS * width
                        && layer.region_coverage.unsupported_overhang_mm == 0.0
                    {
                        return Err(failed(
                            "Gyroid has an unreported continuous unsupported road",
                        ));
                    }
                    cursor = gap.end;
                }
                if cursor < 1.0 {
                    unsupported_run = 0.0;
                }
            }
        }
    }
    Ok(())
}
