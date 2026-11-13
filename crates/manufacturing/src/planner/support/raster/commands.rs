// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Fit support wall proposals to the command grammar without changing S or I.
//! The original center domain and each complete, role-owned source span remain
//! available until its fitted path has been qualified. Parent anchor/clearance
//! checks consume the resulting strokes, never this construction's success.
//! A wall stretch that no admissible command can carry is omitted, splitting
//! the wall, and its length is returned for the layer's report.
use super::*;
use geo::Contains;
use geo::kernels::{Kernel, Orientation, RobustKernel};
use spiling_contracts::manufacturing::program_coordinates::{
    MIN_DEPOSITION_COMMAND_MM, project_xyz6_coordinate_mm,
};
use spiling_geometry::{Interval, IntervalVec2};

const MAX_WALL_FIT_MM: f64 = 0.001;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Chord {
    Fits,
    /// Shorter than one minimum command.
    Short,
    /// An original point lies beyond the wall error bound.
    Deviates,
    /// The chord leaves the original center domain.
    Outside,
}

struct Fit<'a> {
    centers: &'a MultiPolygon<f64>,
    convex: bool,
    work: &'a mut usize,
    foreign_work: usize,
    cancel: &'a AtomicBool,
}
impl Fit<'_> {
    fn charge(&mut self, count: usize) -> Result<(), ManufacturingError> {
        charge(self.work, count, self.foreign_work, self.cancel)
    }

    fn chord(
        &mut self,
        points: &[[f64; 3]],
        start: usize,
        end: usize,
    ) -> Result<bool, ManufacturingError> {
        Ok(self.classify(points, start, end)? == Chord::Fits)
    }

    fn classify(
        &mut self,
        points: &[[f64; 3]],
        start: usize,
        end: usize,
    ) -> Result<Chord, ManufacturingError> {
        self.charge(1)?;
        let a = xy(points[start]);
        let b = xy(points[end]);
        if !command_length(a, b)? {
            return Ok(Chord::Short);
        }
        if end == start + 1 {
            return Ok(Chord::Fits);
        }
        let ai = interval(a);
        let delta = interval(b).sub(ai);
        let squared = delta.norm_squared();
        for &point in &points[start + 1..end] {
            self.charge(1)?;
            let p = interval(xy(point));
            let t = p.sub(ai).dot(delta).div(squared).map_err(conditioning)?;
            let t =
                Interval::new(t.lo.clamp(0.0, 1.0), t.hi.clamp(0.0, 1.0)).map_err(conditioning)?;
            if p.sub(ai.add(delta.scale(t)))
                .norm_upper()
                .map_err(conditioning)?
                > MAX_WALL_FIT_MM
            {
                return Ok(Chord::Deviates);
            }
        }
        // A convex single component with no holes contains every chord between
        // its original wall points. Other domains retain the complete boundary
        // crossing check; proximity alone never grants permission across a void.
        if self.convex {
            return Ok(Chord::Fits);
        }
        self.charge(region::point_count(self.centers))?;
        let midpoint = Point::new(a.x * 0.5 + b.x * 0.5, a.y * 0.5 + b.y * 0.5);
        if !self.centers.contains(&midpoint) {
            return Ok(Chord::Outside);
        }
        for polygon in &self.centers.0 {
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                for edge in ring.0.windows(2) {
                    self.charge(1)?;
                    match line_intersection(Line::new(a, b), Line::new(edge[0], edge[1])) {
                        None => {}
                        Some(LineIntersection::SinglePoint { intersection, .. })
                            if intersection == a || intersection == b => {}
                        _ => return Ok(Chord::Outside),
                    }
                }
            }
        }
        Ok(Chord::Fits)
    }

    /// The chords covering the most original wall length, when no complete
    /// chain exists. Every chosen chord is admissible; the skipped original
    /// edges are each shorter than one command (any longer edge is itself an
    /// admissible chord). Returns the pieces as index chains and the omitted
    /// original length.
    fn cover(
        &mut self,
        points: &[[f64; 3]],
        best: &mut Vec<(f64, usize)>,
    ) -> Result<(Vec<Vec<usize>>, f64), ManufacturingError> {
        let last = points.len() - 1;
        best.clear();
        best.resize(points.len(), (0.0, usize::MAX));
        for start in (0..last).rev() {
            best[start] = (best[start + 1].0, usize::MAX);
            for end in start + 1..=last {
                match self.classify(points, start, end)? {
                    Chord::Fits => {
                        let a = points[start];
                        let b = points[end];
                        let covered = (b[0] - a[0]).hypot(b[1] - a[1]) + best[end].0;
                        if covered > best[start].0 {
                            best[start] = (covered, end);
                        }
                    }
                    // Longer chords from this start only deviate further on
                    // the smooth sampled walls this fallback serves; stopping
                    // forgoes options, never admits an unchecked chord.
                    Chord::Deviates => break,
                    Chord::Short | Chord::Outside => {}
                }
            }
        }
        let mut pieces: Vec<Vec<usize>> = Vec::new();
        let mut omitted = 0.0;
        let mut piece = Vec::new();
        let mut at = 0;
        while at < last {
            self.charge(1)?;
            let next = best[at].1;
            if next == usize::MAX {
                let a = points[at];
                let b = points[at + 1];
                omitted += (b[0] - a[0]).hypot(b[1] - a[1]);
                if !piece.is_empty() {
                    pieces.push(std::mem::take(&mut piece));
                }
                at += 1;
            } else {
                if piece.is_empty() {
                    piece.push(at);
                }
                piece.push(next);
                at = next;
            }
        }
        if !piece.is_empty() {
            pieces.push(piece);
        }
        // A closed ring omitted only between its pieces may restart at the
        // first omission: its final piece continues through the seam.
        if points[0] == points[last]
            && pieces.len() > 1
            && pieces[0][0] == 0
            && pieces.last().and_then(|piece| piece.last()) == Some(&last)
        {
            let first = pieces.remove(0);
            pieces.last_mut().unwrap().extend(first.into_iter().skip(1));
        }
        Ok((pieces, omitted))
    }
}

