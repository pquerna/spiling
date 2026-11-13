// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Concrete interval predicates for actual polygonal material, not a Boolean
//! offset's work tolerance. Distances concern complete segments and filled sets.
use super::GeometryWorkBudget;
use crate::{GeometryError, SectionLoopMm, parametric::Interval};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Copy)]
pub struct PlanarCapsuleClearancePolicy {
    pub radius_mm: f64,
    pub required_gap_mm: f64,
    /// Euclidean enclosure of the physical emitted endpoint versus its supplied
    /// binary64 coordinate. Linear interpolation preserves this endpoint bound.
    pub endpoint_error_mm: f64,
}

/// A straight stroke capsule in the caller's shared orthonormal XY frame.
/// Vertical extent and deposition semantics remain the caller's responsibility.
#[derive(Debug, Clone, Copy)]
pub struct PlanarCapsuleMm {
    pub segment_xy_mm: [[f64; 2]; 2],
    pub radius_mm: f64,
    /// Euclidean endpoint enclosure; interpolation preserves this bound.
    pub endpoint_error_mm: f64,
}

/// A whole-capsule separation fact for supplied segments or filled polygons.
/// It does not certify native provenance or conservatism of an outer envelope.
#[derive(Debug, Clone, Copy)]
pub struct PlanarCapsuleClearance {
    minimum_gap_mm: Option<f64>,
}
impl PlanarCapsuleClearance {
    /// Lower bound after bead radius and physical endpoint enclosure. None means
    /// that the supplied forbidden set has no boundaries/material whatsoever.
    pub fn minimum_gap_mm(&self) -> Option<f64> {
        self.minimum_gap_mm
    }
}

#[derive(Clone, Copy)]
pub(super) enum FillRule {
    EvenOdd,
    PositiveCoverage,
}
pub(super) fn orientation(a: [f64; 2], b: [f64; 2], p: [Interval; 2]) -> Interval {
    let dx = Interval::point(b[0]).sub(Interval::point(a[0]));
    let dy = Interval::point(b[1]).sub(Interval::point(a[1]));
    dx.mul(p[1].sub(Interval::point(a[1])))
        .sub(dy.mul(p[0].sub(Interval::point(a[0]))))
}

