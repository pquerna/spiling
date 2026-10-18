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
pub(in crate::planner) struct AnchorGap {
    pub(in crate::planner) start: f64,
    pub(in crate::planner) end: f64,
    pub(in crate::planner) left: bool,
    pub(in crate::planner) right: bool,
    pub(in crate::planner) direct_ends: bool,
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
pub(in crate::planner) struct AnchorCandidate {
    pub(in crate::planner) endpoints_mm: [[f64; 3]; 2],
    pub(in crate::planner) bead_width_mm: f64,
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

pub(in crate::planner) struct AnchorWorkspace {
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

impl AnchorWorkspace {
    pub(in crate::planner) fn set_budget(&mut self, remaining_records: usize) {
        self.intervals.clear();
        self.uncovered.clear();
        self.record_limit = remaining_records.min(MAX_DEPOSITION_SEGMENTS as usize);
    }

    pub(in crate::planner) fn retained_records(&self) -> usize {
        self.intervals.capacity() + self.uncovered.capacity()
    }

    /// Uncovered normalized centerline intervals with actual endpoint-anchor presence
    /// and provenance. Candidate endpoint disks may certify positive-area anchors;
    /// they do not enlarge interior intervals or shorten measured unsupported spans.
    /// Stroke-pair work is cumulative across queries, not workspace-local.
    pub(in crate::planner) fn gaps<'a>(
        &mut self,
        candidate: AnchorCandidate,
        previous: impl Iterator<Item = &'a DepositionPath>,
        contacts: impl Iterator<Item = &'a DepositionPath>,
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
        for (path, direct) in previous
            .map(|path| (path, true))
            .chain(contacts.map(|path| (path, false)))
        {
            for edge in path.points_mm.windows(2) {
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
                let endpoint_reach = radius + radius;
                if a[0].min(b[0]) > c[0].max(d[0]) + endpoint_reach
                    || a[0].max(b[0]) < c[0].min(d[0]) - endpoint_reach
                    || a[1].min(b[1]) > c[1].max(d[1]) + endpoint_reach
                    || a[1].max(b[1]) < c[1].min(d[1]) - endpoint_reach
                {
                    continue;
                }
                let capsule = SegmentCapsule::new(c, d, radius)?;
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
        let q = [point[0] - self.c[0], point[1] - self.c[1]];
        let along = dot(q, self.unit).clamp(0.0, self.length);
        let distance = (q[0] - along * self.unit[0]).hypot(q[1] - along * self.unit[1]);
        if !distance.is_finite() {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "nonfinite nominal endpoint-footprint distance",
            ));
        }
        // Two positive-radius footprints have positive overlap area iff this strict
        // distance is below their radius sum. Equality is tangent with zero area.
        Ok(distance < self.radius + candidate_radius)
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
