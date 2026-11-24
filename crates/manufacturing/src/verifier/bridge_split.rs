// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Split Bridge rows. A straight skin or solid row whose bridged span is
//! printed as Bridge and whose remainder keeps its role is a chain of
//! collinear same-owner strokes meeting end to end. Each Bridge piece must
//! independently cross a genuine two-ended gap of the previous layer's
//! actual strokes, anchored on its own ends; each piece joined to it in the
//! supported role must be supported throughout. Checked from actual XYZ6
//! endpoints after the emitted-byte/path bijection.
use super::{cancelled, failed, project_xyz6_point_mm};
use crate::planner::support::{
    AnchorCandidate, AnchorWorkspace, GapKind, PreparedAnchorPaths, gap_kind,
};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

fn charge(work: &mut usize, n: usize) -> Result<(), ManufacturingError> {
    *work = work.checked_add(n).ok_or_else(|| {
        ManufacturingError::new(
            ManufacturingErrorCode::ResourceLimit,
            "split bridge verification work exceeded",
        )
    })?;
    Ok(())
}

/// The supported role a split piece of this owner keeps, if any.
fn kept_role(layer: &PrintLayer, owner: Option<DepositionOwner>) -> Option<DepositionKind> {
    match owner? {
        DepositionOwner::MandatorySkin { skin } => Some(match skin {
            ProcessSkinKind::TopSkin => DepositionKind::TopSkin,
            ProcessSkinKind::BottomSkin => DepositionKind::BottomSkin,
        }),
        DepositionOwner::Regional { region_index }
            if layer
                .region_coverage
                .process_regions
                .get(usize::from(region_index))
                .is_some_and(|region| region.fraction == 1.0) =>
        {
            Some(DepositionKind::SolidFill)
        }
        _ => None,
    }
}

fn straight(path: &DepositionPath) -> Option<[[f64; 3]; 2]> {
    (path.points_mm.len() == 2).then(|| [path.points_mm[0], path.points_mm[1]])
}

/// Collinear by exact cross product of the nominal row directions, sharing
/// a nominal endpoint bit for bit.
fn joined(a: [[f64; 3]; 2], b: [[f64; 3]; 2]) -> bool {
    let shared = a.iter().any(|p| b.iter().any(|q| p[..2] == q[..2]));
    let da = [a[1][0] - a[0][0], a[1][1] - a[0][1]];
    let db = [b[1][0] - b[0][0], b[1][1] - b[0][1]];
    let cross = da[0] * db[1] - da[1] * db[0];
    let scale = da[0].hypot(da[1]) * db[0].hypot(db[1]);
    shared && cross.abs() <= super::arithmetic_slack(scale) && da[0] * db[0] + da[1] * db[1] != 0.0
}

pub(super) fn verify(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut checked: Vec<bool> = Vec::new();
    for (index, layer) in plan.layers.iter().enumerate().skip(1) {
        cancelled(cancel)?;
        if !layer
            .paths
            .iter()
            .any(|p| p.kind == DepositionKind::Bridge && kept_role(layer, p.owner).is_some())
        {
            continue;
        }
        checked.clear();
        checked
            .try_reserve(layer.paths.len())
            .map_err(|_| failed("split bridge scratch allocation failed"))?;
        checked.resize(layer.paths.len(), false);
        // The previous layer's model paths, prepared once for this layer.
        let mut lower: Option<PreparedAnchorPaths<'_>> = None;
        for (bridge_index, bridge) in layer.paths.iter().enumerate() {
            cancelled(cancel)?;
            let Some(role) = (bridge.kind == DepositionKind::Bridge)
                .then(|| kept_role(layer, bridge.owner))
                .flatten()
            else {
                continue;
            };
            let Some(bridge_ends) = straight(bridge) else {
                continue;
            };
            let mut split = false;
            for (piece_index, piece) in layer.paths.iter().enumerate() {
                charge(work, 1)?;
                if piece.kind != role || piece.owner != bridge.owner {
                    continue;
                }
                let Some(piece_ends) = straight(piece) else {
                    continue;
                };
                if !joined(bridge_ends, piece_ends) {
                    continue;
                }
                split = true;
                if !checked[piece_index] {
                    checked[piece_index] = true;
                    let lower = prepared_lower(&mut lower, plan, index, cancel)?;
                    supported(
                        plan, index, piece, piece_ends, recipe, lower, work, anchors, cancel,
                    )?;
                }
            }
            if split && !checked[bridge_index] {
                checked[bridge_index] = true;
                let width = super::constant_width(bridge)?;
                if bridge.physical_z_mm != [layer.lower_z_mm, layer.z_mm] {
                    return Err(failed(
                        "split Bridge piece has no ordinary physical interval",
                    ));
                }
                let actual = [
                    project_xyz6_point_mm(bridge_ends[0])?,
                    project_xyz6_point_mm(bridge_ends[1])?,
                ];
                let lower = prepared_lower(&mut lower, plan, index, cancel)?;
                super::interfaces::verify_bridge_lower_prepared(
                    plan, index, actual, width, recipe, true, lower, work, anchors, cancel,
                )?;
            }
        }
    }
    Ok(())
}

fn prepared_lower<'a, 'p>(
    slot: &'a mut Option<PreparedAnchorPaths<'p>>,
    plan: &'p NormalizedPrintPlan,
    index: usize,
    cancel: &AtomicBool,
) -> Result<&'a PreparedAnchorPaths<'p>, ManufacturingError> {
    if slot.is_none() {
        *slot = Some(PreparedAnchorPaths::new(
            plan.layers[index - 1]
                .paths
                .iter()
                .filter(|p| p.kind.is_model()),
            cancel,
        )?);
    }
    Ok(slot.as_ref().expect("prepared lower layer"))
}

#[expect(
    clippy::too_many_arguments,
    reason = "A supported piece is checked against the plan, recipe and shared anchor workspace"
)]
fn supported(
    plan: &NormalizedPrintPlan,
    index: usize,
    piece: &DepositionPath,
    ends: [[f64; 3]; 2],
    recipe: &PlanarPrintRecipe,
    lower: &PreparedAnchorPaths<'_>,
    work: &mut usize,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let layer = &plan.layers[index];
    let previous = &plan.layers[index - 1];
    if piece.physical_z_mm != [layer.lower_z_mm, layer.z_mm]
        || previous.z_mm != piece.physical_z_mm[0]
    {
        return Err(failed(
            "split supported piece crosses a physical lower-deposition gap",
        ));
    }
    let width = super::constant_width(piece)?;
    let actual = [
        project_xyz6_point_mm(ends[0])?,
        project_xyz6_point_mm(ends[1])?,
    ];
    let length = (actual[1][0] - actual[0][0]).hypot(actual[1][1] - actual[0][1]);
    for gap in anchors.gaps_prepared(
        AnchorCandidate {
            endpoints_mm: actual,
            bead_width_mm: width,
        },
        lower,
        work,
        cancel,
    )? {
        if !matches!(
            gap_kind(
                gap,
                (gap.end - gap.start) * length,
                width,
                recipe.process.max_bridge_span_mm
            ),
            GapKind::Supported
        ) {
            return Err(failed(format!(
                "split {:?} piece beside a Bridge is not supported by physically earlier actual strokes: layer {index}, {actual:?}",
                piece.kind
            )));
        }
    }
    Ok(())
}