/// Half-open ray crossings are combinatorial comparisons of exact binary64
/// inputs; the side predicate is outward enclosed. No ambiguous side is snapped.
pub(super) fn material_at(
    p: [f64; 2],
    loops: &[SectionLoopMm],
    rule: FillRule,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<bool>, GeometryError> {
    let mut parity = false;
    let mut degree = 0_i64;
    for ring in loops {
        let mut ring_inside = false;
        for edge in ring.points_mm.windows(2) {
            work.charge(1, cancel)?;
            let a = [edge[0][0], edge[0][1]];
            let b = [edge[1][0], edge[1][1]];
            if (a[1] > p[1]) == (b[1] > p[1]) {
                continue;
            }
            let side = orientation(a, b, p.map(Interval::point));
            if side.contains_zero() {
                return Ok(None);
            }
            if (side.lo > 0.0) == (b[1] > a[1]) {
                ring_inside = !ring_inside;
            }
        }
        if ring_inside {
            parity = !parity;
            degree = degree
                .checked_add(if ring.is_hole { -1 } else { 1 })
                .ok_or_else(|| {
                    GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "polygon coverage degree overflow",
                    )
                })?;
        }
    }
    Ok(Some(match rule {
        FillRule::EvenOdd => parity,
        FillRule::PositiveCoverage => degree > 0,
    }))
}
fn norm_enclosure(delta: [Interval; 2]) -> Result<Interval, GeometryError> {
    let squared = delta[0].square().add(delta[1].square());
    Interval {
        lo: squared.lo.max(0.0),
        hi: squared.hi,
    }
    .sqrt()
}
fn norm_lower(delta: [Interval; 2]) -> Result<f64, GeometryError> {
    Ok(norm_enclosure(delta)?.lo)
}
/// Enclose closest distance for EVERY point of an interval rectangle. This is
/// shared by whole-cell material cover and actual emitted-capsule separation.
pub(super) fn point_segment_distance(
    p: [Interval; 2],
    a: [f64; 2],
    b: [f64; 2],
) -> Result<Interval, GeometryError> {
    let d = std::array::from_fn::<_, 2, _>(|i| Interval::point(b[i]).sub(Interval::point(a[i])));
    let q = std::array::from_fn::<_, 2, _>(|i| p[i].sub(Interval::point(a[i])));
    if a == b {
        return norm_enclosure(q);
    }
    let length = d[0].square().add(d[1].square());
    if length.lo <= 0.0 {
        return Ok(Interval {
            lo: 0.0,
            hi: norm_enclosure(q)?.hi,
        });
    }
    let t = q[0].mul(d[0]).add(q[1].mul(d[1])).div(length)?;
    let t = Interval::new(t.lo.clamp(0.0, 1.0), t.hi.clamp(0.0, 1.0))?;
    norm_enclosure(std::array::from_fn(|i| q[i].sub(t.mul(d[i]))))
}
fn point_segment_lower(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> Result<f64, GeometryError> {
    Ok(point_segment_distance(p.map(Interval::point), a, b)?.lo)
}
pub(super) fn segment_distance_lower(
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
    d: [f64; 2],
) -> Result<f64, GeometryError> {
    if a == b {
        return point_segment_lower(a, c, d);
    }
    if c == d {
        return point_segment_lower(c, a, b);
    }
    let box_delta = std::array::from_fn(|axis| {
        let left = Interval::point(c[axis].min(d[axis]))
            .sub(Interval::point(a[axis].max(b[axis])))
            .lo;
        let right = Interval::point(a[axis].min(b[axis]))
            .sub(Interval::point(c[axis].max(d[axis])))
            .lo;
        Interval::point(left.max(right).max(0.0))
    });
    let box_lower = norm_lower(box_delta)?;
    let same_side =
        |x: Interval, y: Interval| (x.lo > 0.0 && y.lo > 0.0) || (x.hi < 0.0 && y.hi < 0.0);
    let separated = box_lower > 0.0
        || same_side(
            orientation(a, b, c.map(Interval::point)),
            orientation(a, b, d.map(Interval::point)),
        )
        || same_side(
            orientation(c, d, a.map(Interval::point)),
            orientation(c, d, b.map(Interval::point)),
        );
    if !separated {
        return Ok(0.0);
    }
    // In two dimensions, disjoint compact segments attain their minimum at an
    // endpoint of at least one segment. Establish disjointness FIRST; endpoint
    // distances alone would miss a crossing with four distant endpoints.
    Ok(box_lower.max(
        point_segment_lower(a, c, d)?
            .min(point_segment_lower(b, c, d)?)
            .min(point_segment_lower(c, a, b)?)
            .min(point_segment_lower(d, a, b)?),
    ))
}
fn unresolved(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::AnalysisIndeterminate, message)
}

