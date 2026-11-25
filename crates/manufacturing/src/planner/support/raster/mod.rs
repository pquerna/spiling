// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Support walls and hatch rows drawn straight from one layer's kept support
//! cells. Centrelines lie in E, the kept cells eroded by half a bead, so every
//! nominal bead stays inside them; hatch rows are the cell runs of E along
//! each lattice line, and walls follow E's traced boundary.

mod commands;
pub(in crate::planner::support) mod exact;

use super::field::{Field, FieldBuilder};
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
    /// Cells beyond the kept set the exact field proves clear, for final
    /// clearance.
    pub(super) certified: Vec<super::sweep::CertifiedEdge>,
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
///
/// `exclusions`, when given, feeds the exact exclusions the sweep subtracted
/// on this layer into a clearance field and chooses the wall placement
/// (`None` when the layer's cells cannot use a field); walls and row ends
/// bounded by them then sit on a sub-cell offset instead of the staircase.
#[expect(
    clippy::too_many_arguments,
    reason = "Grid operands, density and the actual layer stay explicit beside live inventory and cumulative work"
)]
pub(super) fn paths(
    raster: &Raster<'_>,
    support: &Bits,
    interface: &Bits,
    exclusions: Option<&Exclusions<'_>>,
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
    // Support walls simplify within the support road budget (interface walls
    // within the structure budget); every chord still stays inside kept
    // cells and, where exact, keeps the bead's exact clearance (RFC 11).
    let tolerances = &layer.recipe.road_tolerances;
    let mut budget = tolerances.support_mm - PROFILE_TOLERANCE_MM;
    if !interface.is_empty() {
        budget = budget.min(tolerances.structure_mm - PROFILE_TOLERANCE_MM);
    }
    let mut contour = raster.boundary(&centers, budget)?;
    region::canonicalize(&mut contour);
    region::check_size(&contour)?;
    let exact = exact::Exact::new(
        layer.width_mm * 0.5 + program_coordinates::XYZ6_PROJECTION_SCREEN_MM,
        raster.frame.cell(),
    )
    .simplifying_within(budget);
    let field = match exclusions {
        Some(feed) if !contour.0.is_empty() => {
            exact_field(raster, support, interface, &mut contour, feed, &exact)?
        }
        _ => None,
    };
    let retained = retained
        .checked_add(region::point_count(&contour))
        .filter(|n| *n <= crate::working_set::units())
        .ok_or_else(|| resource("combined support centerline ownership budget exceeded"))?;
    let owned_bytes = owned_bytes
        .saturating_add(super::super::native::material_bytes(&mut contour))
        .saturating_add(centers.bytes())
        .saturating_add(field.as_ref().map_or(0, Field::bytes))
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
            field.as_ref().map(|field| RowEnds {
                field,
                exact: &exact,
                support,
                centers: &centers,
            }),
            &mut paths,
            &mut segments,
            cancel,
        )?;
    }
    let certified = match &field {
        Some(field) => {
            split_row_ends(
                raster,
                &centers,
                &mut paths[wall_paths..],
                2.0 * exact.reach + 2.0 * exact.cell,
                &mut segments,
            )?;
            certify(raster, &centers, field, &paths)?
        }
        None => Vec::new(),
    };
    candidate_budget(&paths, retained, owned_bytes)?;
    Ok((
        paths,
        WallReceipt {
            paths: wall_paths,
            omitted_mm: omitted_wall_mm,
            certified,
        },
    ))
}

/// Splits each row whose end leaves E a wall reach and two cells inside
/// that end, so the stretch that can leave the kept cells is an edge of its
/// own: short enough for its exact certificate, while the rest of the row
/// lies in E.
fn split_row_ends(
    raster: &Raster<'_>,
    centers: &Bits,
    rows: &mut [DepositionPath],
    inset: f64,
    segments: &mut usize,
) -> Result<(), ManufacturingError> {
    for path in rows {
        if path.points_mm.len() != 2 {
            continue;
        }
        let [a, b] = [path.points_mm[0], path.points_mm[1]];
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        if length.is_nan() || length <= 2.0 * inset + program_coordinates::MIN_DEPOSITION_COMMAND_MM
        {
            continue;
        }
        let d = [(b[0] - a[0]) / length, (b[1] - a[1]) / length];
        let at = |t: f64| [a[0] + d[0] * t, a[1] + d[1] * t, a[2]];
        let mut points = Vec::new();
        raster.reserve(&mut points, 4)?;
        points.push(a);
        if !raster.holds_all(centers, [a[0], a[1]]) {
            points.push(at(inset));
        }
        if !raster.holds_all(centers, [b[0], b[1]]) {
            points.push(at(length - inset));
        }
        points.push(b);
        *segments += points.len() - 2;
        path.points_mm = points;
    }
    Ok(())
}

