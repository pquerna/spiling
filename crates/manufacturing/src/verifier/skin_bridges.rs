// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Invoked after the emitted-byte/path bijection. The middle actual path point
//! binds the sole ownership transition; each portion retains its captured domain.
use super::{cancelled, failed, project_xyz6_point_mm};
use crate::planner::support::{
    AnchorCandidate, AnchorWorkspace, GapKind, gap_kind, projected_edges,
};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

fn charge(work: &mut usize, n: usize) -> Result<(), ManufacturingError> {
    *work = work
        .checked_add(n)
        .filter(|n| *n <= 128_000_000)
        .ok_or_else(|| {
            ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "mixed bridge verification work exceeded",
            )
        })?;
    Ok(())
}

/// Reconstruct an ORIGINAL classified row, including holes, then project that
/// interval's endpoints by the sole XYZ6 contract. Classification uses its raw
/// authored row; physical source confinement separately uses the emitted row.
fn contains_portion(
    domain: &[DiaphragmPolygonMm],
    row: f64,
    a: [f64; 3],
    b: [f64; 3],
    complete_raw: bool,
    hits: &mut Vec<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let along = if a[0] == b[0] {
        1
    } else if a[1] == b[1] {
        0
    } else {
        return Ok(false);
    };
    let fixed = 1 - along;
    let required = [a[along].min(b[along]), a[along].max(b[along])];
    let actual = [
        program_coordinates::project_xyz6_coordinate_mm(required[0])?,
        program_coordinates::project_xyz6_coordinate_mm(required[1])?,
    ];
    for polygon in domain {
        cancelled(cancel)?;
        let edges = polygon.outer.len() + polygon.holes.iter().map(Vec::len).sum::<usize>();
        charge(work, edges)?;
        if edges > MAX_DEPOSITION_SEGMENTS as usize {
            return Err(failed("mixed bridge domain exceeds point limit"));
        }
        hits.clear();
        if hits.capacity() < edges {
            hits.try_reserve_exact(edges)
                .map_err(|_| failed("mixed bridge interval allocation failed"))?;
        }
        for ring in std::iter::once(&polygon.outer).chain(&polygon.holes) {
            for edge in ring.windows(2) {
                cancelled(cancel)?;
                let (c, d) = (edge[0], edge[1]);
                if (c[fixed] <= row && row < d[fixed]) || (d[fixed] <= row && row < c[fixed]) {
                    // Preserve the defined raw hatch construction's operation
                    // order. Reassociating division can move a boundary one ULP
                    // and select the other XYZ6 neighbor at a rounding tie.
                    hits.push(
                        c[along] + (row - c[fixed]) * (d[along] - c[along]) / (d[fixed] - c[fixed]),
                    );
                }
            }
        }
        charge(
            work,
            hits.len()
                .saturating_mul((usize::BITS - hits.len().max(1).leading_zeros()) as usize + 1),
        )?;
        hits.sort_unstable_by(f64::total_cmp);
        if !hits.len().is_multiple_of(2) {
            return Err(failed("mixed bridge domain row is ambiguous"));
        }
        for pair in hits.chunks_exact(2) {
            if complete_raw && [pair[0], pair[1]] != required {
                continue;
            }
            let lo = program_coordinates::project_xyz6_coordinate_mm(pair[0])?;
            let hi = program_coordinates::project_xyz6_coordinate_mm(pair[1])?;
            if lo <= actual[0] && actual[1] <= hi {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(super) fn verify(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut hits = Vec::new();
    for (index, layer) in plan.layers.iter().enumerate() {
        for path in &layer.paths {
            let Some(DepositionOwner::RegionalSkinBridge {
                region_index,
                skin,
                skin_first,
            }) = path.owner
            else {
                continue;
            };
            cancelled(cancel)?;
            let width_mm = super::constant_width(path)?;
            if index == 0
                || path.kind != DepositionKind::Bridge
                || path.points_mm.len() != 3
                || path.physical_z_mm != [layer.lower_z_mm, layer.z_mm]
            {
                return Err(failed(
                    "mixed skin bridge requires an ordinary interval and one actual transition",
                ));
            }
            let fixed = match recipe.rectilinear_axis {
                RectilinearAxisPolicy::X => 1,
                RectilinearAxisPolicy::Y => 0,
                RectilinearAxisPolicy::AlternatingXY => usize::from(layer.index.is_multiple_of(2)),
                RectilinearAxisPolicy::AlternatingYX => usize::from(!layer.index.is_multiple_of(2)),
            };
            let along = 1 - fixed;
            let raw_a = path.points_mm[0];
            let raw_transition = path.points_mm[1];
            let raw_b = path.points_mm[2];
            if path
                .points_mm
                .iter()
                .any(|point| point[fixed] != raw_a[fixed] || point[2] != layer.z_mm)
                || !(raw_a[along] < raw_transition[along] && raw_transition[along] < raw_b[along]
                    || raw_b[along] < raw_transition[along] && raw_transition[along] < raw_a[along])
            {
                return Err(failed(
                    "mixed bridge changes its authored regional row, axis or height",
                ));
            }
            let a = project_xyz6_point_mm(path.points_mm[0])?;
            let transition = project_xyz6_point_mm(path.points_mm[1])?;
            let b = project_xyz6_point_mm(path.points_mm[2])?;
            let domain = layer
                .region_coverage
                .skin_bridge_domains
                .iter()
                .find(|domain| domain.region_index == region_index && domain.skin == skin)
                .ok_or_else(|| failed("mixed bridge lost captured source-domain ownership"))?;
            let regional_end = if skin_first { b } else { a };
            let (raw_skin_end, raw_regional_end) = if skin_first {
                (raw_a, raw_b)
            } else {
                (raw_b, raw_a)
            };
            if !contains_portion(
                &domain.source_owner,
                a[fixed],
                raw_a,
                raw_b,
                false,
                &mut hits,
                work,
                cancel,
            )? || !contains_portion(
                &domain.regional,
                raw_a[fixed],
                raw_regional_end,
                raw_transition,
                true,
                &mut hits,
                work,
                cancel,
            )? || !contains_portion(
                &domain.mandatory,
                raw_a[fixed],
                raw_transition,
                raw_skin_end,
                false,
                &mut hits,
                work,
                cancel,
            )? {
                return Err(failed(format!(
                    "actual mixed bridge portions escape their classified regional/mandatory domains: layer {}, {:?} -> {:?} -> {:?}",
                    layer.index, a, transition, b
                )));
            }
            let length = (b[along] - a[along]).abs();
            let previous = &plan.layers[index - 1];
            let actual = || {
                projected_edges(
                    previous.paths.iter().filter(|lower| {
                        lower.physical_z_mm[1] == path.physical_z_mm[0]
                            && !matches!(
                                lower.kind,
                                DepositionKind::Support | DepositionKind::SupportInterface
                            )
                    }),
                    true,
                )
            };
            let core_length = (regional_end[along] - transition[along]).abs();
            let demanded_terminal = anchors
                .gaps_edges(
                    AnchorCandidate {
                        endpoints_mm: [regional_end, transition],
                        bead_width_mm: width_mm,
                    },
                    actual(),
                    work,
                    cancel,
                )?
                .iter()
                .any(|gap| {
                    gap.end == 1.0
                        && matches!(
                            gap_kind(
                                gap,
                                (gap.end - gap.start) * core_length,
                                width_mm,
                                recipe.process.max_bridge_span_mm
                            ),
                            GapKind::Demand
                        )
                });
            if !demanded_terminal {
                return Err(failed(
                    "mixed bridge has no actual demanded regional terminal",
                ));
            }
            // The captured nominal lower strokes are already bijected to actual
            // commands above. Recheck minimal nominal endpoint construction;
            // the separate whole-stroke XYZ6 proof below owns actual anchoring.
            let skin_length = (raw_skin_end[along] - raw_transition[along]).abs();
            let nominal = previous.paths.iter().filter(|lower| {
                lower.physical_z_mm[1] == path.physical_z_mm[0]
                    && !matches!(
                        lower.kind,
                        DepositionKind::Support | DepositionKind::SupportInterface
                    )
            });
            let nearest = anchors.gaps(
                AnchorCandidate {
                    endpoints_mm: [raw_transition, raw_skin_end],
                    bead_width_mm: width_mm,
                },
                nominal,
                std::iter::empty(),
                work,
                cancel,
            )?;
            if nearest.len() != 1
                || nearest[0].start != 0.0
                || (1.0 - nearest[0].end) * skin_length
                    > super::arithmetic_slack(
                        raw_transition[along].abs().max(raw_skin_end[along].abs()),
                    )
            {
                return Err(failed(
                    "mixed bridge extends beyond its nearest nominal lower capsule anchor",
                ));
            }
            let mut genuine = false;
            for gap in anchors.gaps_edges(
                AnchorCandidate {
                    endpoints_mm: [a, b],
                    bead_width_mm: width_mm,
                },
                actual(),
                work,
                cancel,
            )? {
                match gap_kind(
                    gap,
                    (gap.end - gap.start) * length,
                    width_mm,
                    recipe.process.max_bridge_span_mm,
                ) {
                    GapKind::Supported => {}
                    GapKind::Bridge => genuine = true,
                    _ => {
                        return Err(failed(
                            "mixed bridge lacks actual two-sided direct lower anchors",
                        ));
                    }
                }
            }
            if !genuine {
                return Err(failed(
                    "mixed bridge has no genuine actual unsupported span",
                ));
            }
            // A crossing or finite necessary joint overlap is normal. Positive
            // collinear duplicate commanded centerlines are not another joint.
            for other in &layer.paths {
                if std::ptr::eq(path, other) {
                    continue;
                }
                charge(work, other.points_mm.len())?;
                for edge in other.points_mm.windows(2) {
                    let c = project_xyz6_point_mm(edge[0])?;
                    let d = project_xyz6_point_mm(edge[1])?;
                    if c[fixed] == a[fixed]
                        && d[fixed] == a[fixed]
                        && a[along].min(b[along]).max(c[along].min(d[along]))
                            < a[along].max(b[along]).min(c[along].max(d[along]))
                    {
                        return Err(failed(
                            "mixed bridge duplicates an existing commanded stroke",
                        ));
                    }
                }
            }
        }
        for domain in &layer.region_coverage.skin_bridge_domains {
            if !layer.paths.iter().any(|path| matches!(path.owner,
                Some(DepositionOwner::RegionalSkinBridge { region_index, skin, .. }) if region_index == domain.region_index && skin == domain.skin)) {
                return Err(failed("mixed bridge domain has no actual construction"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_row_reconstruction_preserves_the_original_raw_interpolation_order() {
        let domain = [DiaphragmPolygonMm {
            outer: vec![
                [2.0, 0.0],
                [10.0, 0.0],
                [210.0, 3.0],
                [2.0, 3.0],
                [2.0, 0.0],
            ],
            holes: Vec::new(),
        }];
        let original = 10.0 + 200.0 / 3.0;
        let reassociated = 10.0 + (1.0 / 3.0) * 200.0;
        assert_ne!(original, reassociated);
        let cancel = AtomicBool::new(false);
        let mut hits = Vec::new();
        let mut work = 0;
        assert!(
            contains_portion(
                &domain,
                1.0,
                [2.0, 1.0, 2.0],
                [original, 1.0, 2.0],
                true,
                &mut hits,
                &mut work,
                &cancel
            )
            .unwrap()
        );
        assert!(
            !contains_portion(
                &domain,
                1.0,
                [2.0, 1.0, 2.0],
                [reassociated, 1.0, 2.0],
                true,
                &mut hits,
                &mut work,
                &cancel
            )
            .unwrap(),
            "even a one-ULP alternate is not the complete original classified interval"
        );
    }
}
