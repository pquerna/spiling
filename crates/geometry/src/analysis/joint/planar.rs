// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Geometry-only rejection and exact cycle identity. Neither supplies native
//! material, topology, pose, or approximation authority.

use super::*;

struct Node {
    bounds: [[f64; 2]; 2],
    end: usize,
    // Children for a branch; ring/edge indices for a leaf.
    links: [usize; 2],
    leaf: bool,
}

struct SegmentIndex {
    nodes: Vec<Node>,
}

struct SegmentClearanceQuery<'a> {
    ordinal: usize,
    ri: usize,
    edge: usize,
    rings: &'a [Ring],
    bounds: [[f64; 2]; 2],
    margin: f64,
}

impl SegmentIndex {
    fn build(
        &mut self,
        rings: &[Ring],
        first: usize,
        count: usize,
        cursor: &mut [usize; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<usize, GeometryError> {
        work.charge(1, cancel)?;
        let node = if count == 1 {
            while cursor[1] == rings[cursor[0]].points.len() {
                work.charge(1, cancel)?;
                cursor[0] += 1;
                cursor[1] = 0;
            }
            let [ri, edge] = *cursor;
            let points = &rings[ri].points;
            let a = points[edge];
            let b = points[(edge + 1) % points.len()];
            cursor[1] += 1;
            Node {
                bounds: std::array::from_fn(|axis| {
                    if a[axis].is_finite() && b[axis].is_finite() {
                        [a[axis].min(b[axis]), a[axis].max(b[axis])]
                    } else {
                        // Nonfinite bounds cannot establish a rejection.
                        [f64::NEG_INFINITY, f64::INFINITY]
                    }
                }),
                end: first + 1,
                links: [ri, edge],
                leaf: true,
            }
        } else {
            let half = count / 2;
            let left = self.build(rings, first, half, cursor, work, cancel)?;
            let right = self.build(rings, first + half, count - half, cursor, work, cancel)?;
            Node {
                bounds: std::array::from_fn(|axis| {
                    [
                        self.nodes[left].bounds[axis][0].min(self.nodes[right].bounds[axis][0]),
                        self.nodes[left].bounds[axis][1].max(self.nodes[right].bounds[axis][1]),
                    ]
                }),
                end: first + count,
                links: [left, right],
                leaf: false,
            }
        };
        let index = self.nodes.len();
        // Capacity for the entire tree was admitted before construction.
        self.nodes.push(node);
        Ok(index)
    }

    fn query(
        &self,
        node_index: usize,
        query: &SegmentClearanceQuery<'_>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        let &SegmentClearanceQuery {
            ordinal,
            ri,
            edge,
            rings,
            bounds,
            margin,
        } = query;
        work.charge(1, cancel)?;
        let node = &self.nodes[node_index];
        if node.end <= ordinal + 1 {
            return Ok(());
        }
        // Endpoint boxes and their unions are exact. A downward-rounded axis
        // gap strictly beyond the margin is a lower bound for every enclosed
        // segment pair. Equality, overflow and uncertainty retain narrowphase.
        if (0..2).any(|axis| {
            let a = bounds[axis];
            let b = node.bounds[axis];
            [b[0] - a[1], a[0] - b[1]]
                .into_iter()
                .any(|gap| gap.is_finite() && down(gap) > margin)
        }) {
            return Ok(());
        }
        if node.leaf {
            let [rj, other] = node.links;
            let r = &rings[ri];
            if ri == rj
                && ((edge + 1) % r.points.len() == other || (other + 1) % r.points.len() == edge)
            {
                return Ok(());
            }
            work.charge(1, cancel)?;
            let s = &rings[rj];
            if segment_gap(
                r.points[edge],
                r.points[(edge + 1) % r.points.len()],
                s.points[other],
                s.points[(other + 1) % s.points.len()],
            ) <= margin
            {
                return Err(error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    format!(
                        "native union boundary cycle pair {ri}/{rj} has nonincident features meeting native/rounding tubes"
                    ),
                ));
            }
        } else {
            for child in node.links {
                self.query(child, query, work, cancel)?;
            }
        }
        Ok(())
    }
}