/// Prove separation of two complete nominal stroke capsules, including both
/// radii and both emitted-coordinate enclosures. No sampling or offset mask is
/// involved; scratch is constant and work shares the caller's cumulative budget.
pub fn certify_planar_capsule_pair_clearance(
    capsules: [PlanarCapsuleMm; 2],
    required_gap_mm: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PlanarCapsuleClearance, GeometryError> {
    work.charge(1, cancel)?;
    if !required_gap_mm.is_finite()
        || required_gap_mm < 0.0
        || capsules.iter().any(|capsule| {
            capsule
                .segment_xy_mm
                .iter()
                .flatten()
                .any(|value| !value.is_finite())
                || [capsule.radius_mm, capsule.endpoint_error_mm]
                    .into_iter()
                    .any(|value| !value.is_finite() || value < 0.0)
        })
    {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "invalid finite capsule pair or clearance",
        ));
    }
    let reserve = Interval::point(capsules[0].radius_mm)
        .add(Interval::point(capsules[1].radius_mm))
        .add(Interval::point(capsules[0].endpoint_error_mm))
        .add(Interval::point(capsules[1].endpoint_error_mm));
    let required = reserve.add(Interval::point(required_gap_mm));
    if !required.hi.is_finite() {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "capsule-pair clearance overflows",
        ));
    }
    let [a, b] = capsules[0].segment_xy_mm;
    let [c, d] = capsules[1].segment_xy_mm;
    // An outward enclosed axis gap between the segments' bounds lower-bounds
    // their distance; when it already meets the requirement, it is the proof.
    let axis_gap = (0..2)
        .flat_map(|axis| {
            [
                Interval::point(c[axis].min(d[axis]))
                    .sub(Interval::point(a[axis].max(b[axis])))
                    .lo,
                Interval::point(a[axis].min(b[axis]))
                    .sub(Interval::point(c[axis].max(d[axis])))
                    .lo,
            ]
        })
        .fold(f64::NEG_INFINITY, f64::max);
    if axis_gap > 0.0 && axis_gap >= required.hi {
        let gap = Interval::point(axis_gap).sub(reserve).lo;
        if gap >= required_gap_mm {
            return Ok(PlanarCapsuleClearance {
                minimum_gap_mm: Some(gap),
            });
        }
    }
    let lower = segment_distance_lower(a, b, c, d)?;
    let gap = Interval::point(lower).sub(reserve).lo;
    if lower < required.hi || gap < required_gap_mm {
        return Err(unresolved(
            "actual emitted capsule pair does not prove the unchanged requested clearance",
        ));
    }
    Ok(PlanarCapsuleClearance {
        minimum_gap_mm: Some(gap),
    })
}

/// Prove an actual emitted stroke capsule stays outside filled forbidden loops
/// by the unchanged requested gap. Each ring uses even-odd fill; material is
/// positive outer coverage minus hole coverage. Overlapping positive outers ADD
/// and therefore cannot cancel. Native producers establish their loop topology.
/// No offset padding is accepted as evidence. Scratch is constant, and every
/// boundary comparison consumes the caller's cumulative work budget.
pub fn certify_planar_capsule_clearance(
    segment_xy_mm: [[f64; 2]; 2],
    forbidden: &[SectionLoopMm],
    policy: PlanarCapsuleClearancePolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PlanarCapsuleClearance, GeometryError> {
    let required = capsule_requirement(segment_xy_mm, policy, work, cancel)?;
    if forbidden.is_empty() {
        return Ok(PlanarCapsuleClearance {
            minimum_gap_mm: None,
        });
    }
    validate_forbidden(forbidden, work, cancel)?;
    capsule_clearance_core(segment_xy_mm, forbidden, policy, required, work, cancel)
}

/// True when every point within `reserve_mm` of box `a` is at least
/// `required_gap_mm` from every point of box `b`, by an outward enclosed axis
/// gap. A capsule whose centerline lies in `a` with radius plus endpoint error
/// at most `reserve_mm` is then clear of anything inside `b`.
pub fn planar_bounds_clear(
    a: [[f64; 2]; 2],
    b: [[f64; 2]; 2],
    reserve_mm: f64,
    required_gap_mm: f64,
) -> bool {
    let required = Interval::point(reserve_mm).add(Interval::point(required_gap_mm)).hi;
    let gap = (0..2)
        .flat_map(|axis| {
            [
                Interval::point(b[0][axis]).sub(Interval::point(a[1][axis])).lo,
                Interval::point(a[0][axis]).sub(Interval::point(b[1][axis])).lo,
            ]
        })
        .fold(f64::NEG_INFINITY, f64::max);
    a.iter().chain(&b).flatten().all(|v| v.is_finite())
        && reserve_mm >= 0.0
        && required_gap_mm >= 0.0
        && required.is_finite()
        && gap > 0.0
        && gap >= required
}

/// A forbidden filled polygon validated once, with its exact XY bounds.
pub struct PreparedPlanarForbidden<'a> {
    loops: &'a [SectionLoopMm],
    bounds: Option<[[f64; 2]; 2]>,
}

