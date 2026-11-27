// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use crate::planner::support::strokes::StrokeRuns;
use spiling_contracts::manufacturing::program_coordinates::XYZ6_PROJECTION_SCREEN_MM;

type GeometryKey = [u64; 5];

fn key(c: [f64; 2], d: [f64; 2], width: f64) -> GeometryKey {
    [c[0], c[1], d[0], d[1], width].map(f64::to_bits)
}

fn query_key(candidate: AnchorCandidate) -> GeometryKey {
    let [a, b] = candidate.endpoints_mm;
    key([a[0], a[1]], [b[0], b[1]], candidate.bead_width_mm)
}

// The two high bits store the endpoint flags. A larger edge index stays
// on the ordinary proof, as do operands that can refuse conditionally.
const EDGE_ID_MASK: u32 = (1 << 30) - 1;

pub(super) struct Contribution {
    pub(super) endpoints: u8,
    interval: (f64, f64),
}

impl Contribution {
    pub(super) fn interval(&self) -> Option<(f64, f64)> {
        (self.interval.0 != self.interval.1).then_some(self.interval)
    }
}

struct QueryProof {
    key: GeometryKey,
    range: std::ops::Range<usize>,
}

pub(super) struct PreparedQuery<'a> {
    keys: &'a [GeometryKey],
    edges: &'a [u32],
    intervals: &'a [(f64, f64)],
}

impl PreparedQuery<'_> {
    pub(super) fn find(&self, c: [f64; 2], d: [f64; 2], width: f64) -> Option<Contribution> {
        let key = key(c, d, width);
        let index = self
            .edges
            .binary_search_by_key(&key, |&edge| self.keys[(edge & EDGE_ID_MASK) as usize])
            .ok()?;
        Some(Contribution {
            endpoints: (self.edges[index] >> 30) as u8,
            interval: self.intervals[index],
        })
    }
}

pub(in crate::planner::support) struct PreparedLayer {
    keys: Vec<GeometryKey>,
    queries: Vec<QueryProof>,
    edges: Vec<u32>,
    intervals: Vec<(f64, f64)>,
}

impl PreparedLayer {
    pub(super) fn find(&self, candidate: AnchorCandidate) -> Option<PreparedQuery<'_>> {
        let index = self
            .queries
            .binary_search_by_key(&query_key(candidate), |row| row.key)
            .ok()?;
        Some(PreparedQuery {
            keys: &self.keys,
            edges: &self.edges[self.queries[index].range.clone()],
            intervals: &self.intervals[self.queries[index].range.clone()],
        })
    }

    pub(in crate::planner::support) fn build<'a>(
        candidates: &[DepositionPath],
        previous: impl Iterator<Item = &'a DepositionPath> + Clone,
        allowance: usize,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        if candidates.is_empty() {
            return Ok(None);
        }
        let Some(stats) = InventoryStats::new(previous.clone(), std::iter::empty(), cancel)? else {
            return Ok(None);
        };
        if stats.records().saturating_mul(size_of::<[f64; 2]>()) >= allowance {
            return Ok(None);
        }
        let mut lower = NominalAnchorPaths::build(previous, std::iter::empty(), stats, cancel)?;
        lower.prepare_grid(allowance, cancel)?;
        let mut remaining = allowance.saturating_sub(lower.retained_bytes());
        let key_count = lower.indexed_keys().count();
        if key_count == 0 || key_count.saturating_mul(size_of::<GeometryKey>()) >= remaining {
            return Ok(None);
        }
        let mut keys = Vec::with_capacity(key_count);
        keys.extend(lower.indexed_keys());
        remaining -= keys.capacity() * size_of::<GeometryKey>();
        let mut queries = Vec::new();
        let mut edges = Vec::new();
        let mut intervals = Vec::new();
        for path in candidates {
            check_cancel(cancel)?;
            let Ok(runs) = StrokeRuns::for_path(path) else {
                continue;
            };
            let floor = LowerFloor {
                z: path.physical_z_mm[0],
                contacts: false,
                supplied: false,
            };
            for (first, last, bead_width_mm) in runs {
                check_cancel(cancel)?;
                let candidate = AnchorCandidate {
                    endpoints_mm: [path.points_mm[first], path.points_mm[last]],
                    bead_width_mm,
                };
                let [a, b] = candidate.endpoints_mm;
                let radius = bead_width_mm * 0.5;
                let delta = [b[0] - a[0], b[1] - a[1]];
                let length = delta[0].hypot(delta[1]);
                if !(radius > 0.0
                    && radius <= 1e6
                    && length > 0.0
                    && length.is_finite()
                    && a.iter().chain(&b).all(|x| x.abs() <= 1e6))
                {
                    continue;
                }
                let begin = edges.len();
                let direction = [delta[0] / length, delta[1] / length];
                let projected = [
                    ProjectedAnchorPoint::new([a[0], a[1], 0.0])?,
                    ProjectedAnchorPoint::new([b[0], b[1], 0.0])?,
                ];
                if remaining <= 2 * std::mem::size_of::<QueryProof>() {
                    break;
                }
                lower.visit_indexed(candidate, floor, &keys, |edge, c, d, width| {
                    check_cancel(cancel)?;
                    if remaining
                        < 4 * (size_of::<u32>() + size_of::<(f64, f64)>() + size_of::<QueryProof>())
                        || edge > EDGE_ID_MASK
                        || ((d[0] - c[0]).abs() <= XYZ6_PROJECTION_SCREEN_MM
                            && (d[1] - c[1]).abs() <= XYZ6_PROJECTION_SCREEN_MM)
                    {
                        return Ok(());
                    }
                    let capsule = SegmentCapsule::new(c, d, width * 0.5)?;
                    let reach = radius + capsule.radius;
                    let mut endpoints = 0;
                    let mut segment = None;
                    for (index, point) in [[a[0], a[1]], [b[0], b[1]]].into_iter().enumerate() {
                        if capsule.excludes_projected_endpoint(point, reach) {
                            continue;
                        }
                        let segment = match &segment {
                            Some(segment) => segment,
                            None => segment.insert(ProjectedAnchorSegment::new([
                                [c[0], c[1], 0.0],
                                [d[0], d[1], 0.0],
                            ])?),
                        };
                        let (distance, enclosure) = segment.distance_enclosed(projected[index])?;
                        if (distance + enclosure).next_up() < reach {
                            endpoints |= 1 << index;
                        }
                    }
                    let interval = capsule.interval([a[0], a[1]], direction, length)?;
                    remaining -= 4 * (size_of::<u32>() + size_of::<(f64, f64)>());
                    edges.push(edge | (endpoints << 30));
                    intervals.push(interval.unwrap_or((0.0, 0.0)));
                    Ok(())
                })?;
                if edges.len() != begin {
                    // Do not cross the per-layer receipt allowance on a very narrow row.
                    remaining = remaining.saturating_sub(4 * size_of::<QueryProof>());
                    queries.push(QueryProof {
                        key: query_key(candidate),
                        range: begin..edges.len(),
                    });
                }
            }
        }
        queries.sort_unstable_by_key(|row| row.key);
        Ok((!queries.is_empty()).then_some(Self {
            keys,
            queries,
            edges,
            intervals,
        }))
    }

    pub(in crate::planner::support) fn pairs(&self) -> usize {
        self.edges.len()
    }

    pub(in crate::planner::support) fn bytes(&self) -> usize {
        self.keys.capacity() * std::mem::size_of::<GeometryKey>()
            + self.queries.capacity() * std::mem::size_of::<QueryProof>()
            + self.edges.capacity() * size_of::<u32>()
            + self.intervals.capacity() * size_of::<(f64, f64)>()
    }
}

