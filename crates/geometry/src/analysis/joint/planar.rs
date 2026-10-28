// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Geometry-only exact cycle identity, closed nesting, clearance and actual pair collars.
//! No result supplies native material, pose, or approximation authority.

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
            let separation = segment_gap(
                r.points[edge],
                r.points[(edge + 1) % r.points.len()],
                s.points[other],
                s.points[(other + 1) % s.points.len()],
            );
            if separation <= margin {
                return Err(error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    format!(
                        "native union boundary cycle pair {ri}/{rj} has nonincident features meeting native/rounding tubes; edges {edge}/{other}, separation {separation:.17e}, margin {margin:.17e}, first {:?}->{:?}, second {:?}->{:?}",
                        r.points[edge],
                        r.points[(edge + 1) % r.points.len()],
                        s.points[other],
                        s.points[(other + 1) % s.points.len()],
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

fn jordan_joints(
    rings: &[Ring],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    for ring in rings {
        for i in 0..ring.points.len() {
            work.charge(1, cancel)?;
            let vertex = ring.points[i];
            let incoming = delta(
                ring.points[(i + ring.points.len() - 1) % ring.points.len()],
                vertex,
            );
            let outgoing = delta(ring.points[(i + 1) % ring.points.len()], vertex);
            if cross(incoming, outgoing).contains_zero()
                && incoming[0]
                    .mul(outgoing[0])
                    .add(incoming[1].mul(outgoing[1]))
                    .hi
                    >= 0.0
            {
                return Err(error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "registered native cycle has unresolved adjacent-edge overlap",
                ));
            }
        }
    }
    Ok(())
}

/// Used only AFTER complete Jordan and distinct-boundary separation. A winding
/// value labels one connected complementary component, hence the entire cycle;
/// the selected vertex is not itself material/source or containment authority.
fn jordan_parent(
    rings: &[Ring],
    child: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<usize>, GeometryError> {
    let mut parent: Option<usize> = None;
    for (candidate, ring) in rings.iter().enumerate() {
        work.charge(1, cancel)?;
        if candidate == child || !point_in_ring(ring, rings[child].points[0], work, cancel)? {
            continue;
        }
        if let Some(previous) = parent {
            if point_in_ring(&rings[previous], ring.points[0], work, cancel)? {
                parent = Some(candidate);
            } else if !point_in_ring(ring, rings[previous].points[0], work, cancel)? {
                return Err(error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "separated native cycles have inconsistent Jordan nesting",
                ));
            }
        } else {
            parent = Some(candidate);
        }
    }
    if parent.map_or(rings[child].hole, |p| rings[p].hole == rings[child].hole) {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "registered native cycle roots/parents disagree with material-left outer/hole roles",
        ));
    }
    Ok(parent)
}

/// Conservative LOWER clearance of ALL distinct complete boundaries. The
/// narrowphase is the existing interval segment predicate; dyadic endpoint
/// boxes prune only when their lower gap cannot decrease the current bound.
fn distinct_loop_clearance(
    rings: &[Ring],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<f64>, GeometryError> {
    if rings.len() < 2 {
        return Ok(None);
    }
    let mut minimum = f64::INFINITY;
    for (i, ring) in rings.iter().enumerate() {
        for other in &rings[i + 1..] {
            for edge in 0..ring.points.len() {
                let a = ring.points[edge];
                let b = ring.points[(edge + 1) % ring.points.len()];
                for next in 0..other.points.len() {
                    work.charge(1, cancel)?;
                    let c = other.points[next];
                    let d = other.points[(next + 1) % other.points.len()];
                    let lower = (0..2).fold(0.0_f64, |gap, axis| {
                        gap.max(down(c[axis].min(d[axis]) - a[axis].max(b[axis])))
                            .max(down(a[axis].min(b[axis]) - c[axis].max(d[axis])))
                    });
                    if lower >= minimum {
                        continue;
                    }
                    let gap = segment_gap(a, b, c, d);
                    if !gap.is_finite() || gap <= 0.0 {
                        return Err(error(
                            GeometryErrorCode::AnalysisIndeterminate,
                            "registered complete native cycle boundaries have no positive clearance lower bound",
                        ));
                    }
                    minimum = minimum.min(gap);
                }
            }
        }
    }
    Ok(Some(minimum))
}

/// Geometry part of a SOURCE-BOUND leaf registration. The caller has already
/// proved the complete original/candidate correspondence and binds this result
/// to the exact native sample, actual candidate, material realization and pose.
/// `exact` is issued only by the preceding exact_cycle_mapping proof; it reuses
/// its oriented whole-cycle identity, not scalar-zero source displacement.
#[allow(clippy::too_many_arguments)]
pub(super) fn capture_loop_registration(
    source: &[Ring],
    actual: &[Ring],
    registration: &mut [NativeLoopRegistration],
    rounding: f64,
    exact: bool,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<f64>, GeometryError> {
    let unresolved = || {
        error(
            GeometryErrorCode::AnalysisIndeterminate,
            "native/candidate complete cycle registration changes oriented parent incidence",
        )
    };
    if source.len() != actual.len() || registration.len() != actual.len() {
        return Err(unresolved());
    }
    // FIRST prove complete Jordan boundaries and separation. Winding below
    // only labels their complementary components; it cannot manufacture them.
    jordan_joints(source, work, cancel)?;
    feature_clearance(source, rounding, scratch, work, cancel)?;
    if !exact {
        jordan_joints(actual, work, cancel)?;
        feature_clearance(actual, 0.0, scratch, work, cancel)?;
    }
    for (i, original) in registration.iter().enumerate() {
        work.charge(1, cancel)?;
        if original.native_index >= source.len()
            || source[original.native_index].hole != actual[i].hole
        {
            return Err(unresolved());
        }
        for previous in &registration[..i] {
            work.charge(1, cancel)?;
            if previous.native_index == original.native_index {
                return Err(unresolved());
            }
        }
    }
    let clearance = distinct_loop_clearance(if exact { source } else { actual }, work, cancel)?;
    for i in 0..registration.len() {
        let native_parent = jordan_parent(source, registration[i].native_index, work, cancel)?;
        let actual_parent = if exact {
            let mut parent = None;
            if let Some(native_parent) = native_parent {
                for (j, original) in registration.iter().enumerate() {
                    work.charge(1, cancel)?;
                    if original.native_index == native_parent {
                        parent = Some(j);
                        break;
                    }
                }
                if parent.is_none() {
                    return Err(unresolved());
                }
            }
            parent
        } else {
            jordan_parent(actual, i, work, cancel)?
        };
        if actual_parent.map(|parent| registration[parent].native_index) != native_parent {
            return Err(unresolved());
        }
        registration[i].parent = actual_parent;
    }
    Ok(clearance)
}

/// An index-only exact view of ACTUAL footprint coordinates. Removing a
/// duplicate or an exactly collinear between-vertex preserves the entire trace;
/// no reference points, contour buffers or replacement material are constructed.
struct ActualCycle<'a> {
    points: &'a [[f64; 3]],
    vertices: Vec<usize>,
}
impl ActualCycle<'_> {
    fn len(&self) -> usize {
        self.vertices.len()
    }
    fn point(&self, i: usize) -> [f64; 2] {
        let p = self.points[self.vertices[i % self.len()]];
        [p[0], p[1]]
    }
}