/// Validate `forbidden` exactly as [`certify_planar_capsule_clearance`] does,
/// once for any number of capsules.
pub fn prepare_planar_forbidden<'a>(
    forbidden: &'a [SectionLoopMm],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PreparedPlanarForbidden<'a>, GeometryError> {
    validate_forbidden(forbidden, work, cancel)?;
    let bounds = forbidden
        .iter()
        .flat_map(|ring| &ring.points_mm)
        .fold(None, |bounds: Option<[[f64; 2]; 2]>, p| {
            Some(bounds.map_or([[p[0], p[1]], [p[0], p[1]]], |[lo, hi]| {
                [[lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]]
            }))
        });
    Ok(PreparedPlanarForbidden {
        loops: forbidden,
        bounds,
    })
}

/// The same whole-capsule proof against a prepared polygon. When an outward
/// enclosed axis gap between the segment's and the polygon's bounds already
/// meets the requirement, that gap lower-bounds every boundary distance and
/// places the whole centerline outside the filled polygon.
pub fn certify_planar_capsule_clearance_prepared(
    segment_xy_mm: [[f64; 2]; 2],
    forbidden: &PreparedPlanarForbidden<'_>,
    policy: PlanarCapsuleClearancePolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PlanarCapsuleClearance, GeometryError> {
    let required = capsule_requirement(segment_xy_mm, policy, work, cancel)?;
    let Some([lo, hi]) = forbidden.bounds else {
        return Ok(PlanarCapsuleClearance {
            minimum_gap_mm: None,
        });
    };
    let s = segment_xy_mm;
    let gap = (0..2)
        .flat_map(|axis| {
            let (smin, smax) = (s[0][axis].min(s[1][axis]), s[0][axis].max(s[1][axis]));
            [
                Interval::point(smin).sub(Interval::point(hi[axis])).lo,
                Interval::point(lo[axis]).sub(Interval::point(smax)).lo,
            ]
        })
        .fold(f64::NEG_INFINITY, f64::max);
    if gap.is_finite() && gap > 0.0 && gap >= required.hi {
        let reserve =
            Interval::point(policy.radius_mm).add(Interval::point(policy.endpoint_error_mm));
        let minimum = Interval::point(gap).sub(reserve).lo;
        if minimum >= policy.required_gap_mm {
            return Ok(PlanarCapsuleClearance {
                minimum_gap_mm: Some(minimum),
            });
        }
    }
    capsule_clearance_core(segment_xy_mm, forbidden.loops, policy, required, work, cancel)
}

fn capsule_requirement(
    segment_xy_mm: [[f64; 2]; 2],
    policy: PlanarCapsuleClearancePolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Interval, GeometryError> {
    work.charge(1, cancel)?;
    if segment_xy_mm.iter().flatten().any(|x| !x.is_finite())
        || [
            policy.radius_mm,
            policy.required_gap_mm,
            policy.endpoint_error_mm,
        ]
        .into_iter()
        .any(|x| !x.is_finite() || x < 0.0)
    {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "invalid finite support capsule or clearance policy",
        ));
    }
    let reserve = Interval::point(policy.radius_mm).add(Interval::point(policy.endpoint_error_mm));
    let required = reserve.add(Interval::point(policy.required_gap_mm));
    if !required.hi.is_finite() {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "support capsule clearance overflows",
        ));
    }
    Ok(required)
}

fn validate_forbidden(
    forbidden: &[SectionLoopMm],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    // Validate all input before a geometric early refusal, so a malformed later
    // polygon is never disguised as a corridor-clearance limitation.
    for ring in forbidden {
        work.charge(1, cancel)?;
        if ring.points_mm.len() < 4 || ring.points_mm.first() != ring.points_mm.last() {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "clearance forbidden loop is not an explicitly closed polygon",
            ));
        }
        for point in &ring.points_mm {
            work.charge(1, cancel)?;
            if point.iter().any(|x| !x.is_finite()) {
                return Err(GeometryError::new(
                    GeometryErrorCode::InvalidGeometry,
                    "clearance forbidden polygon has nonfinite coordinates",
                ));
            }
        }
    }
    Ok(())
}