impl AnchorWorkspace {
    pub(in crate::planner::support) fn use_prepared(&mut self, proofs: Option<PreparedLayer>) {
        self.prepared = proofs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::manufacturing::{DepositionKind, WidthProfile};

    #[test]
    fn packed_contributions_preserve_gaps_provenance_and_work() {
        let path = |x: f64, half_length: f64| DepositionPath {
            material_domain: None,
            kind: DepositionKind::Perimeter,
            owner: None,
            width: WidthProfile::Constant { width_mm: 0.4 },
            physical_z_mm: [0.0, 0.2],
            points_mm: vec![[x, -half_length, 0.2], [x, half_length, 0.2]],
            arcs: Vec::new(),
            exposed: false,
            connectors: Vec::new(),
        };
        let lower: Vec<_> = (0..80).map(|index| path(index as f64 * 0.5, 0.8)).collect();
        let mut upper = path(0.0, 1.0);
        upper.physical_z_mm = [0.2, 0.4];
        upper.points_mm = vec![[0.1, 0.0, 0.4], [2.9, 0.0, 0.4]];
        let candidate = AnchorCandidate {
            endpoints_mm: [upper.points_mm[0], upper.points_mm[1]],
            bead_width_mm: 0.4,
        };
        let cancel = AtomicBool::new(false);
        let prepared = PreparedLayer::build(
            std::slice::from_ref(&upper),
            lower.iter(),
            256 * 1024,
            &cancel,
        )
        .unwrap()
        .unwrap();
        assert!(prepared.pairs() > 0);
        let mut scalar = AnchorWorkspace::default();
        let mut batched = AnchorWorkspace::default();
        batched.use_prepared(Some(prepared));
        // The same immutable lower geometry takes its provenance from the
        // ordered query. Cache keys cannot turn contacts into direct anchors.
        for split in [0, 2, lower.len()] {
            let summarize = |gaps: &[AnchorGap]| {
                gaps.iter()
                    .map(|gap| (gap.start, gap.end, gap.left, gap.right, gap.direct_ends))
                    .collect::<Vec<_>>()
            };
            let mut scalar_work = 0;
            let mut cached_work = 0;
            let expected = scalar
                .gaps(
                    candidate,
                    lower[..split].iter(),
                    lower[split..].iter(),
                    &mut scalar_work,
                    &cancel,
                )
                .map(summarize)
                .unwrap();
            let actual = batched
                .gaps(
                    candidate,
                    lower[..split].iter(),
                    lower[split..].iter(),
                    &mut cached_work,
                    &cancel,
                )
                .map(summarize)
                .unwrap();
            assert_eq!(expected, actual);
            assert_eq!(scalar_work, cached_work);
        }
    }
}