/// The staged edges that can leave E, each proved against the exact field
/// at its emitted XYZ6 segment: the largest bead radius whose capsule keeps
/// every exclusion's full clearance. Every staged centreline lies outside
/// the exclusion polygons (in E, or reached from E by clear steps), so the
/// boundary distance is the true distance. Final clearance takes these
/// instead of a second exact proof.
fn certify(
    raster: &Raster<'_>,
    centers: &Bits,
    field: &Field,
    paths: &[DepositionPath],
) -> Result<Vec<super::sweep::CertifiedEdge>, ManufacturingError> {
    let mut certified = Vec::new();
    let mut work = 0usize;
    for path in paths {
        check_cancel(raster.cancel)?;
        for edge in path.points_mm.windows(2) {
            let [a, b] = [[edge[0][0], edge[0][1]], [edge[1][0], edge[1][1]]];
            work += 1;
            if raster.holds_all(centers, a) && raster.holds_all(centers, b) {
                continue;
            }
            let segment = [
                [
                    program_coordinates::project_xyz6_coordinate_mm(a[0])?,
                    program_coordinates::project_xyz6_coordinate_mm(a[1])?,
                ],
                [
                    program_coordinates::project_xyz6_coordinate_mm(b[0])?,
                    program_coordinates::project_xyz6_coordinate_mm(b[1])?,
                ],
            ];
            let radius = field.segment_clearance(segment[0], segment[1], &mut work);
            if radius > 0.0 {
                raster.reserve(&mut certified, 1)?;
                certified.push(super::sweep::CertifiedEdge {
                    key: super::sweep::segment_key(segment),
                    radius,
                });
            }
        }
    }
    raster.charge(work);
    Ok(certified)
}

/// One layer's exact exclusions, fed into a clearance field; `false` when
/// the layer takes no field.
pub(super) type Exclusions<'a> =
    dyn Fn(&mut FieldBuilder<'_, '_>) -> Result<bool, ManufacturingError> + Sync + 'a;

/// Builds the layer's exact clearance field over its wall contour and moves
/// every exclusion-bounded wall onto the exact offset. `None` when the layer
/// cannot use a field.
fn exact_field(
    raster: &Raster<'_>,
    kept: &Bits,
    interface: &Bits,
    contour: &mut MultiPolygon<f64>,
    feed: &Exclusions<'_>,
    exact: &exact::Exact,
) -> Result<Option<Field>, ManufacturingError> {
    let rings = exact::samples(raster, contour, exact.cell)?;
    let Some(mut builder) = exact::builder(raster, &rings, exact)? else {
        return Ok(None);
    };
    if !feed(&mut builder)? {
        return Ok(None);
    }
    let field = builder.finish()?;
    if let Some(placed) = exact::displace(raster, &rings, &field, exact)? {
        *contour = placed;
    }
    if !interface.is_empty() {
        exact::extend_interface_turns(raster, kept, interface, contour, &field, exact)?;
        exact::refine_kept_faces(raster, kept, interface, contour, &field, exact)?;
    }
    Ok(Some(field))
}

