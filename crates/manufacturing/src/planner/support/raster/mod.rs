// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{
    AnchorCandidate, AnchorWorkspace, StrokeRuns, candidate_budget, charge_boolean, region, strokes,
};
use crate::{check_cancel, error};
use geo::line_intersection::{LineIntersection, line_intersection};
use geo::{BooleanOps, BoundingRect, Coord, Intersects, Line, MultiPolygon, Point};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

/// Clearance belongs to the physical support union, not to process-role seams.
/// In particular a sub-bead interface strip must not erase the union's wall.
pub(super) struct Domains {
    centers: MultiPolygon<f64>,
    ordinary: Option<MultiPolygon<f64>>,
    interface: Option<MultiPolygon<f64>>,
    all_interface: bool,
    retained: usize,
}

impl Domains {
    pub(super) fn new(
        support: &MultiPolygon<f64>,
        interface: &MultiPolygon<f64>,
        all_interface: bool,
        width: f64,
        retained: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        charge_boolean(support, support, work, cancel)?;
        let centers = region::offset(support, -width * 0.5, cancel)?;
        let mut result = Self {
            centers,
            ordinary: None,
            interface: None,
            all_interface,
            retained,
        };
        result.admit()?;
        if !interface.0.is_empty() && !all_interface {
            charge_boolean(&result.centers, interface, work, cancel)?;
            result.interface = Some(result.centers.intersection(interface));
            region::check_size(result.interface.as_ref().unwrap())?;
            result.admit()?;
            charge_boolean(&result.centers, interface, work, cancel)?;
            result.ordinary = Some(result.centers.difference(interface));
            region::check_size(result.ordinary.as_ref().unwrap())?;
            result.admit()?;
        }
        check_cancel(cancel)?;
        Ok(result)
    }

    fn admit(&self) -> Result<usize, ManufacturingError> {
        let points = std::iter::once(&self.centers)
            .chain(self.ordinary.iter())
            .chain(self.interface.iter())
            .try_fold(self.retained, |total, domain| {
                total
                    .checked_add(region::point_count(domain))
                    .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
                    .ok_or_else(|| {
                        resource("combined support centerline ownership budget exceeded")
                    })
            })?;
        Ok(points)
    }

    pub(super) fn paths(
        &self,
        interface: &MultiPolygon<f64>,
        density: f64,
        layer: &region::HatchLayer<'_>,
        existing_segments: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<DepositionPath>, ManufacturingError> {
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
            .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
            .ok_or_else(|| resource("combined support hatch scratch budget exceeded"))?;
        // Every path has at most twice as many points as segments. Reserve the
        // live geometry allowance before wall/hatch allocation, independently
        // of the combined published object/support segment allowance.
        let allowance = (MAX_DEPOSITION_SEGMENTS as usize - existing_segments)
            .min((MAX_DEPOSITION_SEGMENTS as usize - retained) / 2);
        let mut segments = MAX_DEPOSITION_SEGMENTS as usize - allowance;
        let mut paths = Vec::new();
        if interface.0.is_empty() || self.all_interface {
            region::wall_paths(&self.centers, layer.z, &mut paths, &mut segments, cancel)?;
            for path in &mut paths {
                path.kind = if self.all_interface {
                    DepositionKind::SupportInterface
                } else {
                    DepositionKind::Support
                };
            }
        } else {
            // Preserve the complete exterior/hole wall, including every original
            // vertex. Only process-role crossings split it; no seam is offset.
            walls(
                &self.centers,
                interface,
                layer.z,
                &mut paths,
                &mut segments,
                retained,
                work,
                cancel,
            )?;
        }
        for (domain, kind, fraction) in [
            (ordinary, DepositionKind::Support, density),
            (interface_centers, DepositionKind::SupportInterface, 1.0),
        ] {
            charge_hatch(domain, fraction, layer, work, cancel)?;
            region::hatch(
                domain,
                kind,
                fraction,
                layer,
                &mut paths,
                &mut segments,
                cancel,
            )?;
        }
        candidate_budget(&paths, retained)?;
        Ok(paths)
    }

    pub(super) fn anchor_budget(
        &self,
        paths: &[DepositionPath],
    ) -> Result<usize, ManufacturingError> {
        candidate_budget(paths, self.admit()?)
    }
}

fn resource(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::ResourceLimit, message)
}