pub(super) const PAIR_COLLAR_STACK_BYTES: usize = 8 * std::mem::size_of::<
    crate::exact_coefficient::ExactCoefficient,
>() + 8 * std::mem::size_of::<CollarCell>()
    + std::mem::size_of::<ActualPairCollarProof>();

fn coefficient_sign(
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
    vector: bool,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<i8>, GeometryError> {
    // vector=true: det(b,c-a). Otherwise det(b-a,c-a). Interval zero
    // never establishes incidence: the shared bounded error-free coefficient
    // adapter supplies the exact dyadic endpoint predicate.
    let x = if vector {
        b.map(Interval::point)
    } else {
        delta(b, a)
    };
    let enclosed = cross(x, delta(c, a));
    if enclosed.lo > 0.0 {
        return Ok(Some(1));
    }
    if enclosed.hi < 0.0 {
        return Ok(Some(-1));
    }
    use crate::exact_coefficient::ExactCoefficient as E;
    let (Some(x0), Some(x1), Some(y0), Some(y1)) = (
        if vector {
            Some(E::point(b[0]))
        } else {
            E::difference(b[0], a[0])
        },
        if vector {
            Some(E::point(b[1]))
        } else {
            E::difference(b[1], a[1])
        },
        E::difference(c[0], a[0]),
        E::difference(c[1], a[1]),
    ) else {
        return Ok(None);
    };
    let (Some(mut p), Some(q)) = (
        x0.product(&y1, work, cancel)?,
        x1.product(&y0, work, cancel)?,
    ) else {
        return Ok(None);
    };
    if p.subtract(&q).is_none() {
        return Ok(None);
    }
    Ok(Some(p.sign()))
}

fn actual_cycle<'a>(
    input: &'a SectionLoopMm,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ActualCycle<'a>>, GeometryError> {
    let mut result = ActualCycle {
        points: &input.points_mm,
        vertices: Vec::new(),
    };
    scratch.reserve(
        &mut result.vertices,
        input.points_mm.len().saturating_sub(1),
        work,
        cancel,
    )?;
    for (i, p) in input.points_mm[..input.points_mm.len() - 1]
        .iter()
        .enumerate()
    {
        work.charge(1, cancel)?;
        if result.vertices.last().is_none_or(|&last| {
            let previous = input.points_mm[last];
            p[0] != previous[0] || p[1] != previous[1]
        }) {
            result.vertices.push(i);
        }
    }
    let mut changed = true;
    while changed && result.len() >= 3 {
        changed = false;
        for i in 0..result.len() {
            work.charge(1, cancel)?;
            let a = result.point((i + result.len() - 1) % result.len());
            let b = result.point(i);
            let c = result.point(i + 1);
            if (0..2).all(|axis| between(b[axis], a[axis], c[axis]))
                && coefficient_sign(a, b, c, false, work, cancel)? == Some(0)
            {
                work.charge(result.len() as u64, cancel)?;
                result.vertices.remove(i);
                changed = true;
                break;
            }
        }
    }
    Ok((result.len() >= 3).then_some(result))
}

fn actual_cycle_identity(a: &ActualCycle<'_>, b: &ActualCycle<'_>) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let first = |cycle: &ActualCycle<'_>| {
        (0..cycle.len())
            .min_by(|&a, &b| {
                let a = cycle.point(a);
                let b = cycle.point(b);
                a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1]))
            })
            .unwrap()
    };
    let (i, j) = (first(a), first(b));
    (0..a.len()).all(|k| a.point(i + k) == b.point(j + k))
}

const NESTED_OUTER_STACK_BYTES: usize = 12
    * std::mem::size_of::<crate::exact_coefficient::ExactCoefficient>()
    + 2 * std::mem::size_of::<ActualCycle<'_>>()
    + std::mem::size_of::<Sha256>();