fn xy(p: [f64; 3]) -> Coord<f64> {
    Coord { x: p[0], y: p[1] }
}
fn interval(p: Coord<f64>) -> IntervalVec2 {
    IntervalVec2::point(p.x, p.y)
}
fn conditioning(e: spiling_contracts::geometry::GeometryError) -> ManufacturingError {
    error(
        ManufacturingErrorCode::UnsupportedGeometry,
        format!("support wall command conditioning: {}", e.message),
    )
}
pub(super) fn command_length(a: Coord<f64>, b: Coord<f64>) -> Result<bool, ManufacturingError> {
    // Use the same declared XYZ6 command floor as final realization. Interval
    // rounding must not turn an exactly minimum-length straight command into
    // a newly unsupported path; geometric deviation is enclosed separately.
    let a = [
        project_xyz6_coordinate_mm(a.x)?,
        project_xyz6_coordinate_mm(a.y)?,
    ];
    let b = [
        project_xyz6_coordinate_mm(b.x)?,
        project_xyz6_coordinate_mm(b.y)?,
    ];
    Ok((b[0] - a[0]).hypot(b[1] - a[1]) >= MIN_DEPOSITION_COMMAND_MM)
}

/// `paths` are the just-constructed walls of `centers`, before any hatch is
/// appended. Each path stays on one original ring and has one process role.
/// Returns the original wall length omitted because no admissible command
/// could carry it; such a wall is split around the omission.
#[expect(
    clippy::too_many_arguments,
    reason = "Original center authority, staged output, cumulative work and live retention remain independent"
)]
pub(super) fn fit(
    centers: &MultiPolygon<f64>,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    retained: usize,
    owned_bytes: usize,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<f64, ManufacturingError> {
    charge(work, region::point_count(centers), foreign_work, cancel)?;
    let mut convex = false;
    if centers.0.len() == 1 && centers.0[0].interiors().is_empty() {
        let ring = &centers.0[0].exterior().0;
        let count = ring.len().saturating_sub(1);
        let mut turning = None;
        convex = true;
        for i in 0..count {
            check_cancel(cancel)?;
            let orientation =
                RobustKernel::orient2d(ring[i], ring[(i + 1) % count], ring[(i + 2) % count]);
            if orientation != Orientation::Collinear {
                let previous = *turning.get_or_insert(orientation);
                if previous != orientation {
                    convex = false;
                    break;
                }
            }
        }
        convex &= turning.is_some();
    }
    let outside = |paths: &Vec<DepositionPath>| {
        (
            paths.iter().fold(retained, |total, path| {
                total.saturating_add(path.points_mm.capacity())
            }),
            owned_bytes.saturating_add(super::super::super::bonds::path_bytes(paths)),
        )
    };
    let (mut outside_units, mut outside_bytes) = outside(paths);
    let mut selected = Vec::new();
    let mut best = Vec::new();
    let mut omitted = 0.0;
    let mut fit = Fit {
        centers,
        convex,
        work,
        foreign_work,
        cancel,
    };
    let mut index = 0;
    while index < paths.len() {
        let points = &mut paths[index].points_mm;
        let mut needs_fit = false;
        for edge in points.windows(2) {
            fit.charge(1)?;
            needs_fit |= !command_length(xy(edge[0]), xy(edge[1]))?;
        }
        if !needs_fit {
            index += 1;
            continue;
        }
        selected.clear();
        let live_bytes = outside_bytes.saturating_add(super::super::vec_bytes(&selected));
        super::super::super::remaining_geometry_units(
            outside_units.saturating_add(points.len().max(selected.capacity())),
            live_bytes,
        )?;
        super::super::reserve_owned(&mut selected, points.len(), live_bytes)?;
        super::super::super::remaining_geometry_units(
            outside_units.saturating_add(selected.capacity()),
            outside_bytes.saturating_add(super::super::vec_bytes(&selected)),
        )?;
        let last = points.len() - 1;
        fit.charge(points.len())?;
        selected.resize(points.len(), usize::MAX);
        selected[last] = last;
        for start in (0..last).rev() {
            for end in start + 1..=last {
                fit.charge(1)?;
                if selected[end] != usize::MAX && fit.chord(points, start, end)? {
                    selected[start] = end;
                    break;
                }
            }
        }
        if selected[0] != usize::MAX {
            let mut source = 0;
            let mut output = 0;
            loop {
                fit.charge(1)?;
                points[output] = points[source];
                output += 1;
                if source == last {
                    break;
                }
                source = selected[source];
            }
            let removed = points.len() - output;
            points.truncate(output);
            *segments -= removed;
            index += 1;
            continue;
        }
        // No complete chain: keep the admissible chords covering the most
        // wall and omit the rest. The cover table, piece chains and new
        // point buffers together hold at most a few entries per point.
        let live_bytes = outside_bytes
            .saturating_add(super::super::vec_bytes(&selected))
            .saturating_add(super::super::vec_bytes(&best));
        let staging = points.len().saturating_mul(4);
        super::super::super::remaining_geometry_units(
            outside_units
                .saturating_add(selected.capacity())
                .saturating_add(staging),
            live_bytes.saturating_add(staging.saturating_mul(std::mem::size_of::<[f64; 3]>())),
        )?;
        super::super::reserve_owned(&mut best, points.len(), live_bytes)?;
        let (pieces, gap) = fit.cover(points, &mut best)?;
        omitted += gap;
        let original = std::mem::take(points);
        *segments -= original.len() - 1;
        let template = DepositionPath {
            points_mm: Vec::new(),
            ..paths[index].clone()
        };
        paths.remove(index);
        let live_bytes = outside_bytes.saturating_add(super::super::vec_bytes(&best));
        super::super::reserve_owned(paths, pieces.len(), live_bytes)?;
        fit.charge(paths.len().saturating_add(original.len()))?;
        for piece in pieces {
            *segments += piece.len() - 1;
            let mut path = template.clone();
            path.points_mm = piece.into_iter().map(|k| original[k]).collect();
            paths.insert(index, path);
            index += 1;
        }
        (outside_units, outside_bytes) = outside(paths);
    }
    Ok(omitted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{LineString, Polygon};
    use std::sync::atomic::AtomicBool;

    fn circle() -> LineString<f64> {
        let mut points: Vec<_> = (0..360)
            .map(|i| {
                let theta = (i as f64).to_radians();
                Coord {
                    x: theta.cos(),
                    y: theta.sin(),
                }
            })
            .collect();
        points.push(points[0]);
        LineString::new(points)
    }
    fn path(ring: &LineString<f64>, kind: DepositionKind) -> DepositionPath {
        DepositionPath {
            material_domain: None,
            kind,
            width: WidthProfile::Constant { width_mm: 0.4 },
            physical_z_mm: [0.0, 0.2],
            owner: None,
            points_mm: ring.0.iter().map(|p| [p.x, p.y, 0.2]).collect(),
            arcs: Vec::new(),
        }
    }
    fn attempt(
        centers: &MultiPolygon<f64>,
        paths: &mut Vec<DepositionPath>,
        retained: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<usize, ManufacturingError> {
        Ok(attempt_omitting(centers, paths, retained, work, cancel)?.0)
    }
    fn attempt_omitting(
        centers: &MultiPolygon<f64>,
        paths: &mut Vec<DepositionPath>,
        retained: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(usize, f64), ManufacturingError> {
        let mut segments = paths.iter().map(|p| p.points_mm.len() - 1).sum();
        let omitted = fit(centers, paths, &mut segments, retained, 0, work, 0, cancel)?;
        assert_eq!(
            segments,
            paths.iter().map(|p| p.points_mm.len() - 1).sum::<usize>()
        );
        Ok((segments, omitted))
    }
    fn assert_commands_inside(paths: &[DepositionPath], centers: &MultiPolygon<f64>) {
        for path in paths {
            for edge in path.points_mm.windows(2) {
                let a = edge[0].map(|v| project_xyz6_coordinate_mm(v).unwrap());
                let b = edge[1].map(|v| project_xyz6_coordinate_mm(v).unwrap());
                assert!((b[0] - a[0]).hypot(b[1] - a[1]) >= MIN_DEPOSITION_COMMAND_MM);
                let midpoint = Point::new(
                    edge[0][0] * 0.5 + edge[1][0] * 0.5,
                    edge[0][1] * 0.5 + edge[1][1] * 0.5,
                );
                assert!(geo::Intersects::intersects(centers, &midpoint));
            }
        }
    }

    #[test]
    fn curved_wall_fits_commands_without_losing_closure_dimensions_or_domain() {
        let ring = circle();
        let centers = MultiPolygon(vec![Polygon::new(ring.clone(), vec![])]);
        let original = path(&ring, DepositionKind::SupportInterface);
        assert!(
            original.points_mm.windows(2).all(|e| {
                (e[1][0] - e[0][0]).hypot(e[1][1] - e[0][1]) < MIN_DEPOSITION_COMMAND_MM
            })
        );
        let mut paths = vec![original.clone()];
        let segments = attempt(&centers, &mut paths, 0, &mut 0, &AtomicBool::new(false)).unwrap();
        let fitted = &paths[0];
        assert_eq!(segments, fitted.points_mm.len() - 1);
        assert!(segments < 360 && segments > 100);
        assert_eq!(fitted.points_mm.first(), fitted.points_mm.last());
        assert_eq!(fitted.physical_z_mm, original.physical_z_mm);
        assert_eq!(fitted.width, original.width);
        assert_eq!(fitted.kind, original.kind);
        for edge in fitted.points_mm.windows(2) {
            let a = edge[0].map(|v| project_xyz6_coordinate_mm(v).unwrap());
            let b = edge[1].map(|v| project_xyz6_coordinate_mm(v).unwrap());
            assert!((b[0] - a[0]).hypot(b[1] - a[1]) >= MIN_DEPOSITION_COMMAND_MM);
            assert!(centers.contains(&Point::new(
                edge[0][0] * 0.5 + edge[1][0] * 0.5,
                edge[0][1] * 0.5 + edge[1][1] * 0.5,
            )));
        }
        // Independent analytic circle bound: every original point is within
        // 1 - cos(maximum selected angular span / 2) of its corresponding chord.
        let maximum_chord = fitted
            .points_mm
            .windows(2)
            .map(|e| (e[1][0] - e[0][0]).hypot(e[1][1] - e[0][1]))
            .fold(0.0_f64, f64::max);
        assert!(1.0 - (1.0 - maximum_chord.powi(2) / 4.0).sqrt() < MAX_WALL_FIT_MM);
    }

    #[test]
    fn exact_command_floor_and_closed_straight_corners_are_preserved() {
        let ring = LineString::from(vec![
            (0.0, 0.0),
            (0.02, 0.0),
            (0.02, 0.02),
            (0.0, 0.02),
            (0.0, 0.0),
        ]);
        let centers = MultiPolygon(vec![Polygon::new(ring.clone(), vec![])]);
        let mut paths = vec![path(&ring, DepositionKind::Support)];
        let original = paths.clone();
        assert_eq!(
            attempt(&centers, &mut paths, 0, &mut 0, &AtomicBool::new(false)).unwrap(),
            4
        );
        assert_eq!(paths, original);
    }

    #[test]
    fn wall_fit_selects_a_complete_suffix_instead_of_a_greedy_dead_end() {
        let centers = MultiPolygon(vec![Polygon::new(
            LineString::from(vec![
                (-1.0, -1.0),
                (1.0, -1.0),
                (1.0, 1.0),
                (-1.0, 1.0),
                (-1.0, -1.0),
            ]),
            vec![],
        )]);
        let ring = LineString::from(vec![(0.0, 0.0), (0.021, 0.0), (0.03, 0.0), (0.03, 0.03)]);
        let original = path(&ring, DepositionKind::SupportInterface);
        let mut paths = vec![original.clone()];
        assert_eq!(
            attempt(&centers, &mut paths, 0, &mut 0, &AtomicBool::new(false)).unwrap(),
            2,
        );
        assert_eq!(
            paths[0].points_mm,
            vec![
                original.points_mm[0],
                original.points_mm[2],
                original.points_mm[3]
            ],
        );
        assert_eq!(paths[0].kind, original.kind);
        assert_eq!(paths[0].width, original.width);
        assert_eq!(paths[0].physical_z_mm, original.physical_z_mm);
    }

    #[test]
    fn inward_hole_chords_cannot_borrow_the_outer_wall_permission() {
        let ring = circle();
        let outer = LineString::from(vec![
            (-2.0, -2.0),
            (2.0, -2.0),
            (2.0, 2.0),
            (-2.0, 2.0),
            (-2.0, -2.0),
        ]);
        let centers = MultiPolygon(vec![Polygon::new(outer, vec![ring.clone()])]);
        let mut paths = vec![path(&ring, DepositionKind::Support)];
        let (segments, omitted) =
            attempt_omitting(&centers, &mut paths, 0, &mut 0, &AtomicBool::new(false)).unwrap();
        // Every command-length chord of this sub-command hole wall crosses the
        // hole, so none is admissible: the wall is omitted and reported whole,
        // never bridged across the void.
        assert_eq!(segments, 0);
        assert!(paths.is_empty());
        let perimeter: f64 = ring
            .0
            .windows(2)
            .map(|e| (e[1] - e[0]).x.hypot((e[1] - e[0]).y))
            .sum();
        assert!((omitted - perimeter).abs() < 1e-9);
    }

    #[test]
    fn short_role_pieces_do_not_merge_across_their_required_process_boundary() {
        let ring = LineString::from(vec![
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 1.0),
            (0.0, 1.0),
            (0.0, 0.0),
        ]);
        let centers = MultiPolygon(vec![Polygon::new(ring, vec![])]);
        let mut paths = vec![
            path(
                &LineString::from(vec![(0.0, 0.0), (0.015, 0.0)]),
                DepositionKind::Support,
            ),
            path(
                &LineString::from(vec![(0.015, 0.0), (0.03, 0.0)]),
                DepositionKind::SupportInterface,
            ),
        ];
        // Neither role piece holds one command; joining them would change a
        // process role, so both are omitted and reported.
        let (segments, omitted) =
            attempt_omitting(&centers, &mut paths, 0, &mut 0, &AtomicBool::new(false)).unwrap();
        assert_eq!(segments, 0);
        assert!(paths.is_empty());
        assert!((omitted - 0.03).abs() < 1e-12);
    }

    #[test]
    fn sliver_ring_omits_only_its_sub_command_edge_and_continues_through_the_seam() {
        // A support center domain barely wider than nothing: a long thin
        // triangle whose short edge holds no command and whose other chords
        // leave the error bound.
        let ring = LineString::from(vec![
            (140.2839145312343, 152.1114236626944),
            (144.24994276708702, 152.1114236626944),
            (144.25454310572485, 152.1138049725613),
            (140.2839145312343, 152.1114236626944),
        ]);
        let centers = MultiPolygon(vec![Polygon::new(ring.clone(), vec![])]);
        let original = path(&ring, DepositionKind::Support);
        let mut paths = vec![original.clone()];
        let (segments, omitted) =
            attempt_omitting(&centers, &mut paths, 0, &mut 0, &AtomicBool::new(false)).unwrap();
        assert_eq!(segments, 2);
        assert_eq!(
            paths[0].points_mm,
            vec![
                original.points_mm[2],
                original.points_mm[0],
                original.points_mm[1]
            ]
        );
        let short = (ring.0[2] - ring.0[1]).x.hypot((ring.0[2] - ring.0[1]).y);
        assert!(short < MIN_DEPOSITION_COMMAND_MM);
        assert!((omitted - short).abs() < 1e-12);
        assert_eq!(paths[0].kind, original.kind);
        assert_eq!(paths[0].width, original.width);
        assert_eq!(paths[0].physical_z_mm, original.physical_z_mm);
        assert_commands_inside(&paths, &centers);
    }

    #[test]
    fn short_leading_role_stub_is_omitted_and_the_remaining_wall_fits() {
        let centers = MultiPolygon(vec![Polygon::new(
            LineString::from(vec![
                (-1.0, -1.0),
                (1.0, -1.0),
                (1.0, 1.0),
                (-1.0, 1.0),
                (-1.0, -1.0),
            ]),
            vec![],
        )]);
        // The stub turns sharply into the rest of the wall, so no chord from
        // its first point stays within the error bound.
        let piece = LineString::from(vec![(0.0, 0.0), (0.01, 0.01), (0.5, 0.0), (0.9, 0.3)]);
        let original = path(&piece, DepositionKind::SupportInterface);
        let mut paths = vec![original.clone()];
        let (segments, omitted) =
            attempt_omitting(&centers, &mut paths, 0, &mut 0, &AtomicBool::new(false)).unwrap();
        assert_eq!(segments, 2);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].points_mm, original.points_mm[1..].to_vec());
        assert!((omitted - 0.01_f64.hypot(0.01)).abs() < 1e-12);
        assert_commands_inside(&paths, &centers);
    }

    #[test]
    fn fitting_charges_work_memory_and_cancellation_before_publication() {
        let ring = circle();
        let centers = MultiPolygon(vec![Polygon::new(ring.clone(), vec![])]);
        for (retained, mut work, cancel, expected) in [
            (0, usize::MAX, false, ManufacturingErrorCode::ResourceLimit),
            (
                crate::working_set::units(),
                0,
                false,
                ManufacturingErrorCode::ResourceLimit,
            ),
            (0, 0, true, ManufacturingErrorCode::Cancelled),
        ] {
            let mut paths = vec![path(&ring, DepositionKind::Support)];
            assert_eq!(
                attempt(
                    &centers,
                    &mut paths,
                    retained,
                    &mut work,
                    &AtomicBool::new(cancel)
                )
                .unwrap_err()
                .code,
                expected
            );
        }
    }
}
