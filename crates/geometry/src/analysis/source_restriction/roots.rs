// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::exact_coefficient::ExactCoefficient;
use crate::kernel::source_operand::SOURCE_OPERAND_SCRATCH_BYTES;
use crate::parametric::{AxisSpans, CurveEvaluator, Interval, curve_endpoint_coordinate_equal};
use crate::{GeometryError, GeometryWorkBudget};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

const MAX_DEPTH: usize = 256;
const MAX_RETAINED_ROOTS: usize = 1024;

#[derive(Clone, Copy)]
struct Pending {
    range: Interval,
    depth: usize,
}

/// A sealed root recipe, not a rounded parameter sample. `tube` carries the
/// strictly increasing g proof; `parameter` encloses its established root.
#[derive(Clone, Copy)]
pub(super) struct EndpointRoot {
    parameter: Interval,
    tube: Interval,
    exact: Option<f64>,
    error_mm: f64,
}
impl EndpointRoot {
    pub(super) fn enclosure(&self) -> Interval {
        self.parameter
    }
    pub(super) fn exact_parameter(&self) -> Option<f64> {
        self.exact
    }
    pub(super) fn source_error_mm(&self) -> f64 {
        self.error_mm
    }
}

fn unavailable(message: &'static str) -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
        message,
    )
}
fn resource() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "original endpoint root inventory exceeds its admitted live byte/depth allowance",
    )
}
fn finite(value: Interval) -> Result<Interval, GeometryError> {
    Interval::new(value.lo, value.hi)
}
fn displacement(position: [Interval; 3], vertex: [f64; 3]) -> [Interval; 3] {
    std::array::from_fn(|axis| position[axis].sub(Interval::point(vertex[axis])))
}
fn dot(a: [Interval; 3], b: [Interval; 3]) -> Interval {
    (0..3).fold(Interval::point(0.0), |sum, axis| {
        sum.add(a[axis].mul(b[axis]))
    })
}
fn distance(delta: [Interval; 3]) -> Result<Interval, GeometryError> {
    finite(
        delta
            .into_iter()
            .fold(Interval::point(0.0), |sum, value| sum.add(value.square()))
            .sqrt()?,
    )
}

