// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Support walls and hatch rows drawn straight from one layer's kept support
//! cells. Centrelines lie in E, the kept cells eroded by half a bead, so every
//! nominal bead stays inside them; hatch rows are the cell runs of E along
//! each lattice line, and walls follow E's traced boundary.

mod commands;

use super::grid::{Bits, Raster};
use super::{AnchorCandidate, AnchorWorkspace, StrokeRuns, candidate_budget, region, strokes};
use crate::{check_cancel, error};
use geo::line_intersection::{LineIntersection, line_intersection};
use geo::{Coord, Line, MultiPolygon, Point};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Default)]
pub(super) struct WallReceipt {
    pub(super) paths: usize,
    pub(super) omitted_mm: f64,
}

/// One layer's staged support walls and hatch rows from its kept support
/// cells `support` and the interface cells `interface` among them.
///
/// Interface is a role and density selector, not a separate solid: rows in
/// E ∩ I are full-density `SupportInterface`, rows in E − I are ordinary
/// `Support` at `density`, and walls split only where the interface
/// membership changes along them. `existing_segments` counts toward the
/// segment allowance; `retained` and `owned_bytes` are the caller's live
/// inventory.
#[expect(
    clippy::too_many_arguments,
    reason = "Grid operands, density and the actual layer stay explicit beside live inventory and cumulative work"
)]
pub(super) fn paths(
    raster: &Raster<'_>,
    support: &Bits,
    interface: &Bits,
    density: f64,
    layer: &region::HatchLayer<'_>,
    existing_segments: usize,
    retained: usize,
    owned_bytes: usize,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<(Vec<DepositionPath>, WallReceipt), ManufacturingError> {
    check_cancel(cancel)?;
    // The erosion radius carries the XYZ6 projection error, so a bead
    // centred on E's edge (walls) or a projected endpoint never reaches a
    // cell outside S, even by touching.
    let centers = raster.erode(
        support,
        layer.width_mm * 0.5 + 2.0 * program_coordinates::XYZ6_QUANTUM_MM,
    )?;
    let all_interface = !interface.is_empty() && interface == support;
    let split = if interface.is_empty() || all_interface {
        None
    } else {
        Some((
            raster.difference(&centers, interface)?,
            raster.intersection(&centers, interface)?,
        ))
    };
    let empty = Bits::default();
    let (ordinary, interface_centers) = match &split {
        Some((ordinary, interface)) => (ordinary, interface),
        None if all_interface => (&empty, &centers),
        None => (&centers, &empty),
    };
    let mut contour = raster.boundary(&centers)?;
    region::canonicalize(&mut contour);
    region::check_size(&contour)?;
    let retained = retained
        .checked_add(region::point_count(&contour))
        .filter(|n| *n <= crate::working_set::units())
        .ok_or_else(|| resource("combined support centerline ownership budget exceeded"))?;
    let owned_bytes = owned_bytes
        .saturating_add(super::super::native::material_bytes(&mut contour))
        .saturating_add(centers.bytes())
        .saturating_add(ordinary.bytes())
        .saturating_add(interface_centers.bytes());
    super::super::remaining_geometry_units(retained, owned_bytes)?;
    let mut segments = existing_segments;
    let mut paths = Vec::new();
    if split.is_none() {
        let kind = if all_interface {
            DepositionKind::SupportInterface
        } else {
            DepositionKind::Support
        };
        region::support_contour_paths(
            &contour,
            layer.z,
            layer.lower_z,
            layer.width_mm,
            kind,
            &mut paths,
            &mut segments,
            cancel,
        )?;
    } else {
        // Preserve the complete exterior/hole wall, including every traced
        // vertex. Only interface membership changes split it.
        walls(
            raster,
            &contour,
            interface,
            &centers,
            layer,
            &mut paths,
            &mut segments,
            retained,
            owned_bytes,
            cancel,
        )?;
    }
    let omitted_wall_mm = commands::fit(
        &contour,
        &mut paths,
        &mut segments,
        retained,
        owned_bytes,
        work,
        foreign_work,
        cancel,
    )?;
    let wall_paths = paths.len();
    for (domain, kind, fraction) in [
        (ordinary, DepositionKind::Support, density),
        (interface_centers, DepositionKind::SupportInterface, 1.0),
    ] {
        hatch(
            raster,
            domain,
            kind,
            fraction,
            layer,
            &mut paths,
            &mut segments,
            cancel,
        )?;
    }
    candidate_budget(&paths, retained, owned_bytes)?;
    Ok((
        paths,
        WallReceipt {
            paths: wall_paths,
            omitted_mm: omitted_wall_mm,
        },
    ))
}

/// Rows of the common build-frame support lattice: line `k` lies at
/// `k · pitch` on the fixed axis, the pitch derived from the recipe's nominal
/// layer height and the actual bead width, so clipping, islands and thinner
/// layers never move a row. Each row's road segments are the cell runs of
/// `domain` along it; odd lattice lines run reversed.
#[expect(
    clippy::too_many_arguments,
    reason = "Grid domain, role, density and actual layer stay explicit beside bounded output counters"
)]
fn hatch(
    raster: &Raster<'_>,
    domain: &Bits,
    kind: DepositionKind,
    fraction: f64,
    layer: &region::HatchLayer<'_>,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
        return Err(error(
            ManufacturingErrorCode::UnsupportedGeometry,
            "hatch fraction must be finite and in [0,1]",
        ));
    }
    // Zero density is an intentional void, not a forced row.
    if fraction == 0.0 {
        return Ok(());
    }
    let fixed = usize::from(layer.axis == region::HatchAxis::X);
    let Some([lo, hi]) = domain.extent(&raster.frame, fixed) else {
        return Ok(());
    };
    let width = layer.width_mm;
    let h = layer.recipe.layer_height_mm;
    let solid_spacing = ((width - h) * h + std::f64::consts::PI * (h * 0.5).powi(2)) / h;
    let spacing = solid_spacing / fraction;
    if !(spacing.is_finite() && spacing > 0.0) {
        return Err(error(
            ManufacturingErrorCode::UnsupportedGeometry,
            "support hatch pitch is not finite and positive",
        ));
    }
    let first = (lo / spacing).ceil();
    let last = (hi / spacing).floor();
    if !(first.is_finite() && last.is_finite()) {
        return Err(resource("rectilinear hatch row limit exceeded"));
    }
    if last < first {
        return Ok(());
    }
    let count = last - first + 1.0;
    if count > crate::working_set::units().saturating_sub(*segments) as f64 {
        return Err(resource("rectilinear hatch row limit exceeded"));
    }
    let mut hits = Vec::new();
    for k in 0..count as usize {
        check_cancel(cancel)?;
        let index = first + k as f64;
        let coordinate = index * spacing;
        hits.clear();
        raster.line_runs(domain, fixed, coordinate, &mut hits)?;
        region::append_hatch_row(
            &hits,
            coordinate,
            kind,
            layer,
            index.rem_euclid(2.0) == 1.0,
            paths,
            segments,
        )?;
    }
    Ok(())
}

