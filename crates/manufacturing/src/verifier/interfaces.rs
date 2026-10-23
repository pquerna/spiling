// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Geometry checks run only AFTER complete independent actual-byte/path bijection.
//! Projected stored endpoints therefore name exactly the decoded byte positions;
//! no joined flag or second allocated motion stream supplies contact authority.
use super::constant_width;
use super::{cancelled, failed, project_xyz6_point_mm};
use crate::planner::support::{
    AnchorCandidate, AnchorWorkspace, GapKind, endpoint_bond, gap_kind, projected_edges,
};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

pub(super) fn verify_projected_byte_geometry(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    for (index, layer) in plan.layers.iter().enumerate() {
        let mut affected = false;
        for band in &plan.sparse_bands {
            charge(work)?;
            if band.disposition == SparseBandDisposition::Combined
                && band.first_layer <= layer.index
                && layer.index <= band.last_layer.saturating_add(1)
            {
                affected = true;
                break;
            }
        }
        if index > 0 && affected {
            verify_earlier_fine_paths(plan, index, recipe, work, anchors, cancel)?;
        }
        for path in &layer.paths {
            if path.physical_z_mm[0] == layer.lower_z_mm {
                continue;
            }
            let band = plan
                .sparse_bands
                .iter()
                .find(|band| {
                    band.last_layer == layer.index
                        && band.owner == path.owner
                        && band.disposition == SparseBandDisposition::Combined
                        && plan.layers[band.first_layer as usize].lower_z_mm
                            == path.physical_z_mm[0]
                })
                .ok_or_else(|| failed("coarse byte association lacks its scheduled sparse band"))?;
            let previous = band
                .first_layer
                .checked_sub(1)
                .map(|i| &plan.layers[i as usize])
                .ok_or_else(|| failed("combined sparse band lacks actual lower deposition"))?;
            let mut genuine = false;
            for local in path.checked_edges()? {
                let edge = local.points_mm;
                let ends = [
                    project_xyz6_point_mm(edge[0])?,
                    project_xyz6_point_mm(edge[1])?,
                ];
                let length = (ends[1][0] - ends[0][0]).hypot(ends[1][1] - ends[0][1]);
                let edges = projected_edges(
                    previous.paths.iter().filter(|p| {
                        !matches!(
                            p.kind,
                            DepositionKind::Support | DepositionKind::SupportInterface
                        ) && p.physical_z_mm[1] == path.physical_z_mm[0]
                    }),
                    true,
                );
                for gap in anchors.gaps_edges(
                    AnchorCandidate {
                        endpoints_mm: ends,
                        bead_width_mm: local.width_mm,
                    },
                    edges,
                    work,
                    cancel,
                )? {
                    match gap_kind(
                        gap,
                        (gap.end - gap.start) * length,
                        local.width_mm,
                        recipe.process.max_bridge_span_mm,
                    ) {
                        GapKind::Supported => {}
                        GapKind::Bridge if path.kind == DepositionKind::Bridge => genuine = true,
                        _ => {
                            return Err(failed(
                                "combined sparse path lacks actual base anchors or bounded direct Bridge span",
                            ));
                        }
                    }
                }
            }
            if path.kind == DepositionKind::Bridge && !genuine {
                return Err(failed(
                    "combined RegionalBridge has no genuine gap at its actual band base",
                ));
            }
        }
        for interface in &layer.region_coverage.process_interfaces {
            cancelled(cancel)?;
            if !matches!(
                (recipe.rectilinear_axis, interface.axis),
                (RectilinearAxisPolicy::X, 0) | (RectilinearAxisPolicy::Y, 1)
            ) {
                return Err(failed(
                    "separator has no explicitly fixed compatible hatch construction",
                ));
            }
            if interface.thickness_mm != recipe.bead_width_mm
                || interface.bonding_overlap_mm != recipe.bead_width_mm * 0.5
            {
                return Err(failed(
                    "separator width/bond allowance differs from admitted physical recipe policy",
                ));
            }
            for (line_index, line) in interface.centerlines_mm.iter().enumerate() {
                let expected = line.map(|p| [p[0], p[1], layer.z_mm]);
                let mut actual = layer.paths.iter().enumerate().filter(|(_, p)| {
                    p.kind == DepositionKind::SeparatorWall
                        && p.points_mm.len() == 2
                        && ((p.points_mm[0] == expected[0] && p.points_mm[1] == expected[1])
                            || (p.points_mm[1] == expected[0] && p.points_mm[0] == expected[1]))
                });
                let (separator_index, separator) = actual.next().ok_or_else(|| {
                    failed("required open separator is absent from byte-associated deposition")
                })?;
                if actual.next().is_some() {
                    return Err(failed("separator centerline is deposited more than once"));
                }
                let ends = [
                    project_xyz6_point_mm(expected[0])?,
                    project_xyz6_point_mm(expected[1])?,
                ];
                let bonds = interface
                    .endpoint_bonds
                    .get(line_index)
                    .ok_or_else(|| failed("separator omits its actual terminal witnesses"))?;
                for (endpoint, bond) in ends.into_iter().zip(bonds) {
                    verify_terminal(
                        plan,
                        index,
                        separator_index,
                        endpoint,
                        *bond,
                        recipe,
                        work,
                        anchors,
                        cancel,
                    )?;
                }
                let previous = index
                    .checked_sub(1)
                    .map(|i| &plan.layers[i])
                    .ok_or_else(|| {
                        failed("separator first interval lacks actual lower skin/strokes")
                    })?;
                let low = layer.lower_z_mm;
                let arithmetic =
                    4. * f64::EPSILON * layer.z_mm.abs().max(previous.z_mm.abs()).max(1.);
                if (low - previous.z_mm).abs() > arithmetic {
                    return Err(failed("separator crosses a physical lower-deposition gap"));
                }
                let edges = projected_edges(
                    previous.paths.iter().filter(|p| {
                        !matches!(
                            p.kind,
                            DepositionKind::Support | DepositionKind::SupportInterface
                        )
                    }),
                    true,
                );
                let length = (ends[1][0] - ends[0][0]).hypot(ends[1][1] - ends[0][1]);
                for gap in anchors.gaps_edges(
                    AnchorCandidate {
                        endpoints_mm: ends,
                        bead_width_mm: constant_width(separator)?,
                    },
                    edges,
                    work,
                    cancel,
                )? {
                    let span = (gap.end - gap.start) * length;
                    // Only the existing actual-anchor half-bead terminal overhang
                    // is admitted; no bridging/floating internal wall or diaphragm.
                    if span > constant_width(separator)? * 0.5
                        || !((gap.start == 0. && gap.right) || (gap.end == 1. && gap.left))
                    {
                        return Err(failed(
                            "actual separator has an unsupported lower interval; no floating/roof construction authorized",
                        ));
                    }
                }
            }
            let axis = usize::from(interface.axis);
            let plane = program_coordinates::project_xyz6_coordinate_mm(interface.coordinate_mm)?;
            let half = interface.thickness_mm * 0.5;
            let mut contacts = [false; 2];
            let mut nearest: [Option<(DepositionKind, [[f64; 3]; 2])>; 2] = [None, None];
            for fill in layer.paths.iter().filter(|p| {
                matches!(
                    p.kind,
                    DepositionKind::SparseFill | DepositionKind::SolidFill
                ) || p.kind == DepositionKind::Bridge
                    && p.owner.and_then(DepositionOwner::skin_kind).is_none()
                    && p.owner != Some(DepositionOwner::Wall)
            }) {
                let regional = match fill.owner {
                    Some(DepositionOwner::Regional { region_index }) => Some(region_index),
                    _ => None,
                };
                let row = regional.is_some();
                if row && fill.points_mm.len() != 2 {
                    return Err(failed("regional owner is not one open row"));
                }
                if !row && fill.kind != DepositionKind::Bridge {
                    return Err(failed("regional fill has no original regional owner"));
                }
                let mut genuine = false;
                for (first, last, width_mm) in crate::planner::support::StrokeRuns::for_path(fill)?
                {
                    let raw = [
                        project_xyz6_point_mm(fill.points_mm[first])?,
                        project_xyz6_point_mm(fill.points_mm[last])?,
                    ];
                    let side = usize::from((raw[0][axis] + raw[1][axis]) * 0.5 > plane);
                    let distance = |edge: [[f64; 3]; 2]| {
                        (edge[0][axis] - plane)
                            .abs()
                            .min((edge[1][axis] - plane).abs())
                    };
                    if nearest[side].is_none_or(|(_, edge)| distance(raw) < distance(edge)) {
                        nearest[side] = Some((fill.kind, raw));
                    }
                    for line in &interface.centerlines_mm {
                        charge(work)?;
                        if enters_finite_band(raw, projected(*line, layer.z_mm)?, axis, half) {
                            return Err(failed(format!(
                                "actual deposition duplicates finite separator ownership at layer {index}, kind {:?}, stroke {raw:?}",
                                fill.kind
                            )));
                        }
                    }
                    if !row {
                        genuine |= verify_bridge_lower(
                            plan, index, raw, width_mm, recipe, false, work, anchors, cancel,
                        )?;
                        continue;
                    }
                    let parallel = raw[0][axis] == raw[1][axis];
                    let mut connected = false;
                    for endpoint in raw {
                        for line in &interface.centerlines_mm {
                            charge(work)?;
                            connected |= endpoint_bond(
                                endpoint,
                                projected(*line, layer.z_mm)?,
                                (width_mm + interface.thickness_mm) * 0.5,
                            )?;
                        }
                    }
                    if parallel && fill.kind == DepositionKind::Bridge {
                        let along = 1 - axis;
                        for line in &interface.centerlines_mm {
                            charge(work)?;
                            let edge = projected(*line, layer.z_mm)?;
                            let low = raw[0][along]
                                .min(raw[1][along])
                                .max(edge[0][along].min(edge[1][along]));
                            let high = raw[0][along]
                                .max(raw[1][along])
                                .min(edge[0][along].max(edge[1][along]));
                            if low < high {
                                let mut interior = raw[0];
                                interior[along] = low + (high - low) * 0.5;
                                connected |= endpoint_bond(
                                    interior,
                                    edge,
                                    (width_mm + interface.thickness_mm) * 0.5,
                                )?;
                            }
                        }
                    }
                    if connected {
                        let region_index = regional.ok_or_else(|| {
                            failed("joining stroke has no regional generation owner")
                        })?;
                        if region_index != interface.region_indices[side] {
                            return Err(failed(
                                "joining stroke belongs to another effective interface region",
                            ));
                        }
                        let region = layer
                            .region_coverage
                            .process_regions
                            .get(usize::from(region_index))
                            .ok_or_else(|| {
                                failed("joining region index is outside layer coverage")
                            })?;
                        if !region.contributions.iter().any(|c| {
                            c.occurrence_id == interface.occurrence_id
                                && c.definition_id == interface.definition_id
                        }) {
                            return Err(failed("joining region belongs to another occurrence"));
                        }
                        if parallel {
                            if fill.kind != DepositionKind::Bridge {
                                return Err(failed(
                                    "parallel ordinary fill has no genuine recovery construction",
                                ));
                            }
                            let lower_index = if fill.physical_z_mm[0] == layer.lower_z_mm {
                                index
                            } else {
                                plan.sparse_bands
                                    .iter()
                                    .find(|band| {
                                        band.last_layer == layer.index
                                            && band.owner == fill.owner
                                            && band.disposition == SparseBandDisposition::Combined
                                            && plan.layers[band.first_layer as usize].lower_z_mm
                                                == fill.physical_z_mm[0]
                                    })
                                    .map(|band| band.first_layer as usize)
                                    .ok_or_else(|| {
                                        failed("joining coarse Bridge lost its actual band base")
                                    })?
                            };
                            verify_bridge_lower(
                                plan,
                                lower_index,
                                raw,
                                width_mm,
                                recipe,
                                true,
                                work,
                                anchors,
                                cancel,
                            )?;
                        }
                        contacts[side] = true;
                    }
                }
                if !row && !genuine {
                    return Err(failed(
                        "nonregional Bridge path has no genuine actual supported span",
                    ));
                }
            }
            for path in layer.paths.iter().filter(|p| {
                p.owner.and_then(DepositionOwner::skin_kind).is_some() && p.points_mm.len() == 2
            }) {
                let a = project_xyz6_point_mm(path.points_mm[0])?;
                let b = project_xyz6_point_mm(path.points_mm[1])?;
                if a[axis] != b[axis] || (a[axis] - plane).abs() >= half {
                    continue;
                }
                for line in &interface.centerlines_mm {
                    charge(work)?;
                    let line = projected(*line, layer.z_mm)?;
                    let along = 1 - axis;
                    if a[along].min(b[along]) < line[0][along].max(line[1][along])
                        && a[along].max(b[along]) > line[0][along].min(line[1][along])
                    {
                        return Err(failed(
                            "mandatory skin/Bridge duplicates longitudinal separator ownership",
                        ));
                    }
                }
            }
            for (side, contact) in contacts.iter_mut().enumerate() {
                if !*contact {
                    *contact = deferred_sparse_contact(plan, index, interface, side, work, cancel)?;
                }
            }
            if contacts != [true, true] {
                return Err(failed(format!(
                    "adjoining density generators have no actual contacting row on both separator sides at layer {index} Z {}: contacts {contacts:?}, centerlines {:?}, nearest {nearest:?}",
                    layer.z_mm, interface.centerlines_mm
                )));
            }
        }
    }
    Ok(())
}

