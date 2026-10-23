// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use crate::{check_cancel, error};
use spiling_contracts::manufacturing::{
    DepositionPath, MAX_DEPOSITION_SEGMENTS, ManufacturingError, ManufacturingErrorCode,
};
use std::sync::atomic::AtomicBool;

pub(super) const MAX_ANCHOR_PAIR_WORK: usize = 128_000_000;

/// Exact centerline intersection with previous nominal deposited capsules plus
/// separately certified positive-area overlap of actual candidate endpoint disks.
/// No source-material anchor, polygonized circle or overlay tolerance expansion.
#[derive(Debug)]
pub(crate) struct AnchorGap {
    pub(crate) start: f64,
    pub(crate) end: f64,
    pub(crate) left: bool,
    pub(crate) right: bool,
    pub(crate) direct_ends: bool,
}

#[derive(Default)]
struct EndpointAnchor {
    present: bool,
    direct: bool,
}

struct SegmentCapsule {
    c: [f64; 2],
    d: [f64; 2],
    unit: [f64; 2],
    length: f64,
    radius: f64,
}

/// A nominal candidate bead segment in world millimetres. Only its endpoint
/// coordinates are copied; previous/contact path arrays remain borrowed.
#[derive(Clone, Copy)]
pub(crate) struct AnchorCandidate {
    pub(crate) endpoints_mm: [[f64; 3]; 2],
    pub(crate) bead_width_mm: f64,
}

impl AnchorCandidate {
    /// Connectivity on one rectilinear current emitted row only. Identical
    /// projected fixed coordinates name the same physical decimal coordinate.
    /// This supplies no previous/contact anchor or unsupported-span authority.
    pub(in crate::planner) fn emitted_endpoint_footprint_overlaps(
        &self,
        endpoint_mm: [f64; 3],
        endpoint_bead_width_mm: f64,
    ) -> Result<bool, ManufacturingError> {
        let radius = self.bead_width_mm * 0.5;
        let endpoint_radius = endpoint_bead_width_mm * 0.5;
        if !radius.is_finite()
            || radius <= 0.0
            || !endpoint_radius.is_finite()
            || endpoint_radius <= 0.0
        {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "invalid current nominal row footprint width",
            ));
        }
        use spiling_contracts::manufacturing::program_coordinates::{
            enclosed_vertical_gap_mm, project_xyz6_point_mm,
        };
        let a = project_xyz6_point_mm(self.endpoints_mm[0])?;
        let b = project_xyz6_point_mm(self.endpoints_mm[1])?;
        let endpoint = project_xyz6_point_mm(endpoint_mm)?;
        let along = if a[1] == b[1] && a[0] != b[0] {
            0
        } else if a[0] == b[0] && a[1] != b[1] {
            1
        } else {
            return Ok(false);
        };
        if endpoint[1 - along] != a[1 - along] {
            return Ok(false);
        }
        let lo = a[along].min(b[along]);
        let hi = a[along].max(b[along]);
        let (above, below) = if endpoint[along] < lo {
            (lo, endpoint[along])
        } else if endpoint[along] > hi {
            (endpoint[along], hi)
        } else {
            // XYZ6 ordering is unique: this physical decimal point is inside
            // the actual row segment, so positive-radius footprints overlap.
            return Ok(true);
        };
        let separation_upper = -emitted_coordinate_separation_lower_mm(below, above)?;
        let reach_lower = enclosed_vertical_gap_mm(radius, -endpoint_radius)?;
        Ok(separation_upper < reach_lower)
    }
}

/// Physical-decimal separation lower bound for already projected XYZ6
/// coordinates. Negating the reversed bound gives the corresponding upper
/// bound. This is representation/difference enclosure, not policy slack.
pub(super) fn emitted_coordinate_separation_lower_mm(
    above: f64,
    below: f64,
) -> Result<f64, ManufacturingError> {
    use spiling_contracts::manufacturing::program_coordinates::{
        enclosed_vertical_gap_mm, xyz6_coordinate_error_mm,
    };
    let mut lower = enclosed_vertical_gap_mm(above, below)?;
    for coordinate in [above, below] {
        let error = xyz6_coordinate_error_mm(coordinate)?;
        lower = enclosed_vertical_gap_mm(lower, error)?;
    }
    Ok(lower)
}

pub(crate) struct AnchorWorkspace {
    intervals: Vec<(f64, f64, bool)>,
    uncovered: Vec<AnchorGap>,
    record_limit: usize,
}