pub(super) fn feature_clearance(
    rings: &[Ring],
    radius: f64,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let checkpoint = scratch.bytes;
    let result = (|| {
        work.charge(1, cancel)?;
        let mut count = 0_usize;
        for ring in rings {
            work.charge(1, cancel)?;
            count = count.checked_add(ring.points.len()).ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "segment index count overflow",
                )
            })?;
        }
        if count == 0 {
            return Ok(());
        }
        let capacity = count
            .checked_mul(2)
            .and_then(|n| n.checked_sub(1))
            .ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "segment index node count overflow",
                )
            })?;
        let mut index = SegmentIndex { nodes: Vec::new() };
        scratch.reserve(&mut index.nodes, capacity, work, cancel)?;
        let root = index.build(rings, 0, count, &mut [0, 0], work, cancel)?;
        let margin = up(4.0 * radius);
        let mut ordinal = 0;
        for (ri, ring) in rings.iter().enumerate() {
            for edge in 0..ring.points.len() {
                work.charge(1, cancel)?;
                let a = ring.points[edge];
                let b = ring.points[(edge + 1) % ring.points.len()];
                if delta(a, b).iter().all(|v| {
                    let lower = if v.lo > 0.0 {
                        v.lo
                    } else if v.hi < 0.0 {
                        -v.hi
                    } else {
                        0.0
                    };
                    lower <= margin
                }) {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        format!(
                            "native union boundary cycle {ri} edge cannot be separated from its endpoint/rounding tube"
                        ),
                    ));
                }
                let bounds = std::array::from_fn(|axis| {
                    if a[axis].is_finite() && b[axis].is_finite() {
                        [a[axis].min(b[axis]), a[axis].max(b[axis])]
                    } else {
                        [f64::NEG_INFINITY, f64::INFINITY]
                    }
                });
                index.query(
                    root,
                    &SegmentClearanceQuery {
                        ordinal,
                        ri,
                        edge,
                        rings,
                        bounds,
                        margin,
                    },
                    work,
                    cancel,
                )?;
                ordinal += 1;
            }
        }
        Ok(())
    })();
    // The tree is gone on every exit; failed/cancelled work is never refunded.
    scratch.bytes = checkpoint;
    result
}