/// Eventual XY bonding is not lower support: this reads only a validated
/// terminal association whose full physical interval contains the fine wall.
pub(super) fn deferred_sparse_contact(
    plan: &NormalizedPrintPlan,
    index: usize,
    interface: &ProcessInterfaceCoverage,
    side: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let layer = &plan.layers[index];
    let region = layer
        .region_coverage
        .process_regions
        .get(usize::from(interface.region_indices[side]))
        .ok_or_else(|| failed("deferred interface owner is outside fine layer"))?;
    if !region.contributions.iter().any(|contribution| {
        contribution.occurrence_id == interface.occurrence_id
            && contribution.definition_id == interface.definition_id
    }) {
        return Err(failed(
            "deferred adjoining region belongs to another occurrence",
        ));
    }
    let axis = usize::from(interface.axis);
    let plane = program_coordinates::project_xyz6_coordinate_mm(interface.coordinate_mm)?;
    for band in &plan.sparse_bands {
        cancelled(cancel)?;
        charge(work)?;
        if band.disposition != SparseBandDisposition::Combined
            || band.first_layer > layer.index
            || band.last_layer <= layer.index
        {
            continue;
        }
        let terminal = &plan.layers[band.last_layer as usize];
        let Some(DepositionOwner::Regional { region_index }) = band.owner else {
            continue;
        };
        let Some(terminal_region) = terminal
            .region_coverage
            .process_regions
            .get(usize::from(region_index))
        else {
            return Err(failed("deferred terminal owner is outside terminal layer"));
        };
        if terminal_region.fraction != region.fraction
            || terminal_region.contributions != region.contributions
        {
            continue;
        }
        for covered in &plan.layers[band.first_layer as usize..=band.last_layer as usize] {
            charge(work)?;
            if !covered
                .region_coverage
                .process_regions
                .iter()
                .any(|candidate| {
                    candidate.fraction == region.fraction
                        && candidate.contributions == region.contributions
                })
            {
                return Err(failed(
                    "deferred sparse bond changes effective source/profile",
                ));
            }
        }
        let expected_interval = [
            plan.layers[band.first_layer as usize].lower_z_mm,
            terminal.z_mm,
        ];
        for path in terminal.paths.iter().filter(|path| {
            matches!(
                path.kind,
                DepositionKind::SparseFill | DepositionKind::Bridge
            ) && path.owner == band.owner
        }) {
            cancelled(cancel)?;
            charge(work)?;
            if path.physical_z_mm != expected_interval
                || path.physical_z_mm[0] > layer.lower_z_mm
                || path.physical_z_mm[1] < layer.z_mm
            {
                return Err(failed(
                    "deferred sparse association does not cover complete fine interval",
                ));
            }
            let coarse_inner =
                program_coordinates::project_xyz6_bead_inner_height_mm(path.physical_z_mm)?;
            for local in path.checked_edges()? {
                let edge = local.points_mm;
                let actual = [
                    project_xyz6_point_mm(edge[0])?,
                    project_xyz6_point_mm(edge[1])?,
                ];
                if usize::from((actual[0][axis] + actual[1][axis]) * 0.5 > plane) != side {
                    continue;
                }
                // A parallel ordinary row cannot substitute for a crossing bond.
                if actual[0][axis] == actual[1][axis] {
                    continue;
                }
                let Some(coarse_core) =
                    nominal_stadium_core_width_mm(local.width_mm, path.height_mm().next_up())
                else {
                    continue;
                };
                let mut full_height_bond = true;
                for covered in &plan.layers[band.first_layer as usize..=band.last_layer as usize] {
                    cancelled(cancel)?;
                    charge(work)?;
                    let fine_inner = program_coordinates::project_xyz6_bead_inner_height_mm([
                        covered.lower_z_mm,
                        covered.z_mm,
                    ])?;
                    if coarse_inner[0].max(fine_inner[0]) >= coarse_inner[1].min(fine_inner[1]) {
                        full_height_bond = false;
                        break;
                    }
                    let Some(fine_interface) = covered
                        .region_coverage
                        .process_interfaces
                        .iter()
                        .find(|candidate| {
                            candidate.occurrence_id == interface.occurrence_id
                                && candidate.definition_id == interface.definition_id
                                && candidate.provenance == interface.provenance
                                && candidate.axis == interface.axis
                                && candidate.coordinate_mm == interface.coordinate_mm
                                && covered
                                    .region_coverage
                                    .process_regions
                                    .get(usize::from(candidate.region_indices[side]))
                                    .is_some_and(|candidate| {
                                        candidate.fraction == region.fraction
                                            && candidate.contributions == region.contributions
                                    })
                        })
                    else {
                        full_height_bond = false;
                        break;
                    };
                    let Some(fine_core) = nominal_stadium_core_width_mm(
                        fine_interface.thickness_mm,
                        (covered.z_mm - covered.lower_z_mm).next_up(),
                    ) else {
                        full_height_bond = false;
                        break;
                    };
                    let outer = (local.width_mm + fine_interface.thickness_mm) * 0.5;
                    let core_radius_sum = ((coarse_core + fine_core) * 0.5).next_down();
                    let mut bonded = false;
                    for line in &fine_interface.centerlines_mm {
                        charge(work)?;
                        let wall = projected(*line, covered.z_mm)?;
                        if enters_finite_band(actual, wall, axis, fine_interface.thickness_mm * 0.5)
                        {
                            return Err(failed(
                                "deferred sparse stroke duplicates finite separator ownership",
                            ));
                        }
                        for endpoint in actual {
                            charge(work)?;
                            // The outer maximum-overlap gate implies the inner
                            // predicate's lower distance bound. Its strict upper
                            // bound proves positive central-core overlap.
                            bonded |= endpoint_bond(endpoint, wall, outer)?
                                && endpoint_bond(endpoint, wall, core_radius_sum)?;
                        }
                    }
                    if !bonded {
                        full_height_bond = false;
                        break;
                    }
                }
                if full_height_bond {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// Stability is time-directed. Future full-height bonding never participates in
/// this immediate-prior actual-stroke iterator.
pub(super) fn verify_earlier_fine_paths(
    plan: &NormalizedPrintPlan,
    index: usize,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let layer = &plan.layers[index];
    let previous = &plan.layers[index - 1];
    for path in layer.paths.iter().filter(|path| {
        matches!(
            path.kind,
            DepositionKind::Perimeter
                | DepositionKind::SeparatorWall
                | DepositionKind::BottomSkin
                | DepositionKind::TopSkin
                | DepositionKind::SolidFill
                | DepositionKind::SparseFill
        ) || path.kind == DepositionKind::Bridge
            && matches!(path.owner, Some(DepositionOwner::Regional { .. }))
    }) {
        cancelled(cancel)?;
        if matches!(
            path.kind,
            DepositionKind::SparseFill | DepositionKind::Bridge
        ) && path.physical_z_mm != [layer.lower_z_mm, layer.z_mm]
        {
            continue; // Its coarse lower landing is proved separately at the band's first interval.
        }
        if path.physical_z_mm != [layer.lower_z_mm, layer.z_mm]
            || previous.z_mm != path.physical_z_mm[0]
        {
            return Err(failed(
                "fine deposition lacks a contiguous physically earlier layer",
            ));
        }
        if path.kind == DepositionKind::Bridge {
            if path.points_mm.len() != 2 {
                return Err(failed(
                    "affected regional Bridge is not one complete open row",
                ));
            }
            let actual = [
                project_xyz6_point_mm(path.points_mm[0])?,
                project_xyz6_point_mm(path.points_mm[1])?,
            ];
            verify_bridge_lower(
                plan,
                index,
                actual,
                constant_width(path)?,
                recipe,
                true,
                work,
                anchors,
                cancel,
            )?;
            continue;
        }
        for local in path.checked_edges()? {
            let edge = local.points_mm;
            let actual = [
                project_xyz6_point_mm(edge[0])?,
                project_xyz6_point_mm(edge[1])?,
            ];
            let length = (actual[1][0] - actual[0][0]).hypot(actual[1][1] - actual[0][1]);
            let earlier = projected_edges(
                previous.paths.iter().filter(|path| {
                    !matches!(
                        path.kind,
                        DepositionKind::Support | DepositionKind::SupportInterface
                    ) && path.physical_z_mm[1] == layer.lower_z_mm
                }),
                true,
            );
            for gap in anchors.gaps_edges(
                AnchorCandidate {
                    endpoints_mm: actual,
                    bead_width_mm: local.width_mm,
                },
                earlier,
                work,
                cancel,
            )? {
                if !matches!(
                    gap_kind(
                        gap,
                        (gap.end - gap.start) * length,
                        local.width_mm,
                        recipe.process.max_bridge_span_mm
                    ),
                    GapKind::Supported
                ) {
                    return Err(failed(
                        "fine ordinary deposition is unsupported by physically earlier actual strokes",
                    ));
                }
            }
        }
    }
    Ok(())
}
fn charge(work: &mut usize) -> Result<(), ManufacturingError> {
    *work = work
        .checked_add(1)
        .filter(|n| *n <= 128_000_000)
        .ok_or_else(|| failed("independent interface capsule work budget exceeded"))?;
    Ok(())
}

fn projected(line: [[f64; 2]; 2], z: f64) -> Result<[[f64; 3]; 2], ManufacturingError> {
    Ok([
        project_xyz6_point_mm([line[0][0], line[0][1], z])?,
        project_xyz6_point_mm([line[1][0], line[1][1], z])?,
    ])
}
fn same_segment(a: [[f64; 3]; 2], b: [[f64; 3]; 2]) -> bool {
    a == b || a == [b[1], b[0]]
}

fn actual_wall_before(
    layer: &PrintLayer,
    before: usize,
    witness: [[f64; 3]; 2],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<f64, ManufacturingError> {
    for path in layer.paths[..before]
        .iter()
        .filter(|p| p.owner == Some(DepositionOwner::Wall))
    {
        for local in path.checked_edges()? {
            let edge = local.points_mm;
            cancelled(cancel)?;
            charge(work)?;
            let actual = [
                project_xyz6_point_mm(edge[0])?,
                project_xyz6_point_mm(edge[1])?,
            ];
            if same_segment(actual, witness) {
                return Ok(local.width_mm);
            }
        }
    }
    Err(failed(
        "separator wall witness is absent from earlier actual deposition",
    ))
}

fn verify_terminal(
    plan: &NormalizedPrintPlan,
    layer_index: usize,
    separator_index: usize,
    endpoint: [f64; 3],
    bond: ProcessEndpointBond,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let layer = &plan.layers[layer_index];
    let width = constant_width(&layer.paths[separator_index])?;
    match bond {
        ProcessEndpointBond::Wall { wall_segment_mm } => {
            let wall = projected(wall_segment_mm, layer.z_mm)?;
            let wall_width = actual_wall_before(layer, separator_index, wall, work, cancel)?;
            charge(work)?;
            if !endpoint_bond(endpoint, wall, (width + wall_width) * 0.5)? {
                return Err(failed(
                    "actual direct separator/wall contact exceeds the bounded physical overlap policy",
                ));
            }
        }
        ProcessEndpointBond::Skin {
            skin_kind,
            process_kind,
            skin_segment_mm,
            wall_segment_mm,
        } => {
            let stroke = projected(skin_segment_mm, layer.z_mm)?;
            let wall = projected(wall_segment_mm, layer.z_mm)?;
            let mut skin_index = None;
            for (index, path) in
                layer.paths[..separator_index]
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| {
                        p.kind == process_kind
                            && p.owner == Some(DepositionOwner::MandatorySkin { skin: skin_kind })
                            && p.points_mm.len() == 2
                    })
            {
                cancelled(cancel)?;
                charge(work)?;
                let actual = [
                    project_xyz6_point_mm(path.points_mm[0])?,
                    project_xyz6_point_mm(path.points_mm[1])?,
                ];
                if same_segment(actual, stroke) {
                    skin_index = Some(index);
                    break;
                }
            }
            let skin_index=skin_index.ok_or_else(||failed("mandatory continuous skin witness was omitted, relabeled, fragmented or printed after the separator"))?;
            let wall_width = actual_wall_before(layer, skin_index, wall, work, cancel)?;
            let skin_width = constant_width(&layer.paths[skin_index])?;
            charge(work)?;
            if !endpoint_bond(endpoint, stroke, (width + skin_width) * 0.5)? {
                return Err(failed(
                    "actual separator/skin contact exceeds bounded physical overlap",
                ));
            }
            let mut rooted = false;
            for skin_end in stroke {
                charge(work)?;
                rooted |= endpoint_bond(skin_end, wall, (skin_width + wall_width) * 0.5)?;
            }
            if !rooted {
                return Err(failed(
                    "actual surrounding skin is not connected to its same-occurrence wall witness",
                ));
            }
            if process_kind == DepositionKind::Bridge {
                verify_bridge_lower(
                    plan,
                    layer_index,
                    stroke,
                    skin_width,
                    recipe,
                    false,
                    work,
                    anchors,
                    cancel,
                )?;
            }
        }
    }
    Ok(())
}

pub(super) fn verify_bridge_lower(
    plan: &NormalizedPrintPlan,
    index: usize,
    stroke: [[f64; 3]; 2],
    width: f64,
    recipe: &PlanarPrintRecipe,
    require_span: bool,
    work: &mut usize,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let layer = &plan.layers[index];
    let previous = index
        .checked_sub(1)
        .map(|i| &plan.layers[i])
        .ok_or_else(|| failed("actual Bridge has no lower layer"))?;
    let arithmetic = 4. * f64::EPSILON * layer.z_mm.abs().max(previous.z_mm.abs()).max(1.);
    if (layer.lower_z_mm - previous.z_mm).abs() > arithmetic {
        return Err(failed("actual Bridge crosses a lower deposition gap"));
    }
    let edges = projected_edges(
        previous.paths.iter().filter(|p| {
            !matches!(
                p.kind,
                DepositionKind::Support | DepositionKind::SupportInterface
            )
        }),
        true,
    );
    let length = (stroke[1][0] - stroke[0][0]).hypot(stroke[1][1] - stroke[0][1]);
    let mut genuine = false;
    for gap in anchors.gaps_edges(
        AnchorCandidate {
            endpoints_mm: stroke,
            bead_width_mm: width,
        },
        edges,
        work,
        cancel,
    )? {
        match gap_kind(
            gap,
            (gap.end - gap.start) * length,
            width,
            recipe.process.max_bridge_span_mm,
        ) {
            GapKind::Supported => {}
            GapKind::Bridge => genuine = true,
            _ => {
                return Err(failed(
                    "actual Bridge lacks independently certified lower anchors/span",
                ));
            }
        }
    }
    if require_span && !genuine {
        return Err(failed(
            "parallel sparse recovery has no genuine actual Bridge span",
        ));
    }
    Ok(genuine)
}

/// Strict positive intersection with the finite actual interface rectangle.
/// Robust set predicates distinguish corner tangency from entering its interior.
pub(super) fn enters_finite_band(
    stroke: [[f64; 3]; 2],
    wall: [[f64; 3]; 2],
    axis: usize,
    half: f64,
) -> bool {
    use geo::line_intersection::{LineIntersection, line_intersection};
    use geo::{Coord, Intersects, Line, Point};
    let mut low = [wall[0][0].min(wall[1][0]), wall[0][1].min(wall[1][1])];
    let mut high = [wall[0][0].max(wall[1][0]), wall[0][1].max(wall[1][1])];
    low[axis] -= half;
    high[axis] += half;
    let inside = |p: [f64; 3]| p[0] > low[0] && p[0] < high[0] && p[1] > low[1] && p[1] < high[1];
    if stroke.into_iter().any(inside) {
        return true;
    }
    let segment = Line::new(
        Coord {
            x: stroke[0][0],
            y: stroke[0][1],
        },
        Coord {
            x: stroke[1][0],
            y: stroke[1][1],
        },
    );
    let center = Point::new(
        low[0] + (high[0] - low[0]) * 0.5,
        low[1] + (high[1] - low[1]) * 0.5,
    );
    if segment.intersects(&center) {
        return true;
    }
    let corners = [
        Coord {
            x: low[0],
            y: low[1],
        },
        Coord {
            x: high[0],
            y: low[1],
        },
        Coord {
            x: high[0],
            y: high[1],
        },
        Coord {
            x: low[0],
            y: high[1],
        },
    ];
    (0..4).any(|i| {
        matches!(
            line_intersection(segment, Line::new(corners[i], corners[(i + 1) % 4])),
            Some(LineIntersection::SinglePoint {
                is_proper: true,
                ..
            })
        )
    })
}