fn charge(work: &mut usize, amount: usize, cancel: &AtomicBool) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    *work = work
        .checked_add(amount)
        .filter(|n| *n <= 20_000_000)
        .ok_or_else(|| resource("cumulative support raster ownership work budget exceeded"))?;
    Ok(())
}

fn charge_hatch(
    domain: &MultiPolygon<f64>,
    fraction: f64,
    layer: &region::HatchLayer<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let width = layer.recipe.bead_width_mm;
    let h = layer.height;
    let spacing = ((width - h) * h + std::f64::consts::PI * (h * 0.5).powi(2)) / h;
    for polygon in &domain.0 {
        check_cancel(cancel)?;
        let bounds = polygon.bounding_rect().ok_or_else(|| {
            error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "support hatch has no finite bounds",
            )
        })?;
        let extent = if layer.axis == region::HatchAxis::X {
            bounds.max().y - bounds.min().y
        } else {
            bounds.max().x - bounds.min().x
        };
        let rows = ((extent / spacing) * fraction).ceil().max(1.0);
        if !rows.is_finite() || rows > MAX_DEPOSITION_SEGMENTS as f64 {
            return Err(resource("support hatch row budget exceeded"));
        }
        let edges = polygon.exterior().0.len()
            + polygon
                .interiors()
                .iter()
                .map(|ring| ring.0.len())
                .sum::<usize>();
        charge(
            work,
            (rows as usize)
                .checked_mul(edges)
                .ok_or_else(|| resource("support hatch work overflow"))?,
            cancel,
        )?;
    }
    Ok(())
}