/// What row ends extend against: the exact field and the layer's kept and
/// centre cells.
struct RowEnds<'f> {
    field: &'f Field,
    exact: &'f exact::Exact,
    support: &'f Bits,
    centers: &'f Bits,
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
    ends: Option<RowEnds<'_>>,
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
        if let Some(ends) = &ends {
            exact::extend_runs(
                raster,
                ends.support,
                ends.centers,
                ends.field,
                ends.exact,
                fixed,
                coordinate,
                &mut hits,
            );
        }
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
        None,
        recipe,
        anchors,
        work,
        cancel,
        first,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "Deposited support and established part cells retain distinct anchor provenance"
)]
pub(super) fn scan_with_lower<'a>(
    paths: &[DepositionPath],
    lower: &mut strokes::LowerStrokes<
        'a,
        impl Iterator<Item = &'a DepositionPath> + Clone,
        impl Iterator<Item = &'a DepositionPath> + Clone,
    >,
    cells: Option<&super::established::CellContacts<'_, '_>>,
    recipe: &PlanarPrintRecipe,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
    first: bool,
) -> Result<Option<strokes::Failure>, ManufacturingError> {
    let mut worst: Option<strokes::Failure> = None;
    let mut pieces = Vec::new();
    let mut intervals = Vec::new();
    for (path_index, path) in paths.iter().enumerate() {
        // Only interface lands on the part, as for separated contacts.
        let cells = cells.filter(|_| path.kind != DepositionKind::Support);
        for (first_point, last, width_mm) in StrokeRuns::for_path(path)? {
            let a = path.points_mm[first_point];
            let b = path.points_mm[last];
            let length = (b[0] - a[0]).hypot(b[1] - a[1]);
            let ends = if let Some(contacts) = cells {
                contacts.stroke([a, b], width_mm * 0.5, &mut pieces, &mut intervals)?
            } else {
                pieces.clear();
                pieces.push((1.0, false));
                [false; 2]
            };
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
                super::established::CellContacts::refine(
                    &pieces,
                    gap,
                    ends,
                    |start, end, left, right, direct_ends| {
                        let span = (end - start) * length;
                        let piece = super::anchors::AnchorGap {
                            start,
                            end,
                            left,
                            right,
                            direct_ends,
                        };
                        let kind = strokes::gap_kind(
                            &piece,
                            span,
                            width_mm,
                            recipe.process.max_bridge_span_mm,
                        );
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
                                start,
                                end,
                                span,
                            });
                        }
                        Ok(())
                    },
                )?;
                if first && worst.is_some() {
                    return Ok(worst);
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
        impl Iterator<Item = &'a DepositionPath> + Clone + Send + Sync,
        impl Iterator<Item = &'a DepositionPath> + Clone + Send + Sync,
    >,
    cells: Option<&super::established::CellContacts<'_, '_>>,
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
    if let Some(parallel) = retain_in_groups(
        paths,
        wall_paths,
        lower,
        cells,
        recipe,
        [original_units, original_bytes],
        work,
        cancel,
    )? {
        return Ok(parallel);
    }
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
                cells,
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
            if let Some(row) = trim_row_ends(&path, lower, cells, recipe, anchors, work, cancel)? {
                let live_bytes = original_bytes
                    .saturating_add(super::super::bonds::path_bytes(paths))
                    .saturating_add(anchors.retained_bytes());
                super::reserve_owned(paths, 1, live_bytes)?;
                paths.push(row);
            }
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
                cells,
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

/// A straight hatch row (collinear vertices allowed) whose only unanchored
/// stretches are its end stubs, cut back to where the lower strokes hold it,
/// and proved again.
/// A row with an unanchored stretch inside it, or one left shorter than a
/// bead, is `None`: dropped, as before. Dropping a whole row for an end stub
/// past the lower layer's last stroke would undermine every row above it.
fn trim_row_ends<'a>(
    path: &DepositionPath,
    lower: &mut strokes::LowerStrokes<
        'a,
        impl Iterator<Item = &'a DepositionPath> + Clone,
        impl Iterator<Item = &'a DepositionPath> + Clone,
    >,
    cells: Option<&super::established::CellContacts<'_, '_>>,
    recipe: &PlanarPrintRecipe,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<DepositionPath>, ManufacturingError> {
    if !super::support_role(path.kind) || !matches!(path.width, WidthProfile::Constant { .. }) {
        return Ok(None);
    }
    let Some(ends) = straight_ends(&path.points_mm) else {
        return Ok(None);
    };
    // Collapsing vertices would orphan anything indexed by them.
    if path.points_mm.len() > 2 && (!path.arcs.is_empty() || !path.connectors.is_empty()) {
        return Ok(None);
    }
    let mut row = path.clone();
    row.points_mm = ends.to_vec();
    // Each pass cuts one end; a third failure is an inside gap or a
    // remainder that still does not hold.
    for _ in 0..3 {
        let [a, b] = [row.points_mm[0], row.points_mm[1]];
        if (b[0] - a[0]).hypot(b[1] - a[1]) < recipe.bead_width_mm {
            return Ok(None);
        }
        let Some(failure) = scan_with_lower(
            std::slice::from_ref(&row),
            lower,
            cells,
            recipe,
            anchors,
            work,
            cancel,
            true,
        )?
        else {
            return Ok(Some(row));
        };
        let at = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2]];
        if failure.start <= 0.0 && failure.end < 1.0 {
            row.points_mm[0] = at(failure.end);
        } else if failure.end >= 1.0 && failure.start > 0.0 {
            row.points_mm[1] = at(failure.start);
        } else {
            return Ok(None);
        }
    }
    Ok(None)
}