fn resource(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::ResourceLimit, message)
}

fn charge(
    work: &mut usize,
    amount: usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    super::charge_owned_work(work, foreign_work, amount, cancel)
}

struct WallRing {
    z: f64,
    lower_z: f64,
    bead_width_mm: f64,
    first_path: usize,
}

/// Walls along the traced contour of E, each edge split where the interface
/// cells it crosses change; each piece takes the role of the cell holding its
/// midpoint. Same-role pieces continue one path; no seam outline is added.
#[expect(
    clippy::too_many_arguments,
    reason = "Wall construction keeps grid ownership, output counters and live inventory explicit"
)]
fn walls(
    raster: &Raster<'_>,
    contour: &MultiPolygon<f64>,
    interface: &Bits,
    centers: &Bits,
    layer: &region::HatchLayer<'_>,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    retained: usize,
    owned_bytes: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut pieces = Vec::new();
    let mut path_points = 0usize;
    for polygon in &contour.0 {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            let wall_ring = WallRing {
                z: layer.z,
                lower_z: layer.lower_z,
                bead_width_mm: layer.width_mm,
                first_path: paths.len(),
            };
            for edge in ring.0.windows(2) {
                check_cancel(cancel)?;
                let (a, b) = (edge[0], edge[1]);
                raster.along(interface, centers, [a.x, a.y], [b.x, b.y], &mut pieces)?;
                let mut start = a;
                for (k, &(t, inside)) in pieces.iter().enumerate() {
                    // The original edge endpoint is always retained.
                    let end = if k + 1 == pieces.len() {
                        b
                    } else {
                        Coord {
                            x: a.x + (b.x - a.x) * t,
                            y: a.y + (b.y - a.y) * t,
                        }
                    };
                    let kind = if inside {
                        DepositionKind::SupportInterface
                    } else {
                        DepositionKind::Support
                    };
                    wall_piece(
                        [start, end],
                        kind,
                        &wall_ring,
                        paths,
                        segments,
                        &mut path_points,
                        crate::working_set::units()
                            .saturating_sub(retained)
                            .saturating_sub(pieces.len()),
                        owned_bytes.saturating_add(super::vec_bytes(&pieces)),
                    )?;
                    start = end;
                }
            }
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "One wall piece borrows its ring authority while charging path, point and segment growth separately"
)]
fn wall_piece(
    edge: [Coord<f64>; 2],
    kind: DepositionKind,
    ring: &WallRing,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    path_points: &mut usize,
    point_budget: usize,
    owned_bytes: usize,
) -> Result<(), ManufacturingError> {
    let WallRing {
        z,
        lower_z,
        bead_width_mm,
        first_path,
    } = *ring;
    let [start, end] = edge;
    let length = (end.x - start.x).hypot(end.y - start.y);
    if !length.is_finite() {
        return Err(error(
            ManufacturingErrorCode::UnsupportedGeometry,
            "support wall role crossing creates unresolved deposition",
        ));
    }
    if length == 0.0 {
        return Ok(());
    }
    if *segments >= crate::working_set::units() {
        return Err(resource("retained support wall segment budget exceeded"));
    }
    let continuation =
        paths.len() > first_path && paths.last().is_some_and(|path| path.kind == kind);
    let additional = if continuation { 1 } else { 2 };
    if *path_points + additional > point_budget {
        return Err(resource(
            "combined support wall/crossing point budget exceeded",
        ));
    }
    *segments += 1;
    *path_points += additional;
    let from = [start.x, start.y, z];
    let to = [end.x, end.y, z];
    let mut live_bytes = owned_bytes.saturating_add(super::super::bonds::path_bytes(paths));
    if continuation {
        super::reserve_owned(&mut paths.last_mut().unwrap().points_mm, 1, live_bytes)?;
    } else {
        super::reserve_owned(paths, 1, live_bytes)?;
        live_bytes = owned_bytes.saturating_add(super::super::bonds::path_bytes(paths));
        super::super::remaining_geometry_units(
            0,
            live_bytes.saturating_add(2 * std::mem::size_of::<[f64; 3]>()),
        )?;
    }
    if continuation {
        paths.last_mut().unwrap().points_mm.push(to);
    } else {
        paths.push(DepositionPath {
            material_domain: None,
            kind,
            width: WidthProfile::Constant {
                width_mm: bead_width_mm,
            },
            physical_z_mm: [lower_z, z],
            owner: None,
            points_mm: vec![from, to],
            arcs: Vec::new(),
            exposed: false,
            connectors: Vec::new(),
        });
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn failure<'a>(
    paths: &[DepositionPath],
    previous: impl Iterator<Item = &'a DepositionPath> + Clone,
    recipe: &PlanarPrintRecipe,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<strokes::Failure>, ManufacturingError> {
    scan(
        paths,
        previous,
        std::iter::empty(),
        recipe,
        anchors,
        work,
        cancel,
        true,
    )
}

/// The longest unanchored demand span of any stroke, when the raster has one.
/// A layer whose rasters all have one prints the least-unanchored of them as
/// authored and reports this span.
#[cfg(test)]
pub(super) fn worst_failure<'a>(
    paths: &[DepositionPath],
    previous: impl Iterator<Item = &'a DepositionPath> + Clone,
    recipe: &PlanarPrintRecipe,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<strokes::Failure>, ManufacturingError> {
    scan(
        paths,
        previous,
        std::iter::empty(),
        recipe,
        anchors,
        work,
        cancel,
        false,
    )
}

#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "Direct support and separated object contact retain distinct anchor provenance"
)]
pub(super) fn scan<'a>(
    paths: &[DepositionPath],
    previous: impl Iterator<Item = &'a DepositionPath> + Clone,
    contacts: impl Iterator<Item = &'a DepositionPath> + Clone,
    recipe: &PlanarPrintRecipe,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
    first: bool,
) -> Result<Option<strokes::Failure>, ManufacturingError> {
    scan_with_lower(
        paths,
        &mut strokes::LowerStrokes::new(previous, contacts),
        recipe,
        anchors,
        work,
        cancel,
        first,
    )
}