impl Default for AnchorWorkspace {
    fn default() -> Self {
        Self {
            intervals: Vec::new(),
            uncovered: Vec::new(),
            record_limit: MAX_DEPOSITION_SEGMENTS as usize,
        }
    }
}

/// Borrow every local-width capsule, propagating a malformed path before any
/// of its edges rather than dropping a failed checked iterator.
pub(crate) fn nominal_edges<'a>(
    paths: impl Iterator<Item = &'a DepositionPath>,
    direct: bool,
) -> impl Iterator<Item = (Result<[[f64; 3]; 2], ManufacturingError>, bool, f64)> {
    paths.flat_map(move |path| {
        let (edges, failure) = match path.checked_edges() {
            Ok(edges) => (Some(edges), None),
            Err(failure) => (None, Some(failure)),
        };
        failure
            .into_iter()
            .map(move |failure| (Err(failure), direct, 0.0))
            .chain(edges.into_iter().flatten().map(move |edge| {
                (
                    Ok([edge.points_mm[0], edge.points_mm[1]]),
                    direct,
                    edge.width_mm,
                )
            }))
    })
}

pub(crate) fn projected_edges<'a>(
    paths: impl Iterator<Item = &'a DepositionPath>,
    direct: bool,
) -> impl Iterator<Item = (Result<[[f64; 3]; 2], ManufacturingError>, bool, f64)> {
    let project = spiling_contracts::manufacturing::program_coordinates::project_xyz6_point_mm;
    nominal_edges(paths, direct).map(move |(points, direct, width)| {
        (
            points.and_then(|[a, b]| Ok([project(a)?, project(b)?])),
            direct,
            width,
        )
    })
}

impl AnchorWorkspace {
    pub(in crate::planner) fn set_budget(&mut self, remaining_records: usize) {
        self.intervals.clear();
        self.uncovered.clear();
        self.record_limit = remaining_records.min(MAX_DEPOSITION_SEGMENTS as usize);
    }

    pub(in crate::planner) fn retained_records(&self) -> usize {
        self.intervals.capacity() + self.uncovered.capacity()
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.intervals.capacity() * std::mem::size_of::<(f64, f64, bool)>()
            + self.uncovered.capacity() * std::mem::size_of::<AnchorGap>()
    }