/// The two ends of a polyline whose points advance along one straight line
/// (a hatch row split by collinear vertices), or `None` for any bend.
fn straight_ends(points: &[[f64; 3]]) -> Option<[[f64; 3]; 2]> {
    let (&first, &last) = (points.first()?, points.last()?);
    let along = [last[0] - first[0], last[1] - first[1]];
    let length = along[0].hypot(along[1]);
    if points.len() < 2 || length <= 0.0 {
        return None;
    }
    let mut previous = 0.0;
    for point in &points[1..points.len() - 1] {
        let offset = [point[0] - first[0], point[1] - first[1]];
        let t = (offset[0] * along[0] + offset[1] * along[1]) / length;
        let off_line = (offset[0] * along[1] - offset[1] * along[0]).abs() / length;
        if off_line > STRAIGHT_ROW_MM || t < previous || t > length {
            return None;
        }
        previous = t;
    }
    Some([first, last])
}

/// Off-line distance below which a vertex leaves a hatch row straight: the
/// emitted coordinates' own resolution.
const STRAIGHT_ROW_MM: f64 = 1e-6;

/// Least support candidates per anchoring group.
const ANCHOR_GROUP_PATHS: usize = 16;

/// What anchoring decided for one staged path.
enum Verdict {
    /// Kept whole.
    Keep,
    /// Replaced by the anchored stroke runs `(first, last, width)`, possibly
    /// none, with the edge lengths of the omitted runs in order.
    Runs(Vec<(usize, usize, f64)>, Vec<f64>),
    /// A hatch row kept without its unanchored end stubs.
    Trimmed(DepositionPath),
}

/// One group's verdicts, by staged path index, and its anchor work.
struct GroupVerdicts {
    verdicts: Vec<(usize, Verdict)>,
    work: usize,
}

/// The stroke-run fragment `first..=last` of `path` at `width_mm`.
fn run_fragment(path: &DepositionPath, first: usize, last: usize, width_mm: f64) -> DepositionPath {
    DepositionPath {
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
    }
}

/// [`retain_anchored`] judged by groups of paths (every `groups`-th path)
/// on separate threads, each with its own inventory over the same immutable
/// lower strokes, then applied in path order. Each path's verdict depends
/// only on that path and the lower strokes, so the result, the omitted
/// length (summed in path order) and the charged work equal the serial
/// pass. Groups are admitted as if every staged path were retained, the
/// remaining records divided evenly between them; `None` (serial pass
/// instead, with only its own charges) when the layer is too small to split
/// or a group is not admitted.
#[expect(
    clippy::too_many_arguments,
    reason = "The serial pass's operands stay explicit beside its live inventory"
)]
fn retain_in_groups<'a, P, C>(
    paths: &mut Vec<DepositionPath>,
    wall_paths: usize,
    lower: &strokes::LowerStrokes<'a, P, C>,
    cells: Option<&super::established::CellContacts<'_, '_>>,
    recipe: &PlanarPrintRecipe,
    [original_units, original_bytes]: [usize; 2],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<WallReceipt>, ManufacturingError>