fn cycle_anchor(
    ring: &Ring,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<usize>, GeometryError> {
    let mut anchor = 0;
    let mut unique = true;
    for (i, point) in ring.points.iter().enumerate() {
        work.charge(1, cancel)?;
        if point.iter().any(|v| !v.is_finite()) {
            return Ok(None);
        }
        let first = ring.points[anchor];
        // Numeric comparison deliberately treats both signed zeros as the
        // same exact geometric coordinate, just like point equality below.
        if point[0] < first[0] || point[0] == first[0] && point[1] < first[1] {
            anchor = i;
            unique = true;
        } else if i != anchor && *point == first {
            unique = false;
        }
    }
    Ok((unique && !ring.points.is_empty()).then_some(anchor))
}

/// A complete exact oriented inventory, not a proximity match or a hash proof.
/// Returned indices map each target cycle to its source. Native uncertainty
/// and embedding obligations must still be discharged by the caller.
pub(super) fn exact_cycle_mapping(
    source: &[Ring],
    target: &[Ring],
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Vec<usize>>, GeometryError> {
    let checkpoint = scratch.bytes;
    let result = (|| {
        work.charge(1, cancel)?;
        if source.len() != target.len() {
            return Ok(None);
        }
        let mut anchors = Vec::new();
        let mut used = Vec::new();
        let mut mapping = Vec::new();
        scratch.reserve(&mut anchors, source.len(), work, cancel)?;
        scratch.reserve(&mut used, source.len(), work, cancel)?;
        scratch.reserve(&mut mapping, target.len(), work, cancel)?;
        used.resize(source.len(), false);
        for ring in source {
            let Some(anchor) = cycle_anchor(ring, work, cancel)? else {
                return Ok(None);
            };
            anchors.push(anchor);
        }
        for ring in target {
            let Some(anchor) = cycle_anchor(ring, work, cancel)? else {
                return Ok(None);
            };
            let mut matched = None;
            for (index, original) in source.iter().enumerate() {
                work.charge(1, cancel)?;
                if used[index]
                    || original.hole != ring.hole
                    || original.points.len() != ring.points.len()
                    || original.points[anchors[index]] != ring.points[anchor]
                {
                    continue;
                }
                let mut equal = true;
                for offset in 0..ring.points.len() {
                    work.charge(1, cancel)?;
                    if original.points[(anchors[index] + offset) % original.points.len()]
                        != ring.points[(anchor + offset) % ring.points.len()]
                    {
                        equal = false;
                        break;
                    }
                }
                if equal {
                    if matched.is_some() {
                        return Ok(None);
                    }
                    matched = Some(index);
                }
            }
            let Some(index) = matched else {
                return Ok(None);
            };
            used[index] = true;
            mapping.push(index);
        }
        Ok(Some(mapping))
    })();
    scratch.bytes = checkpoint;
    if let Ok(Some(mapping)) = &result {
        scratch.bytes += mapping.capacity() * std::mem::size_of::<usize>();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polygon(points: &[[f64; 2]], hole: bool) -> Ring {
        Ring {
            hole,
            points: points.to_vec(),
        }
    }

    fn square(x: f64, y: f64, side: f64, hole: bool) -> Ring {
        let mut points = vec![[x, y], [x + side, y], [x + side, y + side], [x, y + side]];
        if hole {
            points.reverse();
        }
        Ring { hole, points }
    }

    // Independent exhaustive enumeration: no boxes, tree, ordinals or index
    // rejection. The unchanged narrowphase defines the conservative proof.
    fn exhaustive(rings: &[Ring], radius: f64, work: &mut GeometryWorkBudget) -> bool {
        let cancel = AtomicBool::new(false);
        let margin = up(4.0 * radius);
        for (ri, r) in rings.iter().enumerate() {
            for i in 0..r.points.len() {
                work.charge(1, &cancel).unwrap();
                let a = r.points[i];
                let b = r.points[(i + 1) % r.points.len()];
                let lower = delta(a, b).map(|v| {
                    if v.lo > 0.0 {
                        v.lo
                    } else if v.hi < 0.0 {
                        -v.hi
                    } else {
                        0.0
                    }
                });
                if lower.iter().all(|v| *v <= margin) {
                    return false;
                }
                for (rj, s) in rings.iter().enumerate().skip(ri) {
                    for j in 0..s.points.len() {
                        if ri == rj
                            && (j <= i
                                || j == (i + 1) % r.points.len()
                                || i == (j + 1) % r.points.len())
                        {
                            continue;
                        }
                        work.charge(1, &cancel).unwrap();
                        if segment_gap(a, b, s.points[j], s.points[(j + 1) % s.points.len()])
                            <= margin
                        {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    fn compare(rings: &[Ring], radius: f64) -> bool {
        let expected = exhaustive(rings, radius, &mut GeometryWorkBudget::new(u64::MAX));
        let mut scratch = Scratch { bytes: 19 };
        let mut work = GeometryWorkBudget::new(u64::MAX);
        let actual = feature_clearance(
            rings,
            radius,
            &mut scratch,
            &mut work,
            &AtomicBool::new(false),
        );
        assert_eq!(scratch.bytes, 19);
        assert_eq!(actual.is_ok(), expected);
        if let Err(failure) = actual {
            assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
        }
        expected
    }

    #[test]
    fn hierarchy_matches_exhaustive_crossing_touching_and_tube_gaps() {
        assert!(!compare(
            &[polygon(
                &[[0.0, 0.0], [4.0, 4.0], [0.0, 4.0], [4.0, 0.0]],
                false,
            )],
            0.0
        ));
        assert!(!compare(
            &[square(0.0, 0.0, 4.0, false), square(4.0, 1.0, 1.0, false)],
            0.0
        ));
        assert!(!compare(
            &[square(0.0, 0.0, 4.0, false), square(2.0, 2.0, 4.0, false)],
            0.0
        ));
        for radius in [0.0, 0.001, 0.125] {
            for gap in [0.0, 2.0 * radius, 4.0 * radius, 8.0 * radius + 0.01] {
                compare(
                    &[
                        square(0.0, 0.0, 4.0, false),
                        square(4.0 + gap, 1.0, 1.0, false),
                    ],
                    radius,
                );
            }
        }
        assert!(compare(
            &[square(0.0, 0.0, 12.0, false), square(3.0, 3.0, 6.0, true)],
            0.01
        ));
        assert!(!compare(
            &[square(0.0, 0.0, 12.0, false), square(0.01, 3.0, 6.0, true)],
            0.01
        ));
    }

    fn detailed(count: usize, x: f64, y: f64) -> Ring {
        Ring {
            hole: false,
            points: (0..count)
                .map(|i| {
                    let phase = (i as f64) * std::f64::consts::TAU / count as f64;
                    [x + 20.0 * phase.cos(), y + 20.0 * phase.sin()]
                })
                .collect(),
        }
    }

    #[test]
    fn hierarchy_matches_detailed_contours_and_reduces_actual_work() {
        for count in [128, 256, 512] {
            let rings = [detailed(count, 0.0, 0.0), detailed(count, 50.0, 7.0)];
            let mut oracle_work = GeometryWorkBudget::new(u64::MAX);
            assert!(exhaustive(&rings, 0.00001, &mut oracle_work));
            let mut indexed_work = GeometryWorkBudget::new(u64::MAX);
            feature_clearance(
                &rings,
                0.00001,
                &mut Scratch { bytes: 0 },
                &mut indexed_work,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert!(indexed_work.consumed() < oracle_work.consumed());
            for shift in [0.0, 5.0, 39.9, 40.0, 40.1] {
                compare(
                    &[detailed(count, 0.0, 0.0), detailed(count, shift, 0.0)],
                    0.00001,
                );
            }
        }
    }

    fn exact(source: &[Ring], target: &[Ring]) -> Option<Vec<usize>> {
        let mut scratch = Scratch { bytes: 23 };
        let mapping = exact_cycle_mapping(
            source,
            target,
            &mut scratch,
            &mut GeometryWorkBudget::new(u64::MAX),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            scratch.bytes,
            23 + mapping
                .as_ref()
                .map_or(0, |m| { m.capacity() * std::mem::size_of::<usize>() })
        );
        mapping
    }

    #[test]
    fn identity_is_complete_oriented_exact_and_role_preserving() {
        let source = [square(0.0, 0.0, 10.0, false), square(2.0, 2.0, 3.0, true)];
        let mut outer = square(0.0, 0.0, 10.0, false);
        outer.points.rotate_left(2);
        let mut hole = square(2.0, 2.0, 3.0, true);
        hole.points.rotate_left(1);
        assert_eq!(exact(&source, &[hole, outer]), Some(vec![1, 0]));
        let mut reversed = square(0.0, 0.0, 10.0, false);
        reversed.points.reverse();
        assert!(exact(&source[..1], &[reversed]).is_none());
        let mut wrong_role = square(0.0, 0.0, 10.0, false);
        wrong_role.hole = true;
        assert!(exact(&source[..1], &[wrong_role]).is_none());
        let mut perturbed = square(0.0, 0.0, 10.0, false);
        perturbed.points[1][0] = up(perturbed.points[1][0]);
        assert!(exact(&source[..1], &[perturbed]).is_none());
        assert!(exact(&source, &[square(0.0, 0.0, 10.0, false)]).is_none());
        assert!(
            exact(
                &source,
                &[square(0.0, 0.0, 10.0, false), square(0.0, 0.0, 10.0, false),]
            )
            .is_none()
        );
        let duplicates = [square(0.0, 0.0, 10.0, false), square(0.0, 0.0, 10.0, false)];
        assert!(exact(&duplicates, &duplicates).is_none());
        let repeated = polygon(&[[0.0, 0.0], [2.0, 0.0], [0.0, 0.0], [0.0, 2.0]], false);
        assert!(
            exact(
                std::slice::from_ref(&repeated),
                std::slice::from_ref(&repeated)
            )
            .is_none()
        );
    }

    fn section(ring: Ring) -> SectionLoopMm {
        let mut points_mm: Vec<_> = ring.points.into_iter().map(|p| [p[0], p[1], 4.0]).collect();
        points_mm.push(points_mm[0]);
        SectionLoopMm {
            is_hole: ring.hole,
            points_mm,
        }
    }

    #[test]
    fn filled_identity_keeps_native_tube_and_general_nonidentical_path() {
        let source = [
            section(square(0.0, 0.0, 10.0, false)),
            section(square(0.01, 2.0, 3.0, true)),
        ];
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(u64::MAX);
        assert_eq!(filled_projection_distance(
            &source, &source, 0.0, 0.0, MAX_SCRATCH, &mut work, &cancel,
        ).unwrap(), 0.0);
        assert_eq!(
            filled_projection_distance(
                &source,
                &source,
                0.01,
                0.1,
                MAX_SCRATCH,
                &mut work,
                &cancel,
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
        let original = [section(square(0.0, 0.0, 10.0, false))];
        let moved = [section(square(0.001, 0.002, 10.0, false))];
        let distance = filled_projection_distance(
            &original,
            &moved,
            0.0,
            0.01,
            MAX_SCRATCH,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(distance > 0.0 && distance <= 0.01);
        let mut reversed = square(0.0, 0.0, 10.0, false);
        reversed.points.reverse();
        assert!(
            filled_projection_distance(
                &original,
                &[section(reversed)],
                0.0,
                0.01,
                MAX_SCRATCH,
                &mut work,
                &cancel,
            )
            .is_err()
        );
    }

    #[test]
    fn index_and_identity_refuse_resources_and_keep_failed_work() {
        let rings = [detailed(128, 0.0, 0.0)];
        let cancel = AtomicBool::new(false);
        for bytes in [0, MAX_SCRATCH] {
            let mut scratch = Scratch { bytes };
            let mut work = GeometryWorkBudget::new(if bytes == 0 { 1 } else { u64::MAX });
            assert_eq!(
                feature_clearance(&rings, 0.0, &mut scratch, &mut work, &cancel,)
                    .unwrap_err()
                    .code,
                GeometryErrorCode::ResourceLimit
            );
            assert_eq!(scratch.bytes, bytes);
            assert!(work.consumed() > 0);
        }
        let mut limited_scratch = Scratch { bytes: 0 };
        let mut limited_work = GeometryWorkBudget::new(1);
        assert_eq!(
            exact_cycle_mapping(
                &rings,
                &rings,
                &mut limited_scratch,
                &mut limited_work,
                &cancel,
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(limited_scratch.bytes, 0);
        assert!(limited_work.consumed() > 0);
        let mut scratch = Scratch { bytes: MAX_SCRATCH };
        let mut work = GeometryWorkBudget::new(u64::MAX);
        assert_eq!(
            exact_cycle_mapping(&rings, &rings, &mut scratch, &mut work, &cancel,)
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(scratch.bytes, MAX_SCRATCH);
        assert!(work.consumed() > 0);
        let cancelled = AtomicBool::new(true);
        let before = work.consumed();
        assert_eq!(
            feature_clearance(&rings, 0.0, &mut scratch, &mut work, &cancelled,)
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled
        );
        assert_eq!(
            exact_cycle_mapping(&rings, &rings, &mut scratch, &mut work, &cancelled,)
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled
        );
        assert!(work.consumed() > before);
        assert_eq!(scratch.bytes, MAX_SCRATCH);
    }
}