    /// Uncovered normalized centerline intervals with actual endpoint-anchor presence
    /// and provenance. Candidate endpoint disks may certify positive-area anchors;
    /// they do not enlarge interior intervals or shorten measured unsupported spans.
    /// Stroke-pair work is cumulative across queries, not workspace-local.
    pub(crate) fn gaps<'a>(
        &mut self,
        candidate: AnchorCandidate,
        previous: impl Iterator<Item = &'a DepositionPath>,
        contacts: impl Iterator<Item = &'a DepositionPath>,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<&[AnchorGap], ManufacturingError> {
        self.gaps_edges(
            candidate,
            nominal_edges(previous, true).chain(nominal_edges(contacts, false)),
            work,
            cancel,
        )
    }

    /// Borrow/project already byte-associated segment endpoints without making
    /// a second motion stream. Shared math supplies no planner join authority.
    pub(crate) fn gaps_edges(
        &mut self,
        candidate: AnchorCandidate,
        edges: impl Iterator<Item = (Result<[[f64; 3]; 2], ManufacturingError>, bool, f64)>,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<&[AnchorGap], ManufacturingError> {
        let [a, b] = candidate.endpoints_mm;
        check_cancel(cancel)?;
        let delta = [b[0] - a[0], b[1] - a[1]];
        let length = delta[0].hypot(delta[1]);
        let radius = candidate.bead_width_mm * 0.5;
        if !length.is_finite()
            || length <= 0.0
            || !radius.is_finite()
            || radius <= 0.0
            || a.iter().chain(&b).any(|x| !x.is_finite())
        {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "nonfinite or zero nominal anchor query",
            ));
        }
        let direction = [delta[0] / length, delta[1] / length];
        self.intervals.clear();
        self.uncovered.clear();
        let mut endpoints = [EndpointAnchor::default(), EndpointAnchor::default()];
        for (edge, direct, previous_width) in edges {
            let edge = edge?;
            let previous_radius = previous_width * 0.5;
            if !previous_radius.is_finite() || previous_radius <= 0.0 {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    "nonfinite or zero deposited anchor bead width",
                ));
            }
            check_cancel(cancel)?;
            *work = work
                .checked_add(1)
                .filter(|n| *n <= MAX_ANCHOR_PAIR_WORK)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "cumulative deposited-stroke anchor work budget exceeded",
                    )
                })?;
            let c = [edge[0][0], edge[0][1]];
            let d = [edge[1][0], edge[1][1]];
            if c.iter().chain(&d).any(|x| !x.is_finite()) {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    "nonfinite deposited anchor segment",
                ));
            }
            // A broad-phase box may reject only when both the original-radius
            // centerline and the two endpoint-disk footprints cannot intersect.
            let endpoint_reach = radius + previous_radius;
            if a[0].min(b[0]) > c[0].max(d[0]) + endpoint_reach
                || a[0].max(b[0]) < c[0].min(d[0]) - endpoint_reach
                || a[1].min(b[1]) > c[1].max(d[1]) + endpoint_reach
                || a[1].max(b[1]) < c[1].min(d[1]) - endpoint_reach
            {
                continue;
            }
            let capsule = SegmentCapsule::new(c, d, previous_radius)?;
            for (endpoint, point) in endpoints.iter_mut().zip([[a[0], a[1]], [b[0], b[1]]]) {
                if (!endpoint.present || (direct && !endpoint.direct))
                    && capsule.overlaps_endpoint(point, radius)?
                {
                    endpoint.present = true;
                    endpoint.direct |= direct;
                }
            }
            if let Some((start, end)) = capsule.interval([a[0], a[1]], direction, length)? {
                if self.intervals.len() >= self.record_limit {
                    return Err(error(
                        ManufacturingErrorCode::ResourceLimit,
                        "retained deposited-anchor interval budget exceeded",
                    ));
                }
                super::reserve_component_record(
                    &mut self.intervals,
                    1,
                    self.uncovered.capacity(),
                    self.record_limit,
                )?;
                self.intervals.push((start / length, end / length, direct));
            }
        }
        self.intervals
            .sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let mut covered_until: f64 = 0.0;
        let mut direct_until: f64 = 0.0;
        for index in 0..self.intervals.len() {
            let (start, end, direct) = self.intervals[index];
            check_cancel(cancel)?;
            if start > covered_until {
                let direct_right = self.intervals[index..]
                    .iter()
                    .take_while(|interval| interval.0 == start)
                    .any(|interval| interval.2);
                let left_direct = if covered_until > 0.0 {
                    direct_until >= covered_until
                } else {
                    endpoints[0].direct
                };
                self.push_gap(AnchorGap {
                    start: covered_until,
                    end: start,
                    left: covered_until > 0.0 || endpoints[0].present,
                    right: true,
                    direct_ends: left_direct && direct_right,
                })?;
            }
            covered_until = covered_until.max(end);
            if direct {
                direct_until = direct_until.max(end);
            }
        }
        if covered_until < 1.0 {
            let left_direct = if covered_until > 0.0 {
                direct_until >= covered_until
            } else {
                endpoints[0].direct
            };
            self.push_gap(AnchorGap {
                start: covered_until,
                end: 1.0,
                left: covered_until > 0.0 || endpoints[0].present,
                right: endpoints[1].present,
                direct_ends: left_direct && endpoints[1].direct,
            })?;
        }
        Ok(&self.uncovered)
    }

    fn push_gap(&mut self, gap: AnchorGap) -> Result<(), ManufacturingError> {
        if self.intervals.len() + self.uncovered.len() >= self.record_limit {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "combined retained anchor interval budget exceeded",
            ));
        }
        super::reserve_component_record(
            &mut self.uncovered,
            1,
            self.intervals.capacity(),
            self.record_limit,
        )?;
        self.uncovered.push(gap);
        Ok(())
    }
}

fn endpoint_distance_enclosed(
    point: [f64; 3],
    segment: [[f64; 3]; 2],
) -> Result<(f64, f64), ManufacturingError> {
    use spiling_contracts::manufacturing::program_coordinates::{
        project_xyz6_point_mm, xyz6_coordinate_error_mm,
    };
    let p = project_xyz6_point_mm(point)?;
    let a = project_xyz6_point_mm(segment[0])?;
    let b = project_xyz6_point_mm(segment[1])?;
    let error_at = |p: [f64; 3]| -> Result<f64, ManufacturingError> {
        Ok(xyz6_coordinate_error_mm(p[0])?.hypot(xyz6_coordinate_error_mm(p[1])?))
    };
    let scale = p
        .into_iter()
        .chain(a)
        .chain(b)
        .fold(1_f64, |s, x| s.max(x.abs()));
    let enclosure = error_at(p)? + error_at(a)?.max(error_at(b)?) + scale * 64. * f64::EPSILON;
    let distance =
        SegmentCapsule::new([a[0], a[1]], [b[0], b[1]], 0.)?.endpoint_distance([p[0], p[1]])?;
    Ok((distance, enclosure))
}