// Sign of the EXACT dyadic midpoint minus a coordinate. Never round a new
// reference point off the actual edge to decide closed-set containment.
fn midpoint_offset_sign(
    p: f64,
    q: f64,
    coordinate: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<i8>, GeometryError> {
    work.charge(1, cancel)?;
    let value = Interval::point(p)
        .sub(Interval::point(coordinate))
        .add(Interval::point(q).sub(Interval::point(coordinate)));
    if value.lo > 0.0 {
        return Ok(Some(1));
    }
    if value.hi < 0.0 {
        return Ok(Some(-1));
    }
    use crate::exact_coefficient::ExactCoefficient as E;
    let Some(mut exact) = E::difference(p, coordinate) else {
        return Ok(None);
    };
    if exact.push(q).is_none() || exact.push(-coordinate).is_none() {
        return Ok(None);
    }
    Ok(Some(exact.sign()))
}

fn midpoint_orientation(
    a: [f64; 2],
    b: [f64; 2],
    p: [f64; 2],
    q: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<i8>, GeometryError> {
    let relative = std::array::from_fn(|axis| {
        Interval::point(p[axis])
            .sub(Interval::point(a[axis]))
            .add(Interval::point(q[axis]).sub(Interval::point(a[axis])))
    });
    let enclosed = cross(delta(b, a), relative);
    if enclosed.lo > 0.0 {
        return Ok(Some(1));
    }
    if enclosed.hi < 0.0 {
        return Ok(Some(-1));
    }
    use crate::exact_coefficient::ExactCoefficient as E;
    let (Some(dx), Some(dy), Some(mut rx), Some(mut ry)) = (
        E::difference(b[0], a[0]),
        E::difference(b[1], a[1]),
        E::difference(p[0], a[0]),
        E::difference(p[1], a[1]),
    ) else {
        return Ok(None);
    };
    if rx.push(q[0]).is_none()
        || rx.push(-a[0]).is_none()
        || ry.push(q[1]).is_none()
        || ry.push(-a[1]).is_none()
    {
        return Ok(None);
    }
    let (Some(mut first), Some(second)) = (
        dx.product(&ry, work, cancel)?,
        dy.product(&rx, work, cancel)?,
    ) else {
        return Ok(None);
    };
    if first.subtract(&second).is_none() {
        return Ok(None);
    }
    Ok(Some(first.sign()))
}

/// Called only after the complete crossing inventory has separated the OPEN
/// edge piece from the container boundary. Winding labels that connected
/// complementary component; the midpoint does not manufacture containment.
fn midpoint_inside_cycle(
    cycle: &ActualCycle<'_>,
    p: [f64; 2],
    q: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<bool>, GeometryError> {
    let mut inside = false;
    for edge in 0..cycle.len() {
        work.charge(1, cancel)?;
        let (a, b) = (cycle.point(edge), cycle.point(edge + 1));
        let (Some(sa), Some(sb)) = (
            midpoint_offset_sign(p[1], q[1], a[1], work, cancel)?,
            midpoint_offset_sign(p[1], q[1], b[1], work, cancel)?,
        ) else {
            return Ok(None);
        };
        if (sa < 0) == (sb < 0) {
            continue;
        }
        let Some(side) = midpoint_orientation(a, b, p, q, work, cancel)? else {
            return Ok(None);
        };
        if side == 0 {
            return Ok(None);
        }
        if (side > 0) == (b[1] > a[1]) {
            inside = !inside;
        }
    }
    Ok(Some(inside))
}

fn closed_cycle_contains(
    outer: &ActualCycle<'_>,
    inner: &ActualCycle<'_>,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<bool>, GeometryError> {
    let mut cuts = Vec::new();
    let capacity = outer.len().checked_add(2).ok_or_else(|| {
        error(
            GeometryErrorCode::ResourceLimit,
            "actual nesting edge-cut capacity overflow",
        )
    })?;
    scratch.reserve(&mut cuts, capacity, work, cancel)?;
    for edge in 0..inner.len() {
        cuts.clear();
        let (a, b) = (inner.point(edge), inner.point(edge + 1));
        let axis = usize::from(a[0] == b[0]);
        cuts.push(a);
        cuts.push(b);
        for other in 0..outer.len() {
            work.charge(1, cancel)?;
            let (c, d) = (outer.point(other), outer.point(other + 1));
            if (0..2).any(|i| a[i].max(b[i]) < c[i].min(d[i]) || c[i].max(d[i]) < a[i].min(b[i])) {
                continue;
            }
            let (Some(sc), Some(sd), Some(sa), Some(sb)) = (
                coefficient_sign(a, b, c, false, work, cancel)?,
                coefficient_sign(a, b, d, false, work, cancel)?,
                coefficient_sign(c, d, a, false, work, cancel)?,
                coefficient_sign(c, d, b, false, work, cancel)?,
            ) else {
                return Ok(None);
            };
            if sc * sd < 0 && sa * sb < 0 {
                return Ok(Some(false));
            }
            if sc == 0 && sd == 0 {
                let lo = a[axis].min(b[axis]).max(c[axis].min(d[axis]));
                let hi = a[axis].max(b[axis]).min(c[axis].max(d[axis]));
                if lo < hi && (b[axis] > a[axis]) != (d[axis] > c[axis]) {
                    return Ok(Some(false));
                }
            }
            // Every non-proper intersection is an endpoint contact or exact
            // collinear overlap. Existing container vertices partition all its
            // interior contacts; inner endpoints are already in this inventory.
            if sc == 0 && (0..2).all(|i| between(c[i], a[i], b[i])) {
                cuts.push(c);
            }
        }
        let sort_work = cuts.len().checked_mul(cuts.len()).ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "actual nesting edge-cut work overflow",
            )
        })?;
        work.charge(sort_work as u64, cancel)?;
        cuts.sort_unstable_by(|p, q| p[axis].total_cmp(&q[axis]));
        cuts.dedup();
        for piece in cuts.windows(2) {
            let (p, q) = (piece[0], piece[1]);
            let mut shared = false;
            for other in 0..outer.len() {
                work.charge(1, cancel)?;
                let (c, d) = (outer.point(other), outer.point(other + 1));
                if !(0..2).all(|i| between(p[i], c[i], d[i]) && between(q[i], c[i], d[i])) {
                    continue;
                }
                let (Some(sp), Some(sq)) = (
                    coefficient_sign(c, d, p, false, work, cancel)?,
                    coefficient_sign(c, d, q, false, work, cancel)?,
                ) else {
                    return Ok(None);
                };
                if sp == 0 && sq == 0 {
                    if (b[axis] > a[axis]) != (d[axis] > c[axis]) {
                        return Ok(Some(false));
                    }
                    shared = true;
                    break;
                }
            }
            if !shared {
                match midpoint_inside_cycle(outer, p, q, work, cancel)? {
                    Some(true) => {}
                    Some(false) => return Ok(Some(false)),
                    None => return Ok(None),
                }
            }
        }
    }
    Ok(Some(true))
}

#[derive(Debug)]
pub(super) struct ActualOuterNesting {
    /// Index of the EXISTING complete outer region equal to the intersection.
    pub(super) inner: usize,
    pub(super) geometry: [u8; 32],
}