fn capsule_clearance_core(
    segment_xy_mm: [[f64; 2]; 2],
    forbidden: &[SectionLoopMm],
    policy: PlanarCapsuleClearancePolicy,
    required: Interval,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PlanarCapsuleClearance, GeometryError> {
    let reserve = Interval::point(policy.radius_mm).add(Interval::point(policy.endpoint_error_mm));
    let mut lower = f64::INFINITY;
    for ring in forbidden {
        for edge in ring.points_mm.windows(2) {
            work.charge(1, cancel)?;
            lower = lower.min(segment_distance_lower(
                segment_xy_mm[0],
                segment_xy_mm[1],
                [edge[0][0], edge[0][1]],
                [edge[1][0], edge[1][1]],
            )?);
            if lower < required.hi || lower == 0.0 {
                return Err(unresolved(
                    "actual emitted capsule clearance from a filled forbidden boundary is unproved",
                ));
            }
        }
    }
    // A connected segment separated from every boundary cannot change filled
    // membership. One robust starting membership therefore proves its ENTIRE
    // centerline is outside; this is not a sampled minimum-distance test.
    if material_at(
        segment_xy_mm[0],
        forbidden,
        FillRule::PositiveCoverage,
        work,
        cancel,
    )? != Some(false)
    {
        return Err(unresolved(
            "actual emitted capsule starts in, or has unresolved membership against, forbidden material",
        ));
    }
    let gap = Interval::point(lower).sub(reserve).lo;
    if gap < policy.required_gap_mm {
        return Err(unresolved(
            "capsule arithmetic reserve prevents proof of the unchanged requested gap",
        ));
    }
    Ok(PlanarCapsuleClearance {
        minimum_gap_mm: Some(gap),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rectangle(bounds: [[f64; 2]; 2], hole: bool) -> SectionLoopMm {
        let [[x0, x1], [y0, y1]] = bounds;
        SectionLoopMm {
            is_hole: hole,
            points_mm: vec![
                [x0, y0, 0.0],
                [x1, y0, 0.0],
                [x1, y1, 0.0],
                [x0, y1, 0.0],
                [x0, y0, 0.0],
            ],
        }
    }
    fn policy() -> PlanarCapsuleClearancePolicy {
        PlanarCapsuleClearancePolicy {
            radius_mm: 0.125,
            required_gap_mm: 0.125,
            endpoint_error_mm: 0.0,
        }
    }
    #[test]
    fn prepared_bounds_proof_accepts_exactly_what_the_full_proof_accepts() {
        let mask = [
            rectangle([[0.0, 10.0], [0.0, 10.0]], false),
            rectangle([[3.0, 7.0], [3.0, 7.0]], true),
        ];
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::query();
        let prepared = prepare_planar_forbidden(&mask, &mut work, &cancel).unwrap();
        let mut checked = 0;
        for i in -40..=40 {
            for j in -40..=40 {
                let (x, y) = (5.0 + f64::from(i) * 0.2, 5.0 + f64::from(j) * 0.2);
                for d in [[0.0, 0.0], [0.25, 0.0], [0.0, 0.3], [3.0, 1.0]] {
                    let segment = [[x, y], [x + d[0], y + d[1]]];
                    let full = certify_planar_capsule_clearance(
                        segment, &mask, policy(), &mut work, &cancel,
                    );
                    let fast = certify_planar_capsule_clearance_prepared(
                        segment, &prepared, policy(), &mut work, &cancel,
                    );
                    assert_eq!(full.is_ok(), fast.is_ok(), "{segment:?}");
                    if let (Ok(full), Ok(fast)) = (full, fast) {
                        let (full, fast) =
                            (full.minimum_gap_mm().unwrap(), fast.minimum_gap_mm().unwrap());
                        assert!(fast >= policy().required_gap_mm);
                        let inside = |p: [f64; 2]| p.iter().all(|v| (0.0..=10.0).contains(v));
                        if inside(segment[0]) || inside(segment[1]) {
                            // Inside the polygon's bounds: the full proof itself.
                            assert_eq!(fast, full);
                            continue;
                        }
                        // Accepted capsules here lie outside the outer square; any
                        // sampled centerline distance bounds the true minimum.
                        let sampled = (0..=64)
                            .map(|k| {
                                let t = f64::from(k) / 64.0;
                                let p = [x + t * d[0], y + t * d[1]];
                                let q = [p[0].clamp(0.0, 10.0), p[1].clamp(0.0, 10.0)];
                                (p[0] - q[0]).hypot(p[1] - q[1])
                            })
                            .fold(f64::INFINITY, f64::min);
                        assert!(fast <= sampled - policy().radius_mm + 1e-12, "{segment:?}");
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked > 0);
        let empty = prepare_planar_forbidden(&[], &mut work, &cancel).unwrap();
        assert!(certify_planar_capsule_clearance_prepared(
            [[0.0, 0.0], [1.0, 0.0]], &empty, policy(), &mut work, &cancel,
        )
        .unwrap()
        .minimum_gap_mm()
        .is_none());
    }
    #[test]
    fn filled_material_and_thin_crossings_are_not_boundary_only_clearance() {
        let mask = [rectangle([[0.0, 10.0], [0.0, 10.0]], false)];
        for segment in [[[2.0, 2.0], [8.0, 8.0]], [[-1.0, 5.0], [11.0, 5.0]]] {
            let mut work = GeometryWorkBudget::new(10_000);
            assert!(
                certify_planar_capsule_clearance(
                    segment,
                    &mask,
                    policy(),
                    &mut work,
                    &AtomicBool::new(false)
                )
                .is_err()
            );
        }
    }
    #[test]
    fn holes_remain_free_and_overlapping_positive_outers_do_not_cancel() {
        let mask = [
            rectangle([[0.0, 10.0], [0.0, 10.0]], false),
            rectangle([[2.0, 8.0], [2.0, 8.0]], true),
        ];
        let mut work = GeometryWorkBudget::new(10_000);
        assert!(
            certify_planar_capsule_clearance(
                [[3.0, 3.0], [7.0, 7.0]],
                &mask,
                policy(),
                &mut work,
                &AtomicBool::new(false)
            )
            .unwrap()
            .minimum_gap_mm()
            .unwrap()
                > 0.125
        );
        let overlap = [
            rectangle([[0.0, 10.0], [0.0, 10.0]], false),
            rectangle([[1.0, 9.0], [1.0, 9.0]], false),
        ];
        assert!(
            certify_planar_capsule_clearance(
                [[3.0, 3.0], [7.0, 7.0]],
                &overlap,
                policy(),
                &mut work,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
    #[test]
    fn whole_segment_distance_and_emitted_endpoint_reserve_are_enclosed() {
        let mask = [rectangle([[0.0, 1.0], [0.0, 1.0]], false)];
        let mut work = GeometryWorkBudget::new(10_000);
        let good = certify_planar_capsule_clearance(
            [[-1.0, -0.5], [2.0, -0.5]],
            &mask,
            policy(),
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(good.minimum_gap_mm().unwrap() > 0.25);
        let mut bad = policy();
        bad.endpoint_error_mm = 0.375;
        assert!(
            certify_planar_capsule_clearance(
                [[-1.0, -0.5], [2.0, -0.5]],
                &mask,
                bad,
                &mut work,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
    #[test]
    fn capsule_pair_reserves_both_radii_and_both_emission_errors() {
        let mut capsules = [
            PlanarCapsuleMm {
                segment_xy_mm: [[0.0, 0.0], [10.0, 0.0]],
                radius_mm: 0.2,
                endpoint_error_mm: 0.0,
            },
            PlanarCapsuleMm {
                segment_xy_mm: [[0.0, 0.7], [10.0, 0.7]],
                radius_mm: 0.3,
                endpoint_error_mm: 0.0,
            },
        ];
        let mut work = GeometryWorkBudget::new(100);
        let cancel = AtomicBool::new(false);
        let gap = certify_planar_capsule_pair_clearance(capsules, 0.1, &mut work, &cancel)
            .unwrap()
            .minimum_gap_mm()
            .unwrap();
        assert!(gap > 0.19 && gap <= 0.2);
        assert_eq!(
            certify_planar_capsule_pair_clearance(capsules, 0.25, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
        capsules[0].endpoint_error_mm = 0.06;
        capsules[1].endpoint_error_mm = 0.06;
        assert_eq!(
            certify_planar_capsule_pair_clearance(capsules, 0.1, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
    }
    #[test]
    fn capsule_pair_bounds_gap_accepts_only_what_the_distance_proof_accepts() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::query();
        let base = [[0.0, 0.0], [2.0, 1.0]];
        for i in -30..=30 {
            for j in -30..=30 {
                let o = [f64::from(i) * 0.11, f64::from(j) * 0.07];
                for d in [[0.0, 0.0], [1.0, 0.0], [0.3, -0.8], [-1.5, 1.5]] {
                    let other = [o, [o[0] + d[0], o[1] + d[1]]];
                    let capsules = [
                        PlanarCapsuleMm { segment_xy_mm: base, radius_mm: 0.2, endpoint_error_mm: 1e-6 },
                        PlanarCapsuleMm { segment_xy_mm: other, radius_mm: 0.25, endpoint_error_mm: 2e-6 },
                    ];
                    let reserve = Interval::point(0.2)
                        .add(Interval::point(0.25))
                        .add(Interval::point(1e-6))
                        .add(Interval::point(2e-6));
                    let required = reserve.add(Interval::point(0.15));
                    let lower = segment_distance_lower(base[0], base[1], other[0], other[1]).unwrap();
                    let reference = lower >= required.hi
                        && Interval::point(lower).sub(reserve).lo >= 0.15;
                    let result =
                        certify_planar_capsule_pair_clearance(capsules, 0.15, &mut work, &cancel);
                    if let Ok(result) = result {
                        // The bounds gap is itself a lower bound of the distance.
                        let gap = result.minimum_gap_mm().unwrap();
                        assert!(gap >= 0.15 && gap <= Interval::point(lower).sub(reserve).hi + 1e-9);
                        if !reference {
                            panic!("bounds path accepted beyond the distance proof: {other:?}");
                        }
                    } else {
                        assert!(!reference, "{other:?}");
                    }
                }
            }
        }
    }
    #[test]
    fn bounds_clearance_requires_the_whole_reserve_and_gap() {
        let a = [[0.0, 0.0], [1.0, 1.0]];
        let b = [[1.5, 0.0], [2.0, 1.0]];
        assert!(planar_bounds_clear(a, b, 0.2, 0.25));
        assert!(planar_bounds_clear(b, a, 0.2, 0.25));
        assert!(!planar_bounds_clear(a, b, 0.3, 0.25));
        assert!(!planar_bounds_clear(a, [[0.5, 0.5], [3.0, 3.0]], 0.0, 0.0));
        assert!(!planar_bounds_clear(a, [[f64::NAN, 0.0], [2.0, 1.0]], 0.0, 0.0));
    }
    #[test]
    fn crossing_capsules_are_not_separated_by_distant_endpoints() {
        let capsules = [
            PlanarCapsuleMm {
                segment_xy_mm: [[-10.0, 0.0], [10.0, 0.0]],
                radius_mm: 0.1,
                endpoint_error_mm: 0.0,
            },
            PlanarCapsuleMm {
                segment_xy_mm: [[0.0, -10.0], [0.0, 10.0]],
                radius_mm: 0.1,
                endpoint_error_mm: 0.0,
            },
        ];
        let mut work = GeometryWorkBudget::new(100);
        assert_eq!(
            certify_planar_capsule_pair_clearance(
                capsules,
                0.1,
                &mut work,
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
    }
    #[test]
    fn cancellation_and_shared_work_are_not_reset() {
        let mut work = GeometryWorkBudget::new(1);
        assert_eq!(
            certify_planar_capsule_clearance(
                [[0.0, 0.0], [1.0, 1.0]],
                &[],
                policy(),
                &mut work,
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
        assert_eq!(work.consumed(), 1);
    }
}