where
    P: Iterator<Item = &'a DepositionPath> + Clone + Send + Sync,
    C: Iterator<Item = &'a DepositionPath> + Clone + Send + Sync,
{
    use rayon::prelude::*;
    let candidates = paths
        .iter()
        .filter(|path| super::support_role(path.kind))
        .count();
    let groups = (candidates / ANCHOR_GROUP_PATHS).min(rayon::current_num_threads());
    if groups < 2 {
        return Ok(None);
    }
    // Every staged path counted as retained bounds what the serial pass
    // holds beside any candidate.
    let units = paths.iter().fold(original_units, |total, path| {
        total.saturating_add(path.points_mm.capacity())
    });
    let bytes = original_bytes.saturating_add(super::super::bonds::path_bytes(paths));
    let Ok(budget) = super::workspace_budget(units, bytes) else {
        return Ok(None);
    };
    let share = budget / groups;
    let contact_work =
        cells.map(|cells| cells.raster.work.load(std::sync::atomic::Ordering::Relaxed));
    let working_set = crate::working_set::current();
    let staged: &[DepositionPath] = paths;
    let judged: Vec<Result<Option<GroupVerdicts>, ManufacturingError>> = (0..groups)
        .into_par_iter()
        .map(|group| {
            crate::working_set::scope(working_set, || {
                let mut lower = lower.fresh();
                let mut anchors = AnchorWorkspace::default();
                let mut out = GroupVerdicts {
                    verdicts: Vec::new(),
                    work: 0,
                };
                out.verdicts
                    .try_reserve_exact(staged.len().div_ceil(groups))
                    .map_err(|_| resource("support anchoring verdicts allocation refused"))?;
                for index in (group..staged.len()).step_by(groups) {
                    let path = &staged[index];
                    anchors.set_budget(share);
                    if !super::support_role(path.kind)
                        || scan_with_lower(
                            std::slice::from_ref(path),
                            &mut lower,
                            cells,
                            recipe,
                            &mut anchors,
                            &mut out.work,
                            cancel,
                            true,
                        )?
                        .is_none()
                    {
                        out.verdicts.push((index, Verdict::Keep));
                        continue;
                    }
                    let (mut runs, mut omitted) = (Vec::new(), Vec::new());
                    if index < wall_paths {
                        for (first, last, width_mm) in StrokeRuns::for_path(path)? {
                            let fragment = run_fragment(path, first, last, width_mm);
                            let extra = fragment.points_mm.capacity();
                            if super::super::remaining_geometry_units(
                                units.saturating_add(extra),
                                bytes.saturating_add(
                                    extra.saturating_mul(std::mem::size_of::<[f64; 3]>()),
                                ),
                            )
                            .is_err()
                            {
                                return Ok(None);
                            }
                            anchors.set_budget(share);
                            if scan_with_lower(
                                std::slice::from_ref(&fragment),
                                &mut lower,
                                cells,
                                recipe,
                                &mut anchors,
                                &mut out.work,
                                cancel,
                                true,
                            )?
                            .is_none()
                            {
                                runs.try_reserve(1).map_err(|_| {
                                    resource("support anchoring runs allocation refused")
                                })?;
                                runs.push((first, last, width_mm));
                            } else {
                                for edge in fragment.checked_edges()? {
                                    super::super::native::charge_support_clearance(
                                        &mut out.work,
                                        cancel,
                                    )?;
                                    let start = edge.points_mm[0];
                                    let end = edge.points_mm[1];
                                    omitted.try_reserve(1).map_err(|_| {
                                        resource("support anchoring omission allocation refused")
                                    })?;
                                    omitted.push((end[0] - start[0]).hypot(end[1] - start[1]));
                                }
                            }
                        }
                    }
                    if index >= wall_paths {
                        anchors.set_budget(share);
                        if let Some(row) = trim_row_ends(
                            path,
                            &mut lower,
                            cells,
                            recipe,
                            &mut anchors,
                            &mut out.work,
                            cancel,
                        )? {
                            out.verdicts.push((index, Verdict::Trimmed(row)));
                            continue;
                        }
                    }
                    out.verdicts.push((index, Verdict::Runs(runs, omitted)));
                }
                Ok(Some(out))
            })
        })
        .collect();
    let mut by_path: Vec<Option<Verdict>> = Vec::new();
    by_path
        .try_reserve_exact(paths.len())
        .map_err(|_| resource("support anchoring verdicts allocation refused"))?;
    by_path.resize_with(paths.len(), || None);
    let mut group_work = 0usize;
    for result in judged {
        match result {
            Ok(Some(group)) => {
                group_work = group_work
                    .checked_add(group.work)
                    .ok_or_else(|| resource("support anchoring work counter overflow"))?;
                for (index, verdict) in group.verdicts {
                    by_path[index] = Some(verdict);
                }
            }
            Err(failure) if failure.code == ManufacturingErrorCode::Cancelled => {
                return Err(failure);
            }
            // A group that is not admitted or refuses leaves the serial
            // pass to decide, with its own charges only.
            Ok(None) | Err(_) => {
                if let (Some(cells), Some(before)) = (cells, contact_work) {
                    cells
                        .raster
                        .work
                        .store(before, std::sync::atomic::Ordering::Relaxed);
                }
                return Ok(None);
            }
        }
    }
    let mut receipt = WallReceipt::default();
    let original = std::mem::take(paths);
    for ((path_index, path), verdict) in original.into_iter().enumerate().zip(by_path) {
        match verdict {
            Some(Verdict::Keep) => {
                receipt.paths += usize::from(path_index < wall_paths);
                super::reserve_owned(paths, 1, bytes)?;
                paths.push(path);
            }
            Some(Verdict::Trimmed(row)) => {
                super::reserve_owned(paths, 1, bytes)?;
                paths.push(row);
            }
            Some(Verdict::Runs(runs, omitted)) => {
                for (first, last, width_mm) in runs {
                    super::reserve_owned(paths, 1, bytes)?;
                    paths.push(run_fragment(&path, first, last, width_mm));
                    receipt.paths += 1;
                }
                for length in omitted {
                    receipt.omitted_mm += length;
                }
            }
            None => {
                return Err(error(
                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                    "support anchoring groups lost a verdict",
                ));
            }
        }
    }
    *work = work
        .checked_add(group_work)
        .ok_or_else(|| resource("support anchoring work counter overflow"))?;
    Ok(Some(receipt))
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