/// Both inputs must already be registered material-left Jordan OUTER cycles.
/// Exact whole-edge classification, not a zero-distance clearance exception,
/// permits same-sided coincident boundary traces. Closed containment makes the
/// filled intersection one existing Jordan disk. This proves no native source,
/// error transport, protected-hole or physical attachment fact.
pub(super) fn actual_outer_nesting(
    lower: &SectionLoopMm,
    upper: &SectionLoopMm,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ActualOuterNesting>, GeometryError> {
    scratch.owned(NESTED_OUTER_STACK_BYTES, work, cancel)?;
    if lower.is_hole || upper.is_hole {
        return Ok(None);
    }
    let Some(a) = actual_cycle(lower, scratch, work, cancel)? else {
        return Ok(None);
    };
    let Some(b) = actual_cycle(upper, scratch, work, cancel)? else {
        return Ok(None);
    };
    let checkpoint = scratch.bytes;
    let forward = closed_cycle_contains(&a, &b, scratch, work, cancel);
    // Each edge-cut workspace is gone before trying the other direction.
    // Failed/cancelled numerical work is never refunded.
    scratch.bytes = checkpoint;
    let inner = if forward? == Some(true) {
        1
    } else {
        let reverse = closed_cycle_contains(&b, &a, scratch, work, cancel);
        scratch.bytes = checkpoint;
        if reverse? != Some(true) {
            return Ok(None);
        }
        0
    };
    let mut digest = Sha256::new();
    digest.update(b"spiling-actual-nested-outer-pair-1\0");
    digest.update([inner as u8]);
    for cycle in [lower, upper] {
        work.charge(cycle.points_mm.len() as u64, cancel)?;
        digest.update((cycle.points_mm.len() as u64).to_le_bytes());
        for point in &cycle.points_mm {
            for value in &point[..2] {
                digest.update(value.to_bits().to_le_bytes());
            }
        }
    }
    Ok(Some(ActualOuterNesting {
        inner,
        geometry: digest.finalize().into(),
    }))
}

#[derive(Clone, Copy)]
struct CollarCell {
    cycle: usize,
    edge: usize,
    p: [f64; 2],
    next: [f64; 2],
    q: [f64; 2],
    next_q: [f64; 2],
    width: f64,
    // CCW: negative rim, right spoke, positive rim, left spoke.
    quad: [[Interval; 2]; 4],
}
fn relative_quad(cell: &CollarCell, origin: [f64; 2]) -> [[Interval; 2]; 4] {
    let offset = |p: [f64; 2], q: [f64; 2], t: f64| {
        let p = delta(p, origin);
        std::array::from_fn(|i| p[i].add(Interval::point(t).mul(Interval::point(q[i]))))
    };
    [
        offset(cell.p, cell.q, -cell.width),
        offset(cell.next, cell.next_q, -cell.width),
        offset(cell.next, cell.next_q, cell.width),
        offset(cell.p, cell.q, cell.width),
    ]
}
fn collar_halfplane(cell: &CollarCell, side: usize, relative: [Interval; 2]) -> Interval {
    let q = cell.q.map(Interval::point);
    if side == 1 {
        return cross(cell.next_q.map(Interval::point), relative);
    }
    if side == 3 {
        return cross(q, relative).neg();
    }
    let t = Interval::point(if side == 0 { -cell.width } else { cell.width });
    let e = delta(cell.next, cell.p);
    let dq = delta(cell.next_q, cell.q);
    let edge = std::array::from_fn(|i| e[i].add(t.mul(dq[i])));
    let point = std::array::from_fn(|i| relative[i].sub(t.mul(q[i])));
    let side_value = cross(edge, point);
    if side == 0 {
        side_value
    } else {
        side_value.neg()
    }
}
fn collar_cell(
    cycle: usize,
    edge: usize,
    p: [f64; 2],
    next: [f64; 2],
    q: [f64; 2],
    next_q: [f64; 2],
    width: f64,
) -> Result<CollarCell, ActualPairCollarRefusal> {
    let e = delta(next, p);
    let dq = delta(next_q, q);
    // This is the WHOLE FIRST determinant, affine in s,t because the exact
    // repeated column has det(dq,dq)=0. It is not a sampled normal test.
    let determinant = cross(e, q.map(Interval::point))
        .add(Interval { lo: 0.0, hi: 1.0 }.mul(cross(e, dq)))
        .add(
            Interval {
                lo: -width,
                hi: width,
            }
            .mul(cross(dq, q.map(Interval::point))),
        );
    if determinant.lo <= 0.0 {
        return Err(ActualPairCollarRefusal::ChartJacobian {
            cycle,
            edge,
            width,
            determinant,
        });
    }
    let offset = |p: [f64; 2], q: [f64; 2], t: f64| {
        std::array::from_fn(|i| {
            Interval::point(p[i]).add(Interval::point(t).mul(Interval::point(q[i])))
        })
    };
    let quad = [
        offset(p, q, -width),
        offset(next, next_q, -width),
        offset(next, next_q, width),
        offset(p, q, width),
    ];
    // A strictly convex proper boundary and the signed whole first matrix
    // certify a single embedded chart, not arbitrary full-rank injectivity.
    let lower = std::array::from_fn(|i| e[i].sub(Interval::point(width).mul(dq[i])));
    let upper = std::array::from_fn(|i| e[i].add(Interval::point(width).mul(dq[i])).neg());
    let right = next_q.map(|q| {
        Interval::point(2.0)
            .mul(Interval::point(width))
            .mul(Interval::point(q))
    });
    let left = q.map(|q| {
        Interval::point(-2.0)
            .mul(Interval::point(width))
            .mul(Interval::point(q))
    });
    let edges = [lower, right, upper, left];
    for i in 0..4 {
        let turn = cross(edges[i], edges[(i + 1) % 4]);
        if turn.lo <= 0.0 {
            return Err(ActualPairCollarRefusal::ChartConvexity {
                cycle,
                edge,
                width,
                corner: i,
                turn,
            });
        }
    }
    Ok(CollarCell {
        cycle,
        edge,
        p,
        next,
        q,
        next_q,
        width,
        quad,
    })
}

fn opposed_spoke(a: &CollarCell, b: &CollarCell) -> bool {
    if a.next != b.p || a.next_q != b.q || a.width != b.width {
        return false;
    }
    let origin = a.next;
    let q = a.next_q.map(Interval::point);
    // Literal common affine trace, and WHOLE convex halves on opposite
    // sides. Index adjacency alone never permits a pair to be skipped.
    let left = relative_quad(a, origin);
    let right = relative_quad(b, origin);
    [left[0], left[3]].into_iter().all(|p| cross(q, p).lo > 0.0)
        && [right[1], right[2]]
            .into_iter()
            .all(|p| cross(q, p).hi < 0.0)
}
fn collar_cells_separated(
    a: &CollarCell,
    b: &CollarCell,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for axis in 0..2 {
        work.charge(1, cancel)?;
        let bounds = |cell: &CollarCell| {
            [
                cell.quad
                    .iter()
                    .map(|p| p[axis].lo)
                    .fold(f64::INFINITY, f64::min),
                cell.quad
                    .iter()
                    .map(|p| p[axis].hi)
                    .fold(f64::NEG_INFINITY, f64::max),
            ]
        };
        let (a, b) = (bounds(a), bounds(b));
        if a[1] < b[0] || b[1] < a[0] {
            return Ok(true);
        }
    }
    for (left, right) in [(a, b), (b, a)] {
        for i in 0..4 {
            work.charge(4, cancel)?;
            let origin = if i == 1 { left.next } else { left.p };
            if relative_quad(right, origin)
                .into_iter()
                .all(|p| collar_halfplane(left, i, p).hi < 0.0)
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[derive(Clone, Copy)]
struct SpokeCrossing {
    edge: usize,
    parameter: Interval,
    transverse: Interval,
}
fn spoke_crossing(
    cell: &CollarCell,
    upper: &ActualCycle<'_>,
    sides: &mut Vec<i8>,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<SpokeCrossing>, GeometryError> {
    sides.clear();
    scratch.reserve(sides, upper.len(), work, cancel)?;
    for i in 0..upper.len() {
        work.charge(1, cancel)?;
        let Some(side) = coefficient_sign(cell.p, cell.q, upper.point(i), true, work, cancel)?
        else {
            return Ok(None);
        };
        sides.push(side);
    }
    let axis = usize::from(cell.q[1].abs() > cell.q[0].abs());
    let mut crossing = None;
    for i in 0..upper.len() {
        work.charge(1, cancel)?;
        let (a, b) = (upper.point(i), upper.point(i + 1));
        let (sa, sb) = (sides[i], sides[(i + 1) % upper.len()]);
        let (parameter, t, positive) = if sa == 0 {
            // A seam vertex belongs ONLY to its outgoing half-open edge.
            let previous = sides[(i + upper.len() - 1) % upper.len()];
            let t = Interval::point(a[axis])
                .sub(Interval::point(cell.p[axis]))
                .div(Interval::point(cell.q[axis]))?;
            (Interval::point(0.0), t, previous > 0 && sb < 0)
        } else if sb != 0 && sa != sb {
            let denominator = cross(cell.q.map(Interval::point), delta(b, a));
            if denominator.contains_zero() {
                return Ok(None);
            }
            let parameter =
                cross(cell.q.map(Interval::point), delta(cell.p, a)).div(denominator)?;
            // Exact opposite side predicates proved the root is in (0,1).
            let parameter = Interval {
                lo: parameter.lo.max(0.0),
                hi: parameter.hi.min(1.0),
            };
            if parameter.lo > parameter.hi {
                return Ok(None);
            }
            let t = cross(delta(a, cell.p), delta(b, a)).div(denominator)?;
            (parameter, t, sa > 0 && sb < 0)
        } else {
            continue;
        };
        if t.lo > cell.width || t.hi < -cell.width {
            continue;
        }
        if !positive || t.lo <= -cell.width || t.hi >= cell.width || crossing.is_some() {
            return Ok(None);
        }
        crossing = Some(SpokeCrossing {
            edge: i,
            parameter,
            transverse: t,
        });
    }
    Ok(crossing)
}

fn strict_rims(cell: &CollarCell, point: [f64; 2]) -> bool {
    let relative = delta(point, cell.p);
    collar_halfplane(cell, 0, relative).lo > 0.0 && collar_halfplane(cell, 2, relative).lo > 0.0
}
fn actual_vertex_in_cell(
    cell: &CollarCell,
    point: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    Ok(strict_rims(cell, point)
        && coefficient_sign(cell.p, cell.q, point, true, work, cancel)? == Some(-1)
        && coefficient_sign(cell.next, cell.next_q, point, true, work, cancel)? == Some(1))
}

fn chart_covers_upper(
    cell: &CollarCell,
    upper: &ActualCycle<'_>,
    start: SpokeCrossing,
    end: SpokeCrossing,
    end_edge: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    // The exact registered trace is p+t*q on each shared spoke. Its certified
    // OPEN t interval and the proper convex chart give strict clearance from
    // BOTH rims without subtracting independently rounded world coordinates.
    if start.transverse.lo <= -cell.width
        || start.transverse.hi >= cell.width
        || end.transverse.lo <= -cell.width
        || end.transverse.hi >= cell.width
    {
        return Ok(false);
    }
    for edge in start.edge..=end_edge {
        work.charge(1, cancel)?;
        if edge == end_edge && end.parameter.lo == 0.0 && end.parameter.hi == 0.0 {
            break;
        }
        let tangent = delta(upper.point(edge + 1), upper.point(edge));
        // A straight piece inside this convex embedded chart cannot change
        // the sign of det(tangent,q(s)): a zero would make its supporting
        // line the literal s-spoke, so the entire piece would have constant s.
        // For the first/last clipped piece, its registered endpoint supplies
        // s=0/1 exactly. Testing the unused far column instead can reject a
        // transverse corner piece that never reaches that part of the chart.
        // Full interior pieces retain the whole-column sufficient test.
        let transverse = if edge == start.edge {
            cross(tangent, cell.q.map(Interval::point)).lo > 0.0
        } else if edge == end_edge {
            cross(tangent, cell.next_q.map(Interval::point)).lo > 0.0
        } else {
            cross(tangent, cell.q.map(Interval::point)).lo > 0.0
                && cross(tangent, cell.next_q.map(Interval::point)).lo > 0.0
        };
        if !transverse {
            return Ok(false);
        }
        if edge > start.edge && !actual_vertex_in_cell(cell, upper.point(edge), work, cancel)? {
            return Ok(false);
        }
    }
    // The endpoint traces are the literal registered spokes, in their OPEN
    // rim intervals. Convexity then covers every full clipped straight piece;
    // the common endpoints are reused, not rounded/re-cut independently.
    Ok(true)
}

fn proposal_distance(
    a: &ActualCycle<'_>,
    b: &ActualCycle<'_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    let mut maximum = 0.0_f64;
    for i in 0..a.len() {
        let p = a.point(i);
        let mut nearest = f64::INFINITY;
        for j in 0..b.len() {
            work.charge(1, cancel)?;
            let (a, b) = (b.point(j), b.point(j + 1));
            let e = [b[0] - a[0], b[1] - a[1]];
            let square = e[0] * e[0] + e[1] * e[1];
            if square <= 0.0 || !square.is_finite() {
                continue;
            }
            let t = (((p[0] - a[0]) * e[0] + (p[1] - a[1]) * e[1]) / square).clamp(0.0, 1.0);
            nearest = nearest.min((p[0] - (a[0] + t * e[0])).hypot(p[1] - (a[1] + t * e[1])));
        }
        maximum = maximum.max(nearest);
    }
    Ok(maximum)
}

/// A refusal is a bounded result of the existing sufficient theorem, not
/// geometry authority. Query resource/cancellation failures remain outer Err.
#[derive(Debug, Clone, Copy)]
pub(super) enum ActualPairCollarRefusal {
    InvalidCycle {
        side: usize,
        cycle: usize,
        points: usize,
    },
    ProposalEdge {
        cycle: usize,
        vertex: usize,
        lengths: [f64; 2],
    },
    ProposalNormal {
        cycle: usize,
        vertex: usize,
        length: f64,
    },
    ProposalDirection {
        cycle: usize,
        vertex: usize,
        side: usize,
        numerator: Interval,
        length: Interval,
    },
    InvalidWidth {
        proposal: f64,
        cosine: f64,
        shortest: f64,
        width: f64,
    },
    ChartJacobian {
        cycle: usize,
        edge: usize,
        width: f64,
        determinant: Interval,
    },
    ChartConvexity {
        cycle: usize,
        edge: usize,
        width: f64,
        corner: usize,
        turn: Interval,
    },
    ChartPair {
        left: [usize; 2],
        right: [usize; 2],
        width: f64,
        adjacent: bool,
    },
    SpokeCrossing {
        cycle: usize,
        edge: usize,
        target: usize,
        width: f64,
    },
    SeamOrder {
        cycle: usize,
        cut: usize,
        base: Interval,
        end: Interval,
    },
    CyclicOrder {
        cycle: usize,
        cut: usize,
        previous: usize,
        end: usize,
        base: usize,
        target_edges: usize,
    },
    UpperCoverage {
        cycle: usize,
        edge: usize,
        start_edge: usize,
        end_edge: usize,
    },
    IncompleteTurn {
        cycle: usize,
        actual: usize,
        expected: usize,
    },
}

pub(super) type ActualPairCollarProof = Result<[u8; 32], ActualPairCollarRefusal>;

/// A finite ACTUAL pair theorem, not a source-cut homotopy inferred from a
/// Hausdorff scalar. Source/leaf/cut/role/parent matching is sealed by its caller.
///
/// Proper convex charts, opposed literal spokes and ALL global pair exclusions
/// embed disjoint two-sided S1 collars. ONE complete oriented upper graph in
/// each collar leaves the complementary cores unchanged. Material intersection
/// is t>=max(0,g(s)) and union t>=min(0,g(s)); both are collar deformations of the
/// same whole Jordan forest. This proves CONNECTED material AND void overlap,
/// not just one shared point/cell, and excludes the two-patch extra-tunnel case.
pub(super) fn actual_pair_collar(
    lower: &[SectionLoopMm],
    upper: &[SectionLoopMm],
    matching: &[usize],
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ActualPairCollarProof, GeometryError> {
    scratch.owned(PAIR_COLLAR_STACK_BYTES, work, cancel)?;
    let mut a = Vec::new();
    let mut b = Vec::new();
    scratch.reserve(&mut a, lower.len(), work, cancel)?;
    scratch.reserve(&mut b, upper.len(), work, cancel)?;
    for (cycle, loop_) in lower.iter().enumerate() {
        let Some(view) = actual_cycle(loop_, scratch, work, cancel)? else {
            return Ok(Err(ActualPairCollarRefusal::InvalidCycle {
                side: 0,
                cycle,
                points: loop_.points_mm.len(),
            }));
        };
        a.push(view);
    }
    for (cycle, loop_) in upper.iter().enumerate() {
        let Some(view) = actual_cycle(loop_, scratch, work, cancel)? else {
            return Ok(Err(ActualPairCollarRefusal::InvalidCycle {
                side: 1,
                cycle,
                points: loop_.points_mm.len(),
            }));
        };
        b.push(view);
    }
    let mut digest = Sha256::new();
    digest.update(b"spiling-actual-registered-pair-collar-1\0");
    let mut exact = true;
    for (i, &j) in matching.iter().enumerate() {
        work.charge((2 * a[i].len() + b[j].len()) as u64, cancel)?;
        exact &= actual_cycle_identity(&a[i], &b[j]);
    }
    if exact {
        // Whole oriented exact trace identity after exact between-vertex
        // elimination, not small displacement or interval-zero promotion.
        digest.update(b"exact-oriented-traces");
        return Ok(Ok(digest.finalize().into()));
    }
    let mut columns = Vec::new();
    scratch.reserve(&mut columns, a.len(), work, cancel)?;
    let mut proposal = 0.0_f64;
    let mut cosine = 1.0_f64;
    let mut shortest = f64::INFINITY;
    for (i, &j) in matching.iter().enumerate() {
        proposal = proposal
            .max(proposal_distance(&a[i], &b[j], work, cancel)?)
            .max(proposal_distance(&b[j], &a[i], work, cancel)?);
        let mut q = Vec::new();
        scratch.reserve(&mut q, a[i].len(), work, cancel)?;
        for vertex in 0..a[i].len() {
            work.charge(1, cancel)?;
            let p = a[i].point(vertex);
            let previous = a[i].point((vertex + a[i].len() - 1) % a[i].len());
            let next = a[i].point(vertex + 1);
            let u = [p[0] - previous[0], p[1] - previous[1]];
            let v = [next[0] - p[0], next[1] - p[1]];
            let (lu, lv) = (u[0].hypot(u[1]), v[0].hypot(v[1]));
            if lu <= 0.0 || lv <= 0.0 || !lu.is_finite() || !lv.is_finite() {
                return Ok(Err(ActualPairCollarRefusal::ProposalEdge {
                    cycle: i,
                    vertex,
                    lengths: [lu, lv],
                }));
            }
            shortest = shortest.min(lu).min(lv);
            let n = [-u[1] / lu - v[1] / lv, u[0] / lu + v[0] / lv];
            let length = n[0].hypot(n[1]);
            if length <= 0.0 || !length.is_finite() {
                return Ok(Err(ActualPairCollarRefusal::ProposalNormal {
                    cycle: i,
                    vertex,
                    length,
                }));
            }
            let direction = [n[0] / length, n[1] / length];
            for (side, edge) in [delta(p, previous), delta(next, p)].into_iter().enumerate() {
                let numerator = cross(edge, direction.map(Interval::point));
                let length = edge[0].square().add(edge[1].square()).sqrt()?;
                if numerator.lo <= 0.0 || length.lo <= 0.0 {
                    return Ok(Err(ActualPairCollarRefusal::ProposalDirection {
                        cycle: i,
                        vertex,
                        side,
                        numerator,
                        length,
                    }));
                }
                cosine = cosine.min(numerator.div(length)?.lo);
            }
            q.push(direction);
        }
        columns.push(q);
    }
    // Proposals select a chart width ONLY. Every closed chart, rim, trace,
    // coverage and global embedding predicate below must still pass.
    let width = if proposal > 0.0 {
        up(8.0 * proposal / cosine)
    } else {
        shortest * 0.125
    };
    if !width.is_finite() || width <= 0.0 {
        return Ok(Err(ActualPairCollarRefusal::InvalidWidth {
            proposal,
            cosine,
            shortest,
            width,
        }));
    }
    let count = a
        .iter()
        .try_fold(0usize, |n, cycle| n.checked_add(cycle.len()))
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "actual collar chart count overflow",
            )
        })?;
    let mut cells = Vec::new();
    scratch.reserve(&mut cells, count, work, cancel)?;
    for (cycle, view) in a.iter().enumerate() {
        for edge in 0..view.len() {
            work.charge(16, cancel)?;
            let cell = match collar_cell(
                cycle,
                edge,
                view.point(edge),
                view.point(edge + 1),
                columns[cycle][edge],
                columns[cycle][(edge + 1) % view.len()],
                width,
            ) {
                Ok(cell) => cell,
                Err(reason) => return Ok(Err(reason)),
            };
            cells.push(cell);
        }
    }
    drop(columns);
    for (i, left) in cells.iter().enumerate() {
        for right in &cells[i + 1..] {
            work.charge(1, cancel)?;
            let accepted = if left.cycle == right.cycle
                && (left.edge + 1) % a[left.cycle].len() == right.edge
            {
                work.charge(8, cancel)?;
                opposed_spoke(left, right)
            } else if left.cycle == right.cycle
                && (right.edge + 1) % a[left.cycle].len() == left.edge
            {
                work.charge(8, cancel)?;
                opposed_spoke(right, left)
            } else {
                collar_cells_separated(left, right, work, cancel)?
            };
            if !accepted {
                let adjacent = left.cycle == right.cycle
                    && ((left.edge + 1) % a[left.cycle].len() == right.edge
                        || (right.edge + 1) % a[left.cycle].len() == left.edge);
                return Ok(Err(ActualPairCollarRefusal::ChartPair {
                    left: [left.cycle, left.edge],
                    right: [right.cycle, right.edge],
                    width,
                    adjacent,
                }));
            }
        }
    }
    // The midlines are the already registered COMPLETE Jordan parent forest.
    // The proper, globally disjoint two-sided annuli above are an ambient
    // collar of that entire forest: both rims retain its nesting, and removing
    // the material/void-side boundary collars leaves each complementary cell
    // with the SAME connected nonempty core. This global collar isotopy, not a
    // point in one chart, supplies the common-core premise for the graph proof.
    let mut offset = 0;
    for (cycle, &target) in matching.iter().enumerate() {
        let charts = &cells[offset..offset + a[cycle].len()];
        offset += a[cycle].len();
        let mut crossings = Vec::new();
        let mut sides = Vec::new();
        scratch.reserve(&mut crossings, charts.len(), work, cancel)?;
        for cell in charts {
            let Some(crossing) =
                spoke_crossing(cell, &b[target], &mut sides, scratch, work, cancel)?
            else {
                return Ok(Err(ActualPairCollarRefusal::SpokeCrossing {
                    cycle,
                    edge: cell.edge,
                    target,
                    width,
                }));
            };
            crossings.push(crossing);
        }
        let base = crossings[0];
        let n = b[target].len();
        let mut previous = base;
        for i in 0..charts.len() {
            work.charge(1, cancel)?;
            let mut end = if i + 1 == charts.len() {
                base
            } else {
                crossings[i + 1]
            };
            let end_edge = if i + 1 == charts.len() {
                base.edge + n
            } else {
                let mut edge = end.edge;
                if edge < base.edge {
                    edge += n;
                } else if edge == base.edge {
                    if end.parameter.hi < base.parameter.lo {
                        edge += n;
                    } else if end.parameter.lo <= base.parameter.hi {
                        return Ok(Err(ActualPairCollarRefusal::SeamOrder {
                            cycle,
                            cut: i,
                            base: base.parameter,
                            end: end.parameter,
                        }));
                    }
                }
                edge
            };
            if end_edge < previous.edge
                || (end_edge == previous.edge && end.parameter.lo <= previous.parameter.hi)
            {
                return Ok(Err(ActualPairCollarRefusal::CyclicOrder {
                    cycle,
                    cut: i,
                    previous: previous.edge,
                    end: end_edge,
                    base: base.edge,
                    target_edges: n,
                }));
            }
            if end_edge > base.edge + n {
                return Ok(Err(ActualPairCollarRefusal::CyclicOrder {
                    cycle,
                    cut: i,
                    previous: previous.edge,
                    end: end_edge,
                    base: base.edge,
                    target_edges: n,
                }));
            }
            if !chart_covers_upper(
                &charts[i], &b[target], previous, end, end_edge, work, cancel,
            )? {
                return Ok(Err(ActualPairCollarRefusal::UpperCoverage {
                    cycle,
                    edge: charts[i].edge,
                    start_edge: previous.edge,
                    end_edge,
                }));
            }
            end.edge = end_edge;
            previous = end;
        }
        // Complete coherent cyclic lift: exactly one turn, with the original
        // first cut object reused at the closing seam. No segment is omitted.
        if previous.edge != base.edge + n {
            return Ok(Err(ActualPairCollarRefusal::IncompleteTurn {
                cycle,
                actual: previous.edge,
                expected: base.edge + n,
            }));
        }
    }
    for cell in &cells {
        work.charge(1, cancel)?;
        digest.update((cell.cycle as u64).to_le_bytes());
        digest.update((cell.edge as u64).to_le_bytes());
        for value in cell.p.into_iter().chain(cell.q).chain([cell.width]) {
            digest.update(value.to_bits().to_le_bytes());
        }
    }
    Ok(Ok(digest.finalize().into()))
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

    fn footprint(points: &[[f64; 2]], hole: bool) -> SectionLoopMm {
        let mut points_mm: Vec<_> = points.iter().map(|p| [p[0], p[1], 0.0]).collect();
        points_mm.push(points_mm[0]);
        SectionLoopMm {
            is_hole: hole,
            points_mm,
        }
    }

    #[test]
    fn actual_outer_nesting_accepts_wide_concave_shared_boundary_regions() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let floor = footprint(
            &[
                [0.0, 0.0],
                [30.0, 0.0],
                [30.0, 22.0],
                [20.0, 22.0],
                [20.0, 20.0],
                [15.0, 20.0],
                [15.0, 22.0],
                [0.0, 22.0],
            ],
            false,
        );
        let shell = footprint(
            &[
                [0.0, 0.0],
                [30.0, 0.0],
                [30.0, 22.0],
                [20.0, 22.0],
                [20.0, 20.2],
                [28.2, 20.2],
                [28.2, 1.8],
                [1.8, 1.8],
                [1.8, 20.2],
                [15.0, 20.2],
                [15.0, 22.0],
                [0.0, 22.0],
            ],
            false,
        );
        let proof = actual_outer_nesting(
            &floor,
            &shell,
            &mut Scratch { bytes: 0 },
            &mut work,
            &cancel,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            proof.inner, 1,
            "intersection is the existing shell, not a new contour"
        );
        let reverse = actual_outer_nesting(
            &shell,
            &floor,
            &mut Scratch { bytes: 0 },
            &mut work,
            &cancel,
        )
        .unwrap()
        .unwrap();
        assert_eq!(reverse.inner, 0);
        let equal = actual_outer_nesting(
            &floor,
            &floor,
            &mut Scratch { bytes: 0 },
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            equal.is_some(),
            "exact same-sided shared boundaries permit equality"
        );
        assert_eq!(
            actual_outer_nesting(
                &floor,
                &shell,
                &mut Scratch { bytes: MAX_SCRATCH },
                &mut work,
                &cancel
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(
            actual_outer_nesting(
                &floor,
                &shell,
                &mut Scratch { bytes: 0 },
                &mut work,
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
    }

    #[test]
    fn actual_outer_nesting_classifies_nonproper_contacts_and_near_coincidence() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let l = footprint(
            &[
                [0.0, 0.0],
                [4.0, 0.0],
                [4.0, 1.0],
                [1.0, 1.0],
                [1.0, 4.0],
                [0.0, 4.0],
            ],
            false,
        );
        let tangent = footprint(&[[0.5, 0.5], [1.5, 0.5], [0.5, 1.5]], false);
        assert!(
            actual_outer_nesting(&l, &tangent, &mut Scratch { bytes: 0 }, &mut work, &cancel)
                .unwrap()
                .is_some(),
            "an existing outer vertex in an inner-edge interior is an exact cut"
        );
        let notch = footprint(
            &[
                [0.0, 0.0],
                [4.0, 0.0],
                [4.0, 4.0],
                [3.0, 4.0],
                [3.0, 2.0],
                [2.0, 1.0],
                [1.0, 2.0],
                [1.0, 4.0],
                [0.0, 4.0],
            ],
            false,
        );
        let across_notch = footprint(&[[0.5, 2.0], [2.0, 0.5], [3.5, 2.0]], false);
        assert!(
            actual_outer_nesting(
                &notch,
                &across_notch,
                &mut Scratch { bytes: 0 },
                &mut work,
                &cancel
            )
            .unwrap()
            .is_none(),
            "all inner vertices can be inside while an edge exits through outer vertices"
        );
        let square = footprint(&[[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]], false);
        let epsilon = 8.0 * f64::EPSILON;
        let shifted = footprint(
            &[
                [epsilon, 0.0],
                [4.0 + epsilon, 0.0],
                [4.0 + epsilon, 4.0],
                [epsilon, 4.0],
            ],
            false,
        );
        assert!(
            actual_outer_nesting(
                &square,
                &shifted,
                &mut Scratch { bytes: 0 },
                &mut work,
                &cancel
            )
            .unwrap()
            .is_none(),
            "near-coincident crossing regions are not promoted to exact containment"
        );
        let touching = footprint(&[[4.0, 0.0], [8.0, 0.0], [8.0, 4.0], [4.0, 4.0]], false);
        assert!(
            actual_outer_nesting(
                &square,
                &touching,
                &mut Scratch { bytes: 0 },
                &mut work,
                &cancel
            )
            .unwrap()
            .is_none(),
            "opposed shared traces do not prove filled outer overlap"
        );
        let mut hole = tangent;
        hole.is_hole = true;
        assert!(
            actual_outer_nesting(&l, &hole, &mut Scratch { bytes: 0 }, &mut work, &cancel)
                .unwrap()
                .is_none(),
            "outer nesting cannot discharge an arbitrary hole/forest pair"
        );
    }

    #[test]
    fn actual_pair_collar_proves_whole_material_void_core_and_rejects_two_patches() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let concave = [
            [0.0, 0.0],
            [8.0, 0.0],
            [8.0, 8.0],
            [6.0, 8.0],
            [6.0, 2.0],
            [2.0, 2.0],
            [2.0, 8.0],
            [0.0, 8.0],
        ];
        let lower = [
            footprint(&concave, false),
            footprint(&[[0.5, 3.0], [0.5, 3.5], [1.0, 3.5], [1.0, 3.0]], true),
        ];
        let mut upper = lower.each_ref().map(|cycle| SectionLoopMm {
            is_hole: cycle.is_hole,
            points_mm: cycle.points_mm.clone(),
        });
        for cycle in &mut upper {
            for point in &mut cycle.points_mm {
                point[0] += 0.001;
            }
        }
        assert!(
            actual_pair_collar(
                &lower,
                &upper,
                &[0, 1],
                &mut Scratch { bytes: 0 },
                &mut work,
                &cancel
            )
            .unwrap()
            .is_ok(),
            "whole two-sided collars preserve the connected concave material core AND its void core"
        );
        let strip = [footprint(
            &[[0.0, 0.0], [8.0, 0.0], [8.0, 0.01], [0.0, 0.01]],
            false,
        )];
        let shifted = [footprint(
            &[[0.0, 0.02], [8.0, 0.02], [8.0, 0.03], [0.0, 0.03]],
            false,
        )];
        assert!(
            actual_pair_collar(
                &strip,
                &shifted,
                &[0],
                &mut Scratch { bytes: 0 },
                &mut work,
                &cancel
            )
            .unwrap()
            .is_err(),
            "a regular translating thin strip does not certify disjoint ACTUAL prisms"
        );
        let inverted = [footprint(
            &[
                [0.0, 0.0],
                [2.0, 0.0],
                [2.0, 6.0],
                [6.0, 6.0],
                [6.0, 0.0],
                [8.0, 0.0],
                [8.0, 8.0],
                [0.0, 8.0],
            ],
            false,
        )];
        assert!(
            actual_pair_collar(
                &lower[..1],
                &inverted,
                &[0],
                &mut Scratch { bytes: 0 },
                &mut work,
                &cancel
            )
            .unwrap()
            .is_err(),
            "two Jordan disks sharing two disconnected patches must not create a printed tunnel"
        );
        let triangle = [footprint(&[[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]], false)];
        let subdivided = [footprint(
            &[[0.0, 0.0], [2.0, 0.0], [4.0, 0.0], [2.0, 2.0], [0.0, 4.0]],
            false,
        )];
        assert!(
            actual_pair_collar(
                &triangle,
                &subdivided,
                &[0],
                &mut Scratch { bytes: 0 },
                &mut work,
                &cancel
            )
            .unwrap()
            .is_ok(),
            "exact full trace identity is geometric, including nonaxis between-vertices"
        );
        assert_eq!(
            actual_pair_collar(
                &lower,
                &upper,
                &[0, 1],
                &mut Scratch { bytes: MAX_SCRATCH },
                &mut work,
                &cancel
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(
            actual_pair_collar(
                &lower,
                &upper,
                &[0, 1],
                &mut Scratch { bytes: 0 },
                &mut work,
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
    }

    #[test]
    fn registered_jordan_parent_census_uses_whole_boundaries_not_a_star_kernel() {
        // Geometry-only controls: native material/pose/sample authority is
        // supplied separately by the owning source-leaf constructor.
        let source = [
            polygon(
                &[
                    [0.0, 0.0],
                    [8.0, 0.0],
                    [8.0, 8.0],
                    [6.0, 8.0],
                    [6.0, 2.0],
                    [2.0, 2.0],
                    [2.0, 8.0],
                    [0.0, 8.0],
                ],
                false,
            ),
            square(0.5, 3.0, 0.5, true),
            square(12.0, 0.0, 8.0, false),
        ];
        let mut actual = [1, 2, 0].map(|i| polygon(&source[i].points, source[i].hole));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let mut scratch = Scratch { bytes: 0 };
        let mapping = exact_cycle_mapping(&source, &actual, &mut scratch, &mut work, &cancel)
            .unwrap()
            .unwrap();
        let mut registration: Vec<_> = mapping
            .into_iter()
            .map(|native_index| NativeLoopRegistration {
                native_index,
                parent: None,
            })
            .collect();
        let lower = capture_loop_registration(
            &source,
            &actual,
            &mut registration,
            0.0,
            true,
            &mut scratch,
            &mut work,
            &cancel,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            registration
                .iter()
                .map(|cycle| cycle.parent)
                .collect::<Vec<_>>(),
            [Some(2), None, None]
        );
        assert!(lower > Interval::point(0.1).mul(Interval::point(2.0)).hi);
        assert!(
            lower <= Interval::point(0.25).mul(Interval::point(2.0)).hi,
            "whole-boundary clearance is a conservative LOWER with a strict two-tube comparison"
        );
        for point in &mut actual[0].points {
            point[0] += 12.0;
        }
        assert!(
            capture_loop_registration(
                &source,
                &actual,
                &mut registration,
                0.0,
                false,
                &mut scratch,
                &mut work,
                &cancel
            )
            .is_err(),
            "the same loop/role bijection cannot silently change the original owning component"
        );
        let mut cancelled_work = GeometryWorkBudget::new(100_000);
        assert_eq!(
            capture_loop_registration(
                &source,
                &source,
                &mut registration,
                0.0,
                false,
                &mut Scratch { bytes: 0 },
                &mut cancelled_work,
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
        let mut bounded_work = GeometryWorkBudget::new(1);
        assert_eq!(
            capture_loop_registration(
                &source,
                &source,
                &mut registration,
                0.0,
                false,
                &mut Scratch { bytes: 0 },
                &mut bounded_work,
                &cancel
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit
        );
        assert!(bounded_work.consumed() > 0);
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