pub(super) fn scan_with_lower<'a>(
    paths: &[DepositionPath],
    lower: &mut strokes::LowerStrokes<
        'a,
        impl Iterator<Item = &'a DepositionPath> + Clone,
        impl Iterator<Item = &'a DepositionPath> + Clone,
    >,
    recipe: &PlanarPrintRecipe,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
    first: bool,
) -> Result<Option<strokes::Failure>, ManufacturingError> {
    let mut worst: Option<strokes::Failure> = None;
    for (path_index, path) in paths.iter().enumerate() {
        for (first_point, last, width_mm) in StrokeRuns::for_path(path)? {
            let a = path.points_mm[first_point];
            let b = path.points_mm[last];
            let length = (b[0] - a[0]).hypot(b[1] - a[1]);
            for gap in lower.gaps_supplied_contacts(
                AnchorCandidate {
                    endpoints_mm: [a, b],
                    bead_width_mm: width_mm,
                },
                path.physical_z_mm[0],
                path.kind != DepositionKind::Support,
                anchors,
                work,
                cancel,
            )? {
                let span = (gap.end - gap.start) * length;
                let kind =
                    strokes::gap_kind(gap, span, width_mm, recipe.process.max_bridge_span_mm);
                if (matches!(kind, strokes::GapKind::Demand)
                    || (matches!(kind, strokes::GapKind::Bridge)
                        && path.kind.is_model()
                        && path.kind != DepositionKind::Bridge))
                    && worst.as_ref().is_none_or(|worst| span > worst.span)
                {
                    worst = Some(strokes::Failure {
                        path: path_index,
                        first: first_point,
                        last,
                        start: gap.start,
                        end: gap.end,
                        span,
                    });
                    if first {
                        return Ok(worst);
                    }
                }
            }
        }
    }
    Ok(worst)
}