/// Canonical decimal dimensions admit exact rational capsule comparison.
/// Doubled micrometres represent both XYZ6 points and a half-bead radius.
/// Checked overflow/noncanonical dimensions use the outward floating proof.
fn exact_decimal_bond(
    point: [f64; 3],
    segment: [[f64; 3]; 2],
    width: f64,
) -> Result<Option<bool>, ManufacturingError> {
    use spiling_contracts::manufacturing::program_coordinates::project_xyz6_point_mm;
    let grid = |value: f64| {
        let integer = (value * 1_000_000.).round();
        if integer.is_finite() && integer / 1_000_000. == value {
            (integer as i128).checked_mul(2)
        } else {
            None
        }
    };
    let p = project_xyz6_point_mm(point)?;
    let a = project_xyz6_point_mm(segment[0])?;
    let b = project_xyz6_point_mm(segment[1])?;
    let ([Some(px), Some(py)], [Some(ax), Some(ay)], [Some(bx), Some(by)], Some(radius)) = (
        [p[0], p[1]].map(grid),
        [a[0], a[1]].map(grid),
        [b[0], b[1]].map(grid),
        grid(width),
    ) else {
        return Ok(None);
    };
    let proof = || -> Option<bool> {
        let v = [bx - ax, by - ay];
        let q = [px - ax, py - ay];
        let square = |v: [i128; 2]| v[0].checked_mul(v[0])?.checked_add(v[1].checked_mul(v[1])?);
        let length = square(v)?;
        if length == 0 {
            return Some(false);
        }
        let dot = q[0]
            .checked_mul(v[0])?
            .checked_add(q[1].checked_mul(v[1])?)?;
        let (numerator, denominator) = if dot <= 0 {
            (square(q)?, 1)
        } else if dot >= length {
            (square([px - bx, py - by])?, 1)
        } else {
            let cross = q[0]
                .checked_mul(v[1])?
                .checked_sub(q[1].checked_mul(v[0])?)?;
            (cross.checked_mul(cross)?, length)
        };
        let half = radius / 2;
        Some(
            numerator >= half.checked_mul(half)?.checked_mul(denominator)?
                && numerator < radius.checked_mul(radius)?.checked_mul(denominator)?,
        )
    };
    Ok(proof())
}

/// Maximum nominal overlap is half a bead. The separately returned Cartesian
/// representation enclosure is not a source-to-projection manufacturing slack.
pub(crate) fn endpoint_bond(
    point: [f64; 3],
    segment: [[f64; 3]; 2],
    width: f64,
) -> Result<bool, ManufacturingError> {
    if let Some(bonded) = exact_decimal_bond(point, segment, width)? {
        return Ok(bonded);
    }
    let (distance, enclosure) = endpoint_distance_enclosed(point, segment)?;
    Ok(distance + enclosure < width && distance - enclosure >= width * 0.5)
}

pub(in crate::planner) fn inward_bond_endpoint(
    endpoint: [f64; 3],
    other: [f64; 3],
    witness: [[f64; 3]; 2],
    width: f64,
) -> Result<Option<[f64; 3]>, ManufacturingError> {
    if endpoint_bond(endpoint, witness, width)? {
        return Ok(Some(endpoint));
    }
    let (distance, enclosure) = endpoint_distance_enclosed(endpoint, witness)?;
    if distance + enclosure >= width {
        return Ok(None);
    }
    let delta = [other[0] - endpoint[0], other[1] - endpoint[1]];
    let length = delta[0].hypot(delta[1]);
    if !length.is_finite() || length <= 0. {
        return Ok(None);
    }
    let direction = [delta[0] / length, delta[1] / length];
    let capsule = SegmentCapsule::new(
        [witness[0][0], witness[0][1]],
        [witness[1][0], witness[1][1]],
        width * 0.75,
    )?;
    let Some((_, exit)) = capsule.interval([endpoint[0], endpoint[1]], direction, length)? else {
        return Ok(None);
    };
    if exit >= length * 0.5 {
        return Ok(None);
    }
    let candidate = [
        endpoint[0] + direction[0] * exit,
        endpoint[1] + direction[1] * exit,
        endpoint[2],
    ];
    Ok(endpoint_bond(candidate, witness, width)?.then_some(candidate))
}