fn exact_vertex(
    curve: &dyn CurveEvaluator,
    parameter: f64,
    vertex: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    // This provider checks actual coefficient/clamped-control-point identities.
    // Overlapping endpoint position intervals cannot enter this proof.
    for axis in 0..3 {
        work.charge(1, cancel)?;
        if !curve_endpoint_coordinate_equal(curve, parameter, axis, vertex[axis], work, cancel)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn retain(
    roots: &mut Vec<EndpointRoot>,
    mut root: EndpointRoot,
    maximum_retained_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let mut index = 0;
    while index < roots.len() {
        work.charge(1, cancel)?;
        let previous = roots[index];
        if previous.tube.intersection(root.tube).is_none() {
            index += 1;
            continue;
        }
        // Both tubes have g' > 0. Their connected union therefore has at most
        // one root. This is equivalence by proof, not by bracket proximity.
        let Some(parameter) = previous.parameter.intersection(root.parameter) else {
            return Err(unavailable(
                "overlapping monotonic endpoint tubes have inconsistent root enclosures",
            ));
        };
        if let (Some(a), Some(b)) = (previous.exact, root.exact) {
            if a != b {
                return Err(unavailable(
                    "endpoint coefficient identities contradict monotonic root authority",
                ));
            }
        }
        root = EndpointRoot {
            parameter,
            tube: previous.tube.hull(root.tube),
            exact: previous.exact.or(root.exact),
            error_mm: previous.error_mm.min(root.error_mm),
        };
        roots.swap_remove(index);
        // A larger union can now meet an earlier tube: close the equivalence
        // transitively rather than leaving duplicate roots in the inventory.
        index = 0;
    }
    if roots.len() >= MAX_RETAINED_ROOTS {
        return Err(resource());
    }
    let live = roots
        .capacity()
        .checked_mul(std::mem::size_of::<EndpointRoot>())
        .ok_or_else(resource)?;
    super::admit_vec(roots, 1, live, maximum_retained_bytes)?;
    roots.push(root);
    Ok(())
}

pub(super) fn isolate_endpoint(
    curve: &dyn CurveEvaluator,
    vertex: [f64; 3],
    search: [f64; 2],
    maximum_source_error_mm: f64,
    maximum_live_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Vec<EndpointRoot>, GeometryError> {
    work.charge(1, cancel)?;
    let search = Interval::new(search[0], search[1])?;
    for value in vertex {
        Interval::new(value, value)?;
    }
    if !maximum_source_error_mm.is_finite() || maximum_source_error_mm < 0.0 {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "original endpoint solving requires a finite nonnegative source allowance",
        ));
    }
    let active = curve.active_range();
    Interval::new(active[0], active[1])?;
    if search.lo < active[0] || search.hi > active[1] {
        return Err(unavailable(
            "original endpoint search exceeds the selected source parameter branch",
        ));
    }

    // Admit stack, retained capacity and the provider's bounded recursive
    // evaluator/exact-coefficient scratch BEFORE any evaluator call/allocation.
    const FIXED_BYTES: usize = SOURCE_OPERAND_SCRATCH_BYTES
        + std::mem::size_of::<[Pending; MAX_DEPTH + 1]>()
        + 256 * std::mem::size_of::<Interval>()
        + 8 * std::mem::size_of::<ExactCoefficient>()
        + std::mem::size_of::<AxisSpans<'static>>()
        + std::mem::size_of::<Vec<EndpointRoot>>();
    let retained_bytes = maximum_live_bytes
        .checked_sub(FIXED_BYTES)
        .ok_or_else(resource)?;
    let capacity = (retained_bytes / std::mem::size_of::<EndpointRoot>()).min(MAX_RETAINED_ROOTS);
    if capacity == 0 {
        return Err(resource());
    }

    // Inventory the complete actual evaluator cover, including shared knots.
    // C1 authority is required at closed fences too: a nonsmooth shared knot
    // must not be mistaken for a root of a differentiable distance function.
    let mut covered = search.lo;
    let mut span_count = 0usize;
    for span in curve.parameter_spans([search.lo, search.hi], work, cancel)? {
        work.charge(1, cancel)?;
        let span = Interval::new(span[0], span[1])?;
        if span.lo < search.lo || span.hi > search.hi || span.lo > covered {
            return Err(unavailable(
                "original endpoint parameter span inventory does not cover its search",
            ));
        }
        if !curve.continuous([span.lo, span.hi], work, cancel)?
            || !curve.smooth([span.lo, span.hi], work, cancel)?
        {
            return Err(unavailable(
                "original endpoint search meets a nonsmooth source knot without endpoint root authority",
            ));
        }
        covered = covered.max(span.hi);
        span_count = span_count.checked_add(1).ok_or_else(resource)?;
    }
    if span_count == 0
        || covered != search.hi
        || !curve.smooth([search.lo, search.hi], work, cancel)?
    {
        return Err(unavailable(
            "original endpoint search lacks a complete smooth source span inventory",
        ));
    }

    let mut roots: Vec<EndpointRoot> = Vec::new();
    // Allocate only roots actually established, not the maximum admissible
    // census for every elementary source endpoint.
    let empty = Pending {
        range: search,
        depth: 0,
    };
    let mut stack = [empty; MAX_DEPTH + 1];
    let mut live = 1usize;
    while live != 0 {
        work.charge(1, cancel)?;
        live -= 1;
        let pending = stack[live];
        let range = pending.range;
        let mut owned = false;
        for root in &roots {
            work.charge(1, cancel)?;
            if root.tube.contains_interval(range) {
                owned = true;
                break;
            }
        }
        if owned {
            continue;
        }

        let jet = curve.tensor([range.lo, range.hi], 2, work, cancel)?;
        let delta = displacement(jet[0], vertex);
        if distance(delta)?.lo > maximum_source_error_mm {
            continue;
        }
        let g = finite(dot(delta, jet[1]))?;
        if !g.contains_zero() {
            continue;
        }
        let derivative = finite(dot(jet[1], jet[1]).add(dot(delta, jet[2])))?;
        if derivative.lo > 0.0 {
            let midpoint = range.mid();
            let point = curve.tensor([midpoint; 2], 1, work, cancel)?;
            let point_delta = displacement(point[0], vertex);
            let point_g = finite(dot(point_delta, point[1]))?;
            let mut exact = None;
            // Include the closed search boundaries. Natural spline endpoints
            // need a genuine actual clamped endpoint identity from the provider.
            for parameter in [range.lo, range.hi, midpoint] {
                work.charge(1, cancel)?;
                if exact_vertex(curve, parameter, vertex, work, cancel)? {
                    exact = Some(parameter);
                    break;
                }
            }
            if let Some(parameter) = exact {
                retain(
                    &mut roots,
                    EndpointRoot {
                        parameter: Interval::point(parameter),
                        tube: range,
                        exact: Some(parameter),
                        error_mm: 0.0,
                    },
                    retained_bytes,
                    work,
                    cancel,
                )?;
                continue;
            }

            let newton = finite(Interval::point(midpoint).sub(point_g.div(derivative)?))?;
            if range.lo < newton.lo && newton.hi < range.hi {
                // Strict interval Newton inclusion establishes existence as well
                // as uniqueness. Keep the entire root recipe, never its midpoint.
                let mut parameter = newton;
                // Root width is epistemic, not geometric endpoint residual.
                // Contract it independently so two representations of the same
                // source curve do not spend the entire physical budget merely
                // by subtracting their independently enclosed root recipes.
                for _ in 0..16 {
                    work.charge(1, cancel)?;
                    if parameter.is_point() {
                        break;
                    }
                    let local = curve.tensor([parameter.lo, parameter.hi], 2, work, cancel)?;
                    if local[0]
                        .iter()
                        .all(|coordinate| coordinate.width() <= maximum_source_error_mm / 64.0)
                    {
                        break;
                    }
                    let local_delta = displacement(local[0], vertex);
                    let local_derivative =
                        finite(dot(local[1], local[1]).add(dot(local_delta, local[2])))?;
                    // The saved positive whole-tube derivative remains valid
                    // on every contraction even when a new enclosure is wider.
                    let slope = local_derivative.intersection(derivative).ok_or_else(|| {
                        unavailable("endpoint derivative contractions contradict their proved tube")
                    })?;
                    let center = parameter.mid();
                    let center_jet = curve.tensor([center; 2], 1, work, cancel)?;
                    let center_g = finite(dot(displacement(center_jet[0], vertex), center_jet[1]))?;
                    let image = finite(Interval::point(center).sub(center_g.div(slope)?))?;
                    let next = parameter.intersection(image).ok_or_else(|| {
                        unavailable("endpoint Newton contraction lost its established root")
                    })?;
                    if next.lo == parameter.lo && next.hi == parameter.hi {
                        break;
                    }
                    parameter = next;
                }
                let position = curve.tensor([parameter.lo, parameter.hi], 0, work, cancel)?[0];
                let error_mm = distance(displacement(position, vertex))?.hi;
                if error_mm <= maximum_source_error_mm {
                    retain(
                        &mut roots,
                        EndpointRoot {
                            parameter,
                            tube: range,
                            exact: parameter.is_point().then_some(parameter.lo),
                            error_mm,
                        },
                        retained_bytes,
                        work,
                        cancel,
                    )?;
                    continue;
                }
            }
        }

        // Overlap children substantially, so a root on a subdivision fence is
        // interior to another child. No one-sided boundary existence is inferred.
        // At an outer search fence, an unproved root ends in an owning error.
        if pending.depth == MAX_DEPTH || live + 2 > stack.len() {
            return Err(resource());
        }
        let midpoint = range.mid();
        let left_end = midpoint * 0.75 + range.hi * 0.25;
        let right_start = range.lo * 0.25 + midpoint * 0.75;
        if !(range.lo < right_start && right_start < left_end && left_end < range.hi) {
            return Err(unavailable(
                "original endpoint root at a search fence or machine-width box lacks an exact identity or strict Newton existence proof",
            ));
        }
        stack[live] = Pending {
            range: Interval::new(right_start, range.hi)?,
            depth: pending.depth + 1,
        };
        stack[live + 1] = Pending {
            range: Interval::new(range.lo, left_end)?,
            depth: pending.depth + 1,
        };
        live += 2;
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::source_operand::PreparedOriginalCurve;
    use solidcraft_step::geom::{BSplineCurve, Curve, CurveForm, KnotSpec, Logical, Placement};
    use std::f64::consts::TAU;

    const BYTES: usize = 16 * 1024 * 1024;

    fn circle() -> Curve {
        Curve::Circle {
            position: Placement {
                origin: [0.0; 3],
                axis: [0.0, 0.0, 1.0],
                ref_direction: [1.0, 0.0, 0.0],
            },
            radius: 2.0,
        }
    }

    #[test]
    fn closed_circle_nearest_root_has_existence_and_a_unique_positive_width_tube() {
        let source = circle();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000_000);
        let range = [-0.4, TAU - 0.4];
        let curve = PreparedOriginalCurve::new(&source, range, BYTES, &mut work, &cancel).unwrap();
        // The nearest point to (2,1) has phase atan(1/2), not the search midpoint.
        // Its radial gap sqrt(5)-2 is strictly below the admitted 0.3 mm.
        let roots = isolate_endpoint(
            &curve,
            [2.0, 1.0, 0.0],
            range,
            0.3,
            BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(roots.len(), 1);
        let root = roots[0];
        let phase = 0.5_f64.atan();
        assert!(root.enclosure().contains(phase));
        assert!(root.enclosure().lo < root.enclosure().hi);
        assert!(root.tube.lo < root.enclosure().lo && root.enclosure().hi < root.tube.hi);
        assert_eq!(root.exact_parameter(), None);
        assert!(root.source_error_mm() >= 5.0_f64.sqrt() - 2.0);
        assert!(root.source_error_mm() <= 0.3);

        // Re-evaluate the actual provider on the retained tube: g' is positive
        // throughout, rather than merely at a rounded candidate parameter.
        let jet = curve
            .tensor([root.tube.lo, root.tube.hi], 2, &mut work, &cancel)
            .unwrap();
        let delta = displacement(jet[0], [2.0, 1.0, 0.0]);
        assert!(dot(jet[1], jet[1]).add(dot(delta, jet[2])).lo > 0.0);
        assert!(
            isolate_endpoint(
                &curve,
                [2.0, 1.0, 0.0],
                [0.7, 1.0],
                0.3,
                BYTES,
                &mut work,
                &cancel
            )
            .unwrap()
            .is_empty()
        );
        // Even the nearest stationary point is outside this smaller allowance.
        assert!(
            isolate_endpoint(
                &curve,
                [2.0, 1.0, 0.0],
                range,
                0.2,
                BYTES,
                &mut work,
                &cancel
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn distinct_original_parameter_roots_are_not_conflated_by_spatial_coincidence() {
        let source = circle();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000_000);
        let range = [-0.4, 2.0 * TAU - 0.4];
        let curve = PreparedOriginalCurve::new(&source, range, BYTES, &mut work, &cancel).unwrap();
        let mut roots = isolate_endpoint(
            &curve,
            [2.0, 1.0, 0.0],
            range,
            0.3,
            BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        roots.sort_by(|a, b| a.enclosure().lo.total_cmp(&b.enclosure().lo));
        assert_eq!(roots.len(), 2);
        let phase = 0.5_f64.atan();
        assert!(roots[0].enclosure().contains(phase));
        assert!(roots[1].enclosure().contains(phase + TAU));
        assert!(
            roots[0]
                .enclosure()
                .intersection(roots[1].enclosure())
                .is_none()
        );
        assert!(roots[0].tube.intersection(roots[1].tube).is_none());
    }

    #[test]
    fn clamped_rational_endpoints_use_exact_control_point_identity() {
        let points = [[1.0, 2.0, 3.0], [2.0, 3.0, 3.0], [4.0, 2.0, 3.0]];
        let source = Curve::BSpline(Box::new(BSplineCurve {
            degree: 2,
            points: points.to_vec(),
            weights: Some(vec![2.0, 3.0, 4.0]),
            knots: vec![0.0, 1.0],
            multiplicities: vec![3, 3],
            form: CurveForm::Unspecified,
            closed: Logical::False,
            self_intersect: Logical::False,
            knot_spec: KnotSpec::Unspecified,
        }));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000_000);
        let curve =
            PreparedOriginalCurve::new(&source, [0.0, 1.0], BYTES, &mut work, &cancel).unwrap();
        for (parameter, vertex, search) in [
            (0.0, points[0], [0.0, 0.0625]),
            (1.0, points[2], [0.9375, 1.0]),
        ] {
            let roots =
                isolate_endpoint(&curve, vertex, search, 0.0, BYTES, &mut work, &cancel).unwrap();
            assert_eq!(roots.len(), 1);
            assert_eq!(roots[0].exact_parameter(), Some(parameter));
            assert_eq!(roots[0].enclosure().lo, parameter);
            assert_eq!(roots[0].enclosure().hi, parameter);
            assert_eq!(roots[0].source_error_mm(), 0.0);
            for axis in 0..3 {
                assert!(
                    curve_endpoint_coordinate_equal(
                        &curve,
                        parameter,
                        axis,
                        vertex[axis],
                        &mut work,
                        &cancel
                    )
                    .unwrap()
                );
            }
            // An overlapping floating enclosure is not an endpoint identity.
            let adjacent_x = f64::from_bits(vertex[0].to_bits() + 1);
            assert!(
                !curve_endpoint_coordinate_equal(
                    &curve, parameter, 0, adjacent_x, &mut work, &cancel
                )
                .unwrap()
            );
        }
    }

    #[test]
    fn original_endpoint_inventory_obeys_work_cancel_and_byte_admission() {
        let source = circle();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000_000);
        let curve =
            PreparedOriginalCurve::new(&source, [0.0, 1.0], BYTES, &mut work, &cancel).unwrap();
        for bytes in [0, SOURCE_OPERAND_SCRATCH_BYTES] {
            assert_eq!(
                isolate_endpoint(
                    &curve,
                    [2.0, 1.0, 0.0],
                    [0.0, 1.0],
                    0.3,
                    bytes,
                    &mut work,
                    &cancel
                )
                .err()
                .unwrap()
                .code,
                GeometryErrorCode::ResourceLimit
            );
        }
        assert_eq!(
            isolate_endpoint(
                &curve,
                [2.0, 1.0, 0.0],
                [0.0, 1.0],
                0.3,
                BYTES,
                &mut GeometryWorkBudget::new(0),
                &cancel
            )
            .err()
            .unwrap()
            .code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(
            isolate_endpoint(
                &curve,
                [2.0, 1.0, 0.0],
                [0.0, 1.0],
                0.3,
                BYTES,
                &mut work,
                &AtomicBool::new(true)
            )
            .err()
            .unwrap()
            .code,
            GeometryErrorCode::Cancelled
        );
        assert!(
            isolate_endpoint(
                &curve,
                [2.0, 1.0, 0.0],
                [-0.1, 1.0],
                0.3,
                BYTES,
                &mut work,
                &cancel
            )
            .is_err()
        );
    }
}