#[expect(
    clippy::too_many_arguments,
    reason = "Retention binds original wall accounting, immutable lower/contact authority and caller-owned proof resources"
)]
pub(super) fn retain_anchored<'a>(
    paths: &mut Vec<DepositionPath>,
    wall_paths: usize,
    lower: &mut strokes::LowerStrokes<
        'a,
        impl Iterator<Item = &'a DepositionPath> + Clone,
        impl Iterator<Item = &'a DepositionPath> + Clone,
    >,
    recipe: &PlanarPrintRecipe,
    retained: usize,
    owned_bytes: usize,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<WallReceipt, ManufacturingError> {
    check_cancel(cancel)?;
    let mut receipt = WallReceipt::default();
    let original_units = paths.iter().fold(retained, |total, path| {
        total.saturating_add(path.points_mm.capacity())
    });
    let original_bytes = owned_bytes.saturating_add(super::super::bonds::path_bytes(paths));
    let original = std::mem::take(paths);
    // The caller's one lower inventory serves every candidate: it is
    // re-admitted against each candidate's budget and falls back to the
    // linear scan when it no longer fits, so verdicts equal a fresh
    // per-candidate scan.
    for (path_index, path) in original.into_iter().enumerate() {
        anchors.set_budget(candidate_budget(paths, original_units, original_bytes)?);
        if !super::support_role(path.kind)
            || scan_with_lower(
                std::slice::from_ref(&path),
                lower,
                recipe,
                anchors,
                work,
                cancel,
                true,
            )?
            .is_none()
        {
            let live_bytes = original_bytes
                .saturating_add(super::super::bonds::path_bytes(paths))
                .saturating_add(anchors.retained_bytes());
            super::reserve_owned(paths, 1, live_bytes)?;
            receipt.paths += usize::from(path_index < wall_paths);
            paths.push(path);
            continue;
        }
        if path_index >= wall_paths {
            continue;
        }
        for (first, last, width_mm) in StrokeRuns::for_path(&path)? {
            let count = if last >= first {
                last - first + 1
            } else {
                path.points_mm.len() - first + last
            };
            let live_bytes = original_bytes
                .saturating_add(super::super::bonds::path_bytes(paths))
                .saturating_add(anchors.retained_bytes());
            super::super::remaining_geometry_units(
                original_units
                    .saturating_add(
                        paths
                            .iter()
                            .map(|path| path.points_mm.capacity())
                            .sum::<usize>(),
                    )
                    .saturating_add(count),
                live_bytes.saturating_add(count.saturating_mul(std::mem::size_of::<[f64; 3]>())),
            )?;
            let fragment = DepositionPath {
                material_domain: path.material_domain,
                kind: path.kind,
                width: WidthProfile::Constant { width_mm },
                physical_z_mm: path.physical_z_mm,
                owner: path.owner,
                points_mm: if last >= first {
                    path.points_mm[first..=last].to_vec()
                } else {
                    path.points_mm[first..path.points_mm.len() - 1]
                        .iter()
                        .chain(&path.points_mm[..=last])
                        .copied()
                        .collect()
                },
                arcs: Vec::new(),
                exposed: path.exposed,
                connectors: Vec::new(),
            };
            anchors.set_budget(candidate_budget(
                paths,
                original_units.saturating_add(fragment.points_mm.capacity()),
                original_bytes.saturating_add(
                    fragment
                        .points_mm
                        .capacity()
                        .saturating_mul(std::mem::size_of::<[f64; 3]>()),
                ),
            )?);
            if scan_with_lower(
                std::slice::from_ref(&fragment),
                lower,
                recipe,
                anchors,
                work,
                cancel,
                true,
            )?
            .is_none()
            {
                super::reserve_owned(
                    paths,
                    1,
                    live_bytes.saturating_add(
                        fragment
                            .points_mm
                            .capacity()
                            .saturating_mul(std::mem::size_of::<[f64; 3]>()),
                    ),
                )?;
                paths.push(fragment);
                receipt.paths += 1;
            } else {
                for edge in fragment.checked_edges()? {
                    super::super::native::charge_support_clearance(work, cancel)?;
                    let start = edge.points_mm[0];
                    let end = edge.points_mm[1];
                    receipt.omitted_mm += (end[0] - start[0]).hypot(end[1] - start[1]);
                }
            }
        }
    }
    Ok(receipt)
}

pub(super) fn nominal_volume(
    paths: &[DepositionPath],
    layer_index: u32,
    recipe: &PlanarPrintRecipe,
) -> Result<f64, ManufacturingError> {
    paths.iter().try_fold(0.0, |sum, path| {
        Ok(sum + region::nominal_volume(path, layer_index, recipe)?)
    })
}

#[cfg(test)]
mod tests;