// A capsule is convex: its rectangle and two endpoint disks yield one connected
// interval on a line. Their interval hull is therefore its exact intersection.
impl SegmentCapsule {
    fn new(c: [f64; 2], d: [f64; 2], radius: f64) -> Result<Self, ManufacturingError> {
        let delta = [d[0] - c[0], d[1] - c[1]];
        let length = delta[0].hypot(delta[1]);
        if !length.is_finite() || length <= 0.0 {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "zero or nonfinite deposited anchor segment",
            ));
        }
        Ok(Self {
            c,
            d,
            unit: [delta[0] / length, delta[1] / length],
            length,
            radius,
        })
    }

    fn overlaps_endpoint(
        &self,
        point: [f64; 2],
        candidate_radius: f64,
    ) -> Result<bool, ManufacturingError> {
        Ok(self.endpoint_distance(point)? < self.radius + candidate_radius)
    }

    fn endpoint_distance(&self, point: [f64; 2]) -> Result<f64, ManufacturingError> {
        let q = [point[0] - self.c[0], point[1] - self.c[1]];
        let along = dot(q, self.unit).clamp(0.0, self.length);
        let distance = (q[0] - along * self.unit[0]).hypot(q[1] - along * self.unit[1]);
        if !distance.is_finite() {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "nonfinite nominal endpoint-footprint distance",
            ));
        }
        Ok(distance)
    }

    fn interval(
        &self,
        a: [f64; 2],
        u: [f64; 2],
        length: f64,
    ) -> Result<Option<(f64, f64)>, ManufacturingError> {
        let c = self.c;
        let d = self.d;
        let v = self.unit;
        let anchor_length = self.length;
        let radius = self.radius;
        let q = [a[0] - c[0], a[1] - c[1]];
        let mut interval = None;
        let mut strip = (0.0, length);
        if restrict(&mut strip, dot(q, v), dot(u, v), 0.0, anchor_length)
            && restrict(&mut strip, cross(v, q), cross(v, u), -radius, radius)
        {
            include(&mut interval, strip);
        }
        for center in [c, d] {
            let q = [center[0] - a[0], center[1] - a[1]];
            let along = dot(q, u);
            let perpendicular = cross(q, u);
            let discriminant = radius * radius - perpendicular * perpendicular;
            if !along.is_finite() || !discriminant.is_finite() {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    "nominal anchor intersection exceeds finite range",
                ));
            }
            if discriminant >= 0.0 {
                let extent = discriminant.sqrt();
                include(
                    &mut interval,
                    ((along - extent).max(0.0), (along + extent).min(length)),
                );
            }
        }
        // A single tangent point does not provide a positive-length anchor interval.
        Ok(interval.filter(|(start, end)| end > start))
    }
}

fn restrict(interval: &mut (f64, f64), value: f64, slope: f64, min: f64, max: f64) -> bool {
    if slope == 0.0 {
        return value >= min && value <= max;
    }
    let a = (min - value) / slope;
    let b = (max - value) / slope;
    interval.0 = interval.0.max(a.min(b));
    interval.1 = interval.1.min(a.max(b));
    interval.1 >= interval.0
}
fn include(into: &mut Option<(f64, f64)>, interval: (f64, f64)) {
    if interval.1 < interval.0 {
        return;
    }
    if let Some(existing) = into {
        existing.0 = existing.0.min(interval.0);
        existing.1 = existing.1.max(interval.1);
    } else {
        *into = Some(interval);
    }
}
fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

#[cfg(test)]
mod dimension_tests {
    use super::*;

    #[test]
    fn previous_capsule_radius_is_independent_of_candidate_endpoint_radius() {
        let cancel = AtomicBool::new(false);
        let mut workspace = AnchorWorkspace::default();
        let candidate = AnchorCandidate {
            endpoints_mm: [[0.0, 0.0, 0.7], [2.0, 0.0, 0.7]],
            bead_width_mm: 0.8,
        };
        for (width, start, end) in [(0.4, 0.4, 0.6), (1.2, 0.2, 0.8)] {
            let gaps = workspace
                .gaps_edges(
                    candidate,
                    std::iter::once((Ok([[1.0, -1.0, 0.4], [1.0, 1.0, 0.4]]), true, width)),
                    &mut 0,
                    &cancel,
                )
                .unwrap();
            assert_eq!(gaps.len(), 2);
            assert!((gaps[0].end - start).abs() < 1e-12);
            assert!((gaps[1].start - end).abs() < 1e-12);
        }
    }
}
