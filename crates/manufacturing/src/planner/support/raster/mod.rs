// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

mod commands;
mod proposal;

use super::{
    AnchorCandidate, AnchorWorkspace, StrokeRuns, candidate_budget, charge_boolean, region, strokes,
};
use crate::{check_cancel, error};
use geo::line_intersection::{LineIntersection, line_intersection};
use geo::{BooleanOps, Coord, Intersects, Line, MultiPolygon, Point};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

/// Clearance belongs to the physical support union, not to process-role seams.
/// In particular a sub-bead interface strip must not erase the union's wall.
pub(super) struct Domains {
    centers: MultiPolygon<f64>,
    wall_centers: Option<MultiPolygon<f64>>,
    ordinary: Option<MultiPolygon<f64>>,
    interface: Option<MultiPolygon<f64>>,
    all_interface: bool,
    retained: usize,
    owned_bytes: usize,
    anchor_owned_bytes: usize,
}

#[derive(Debug, Default)]
pub(super) struct WallReceipt {
    pub(super) paths: usize,
    pub(super) omitted_mm: f64,
}

impl Domains {
    #[expect(
        clippy::too_many_arguments,
        reason = "Support and interface operands retain separate live and anchor byte inventories and cumulative work"
    )]
    pub(super) fn new(
        support: &MultiPolygon<f64>,
        interface: &MultiPolygon<f64>,
        all_interface: bool,
        width: f64,
        retained: usize,
        owned_bytes: usize,
        anchor_owned_bytes: usize,
        work: &mut usize,
        foreign_work: usize,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        let owned_bytes = owned_bytes.saturating_add(anchor_owned_bytes);
        charge_boolean(support, support, work, foreign_work, cancel)?;
        let mut centers = region::offset_admitted(
            support,
            -width * 0.5,
            |scratch, scratch_bytes, overlay| {
                super::super::remaining_geometry_units(
                    retained.saturating_add(scratch),
                    owned_bytes.saturating_add(scratch_bytes),
                )?;
                if let Some((a, b)) = overlay {
                    super::super::boolean_budget(a, b, cancel)?;
                }
                Ok(())
            },
            cancel,
        )?;
        let center_bytes = super::super::native::material_bytes(&mut centers);
        let mut result = Self {
            centers,
            wall_centers: None,
            ordinary: None,
            interface: None,
            all_interface,
            retained,
            owned_bytes: owned_bytes.saturating_add(center_bytes),
            anchor_owned_bytes,
        };
        result.admit()?;
        result.wall_centers = proposal::construct(
            &result.centers,
            (!all_interface && !interface.0.is_empty()).then_some(interface),
            result.admit()?,
            result.owned_bytes,
            work,
            foreign_work,
            cancel,
        )?;
        if let Some(walls) = &mut result.wall_centers {
            result.owned_bytes = result
                .owned_bytes
                .saturating_add(super::super::native::material_bytes(walls));
            result.admit()?;
        }
        if !interface.0.is_empty() && !all_interface {
            charge_boolean(&result.centers, interface, work, foreign_work, cancel)?;
            result.interface = Some(result.centers.intersection(interface));
            region::check_size(result.interface.as_ref().unwrap())?;
            result.owned_bytes =
                result
                    .owned_bytes
                    .saturating_add(super::super::native::material_bytes(
                        result.interface.as_mut().unwrap(),
                    ));
            result.admit()?;
            charge_boolean(&result.centers, interface, work, foreign_work, cancel)?;
            result.ordinary = Some(result.centers.difference(interface));
            region::check_size(result.ordinary.as_ref().unwrap())?;
            result.owned_bytes =
                result
                    .owned_bytes
                    .saturating_add(super::super::native::material_bytes(
                        result.ordinary.as_mut().unwrap(),
                    ));
            result.admit()?;
        }
        check_cancel(cancel)?;
        Ok(result)
    }

    fn admit(&self) -> Result<usize, ManufacturingError> {
        let points = std::iter::once(&self.centers)
            .chain(self.wall_centers.iter())
            .chain(self.ordinary.iter())
            .chain(self.interface.iter())
            .try_fold(self.retained, |total, domain| {
                total
                    .checked_add(region::point_count(domain))
                    .filter(|n| *n <= crate::working_set::units())
                    .ok_or_else(|| {
                        resource("combined support centerline ownership budget exceeded")
                    })
            })?;
        super::super::remaining_geometry_units(points, self.owned_bytes)?;
        Ok(points)
    }

    pub(super) fn owned_bytes(&self) -> usize {
        self.owned_bytes
    }

    pub(super) fn retain_bytes(&mut self, bytes: usize) -> Result<(), ManufacturingError> {
        self.owned_bytes = self.owned_bytes.saturating_add(bytes);
        self.admit().map(|_| ())
    }

    pub(super) fn set_anchor_owned_bytes(&mut self, bytes: usize) {
        self.owned_bytes = self
            .owned_bytes
            .saturating_sub(self.anchor_owned_bytes)
            .saturating_add(bytes);
        self.anchor_owned_bytes = bytes;
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Raster emission binds captured interface geometry and density to the actual layer and existing segment count"
    )]
    pub(super) fn paths(
        &self,
        interface: &MultiPolygon<f64>,
        density: f64,
        layer: &region::HatchLayer<'_>,
        existing_segments: usize,
        work: &mut usize,
        foreign_work: usize,
        cancel: &AtomicBool,
    ) -> Result<(Vec<DepositionPath>, WallReceipt), ManufacturingError> {
        check_cancel(cancel)?;
        let empty = MultiPolygon(Vec::new());
        let ordinary = self.ordinary.as_ref().unwrap_or(if self.all_interface {
            &empty
        } else {
            &self.centers
        });
        let interface_centers = self.interface.as_ref().unwrap_or(if self.all_interface {
            &self.centers
        } else {
            &empty
        });
        // Hatch crossing hits are live alongside domains and staged paths.
        let retained = self
            .admit()?
            .checked_add(region::point_count(ordinary).max(region::point_count(interface_centers)))
            .filter(|n| *n <= crate::working_set::units())
            .ok_or_else(|| resource("combined support hatch scratch budget exceeded"))?;
        // Every path has at most twice as many points as segments. Reserve the
        // live geometry allowance before wall/hatch allocation, independently
        // of the combined published object/support segment allowance.
        // Region producers grow Vecs internally. Reserve their known worst
        // simultaneous old/new path-record and point-buffer capacities before
        // entering them; release this reservation when actual capacities return.
        let scratch_bytes = region::point_count(ordinary)
            .max(region::point_count(interface_centers))
            .saturating_mul(std::mem::size_of::<f64>());
        let live_bytes = self
            .owned_bytes
            .saturating_add(scratch_bytes)
            .saturating_add(4 * std::mem::size_of::<DepositionPath>());
        super::super::remaining_geometry_units(retained, live_bytes)?;
        let output_stride =
            3 * std::mem::size_of::<DepositionPath>() + 6 * std::mem::size_of::<[f64; 3]>();
        let allowance = (crate::working_set::units().saturating_sub(existing_segments))
            .min((crate::working_set::units() - retained) / 2)
            .min((crate::working_set::bytes() - live_bytes) / output_stride);
        let mut segments = crate::working_set::units().saturating_sub(allowance);
        let mut paths = Vec::new();
        let wall_centers = self.wall_centers.as_ref().unwrap_or(&self.centers);
        if interface.0.is_empty() || self.all_interface {
            let kind = if self.all_interface {
                DepositionKind::SupportInterface
            } else {
                DepositionKind::Support
            };
            region::support_contour_paths(
                wall_centers,
                layer.z,
                layer.lower_z,
                layer.width_mm,
                kind,
                &mut paths,
                &mut segments,
                cancel,
            )?;
        } else {
            // Preserve the complete exterior/hole wall, including every original
            // vertex. Only process-role crossings split it; no seam is offset.
            walls(
                WallContours {
                    centers: wall_centers,
                    interface,
                },
                layer,
                &mut paths,
                &mut segments,
                retained,
                self.owned_bytes,
                work,
                foreign_work,
                cancel,
            )?;
        }
        let omitted_wall_mm = commands::fit(
            &self.centers,
            &mut paths,
            &mut segments,
            retained,
            self.owned_bytes,
            work,
            foreign_work,
            cancel,
        )?;
        let wall_paths = paths.len();
        for (domain, kind, fraction) in [
            (ordinary, DepositionKind::Support, density),
            (interface_centers, DepositionKind::SupportInterface, 1.0),
        ] {
            charge_hatch(domain, fraction, layer, work, foreign_work, cancel)?;
            region::hatch_support(
                domain,
                kind,
                fraction,
                layer,
                &mut paths,
                &mut segments,
                cancel,
            )?;
        }
        candidate_budget(&paths, self.admit()?, self.owned_bytes)?;
        Ok((
            paths,
            WallReceipt {
                paths: wall_paths,
                omitted_mm: omitted_wall_mm,
            },
        ))
    }

    pub(super) fn anchor_budget(
        &self,
        paths: &Vec<DepositionPath>,
    ) -> Result<usize, ManufacturingError> {
        // The active query reservation owns the whole reusable workspace;
        // its existing capacity must not also remain in the outside pool.
        candidate_budget(
            paths,
            self.admit()?,
            self.owned_bytes.saturating_sub(self.anchor_owned_bytes),
        )
    }
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