fn walls(
    centers: &MultiPolygon<f64>,
    interface: &MultiPolygon<f64>,
    z: f64,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    retained: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut cuts = Vec::new();
    let mut path_points = 0usize;
    for polygon in &centers.0 {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            let first_path = paths.len();
            for edge in ring.0.windows(2) {
                check_cancel(cancel)?;
                cuts.clear();
                let a = edge[0];
                let b = edge[1];
                let delta = Coord {
                    x: b.x - a.x,
                    y: b.y - a.y,
                };
                let length2 = delta.x * delta.x + delta.y * delta.y;
                let mut add_cut = |point: Coord<f64>| -> Result<(), ManufacturingError> {
                    let t = ((point.x - a.x) * delta.x + (point.y - a.y) * delta.y) / length2;
                    if !t.is_finite() {
                        return Err(error(
                            ManufacturingErrorCode::UnsupportedGeometry,
                            "nonfinite support wall role crossing",
                        ));
                    }
                    if t > 0.0 && t < 1.0 {
                        if cuts.len() + path_points >= MAX_DEPOSITION_SEGMENTS as usize - retained {
                            return Err(resource(
                                "combined support wall/crossing point budget exceeded",
                            ));
                        }
                        cuts.push((t, point));
                    }
                    Ok(())
                };
                for owner in &interface.0 {
                    for boundary in std::iter::once(owner.exterior()).chain(owner.interiors()) {
                        for side in boundary.0.windows(2) {
                            charge(work, 1, cancel)?;
                            match line_intersection(Line::new(a, b), Line::new(side[0], side[1])) {
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
                cuts.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
                cuts.dedup_by(|a, b| a.1 == b.1);
                let mut start = a;
                let mut owned_start = a;
                let mut owned_kind = None;
                for end in cuts.iter().map(|cut| cut.1).chain(std::iter::once(b)) {
                    charge(work, region::point_count(interface), cancel)?;
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
                            z,
                            first_path,
                            paths,
                            segments,
                            &mut path_points,
                            MAX_DEPOSITION_SEGMENTS as usize - retained - cuts.len(),
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
                    z,
                    first_path,
                    paths,
                    segments,
                    &mut path_points,
                    MAX_DEPOSITION_SEGMENTS as usize - retained - cuts.len(),
                )?;
            }
        }
    }
    Ok(())
}

fn wall_piece(
    edge: [Coord<f64>; 2],
    kind: DepositionKind,
    z: f64,
    first_path: usize,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    path_points: &mut usize,
    point_budget: usize,
) -> Result<(), ManufacturingError> {
    let [start, end] = edge;
    let length = (end.x - start.x).hypot(end.y - start.y);
    if !length.is_finite() || length <= 0.000002 {
        return Err(error(
            ManufacturingErrorCode::UnsupportedGeometry,
            "support wall role crossing creates unresolved deposition",
        ));
    }
    if *segments >= MAX_DEPOSITION_SEGMENTS as usize {
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
    if continuation {
        paths.last_mut().unwrap().points_mm.push(to);
    } else {
        paths.push(DepositionPath {
            kind,
            points_mm: vec![from, to],
        });
    }
    Ok(())
}

pub(super) fn failure<'a>(
    paths: &[DepositionPath],
    previous: impl Iterator<Item = &'a DepositionPath> + Clone,
    recipe: &PlanarPrintRecipe,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<strokes::Failure>, ManufacturingError> {
    for (path_index, path) in paths.iter().enumerate() {
        for (first, last) in StrokeRuns::new(&path.points_mm) {
            let a = path.points_mm[first];
            let b = path.points_mm[last];
            let length = (b[0] - a[0]).hypot(b[1] - a[1]);
            for gap in anchors.gaps(
                AnchorCandidate {
                    endpoints_mm: [a, b],
                    bead_width_mm: recipe.bead_width_mm,
                },
                previous.clone(),
                std::iter::empty(),
                work,
                cancel,
            )? {
                let span = (gap.end - gap.start) * length;
                if matches!(
                    strokes::gap_kind(
                        gap,
                        span,
                        recipe.bead_width_mm,
                        recipe.process.max_bridge_span_mm
                    ),
                    strokes::GapKind::Demand
                ) {
                    return Ok(Some(strokes::Failure {
                        path: path_index,
                        first,
                        last,
                        start: gap.start,
                        end: gap.end,
                        span,
                    }));
                }
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{LineString, Polygon};

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
            &mut work,
            &cancel,
        )
        .unwrap();
        let upper = Domains::new(
            &support,
            &upper_interface,
            false,
            0.6,
            0,
            &mut work,
            &cancel,
        )
        .unwrap();
        let previous = lower
            .paths(
                &lower_interface,
                1.0,
                &region::HatchLayer {
                    z: 0.575,
                    height: 0.575,
                    axis: region::HatchAxis::Y,
                    recipe: &recipe,
                },
                0,
                &mut work,
                &cancel,
            )
            .unwrap();
        let current = upper
            .paths(
                &upper_interface,
                1.0,
                &region::HatchLayer {
                    z: 1.15,
                    height: 0.575,
                    axis: region::HatchAxis::X,
                    recipe: &recipe,
                },
                super::super::path_segments(&previous),
                &mut work,
                &cancel,
            )
            .unwrap();
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
        // still refuses when no actual support was deposited underneath it.
        anchors.set_budget(upper.anchor_budget(&current).unwrap());
        assert!(
            failure(
                &current,
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut work,
                &cancel
            )
            .unwrap()
            .is_some()
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
        walls(
            &centers,
            &interface,
            0.6,
            &mut paths,
            &mut segments,
            0,
            &mut 0,
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
        let domains = Domains::new(&support, &interface, false, 0.6, 0, &mut 0, &cancel).unwrap();
        let recipe = crate::planner::tests::recipe(1.0);
        let layer = region::HatchLayer {
            z: 0.4,
            height: 0.4,
            axis: region::HatchAxis::X,
            recipe: &recipe,
        };
        assert_eq!(
            domains
                .paths(
                    &interface,
                    1.0,
                    &layer,
                    MAX_DEPOSITION_SEGMENTS as usize,
                    &mut 0,
                    &cancel
                )
                .unwrap_err()
                .code,
            ManufacturingErrorCode::ResourceLimit
        );
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(
            domains
                .paths(&interface, 1.0, &layer, 0, &mut 0, &cancel)
                .is_err()
        );
    }
}
