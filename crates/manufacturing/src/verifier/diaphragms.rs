// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Called only after the independent emitted-byte/path bijection. Projected
//! positions below therefore identify decoded motion, not a second motion stream.
use super::{cancelled, failed, project_xyz6_point_mm};
use crate::planner::support::{AnchorCandidate, AnchorWorkspace, GapKind, endpoint_bond, gap_kind};
use geo::{Coord, Line, LineString, MultiPolygon, Polygon, Relate};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

fn charge(work: &mut usize, count: usize) -> Result<(), ManufacturingError> {
    *work = work.checked_add(count).filter(|n| *n <= 128_000_000).ok_or_else(||
        ManufacturingError::new(ManufacturingErrorCode::ResourceLimit, "diaphragm emitted-geometry work limit exceeded"))?;
    Ok(())
}
fn ring(points: &[[f64; 2]]) -> LineString<f64> {
    LineString(points.iter().map(|p| Coord { x: p[0], y: p[1] }).collect())
}
fn wall_attachment(layer: &PrintLayer, owner: u16, rooted: &mut Vec<bool>, work: &mut usize, cancel: &AtomicBool) -> Result<(), ManufacturingError> {
    let owns = |path: &DepositionPath| path.owner == Some(DepositionOwner::Regional { region_index: owner });
    cancelled(cancel)?;
    charge(work, layer.paths.len())?;
    rooted.clear();
    rooted.try_reserve_exact(layer.paths.len()).map_err(|_| ManufacturingError::new(
        ManufacturingErrorCode::ResourceLimit, "diaphragm contact workspace allocation refused"))?;
    rooted.resize(layer.paths.len(), false);
    let mut remaining = layer.paths.iter().filter(|path| owns(path)).count();
    while remaining > 0 {
        let before = remaining;
        charge(work, layer.paths.len())?;
        for (index, path) in layer.paths.iter().enumerate().filter(|(_, path)| owns(path)) {
            cancelled(cancel)?;
            if rooted[index] { continue; }
            charge(work, layer.paths.len())?;
            let endpoints = [project_xyz6_point_mm(path.points_mm[0])?,
                project_xyz6_point_mm(*path.points_mm.last().expect("validated path"))?];
            let mut connected = false;
            for (witness_index, witness) in layer.paths.iter().enumerate() {
                cancelled(cancel)?;
                if witness_index == index || !((witness_index < index && witness.kind == DepositionKind::Perimeter)
                    || (owns(witness) && rooted[witness_index])) { continue; }
                for edge in witness.points_mm.windows(2) {
                    cancelled(cancel)?;
                    charge(work, 2)?;
                    let segment = [project_xyz6_point_mm(edge[0])?, project_xyz6_point_mm(edge[1])?];
                    for endpoint in endpoints {
                        connected |= endpoint_bond(endpoint, segment, (path.bead_width_mm + witness.bead_width_mm) * 0.5)?;
                    }
                    if connected { break; }
                }
                if connected { break; }
            }
            if connected { rooted[index] = true; remaining -= 1; }
        }
        if before == remaining {
            return Err(failed("permanent plate has no bounded actual exterior/hole-wall attachment chain"));
        }
    }
    Ok(())
}
fn lower(path: &DepositionPath, previous: &PrintLayer, recipe: &PlanarPrintRecipe,
    anchors: &mut AnchorWorkspace, work: &mut usize, cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if previous.z_mm != path.physical_z_mm[0] {
        return Err(failed("permanent diaphragm or upper landing crosses an actual deposition gap"));
    }
    let mut genuine = false;
    for edge in path.points_mm.windows(2) {
        cancelled(cancel)?;
        let endpoints = [project_xyz6_point_mm(edge[0])?, project_xyz6_point_mm(edge[1])?];
        let length = (endpoints[1][0] - endpoints[0][0]).hypot(endpoints[1][1] - endpoints[0][1]);
        let actual = previous.paths.iter().filter(|p|
            !matches!(p.kind, DepositionKind::Support | DepositionKind::SupportInterface)
                && p.physical_z_mm[1] == path.physical_z_mm[0])
            .flat_map(|p| p.points_mm.windows(2).map(move |edge| (
                project_xyz6_point_mm(edge[0]).and_then(|a| project_xyz6_point_mm(edge[1]).map(|b| [a, b])),
                true, p.bead_width_mm)));
        for gap in anchors.gaps_edges(AnchorCandidate { endpoints_mm: endpoints, bead_width_mm: path.bead_width_mm },
            actual, work, cancel)? {
            match gap_kind(gap, (gap.end - gap.start) * length, path.bead_width_mm, recipe.process.max_bridge_span_mm) {
                GapKind::Supported => {},
                GapKind::Bridge if path.kind == DepositionKind::Bridge => genuine = true,
                _ => return Err(failed("actual permanent diaphragm or upper landing lacks direct lower anchors or bounded genuine Bridge motion")),
            }
        }
    }
    if path.kind == DepositionKind::Bridge && !genuine {
        return Err(failed("permanent diaphragm Bridge label has no genuine actual lower gap"));
    }
    Ok(())
}