fn charge_hatch(
    domain: &MultiPolygon<f64>,
    fraction: f64,
    layer: &region::HatchLayer<'_>,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    for polygon in &domain.0 {
        check_cancel(cancel)?;
        let rows = region::HatchRows::support(polygon, fraction, layer)?;
        charge(work, rows.point_bound()?, foreign_work, cancel)?;
    }
    Ok(())
}

struct WallContours<'a> {
    centers: &'a MultiPolygon<f64>,
    interface: &'a MultiPolygon<f64>,
}

struct WallRing {
    z: f64,
    lower_z: f64,
    bead_width_mm: f64,
    first_path: usize,
}

#[expect(
    clippy::too_many_arguments,
    reason = "Shared role-cut construction keeps immutable ownership, reusable scratch and live resource/work limits explicit"
)]
fn role_cuts(
    [first, last]: [Coord<f64>; 2],
    interface: &MultiPolygon<f64>,
    cuts: &mut Vec<(f64, Coord<f64>)>,
    point_budget: usize,
    owned_bytes: usize,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    cuts.clear();
    let delta = Coord {
        x: last.x - first.x,
        y: last.y - first.y,
    };
    let length2 = delta.x * delta.x + delta.y * delta.y;
    let mut add_cut = |point: Coord<f64>| -> Result<(), ManufacturingError> {
        let parameter = ((point.x - first.x) * delta.x + (point.y - first.y) * delta.y) / length2;
        if !parameter.is_finite() {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "nonfinite support wall role crossing",
            ));
        }
        if parameter > 0.0 && parameter < 1.0 {
            if cuts.len() >= point_budget {
                return Err(resource(
                    "combined support wall/crossing point budget exceeded",
                ));
            }
            let live = owned_bytes.saturating_add(super::vec_bytes(cuts));
            super::reserve_owned(cuts, 1, live)?;
            cuts.push((parameter, point));
        }
        Ok(())
    };
    for owner in &interface.0 {
        for boundary in std::iter::once(owner.exterior()).chain(owner.interiors()) {
            for side in boundary.0.windows(2) {
                charge(work, 1, foreign_work, cancel)?;
                match line_intersection(Line::new(first, last), Line::new(side[0], side[1])) {
                    Some(LineIntersection::SinglePoint { intersection, .. }) => {
                        add_cut(intersection)?
                    }
                    Some(LineIntersection::Collinear { intersection }) => {
                        add_cut(intersection.start)?;
                        add_cut(intersection.end)?;
                    }
                    None => {}
                }
            }
        }
    }
    charge(
        work,
        cuts.len()
            .saturating_mul((usize::BITS - cuts.len().max(1).leading_zeros()) as usize + 1),
        foreign_work,
        cancel,
    )?;
    cuts.sort_unstable_by(|left, right| left.0.total_cmp(&right.0));
    cuts.dedup_by(|left, right| left.1 == right.1);
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "Wall raster construction explicitly charges live geometry, output segments and foreign work"
)]
fn walls(
    contours: WallContours<'_>,
    layer: &region::HatchLayer<'_>,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    retained: usize,
    owned_bytes: usize,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let WallContours { centers, interface } = contours;
    let z = layer.z;
    let mut cuts = Vec::new();
    let mut path_points = 0usize;
    for polygon in &centers.0 {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            let wall_ring = WallRing {
                z,
                lower_z: layer.lower_z,
                bead_width_mm: layer.width_mm,
                first_path: paths.len(),
            };
            for edge in ring.0.windows(2) {
                check_cancel(cancel)?;
                let a = edge[0];
                let b = edge[1];
                role_cuts(
                    [a, b],
                    interface,
                    &mut cuts,
                    crate::working_set::units() - retained - path_points,
                    owned_bytes.saturating_add(super::super::bonds::path_bytes(paths)),
                    work,
                    foreign_work,
                    cancel,
                )?;
                let mut start = a;
                let mut owned_start = a;
                let mut owned_kind = None;
                for end in cuts.iter().map(|cut| cut.1).chain(std::iter::once(b)) {
                    charge(work, region::point_count(interface), foreign_work, cancel)?;
                    let midpoint = Point::new(
                        start.x + (end.x - start.x) * 0.5,
                        start.y + (end.y - start.y) * 0.5,
                    );
                    let kind = if interface.intersects(&midpoint) {
                        DepositionKind::SupportInterface
                    } else {
                        DepositionKind::Support
                    };
                    if let Some(previous_kind) = owned_kind
                        && previous_kind != kind
                    {
                        wall_piece(
                            [owned_start, start],
                            previous_kind,
                            &wall_ring,
                            paths,
                            segments,
                            &mut path_points,
                            crate::working_set::units() - retained - cuts.len(),
                            owned_bytes.saturating_add(super::vec_bytes(&cuts)),
                        )?;
                        owned_start = start;
                    }
                    owned_kind = Some(kind);
                    start = end;
                }
                // Tangencies and same-role crossings insert no extra point.
                // The original edge endpoint is always retained.
                wall_piece(
                    [owned_start, b],
                    owned_kind.unwrap(),
                    &wall_ring,
                    paths,
                    segments,
                    &mut path_points,
                    crate::working_set::units() - retained - cuts.len(),
                    owned_bytes.saturating_add(super::vec_bytes(&cuts)),
                )?;
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
    previous: impl Iterator<Item = &'a DepositionPath> + Clone,
    contacts: impl Iterator<Item = &'a DepositionPath> + Clone,
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
    for (path_index, path) in original.into_iter().enumerate() {
        anchors.set_budget(candidate_budget(paths, original_units, original_bytes)?);
        if !super::support_role(path.kind)
            || scan(
                std::slice::from_ref(&path),
                previous.clone(),
                contacts.clone(),
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
            if scan(
                std::slice::from_ref(&fragment),
                previous.clone(),
                contacts.clone(),
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
mod tests {
    use super::*;
    use geo::{LineString, Polygon};

    #[test]
    fn unrooted_islands_cannot_erase_an_independent_deposited_dependency_chain() {
        let recipe = crate::planner::tests::recipe(1.0);
        let cancel = AtomicBool::new(false);
        for shift in [0.0, 0.37] {
            let row = |start: [f64; 2], end: [f64; 2], bottom, top| DepositionPath {
                material_domain: None,
                kind: DepositionKind::Support,
                width: WidthProfile::Constant { width_mm: 0.6 },
                physical_z_mm: [bottom, top],
                owner: None,
                points_mm: vec![
                    [start[0] + shift, start[1] + shift, top],
                    [end[0] + shift, end[1] + shift, top],
                ],
                arcs: Vec::new(),
                exposed: false,
                connectors: Vec::new(),
            };
            let lower =
                [0.0, 2.0, 4.0, 6.0].map(|offset| row([0.0, offset], [10.0, offset], 0.0, 0.2));
            let rooted = row([5.0, 0.0], [5.0, 6.0], 0.2, 0.4);
            let mut floating = row([20.0, 0.0], [21.0, 0.0], 0.2, 0.4);
            floating.points_mm = [
                [20.0, 0.0],
                [21.0, 0.0],
                [21.0, 1.0],
                [20.0, 1.0],
                [20.0, 0.0],
            ]
            .map(|point| [point[0] + shift, point[1] + shift, 0.4])
            .to_vec();
            let overextended = row([7.0, 0.0], [7.0, 8.0], 0.2, 0.4);
            let mut candidate = vec![floating, rooted.clone(), overextended];
            let mut anchors = AnchorWorkspace::default();
            let mut work = 0;
            let omitted = retain_anchored(
                &mut candidate,
                1,
                lower.iter(),
                std::iter::empty(),
                &recipe,
                0,
                0,
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert_eq!(candidate, [rooted]);
            assert!((omitted.omitted_mm - 4.0).abs() <= crate::planner::OFFSET_TOLERANCE_MM);
            let upper = [row([5.0, 1.0], [5.0, 5.0], 0.4, 0.6)];
            assert!(
                scan(
                    &upper,
                    candidate.iter(),
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                    true
                )
                .unwrap()
                .is_none()
            );
            let cancelled = retain_anchored(
                &mut candidate,
                0,
                lower.iter(),
                std::iter::empty(),
                &recipe,
                0,
                0,
                &mut anchors,
                &mut work,
                &AtomicBool::new(true),
            )
            .unwrap_err();
            assert_eq!(cancelled.code, ManufacturingErrorCode::Cancelled);
        }
    }

    #[test]
    fn a_partially_rooted_closed_wall_keeps_roads_on_both_sides_of_its_seam() {
        let recipe = crate::planner::tests::recipe(1.0);
        let cancel = AtomicBool::new(false);
        for shift in [0.0, 0.37] {
            let make_path = |points: &[[f64; 2]], bottom, top| DepositionPath {
                material_domain: None,
                kind: DepositionKind::Support,
                width: WidthProfile::Constant { width_mm: 0.6 },
                physical_z_mm: [bottom, top],
                owner: None,
                points_mm: points
                    .iter()
                    .map(|point| [point[0] + shift, point[1] + shift, top])
                    .collect(),
                arcs: Vec::new(),
                exposed: false,
                connectors: Vec::new(),
            };
            let lower = make_path(&[[0.0, 0.0], [10.0, 0.0]], 0.0, 0.2);
            let mut candidate = vec![make_path(
                &[
                    [5.0, 0.0],
                    [10.0, 0.0],
                    [10.0, 6.0],
                    [0.0, 6.0],
                    [0.0, 0.0],
                    [5.0, 0.0],
                ],
                0.2,
                0.4,
            )];
            let mut anchors = AnchorWorkspace::default();
            let mut work = 0;
            let omitted = retain_anchored(
                &mut candidate,
                1,
                std::iter::once(&lower),
                std::iter::empty(),
                &recipe,
                0,
                0,
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!((omitted.omitted_mm - 22.0).abs() <= crate::planner::OFFSET_TOLERANCE_MM);
            let retained_length: f64 = candidate
                .iter()
                .flat_map(|path| path.points_mm.windows(2))
                .map(|edge| (edge[1][0] - edge[0][0]).hypot(edge[1][1] - edge[0][1]))
                .sum();
            assert!((retained_length - 10.0).abs() <= crate::planner::OFFSET_TOLERANCE_MM);
            let upper = make_path(&[[1.0, 0.0], [9.0, 0.0]], 0.4, 0.6);
            assert!(
                scan(
                    std::slice::from_ref(&upper),
                    candidate.iter(),
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                    true
                )
                .unwrap()
                .is_none()
            );
        }
    }

    #[test]
    fn a_floating_island_does_not_choose_an_axis_that_loses_rooted_contact() {
        let mut recipe = crate::planner::tests::recipe(1.0);
        recipe.process.max_bridge_span_mm = 0.5;
        let cancel = AtomicBool::new(false);
        let width = recipe.bead_width_mm;
        let pitch = (width - recipe.layer_height_mm
            + std::f64::consts::PI * recipe.layer_height_mm * 0.25)
            / 0.3;
        let lower: Vec<_> = (0..6)
            .map(|index| DepositionPath {
                material_domain: None,
                kind: DepositionKind::Support,
                width: WidthProfile::Constant { width_mm: width },
                physical_z_mm: [0.0, 0.2],
                owner: None,
                points_mm: vec![
                    [f64::from(index) * pitch, 0.0, 0.2],
                    [f64::from(index) * pitch, 10.0, 0.2],
                ],
                arcs: Vec::new(),
                exposed: false,
                connectors: Vec::new(),
            })
            .collect();
        let mut support = rectangle(
            -width * 0.5,
            -width * 0.5,
            10.0 + width * 0.5,
            10.0 + width * 0.5,
        );
        support.0.extend(rectangle(30.0, 0.0, 31.0, 100.0).0);
        let interface = MultiPolygon(Vec::new());
        let mut anchors = AnchorWorkspace::default();
        let mut work = 0;
        let domains = Domains::new(
            &support, &interface, false, width, 0, 0, 0, &mut work, 0, &cancel,
        )
        .unwrap();
        let candidates = [region::HatchAxis::X, region::HatchAxis::Y].map(|axis| {
            let hatch = region::HatchLayer {
                roof: None,
                width_mm: width,
                z: 0.4,
                lower_z: 0.2,
                height: 0.2,
                axis,
                top_axis: axis,
                recipe: &recipe,
            };
            let (mut paths, receipt) = domains
                .paths(&interface, 0.3, &hatch, 0, &mut work, 0, &cancel)
                .unwrap();
            retain_anchored(
                &mut paths,
                receipt.paths,
                lower.iter(),
                std::iter::empty(),
                &recipe,
                0,
                domains.owned_bytes(),
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            paths
        });
        assert!(
            nominal_volume(&candidates[1], 1, &recipe).unwrap()
                > nominal_volume(&candidates[0], 1, &recipe).unwrap()
        );
        let mut upper = lower[2].clone();
        upper.physical_z_mm = [0.4, 0.6];
        for point in &mut upper.points_mm {
            point[2] = 0.6;
        }
        assert!(
            scan(
                std::slice::from_ref(&upper),
                candidates[0].iter(),
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut work,
                &cancel,
                true
            )
            .unwrap()
            .is_some()
        );
        assert!(
            scan(
                std::slice::from_ref(&upper),
                candidates[1].iter(),
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut work,
                &cancel,
                true
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn support_rows_keep_their_physical_lattice_across_clipping_and_thinner_layers() {
        let recipe = crate::planner::tests::recipe(1.0);
        let cancel = AtomicBool::new(false);
        for axis in [region::HatchAxis::X, region::HatchAxis::Y] {
            for density in [0.3, 1.0] {
                let generate = |domain: &MultiPolygon<f64>, height| {
                    let mut paths = Vec::new();
                    region::hatch_support(
                        domain,
                        DepositionKind::Support,
                        density,
                        &region::HatchLayer {
                            roof: None,
                            width_mm: 0.6,
                            z: 0.8,
                            lower_z: 0.8 - height,
                            height,
                            axis,
                            top_axis: axis,
                            recipe: &recipe,
                        },
                        &mut paths,
                        &mut 0,
                        &cancel,
                    )
                    .unwrap();
                    paths
                };
                let full = rectangle(0.0, 0.0, 20.0, 20.0);
                let clipped = rectangle(1.13, 0.27, 17.2, 18.0);
                let original = generate(&full, 0.4);
                let thinner = generate(&clipped, 0.2);
                assert!(!thinner.is_empty());
                let perpendicular = usize::from(axis == region::HatchAxis::X);
                for path in thinner {
                    assert_eq!(path.physical_z_mm, [0.8 - 0.2, 0.8]);
                    let coordinate = path.points_mm[0][perpendicular];
                    assert!(original.iter().any(|reference| {
                        (reference.points_mm[0][perpendicular] - coordinate).abs()
                            <= crate::planner::OFFSET_TOLERANCE_MM
                    }));
                }
            }
        }
    }

    #[test]
    fn optional_role_fragments_reach_command_fitting_and_report_omission() {
        let centers = rectangle(0.0, 0.0, 2.0, 2.0);
        let ring = WallRing {
            z: 1.0,
            lower_z: 0.0,
            bead_width_mm: 1.0,
            first_path: 0,
        };
        let mut paths = Vec::new();
        let mut segments = 0;
        let mut points = 0;
        wall_piece(
            [
                Coord { x: 0.5, y: 0.5 },
                Coord {
                    x: 0.5
                        + spiling_contracts::manufacturing::program_coordinates::XYZ6_QUANTUM_MM
                            * 0.25,
                    y: 0.5,
                },
            ],
            DepositionKind::SupportInterface,
            &ring,
            &mut paths,
            &mut segments,
            &mut points,
            crate::working_set::units(),
            0,
        )
        .unwrap();
        let omitted = commands::fit(
            &centers,
            &mut paths,
            &mut segments,
            0,
            0,
            &mut 0,
            0,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(paths.is_empty());
        assert!(omitted > 0.0);
    }

    #[test]
    fn unpartitioned_support_contours_fit_subcommand_original_edges() {
        let centers = MultiPolygon(vec![Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (0.0, 2.0),
                (2.0, 2.0),
                (2.0, 0.0),
                (0.000001, 0.0),
                (0.0, 0.0),
            ]),
            Vec::new(),
        )]);
        for kind in [DepositionKind::Support, DepositionKind::SupportInterface] {
            let mut paths = Vec::new();
            let mut segments = 0;
            let cancel = AtomicBool::new(false);
            region::support_contour_paths(
                &centers,
                1.0,
                0.8,
                0.4,
                kind,
                &mut paths,
                &mut segments,
                &cancel,
            )
            .unwrap();
            let omitted = commands::fit(
                &centers,
                &mut paths,
                &mut segments,
                0,
                0,
                &mut 0,
                0,
                &cancel,
            )
            .unwrap();
            assert_eq!(omitted, 0.0);
            assert!(!paths.is_empty());
            for path in paths {
                assert_eq!(path.kind, kind);
                assert_eq!(path.points_mm.first(), path.points_mm.last());
                for edge in path.points_mm.windows(2) {
                    assert!(
                        commands::command_length(
                            Coord {
                                x: edge[0][0],
                                y: edge[0][1]
                            },
                            Coord {
                                x: edge[1][0],
                                y: edge[1][1]
                            },
                        )
                        .unwrap()
                    );
                }
            }
        }
    }

    #[test]
    fn support_domain_retention_is_not_a_command_count() {
        let support = rectangle(0.0, 0.0, 2.0, 2.0);
        let interface = MultiPolygon(Vec::new());
        let cancel = AtomicBool::new(false);
        assert!(
            Domains::new(
                &support, &interface, false, 0.6,
                // The former 2M command cap is not a retention limit.
                2_000_001, 0, 0, &mut 0, 0, &cancel
            )
            .is_ok()
        );
        assert!(
            Domains::new(
                &support,
                &interface,
                false,
                0.6,
                0,
                crate::working_set::bytes(),
                0,
                &mut 0,
                0,
                &cancel
            )
            .is_err()
        );
    }

    fn rectangle(x: f64, y: f64, width: f64, height: f64) -> MultiPolygon<f64> {
        MultiPolygon(vec![Polygon::new(
            LineString::from(vec![
                (x, y),
                (x + width, y),
                (x + width, y + height),
                (x, y + height),
                (x, y),
            ]),
            vec![],
        )])
    }

    #[test]
    fn sub_bead_interface_transition_preserves_actual_union_wall_anchors() {
        let cancel = AtomicBool::new(false);
        let mut work = 0;
        let mut recipe = crate::planner::tests::recipe(1.0);
        recipe.layer_height_mm = 0.575;
        let support = rectangle(0.0, 0.0, 20.0, 5.0);
        let lower_interface = rectangle(0.0, 0.0, 20.0, 0.425);
        let upper_interface = rectangle(0.0, 0.0, 20.0, 1.675);
        assert!(
            region::offset(&lower_interface, -0.3, &cancel)
                .unwrap()
                .0
                .is_empty()
        );
        let lower = Domains::new(
            &support,
            &lower_interface,
            false,
            0.6,
            0,
            0,
            0,
            &mut work,
            0,
            &cancel,
        )
        .unwrap();
        let upper = Domains::new(
            &support,
            &upper_interface,
            false,
            0.6,
            0,
            0,
            0,
            &mut work,
            0,
            &cancel,
        )
        .unwrap();
        let previous = lower
            .paths(
                &lower_interface,
                1.0,
                &region::HatchLayer {
                    roof: None,
                    width_mm: recipe.bead_width_mm,
                    z: 0.575,
                    lower_z: 0.0,
                    height: 0.575,
                    axis: region::HatchAxis::Y,
                    top_axis: region::HatchAxis::Y,
                    recipe: &recipe,
                },
                0,
                &mut work,
                0,
                &cancel,
            )
            .unwrap()
            .0;
        let current = upper
            .paths(
                &upper_interface,
                1.0,
                &region::HatchLayer {
                    roof: None,
                    width_mm: recipe.bead_width_mm,
                    z: 1.15,
                    lower_z: 0.575,
                    height: 0.575,
                    axis: region::HatchAxis::X,
                    top_axis: region::HatchAxis::X,
                    recipe: &recipe,
                },
                super::super::path_segments(&previous),
                &mut work,
                0,
                &cancel,
            )
            .unwrap()
            .0;
        assert!(
            previous
                .iter()
                .any(|path| path.kind == DepositionKind::SupportInterface)
        );
        assert!(
            previous
                .iter()
                .any(|path| path.kind == DepositionKind::Support)
        );
        let mut anchors = AnchorWorkspace::default();
        anchors.set_budget(upper.anchor_budget(&current).unwrap());
        assert!(
            failure(
                &current,
                previous.iter(),
                &recipe,
                &mut anchors,
                &mut work,
                &cancel
            )
            .unwrap()
            .is_none()
        );
        // Process ownership is not an anchor certificate: the identical raster
        // still fails when no actual support was deposited underneath it.
        anchors.set_budget(upper.anchor_budget(&current).unwrap());
        let first = failure(
            &current,
            std::iter::empty(),
            &recipe,
            &mut anchors,
            &mut work,
            &cancel,
        )
        .unwrap()
        .unwrap();
        // The span a degraded layer reports is the worst of every stroke,
        // not the first one found.
        let worst = worst_failure(
            &current,
            std::iter::empty(),
            &recipe,
            &mut anchors,
            &mut work,
            &cancel,
        )
        .unwrap()
        .unwrap();
        assert!(worst.span >= first.span);
        let longest = current
            .iter()
            .flat_map(|path| path.points_mm.windows(2))
            .map(|edge| (edge[1][0] - edge[0][0]).hypot(edge[1][1] - edge[0][1]))
            .fold(0.0_f64, f64::max);
        assert!(worst.span >= longest - 1e-9);
        // Fully anchored rasters have no worst failure.
        assert!(
            worst_failure(
                &current,
                previous.iter(),
                &recipe,
                &mut anchors,
                &mut work,
                &cancel
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn role_partition_preserves_each_physical_exterior_and_hole_wall_once() {
        let mut centers = rectangle(0.0, 0.0, 20.0, 5.0);
        let hole = rectangle(1.0, 2.0, 2.0, 2.0).0.remove(0);
        centers.0[0].interiors_push(hole.exterior().clone());
        let interface = rectangle(4.0, -1.0, 8.0, 2.0);
        let mut paths = Vec::new();
        let mut segments = 0;
        let recipe = crate::planner::tests::recipe(1.0);
        walls(
            WallContours {
                centers: &centers,
                interface: &interface,
            },
            &region::HatchLayer {
                roof: None,
                width_mm: recipe.bead_width_mm,
                z: 0.6,
                lower_z: 0.0,
                height: 0.6,
                axis: region::HatchAxis::X,
                top_axis: region::HatchAxis::X,
                recipe: &recipe,
            },
            &mut paths,
            &mut segments,
            0,
            0,
            &mut 0,
            0,
            &AtomicBool::new(false),
        )
        .unwrap();
        let length: f64 = paths
            .iter()
            .flat_map(|p| p.points_mm.windows(2))
            .map(|e| (e[1][0] - e[0][0]).hypot(e[1][1] - e[0][1]))
            .sum();
        assert_eq!(length, 58.0);
        assert_eq!(segments, 10);
        for ring in std::iter::once(centers.0[0].exterior()).chain(centers.0[0].interiors()) {
            for vertex in &ring.0 {
                assert!(
                    paths
                        .iter()
                        .flat_map(|p| &p.points_mm)
                        .any(|p| p[0] == vertex.x && p[1] == vertex.y)
                );
            }
        }
        assert!(
            paths
                .iter()
                .filter(|p| p.kind == DepositionKind::SupportInterface)
                .flat_map(|p| p.points_mm.windows(2))
                .all(|e| e[0][1] == 0.0 && e[1][1] == 0.0)
        );
    }

    #[test]
    fn support_raster_staging_preserves_segment_and_cancellation_admission() {
        let cancel = AtomicBool::new(false);
        let support = rectangle(0.0, 0.0, 20.0, 5.0);
        let interface = MultiPolygon(Vec::new());
        let domains = Domains::new(
            &support, &interface, false, 0.6, 0, 0, 0, &mut 0, 0, &cancel,
        )
        .unwrap();
        let recipe = crate::planner::tests::recipe(1.0);
        let layer = region::HatchLayer {
            roof: None,
            width_mm: recipe.bead_width_mm,
            z: 0.4,
            lower_z: 0.0,
            height: 0.4,
            axis: region::HatchAxis::X,
            top_axis: region::HatchAxis::X,
            recipe: &recipe,
        };
        assert_eq!(
            domains
                .paths(
                    &interface,
                    1.0,
                    &layer,
                    crate::working_set::units(),
                    &mut 0,
                    0,
                    &cancel
                )
                .unwrap_err()
                .code,
            ManufacturingErrorCode::ResourceLimit
        );
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(
            domains
                .paths(&interface, 1.0, &layer, 0, &mut 0, 0, &cancel)
                .is_err()
        );
    }
}
