// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Bridge only the bridged span. A finished straight mandatory-skin or solid
//! regional row labelled Bridge is split at its genuine two-ended gaps
//! measured against the previous layer's actual strokes: each gap plus an
//! anchor overlap on both sides stays Bridge, and the supported remainder
//! takes back its own role (top/bottom skin, solid fill), speed and fan.
//! Remainders shorter than the anchor merge into the neighbouring Bridge
//! piece. Every piece is checked with the same actual-coordinate predicates
//! the verifier applies to split rows (Bridge pieces: only supported or
//! genuine Bridge gaps, at least one Bridge; other pieces: supported only);
//! when any piece fails, the row stays whole, never refused.

use super::support::{AnchorCandidate, AnchorWorkspace, GapKind, gap_kind, projected_edges};
use crate::{check_cancel, error};
use spiling_contracts::manufacturing::program_coordinates::{
    MIN_DEPOSITION_COMMAND_MM, project_xyz6_point_mm,
};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

/// Anchor overlap on each side of a bridged gap, in bead widths. A remainder
/// shorter than this merges into the Bridge piece rather than becoming its
/// own short skin command.
pub(super) const BRIDGE_ANCHOR_WIDTHS: f64 = 2.0;

/// The role a split row's supported pieces take back, or None when the path
/// is not a splittable straight skin/solid row.
pub(super) fn supported_role(layer: &PrintLayer, path: &DepositionPath) -> Option<DepositionKind> {
    if path.kind != DepositionKind::Bridge || path.points_mm.len() != 2 || !path.arcs.is_empty() {
        return None;
    }
    match path.owner? {
        DepositionOwner::MandatorySkin {
            skin: ProcessSkinKind::TopSkin,
        } => Some(DepositionKind::TopSkin),
        DepositionOwner::MandatorySkin {
            skin: ProcessSkinKind::BottomSkin,
        } => Some(DepositionKind::BottomSkin),
        // Separators bind regional rows by endpoint; leave those layers whole.
        DepositionOwner::Regional { region_index }
            if layer.region_coverage.process_interfaces.is_empty()
                && layer
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

pub(super) fn split_bridged_rows(
    layers: &mut [PrintLayer],
    alternation: &[u32],
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut anchors = AnchorWorkspace::default();
    for index in 1..layers.len() {
        check_cancel(cancel)?;
        let (lower, upper) = layers.split_at_mut(index);
        let previous = &lower[index - 1];
        let layer = &mut upper[0];
        if layer.lower_z_mm != previous.z_mm || !layer.region_coverage.process_diaphragms.is_empty()
        {
            continue;
        }
        let mut replacement: Option<Vec<DepositionPath>> = None;
        for path_index in 0..layer.paths.len() {
            check_cancel(cancel)?;
            if let Some(role) = supported_fitted_skin(
                layer,
                &layer.paths[path_index],
                previous,
                recipe,
                alternation[index],
                &mut anchors,
                work,
                cancel,
            )? {
                layer.paths[path_index].kind = role;
            }
            let path = &layer.paths[path_index];
            let pieces = split_path(
                layer,
                path,
                previous,
                recipe,
                alternation[index],
                &mut anchors,
                work,
                cancel,
            )?;
            match (pieces, &mut replacement) {
                (Some(pieces), slot) => {
                    let out = match slot {
                        Some(out) => out,
                        None => {
                            let mut out = Vec::new();
                            out.try_reserve_exact(layer.paths.len() + 2)
                                .map_err(|_| resource())?;
                            out.extend(layer.paths[..path_index].iter().cloned());
                            slot.insert(out)
                        }
                    };
                    out.try_reserve(pieces.len()).map_err(|_| resource())?;
                    out.extend(pieces);
                }
                (None, Some(out)) => {
                    out.try_reserve(1).map_err(|_| resource())?;
                    out.push(layer.paths[path_index].clone());
                }
                (None, None) => {}
            }
        }
        if let Some(out) = replacement {
            layer.paths = out;
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "A fitted row retains its exact witness, authored axis and earlier command support"
)]
fn supported_fitted_skin(
    layer: &PrintLayer,
    path: &DepositionPath,
    previous: &PrintLayer,
    recipe: &PlanarPrintRecipe,
    course: u32,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<DepositionKind>, ManufacturingError> {
    let Some(DepositionOwner::MandatorySkin { skin }) = path.owner else {
        return Ok(None);
    };
    let [start, end] = path.points_mm.as_slice() else {
        return Ok(None);
    };
    if path.kind != DepositionKind::Bridge
        || path.physical_z_mm != [layer.lower_z_mm, layer.z_mm]
        || !path.arcs.is_empty()
        || !layer.skin_command_fits.iter().any(|domain| {
            domain.skin == skin
                && domain
                    .fits
                    .iter()
                    .any(|fit| fit.fitted_mm == [*start, *end] || fit.fitted_mm == [*end, *start])
        })
        || separator_witness(layer, path)
    {
        return Ok(None);
    }
    let role = match skin {
        ProcessSkinKind::TopSkin => DepositionKind::TopSkin,
        ProcessSkinKind::BottomSkin => DepositionKind::BottomSkin,
    };
    let Some(width) = path.width.constant_width_mm() else {
        return Ok(None);
    };
    if !on_authored_axis(path, role, recipe, course)? {
        return Ok(None);
    }
    let actual = [project_xyz6_point_mm(*start)?, project_xyz6_point_mm(*end)?];
    let supported =
        classify_actual(actual, width, previous, recipe, anchors, work, cancel)? == Some(false);
    Ok(supported.then_some(role))
}

/// The pieces one path of `layer` is split into against the previous layer,
/// or None when it stays whole: the same rule [`split_bridged_rows`] applies.
#[expect(
    clippy::too_many_arguments,
    reason = "the split binds its layer, lower layer, recipe, course and caller-owned proof resources"
)]
pub(super) fn split_path(
    layer: &PrintLayer,
    path: &DepositionPath,
    previous: &PrintLayer,
    recipe: &PlanarPrintRecipe,
    course: u32,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<Vec<DepositionPath>>, ManufacturingError> {
    match splittable(layer, path, previous, recipe, course)? {
        Some(role) => split_row(path, role, previous, recipe, anchors, work, cancel),
        None => Ok(None),
    }
}

/// The role a path of `layer` would take back if split against the previous
/// layer, or None when the split rule leaves it whole regardless of gaps.
pub(super) fn splittable(
    layer: &PrintLayer,
    path: &DepositionPath,
    previous: &PrintLayer,
    recipe: &PlanarPrintRecipe,
    course: u32,
) -> Result<Option<DepositionKind>, ManufacturingError> {
    if layer.lower_z_mm != previous.z_mm || !layer.region_coverage.process_diaphragms.is_empty() {
        return Ok(None);
    }
    Ok(match supported_role(layer, path) {
        Some(role)
            if path.physical_z_mm == [layer.lower_z_mm, layer.z_mm]
                && on_authored_axis(path, role, recipe, course)?
                && !separator_witness(layer, path)
                && !fitted(layer, path) =>
        {
            Some(role)
        }
        _ => None,
    })
}

fn resource() -> ManufacturingError {
    error(
        ManufacturingErrorCode::ResourceLimit,
        "bridge row split allocation refused",
    )
}

/// The kept role binds the layer's authored hatch axis at actual XYZ6
/// coordinates (plan validation's rule); an orthogonal recovery Bridge row
/// crosses it and stays whole.
fn on_authored_axis(
    path: &DepositionPath,
    role: DepositionKind,
    recipe: &PlanarPrintRecipe,
    course: u32,
) -> Result<bool, ManufacturingError> {
    let policy = if role == DepositionKind::TopSkin {
        recipe.top_skin_axis
    } else {
        recipe.rectilinear_axis
    };
    let fixed = match policy {
        RectilinearAxisPolicy::X => 1,
        RectilinearAxisPolicy::Y => 0,
        RectilinearAxisPolicy::AlternatingXY => usize::from(course.is_multiple_of(2)),
        RectilinearAxisPolicy::AlternatingYX => usize::from(!course.is_multiple_of(2)),
    };
    let project = spiling_contracts::manufacturing::program_coordinates::project_xyz6_coordinate_mm;
    Ok(path.points_mm[0][fixed] == path.points_mm[1][fixed]
        && project(path.points_mm[0][fixed])? == project(path.points_mm[1][fixed])?)
}

/// A separator endpoint bond names this exact skin stroke; it stays whole.
fn separator_witness(layer: &PrintLayer, path: &DepositionPath) -> bool {
    let ends = [
        [path.points_mm[0][0], path.points_mm[0][1]],
        [path.points_mm[1][0], path.points_mm[1][1]],
    ];
    layer
        .region_coverage
        .process_interfaces
        .iter()
        .flat_map(|i| i.endpoint_bonds.iter().flatten())
        .any(|bond| {
            matches!(bond, ProcessEndpointBond::Skin { skin_segment_mm, .. }
                if *skin_segment_mm == ends || *skin_segment_mm == [ends[1], ends[0]])
        })
}

/// A minimum-command fit binds its whole construction; it stays whole.
fn fitted(layer: &PrintLayer, path: &DepositionPath) -> bool {
    let [a, b] = [path.points_mm[0], path.points_mm[1]];
    layer.skin_command_fits.iter().any(|domain| {
        domain.fits.iter().any(|fit| {
            let [c, d] = fit.fitted_mm;
            (0..2).any(|fixed| {
                let along = 1 - fixed;
                a[fixed] == b[fixed]
                    && c[fixed] == a[fixed]
                    && d[fixed] == a[fixed]
                    && c[along].min(d[along]) <= a[along].max(b[along])
                    && a[along].min(b[along]) <= c[along].max(d[along])
            })
        })
    })
}

/// Classify one actual stroke against the previous layer's actual strokes:
/// Some(true) has a genuine Bridge gap and otherwise only supported gaps,
/// Some(false) is fully supported, None crosses any other gap.
fn classify_actual(
    stroke: [[f64; 3]; 2],
    width: f64,
    previous: &PrintLayer,
    recipe: &PlanarPrintRecipe,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<bool>, ManufacturingError> {
    let length = (stroke[1][0] - stroke[0][0]).hypot(stroke[1][1] - stroke[0][1]);
    let mut genuine = false;
    for gap in anchors.gaps_edges(
        AnchorCandidate {
            endpoints_mm: stroke,
            bead_width_mm: width,
        },
        projected_edges(previous.paths.iter().filter(|p| p.kind.is_model()), true),
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
            GapKind::Contact | GapKind::Demand => return Ok(None),
        }
    }
    Ok(Some(genuine))
}

fn split_row(
    path: &DepositionPath,
    role: DepositionKind,
    previous: &PrintLayer,
    recipe: &PlanarPrintRecipe,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<Vec<DepositionPath>>, ManufacturingError> {
    let Some(width) = path.width.constant_width_mm() else {
        return Ok(None);
    };
    let [a, b] = [path.points_mm[0], path.points_mm[1]];
    let actual = [project_xyz6_point_mm(a)?, project_xyz6_point_mm(b)?];
    let length = (actual[1][0] - actual[0][0]).hypot(actual[1][1] - actual[0][1]);
    let anchor = BRIDGE_ANCHOR_WIDTHS * width;
    if !length.is_finite() || length <= 2.0 * anchor + MIN_DEPOSITION_COMMAND_MM {
        return Ok(None);
    }
    // Bridged intervals in parameter along the actual row, each grown by its
    // anchor and merged across remainders shorter than one anchor.
    let mut spans: Vec<[f64; 2]> = Vec::new();
    for gap in anchors.gaps_edges(
        AnchorCandidate {
            endpoints_mm: actual,
            bead_width_mm: width,
        },
        projected_edges(previous.paths.iter().filter(|p| p.kind.is_model()), true),
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
            GapKind::Bridge => {
                let grown = [
                    (gap.start - anchor / length).max(0.0),
                    (gap.end + anchor / length).min(1.0),
                ];
                match spans.last_mut() {
                    Some(last) if grown[0] - last[1] < anchor / length => last[1] = grown[1],
                    _ => {
                        spans.try_reserve(1).map_err(|_| resource())?;
                        spans.push(grown);
                    }
                }
            }
            GapKind::Contact | GapKind::Demand => return Ok(None),
        }
    }
    let (Some(first), Some(last)) = (spans.first().copied(), spans.last().copied()) else {
        return Ok(None);
    };
    if first[0] < anchor / length {
        spans[0][0] = 0.0;
    }
    let end = spans.len() - 1;
    if 1.0 - last[1] < anchor / length {
        spans[end][1] = 1.0;
    }
    if spans.len() == 1 && spans[0] == [0.0, 1.0] {
        return Ok(None);
    }
    // Boundaries in order with the role of the piece that follows each.
    let mut cuts: Vec<(f64, DepositionKind)> = Vec::new();
    cuts.try_reserve_exact(2 * spans.len() + 1)
        .map_err(|_| resource())?;
    if spans[0][0] > 0.0 {
        cuts.push((0.0, role));
    }
    for span in &spans {
        cuts.push((span[0], DepositionKind::Bridge));
        if span[1] < 1.0 {
            cuts.push((span[1], role));
        }
    }
    let point = |t: f64| -> [f64; 3] {
        if t == 0.0 {
            a
        } else if t == 1.0 {
            b
        } else {
            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2]]
        }
    };
    let mut pieces = Vec::new();
    pieces
        .try_reserve_exact(cuts.len())
        .map_err(|_| resource())?;
    for (index, &(t, kind)) in cuts.iter().enumerate() {
        let next = cuts.get(index + 1).map_or(1.0, |cut| cut.0);
        let ends = [point(t), point(next)];
        let stroke = [
            project_xyz6_point_mm(ends[0])?,
            project_xyz6_point_mm(ends[1])?,
        ];
        if (stroke[1][0] - stroke[0][0]).hypot(stroke[1][1] - stroke[0][1])
            < MIN_DEPOSITION_COMMAND_MM
        {
            return Ok(None);
        }
        // The verifier's exact split-row predicates; any miss keeps it whole.
        let expected = Some(kind == DepositionKind::Bridge);
        if classify_actual(stroke, width, previous, recipe, anchors, work, cancel)? != expected {
            return Ok(None);
        }
        pieces.push(DepositionPath {
            kind,
            points_mm: ends.to_vec(),
            exposed: false,
            ..path.clone()
        });
    }
    Ok(Some(pieces))
}

#[cfg(test)]
mod tests;