pub(super) fn verify(plan: &NormalizedPrintPlan, recipe: &PlanarPrintRecipe,
    work: &mut usize, anchors: &mut AnchorWorkspace, rooted: &mut Vec<bool>, cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    for (index, layer) in plan.layers.iter().enumerate() {
        for plate in &layer.region_coverage.process_diaphragms {
            if layer.lower_z_mm != plate.physical_z_mm[0] { continue; }
            cancelled(cancel)?;
            let points = plate.extent.iter().map(|p| p.outer.len() + p.holes.iter().map(Vec::len).sum::<usize>()).sum::<usize>();
            // Conservatively admit the complete filled-polygon sweep/intersection
            // inventory for each exact relation, as in producer construction.
            let relation_work = points.saturating_mul(points);
            let extent = MultiPolygon(plate.extent.iter().map(|p|
                Polygon::new(ring(&p.outer), p.holes.iter().map(|h| ring(h)).collect())).collect());
            let after = plan.layers.partition_point(|candidate| candidate.lower_z_mm < plate.physical_z_mm[1]);
            for covered_index in index..after {
                let covered = &plan.layers[covered_index];
                let owned = covered.region_coverage.process_diaphragms.iter().find(|candidate|
                    candidate.occurrence_id == plate.occurrence_id && candidate.provenance == plate.provenance)
                    .ok_or_else(|| failed("actual diaphragm omits a scheduled construction owner"))?;
                wall_attachment(covered, owned.region_index, rooted, work, cancel)?;
                for path in covered.paths.iter().filter(|path|
                    path.owner == Some(DepositionOwner::Regional { region_index: owned.region_index })) {
                    lower(path, &plan.layers[covered_index - 1], recipe, anchors, work, cancel)?;
                    for edge in path.points_mm.windows(2) {
                        cancelled(cancel)?;
                        charge(work, relation_work)?;
                        let a = project_xyz6_point_mm(edge[0])?;
                        let b = project_xyz6_point_mm(edge[1])?;
                        if !extent.relate(&Line::new(Coord { x: a[0], y: a[1] }, Coord { x: b[0], y: b[1] })).is_covers() {
                            return Err(failed(format!("actual permanent plate stroke escapes its original raw owner or crosses a protected hole: layer={}, kind={:?}, owner={:?}, projected={a:?}..{b:?}",
                                covered.index, path.kind, path.owner)));
                        }
                    }
                }
                for path in covered.paths.iter().filter(|path|
                    matches!(path.kind, DepositionKind::SparseFill | DepositionKind::SolidFill | DepositionKind::Bridge)
                        && path.owner != Some(DepositionOwner::Regional { region_index: owned.region_index })) {
                    // A different ordinary owner cannot deposit inside the plate.
                    for edge in path.points_mm.windows(2) {
                        cancelled(cancel)?;
                        charge(work, relation_work)?;
                        let a = project_xyz6_point_mm(edge[0])?;
                        let b = project_xyz6_point_mm(edge[1])?;
                        if extent.relate(&Line::new(Coord { x: a[0], y: a[1] }, Coord { x: b[0], y: b[1] })).is_intersects() {
                            return Err(failed("ordinary regional deposition duplicates reserved permanent plate ownership"));
                        }
                    }
                }
            }
            let upper = &plan.layers[after];
            for path in &upper.paths {
                let Some(DepositionOwner::Regional { region_index }) = path.owner else { continue; };
                if upper.region_coverage.process_regions[usize::from(region_index)].contributions.iter().any(|c|
                    c.occurrence_id == plate.occurrence_id && c.definition_id == plate.definition_id) {
                    lower(path, &plan.layers[after - 1], recipe, anchors, work, cancel)?;
                }
            }
        }
    }
    Ok(())
}
