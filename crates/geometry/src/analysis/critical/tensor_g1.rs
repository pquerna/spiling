// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::parametric::SurfaceEvaluator;

/// Borrow the actual control net. Original STEP points are Euclidean and their
/// weights are separate; native NURBS points already store homogeneous xyz.
enum TensorControls<'a> {
    Native(&'a solidcore::nurbs::srf_basic::Surface),
    Original(&'a solidcraft_step::geom::BSplineSurface),
}
struct TensorCoefficients<'a> {
    controls: TensorControls<'a>,
    axes: [(&'a [f64], usize); 2],
    counts: [usize; 2],
}
impl TensorCoefficients<'_> {
    fn coefficient(
        &self,
        axis: usize,
        index: usize,
        other: usize,
        coordinate: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<ExactCoefficient>, GeometryError> {
        work.charge(1, cancel)?;
        let [u, v] = if axis == 0 {
            [index, other]
        } else {
            [other, index]
        };
        match &self.controls {
            TensorControls::Native(surface) => {
                let point = surface
                    .cpt(u, v)
                    .expect("validated native tensor control index");
                let value = [
                    point.x,
                    point.y,
                    point.z,
                    if surface.is_rational() { point.w } else { 1.0 },
                ][coordinate];
                Ok(value.is_finite().then(|| ExactCoefficient::point(value)))
            }
            TensorControls::Original(surface) => {
                let weight = surface
                    .weights
                    .as_ref()
                    .map_or(1.0, |weights| weights[u][v]);
                if !weight.is_finite() {
                    return Ok(None);
                }
                if coordinate == 3 {
                    return Ok(Some(ExactCoefficient::point(weight)));
                }
                let value = surface.points[u][v][coordinate];
                if !value.is_finite() {
                    return Ok(None);
                }
                let value = ExactCoefficient::point(value);
                if weight == 1.0 {
                    return Ok(Some(value));
                }
                // Never round xyz*w before forming the identity. Its FMA
                // remainder is part of every derivative and seam coefficient.
                value.product(&ExactCoefficient::point(weight), work, cancel)
            }
        }
    }
}
fn tensor_coefficients<'a>(
    surface: &'a (impl SurfaceEvaluator + ?Sized),
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<TensorCoefficients<'a>>, GeometryError> {
    let (controls, counts) = if let Some(native) = crate::parametric::native_surface_nurbs(surface)?
    {
        (
            TensorControls::Native(native),
            [native.n() + 1, native.m() + 1],
        )
    } else if let Some(mut original) = surface.original_surface() {
        let mut depth = 0;
        while let solidcraft_step::geom::Surface::RectangularTrimmed { basis, .. } = original {
            work.charge(1, cancel)?;
            depth += 1;
            if depth >= 16 {
                return Err(GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "original tensor join wrapper depth exceeded",
                ));
            }
            original = basis;
        }
        let solidcraft_step::geom::Surface::BSpline(original) = original else {
            return Ok(None);
        };
        let counts = [
            original.points.len(),
            original.points.first().map_or(0, Vec::len),
        ];
        (TensorControls::Original(original), counts)
    } else {
        return Ok(None);
    };
    let axes = surface.parameter_axes(domain, work, cancel)?;
    for axis in 0..2 {
        if counts[axis] <= axes[axis].1 || axes[axis].0.len() <= counts[axis] {
            return Ok(None);
        }
    }
    Ok(Some(TensorCoefficients {
        controls,
        axes,
        counts,
    }))
}

/// Exact direct-tensor inventory only. A sweep's nested profile axes never
/// acquire the authority of a root surface coefficient net.
pub(super) fn direct_tensor_axes<'a>(
    surface: &'a (impl SurfaceEvaluator + ?Sized),
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[(&'a [f64], usize, usize); 2]>, GeometryError> {
    Ok(
        tensor_coefficients(surface, domain, work, cancel)?.map(|tensor| {
            std::array::from_fn(|axis| {
                (
                    tensor.axes[axis].0,
                    tensor.axes[axis].1,
                    tensor.counts[axis],
                )
            })
        }),
    )
}

/// At a degree-multiplicity knot prove R=mu*L+lambda*H in homogeneous
/// coordinates, with mu strictly positive. Projectivization removes lambda*H;
/// positive transverse reparameterization removes mu. The two regular charts
/// therefore have an exact common tangent plane, without recognizing a family
/// or declaring a small computed derivative jump to be zero.
pub(in crate::analysis) fn interface_regular_join(
    surface: &(impl SurfaceEvaluator + ?Sized),
    interface: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if interface.iter().all(|axis| axis[0] != axis[1]) {
        // A nonrepresentable reflected knot is an actual interval band, not
        // either endpoint masquerading as the original exact source knot.
        return Ok(false);
    }
    let axis = usize::from(interface[1][0] == interface[1][1]);
    let parameter = interface[axis][0];
    let Some(s) = tensor_coefficients(surface, interface, work, cancel)? else {
        return Ok(false);
    };
    let (knots, degree) = s.axes[axis];
    let count = s.counts[axis];
    let other_count = s.counts[1 - axis];
    let start = knots.partition_point(|value| *value < parameter);
    let end = knots.partition_point(|value| *value <= parameter);
    if degree == 0 || end - start != degree || start == 0 || end >= knots.len() {
        return Ok(false);
    }
    let Some(shared) = end.checked_sub(degree + 1) else {
        return Ok(false);
    };
    if shared == 0 || shared + 1 >= count {
        return Ok(false);
    }
    let Some(left_span) = ExactCoefficient::difference(parameter, knots[start - 1]) else {
        return Ok(false);
    };
    let Some(right_span) = ExactCoefficient::difference(knots[end], parameter) else {
        return Ok(false);
    };
    let control = |index, other, coordinate, work: &mut GeometryWorkBudget| {
        s.coefficient(axis, index, other, coordinate, work, cancel)
    };
    let derivative = |other: usize,
                      coordinate: usize,
                      left: bool,
                      work: &mut GeometryWorkBudget|
     -> Result<Option<ExactCoefficient>, GeometryError> {
        let Some(center) = control(shared, other, coordinate, work)? else {
            return Ok(None);
        };
        let Some(neighbor) = control(
            if left { shared - 1 } else { shared + 1 },
            other,
            coordinate,
            work,
        )?
        else {
            return Ok(None);
        };
        let (mut difference, subtract) = if left {
            (center, neighbor)
        } else {
            (neighbor, center)
        };
        if difference.subtract(&subtract).is_none() {
            return Ok(None);
        }
        difference.product(if left { &right_span } else { &left_span }, work, cancel)
    };
    coefficient_tangent_join(
        other_count,
        |other, coordinate, work| control(shared, other, coordinate, work),
        derivative,
        work,
        cancel,
    )
}

/// Coefficientwise equality with CONSTANT positive projective tangent scale.
/// This is a sufficient polynomial identity, not interpolation at control points.
pub(super) fn coefficient_tangent_join(
    other_count: usize,
    control: impl Fn(
        usize,
        usize,
        &mut GeometryWorkBudget,
    ) -> Result<Option<ExactCoefficient>, GeometryError>,
    derivative: impl Fn(
        usize,
        usize,
        bool,
        &mut GeometryWorkBudget,
    ) -> Result<Option<ExactCoefficient>, GeometryError>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let mut reference = None;
    for other in 0..other_count {
        let Some(weight) = control(other, 3, work)? else {
            return Ok(false);
        };
        if weight.sign() != 0 {
            reference = Some((other, weight));
            break;
        }
    }
    let Some((reference, weight)) = reference else {
        return Ok(false);
    };
    let Some(lw) = derivative(reference, 3, true, work)? else {
        return Ok(false);
    };
    let Some(rw) = derivative(reference, 3, false, work)? else {
        return Ok(false);
    };
    let mut coefficients = None;
    for coordinate in 0..3 {
        let Some(l) = derivative(reference, coordinate, true, work)? else {
            return Ok(false);
        };
        let Some(r) = derivative(reference, coordinate, false, work)? else {
            return Ok(false);
        };
        let Some(point) = control(reference, coordinate, work)? else {
            return Ok(false);
        };
        let Some(mut determinant) = l.product(&weight, work, cancel)? else {
            return Ok(false);
        };
        let Some(term) = lw.product(&point, work, cancel)? else {
            return Ok(false);
        };
        if determinant.subtract(&term).is_none() {
            return Ok(false);
        }
        if determinant.sign() == 0 {
            continue;
        }
        let Some(mut mu) = r.product(&weight, work, cancel)? else {
            return Ok(false);
        };
        let Some(term) = rw.product(&point, work, cancel)? else {
            return Ok(false);
        };
        if mu.subtract(&term).is_none() || mu.sign() != determinant.sign() {
            return Ok(false);
        }
        let Some(mut lambda) = l.product(&rw, work, cancel)? else {
            return Ok(false);
        };
        let Some(term) = lw.product(&r, work, cancel)? else {
            return Ok(false);
        };
        if lambda.subtract(&term).is_none() {
            return Ok(false);
        }
        coefficients = Some((determinant, mu, lambda));
        break;
    }
    let Some((determinant, mu, lambda)) = coefficients else {
        return Ok(false);
    };
    for other in 0..other_count {
        work.charge(1, cancel)?;
        for coordinate in 0..4 {
            let Some(l) = derivative(other, coordinate, true, work)? else {
                return Ok(false);
            };
            let Some(r) = derivative(other, coordinate, false, work)? else {
                return Ok(false);
            };
            let Some(mut identity) = determinant.product(&r, work, cancel)? else {
                return Ok(false);
            };
            let Some(first) = mu.product(&l, work, cancel)? else {
                return Ok(false);
            };
            let Some(value) = control(other, coordinate, work)? else {
                return Ok(false);
            };
            let Some(second) = lambda.product(&value, work, cancel)? else {
                return Ok(false);
            };
            if identity.subtract(&first).is_none()
                || identity.subtract(&second).is_none()
                || identity.len != 0
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Prove a closed rational/B-spline chart's end identification by the same
/// exact homogeneous G1 identity used at internal knots. Endpoint rows may have
/// different homogeneous scales; no coordinate tolerance closes a seam.
pub(super) fn periodic_regular_join(
    surface: &(impl SurfaceEvaluator + ?Sized),
    axis: usize,
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if axis >= 2 {
        return Ok(false);
    }
    let Some(s) = tensor_coefficients(surface, domain, work, cancel)? else {
        return Ok(false);
    };
    let (knots, degree) = s.axes[axis];
    let count = s.counts[axis];
    let other_count = s.counts[1 - axis];
    if degree == 0
        || count < 3
        || knots.len() <= count
        || knots[..=degree].iter().any(|&k| k != knots[degree])
        || knots[count..].iter().any(|&k| k != knots[count])
        || domain[axis] != [knots[degree], knots[count]]
    {
        return Ok(false);
    }
    let control = |index, other, coordinate, work: &mut GeometryWorkBudget| {
        s.coefficient(axis, index, other, coordinate, work, cancel)
    };
    let mut reference = None;
    for other in 0..other_count {
        let Some(first) = control(0, other, 3, work)? else {
            return Ok(false);
        };
        if first.sign() != 0 {
            let Some(last) = control(count - 1, other, 3, work)? else {
                return Ok(false);
            };
            if last.sign() != first.sign() {
                return Ok(false);
            }
            reference = Some((first, last));
            break;
        }
    }
    let Some((first_weight, last_weight)) = reference else {
        return Ok(false);
    };
    // Clear the positive projective scale last_weight/first_weight instead
    // of rounding that quotient and then hoping the endpoint identity survives.
    for other in 0..other_count {
        for coordinate in 0..4 {
            let Some(first) = control(0, other, coordinate, work)? else {
                return Ok(false);
            };
            let Some(last) = control(count - 1, other, coordinate, work)? else {
                return Ok(false);
            };
            let Some(mut identity) = first.product(&last_weight, work, cancel)? else {
                return Ok(false);
            };
            let Some(term) = last.product(&first_weight, work, cancel)? else {
                return Ok(false);
            };
            if identity.subtract(&term).is_none() || identity.sign() != 0 {
                return Ok(false);
            }
        }
    }
    let Some(left_span) = ExactCoefficient::difference(knots[count], knots[count - 1]) else {
        return Ok(false);
    };
    let Some(right_span) = ExactCoefficient::difference(knots[degree + 1], knots[degree]) else {
        return Ok(false);
    };
    if left_span.sign() <= 0 || right_span.sign() <= 0 {
        return Ok(false);
    }
    let derivative = |other: usize,
                      coordinate: usize,
                      left: bool,
                      work: &mut GeometryWorkBudget|
     -> Result<Option<ExactCoefficient>, GeometryError> {
        let (a, b) = if left { (count - 1, count - 2) } else { (1, 0) };
        let Some(mut difference) = control(a, other, coordinate, work)? else {
            return Ok(None);
        };
        let Some(subtract) = control(b, other, coordinate, work)? else {
            return Ok(None);
        };
        if difference.subtract(&subtract).is_none() {
            return Ok(None);
        }
        let Some(difference) =
            difference.product(if left { &right_span } else { &left_span }, work, cancel)?
        else {
            return Ok(None);
        };
        difference.product(
            if left { &first_weight } else { &last_weight },
            work,
            cancel,
        )
    };
    coefficient_tangent_join(
        other_count,
        |other, coordinate, work| control(count - 1, other, coordinate, work),
        derivative,
        work,
        cancel,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::source_operand::PreparedOriginalSurface;
    use solidcraft_step::geom::{
        BSplineSurface, KnotSpec, Logical, Surface as OriginalSurface, SurfaceForm,
    };
    use std::sync::Arc;

    fn original_tensor(points: &[[f64; 2]], weights: &[f64]) -> OriginalSurface {
        let spans = (points.len() - 1) / 2;
        OriginalSurface::BSpline(Box::new(BSplineSurface {
            u_degree: 2,
            v_degree: 1,
            points: points
                .iter()
                .map(|&[x, y]| vec![[x, y, 0.0], [x, y, 1.0]])
                .collect(),
            weights: Some(weights.iter().map(|&w| vec![w; 2]).collect()),
            u_knots: (0..=spans).map(|i| i as f64).collect(),
            u_multiplicities: (0..=spans)
                .map(|i| if i == 0 || i == spans { 3 } else { 2 })
                .collect(),
            v_knots: vec![0.0, 1.0],
            v_multiplicities: vec![2, 2],
            form: SurfaceForm::Unspecified,
            u_closed: Logical::Unknown,
            v_closed: Logical::False,
            self_intersect: Logical::Unknown,
            knot_spec: KnotSpec::PiecewiseBezierKnots,
        }))
    }

    #[test]
    fn original_weighted_xyz_keeps_the_unrounded_product_remainder() {
        let next = f64::from_bits(1.0_f64.to_bits() + 1);
        let source = original_tensor(&[[next, 0.0], [1.0, 1.0], [0.0, 1.0]], &[next, 1.0, 1.0]);
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let provider = PreparedOriginalSurface::new(
            &source,
            [[0.0, 1.0]; 2],
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let tensor = tensor_coefficients(&provider, [[0.0, 1.0]; 2], &mut work, &cancel)
            .unwrap()
            .unwrap();
        let mut exact = tensor
            .coefficient(0, 0, 0, 0, &mut work, &cancel)
            .unwrap()
            .unwrap();
        exact
            .subtract(&ExactCoefficient::point(next * next))
            .unwrap();
        assert_eq!(
            exact.sign(),
            1,
            "the nonzero FMA remainder is part of the original homogeneous coefficient"
        );
    }

    #[test]
    fn original_tensor_g1_uses_exact_weighted_coefficients_at_closed_knots() {
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let source = original_tensor(
            &[[1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [-1.0, 1.0], [-1.0, 0.0]],
            &[1.0, w, 1.0, w, 1.0],
        );
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let provider = PreparedOriginalSurface::new(
            &source,
            [[0.0, 2.0], [0.0, 1.0]],
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            interface_regular_join(&provider, [[1.0; 2], [0.0, 1.0]], &mut work, &cancel).unwrap()
        );
        assert!(
            !interface_regular_join(
                &provider,
                [[down(1.0), up(1.0)], [0.0, 1.0]],
                &mut work,
                &cancel
            )
            .unwrap()
        );
        let OriginalSurface::BSpline(raw) = &source else {
            unreachable!()
        };
        let mut perturbed = raw.as_ref().clone();
        perturbed.points[3][1][2] = up(1.0);
        let perturbed = OriginalSurface::BSpline(Box::new(perturbed));
        let perturbed = PreparedOriginalSurface::new(
            &perturbed,
            [[0.0, 2.0], [0.0, 1.0]],
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            !interface_regular_join(&perturbed, [[1.0; 2], [0.0, 1.0]], &mut work, &cancel)
                .unwrap(),
            "a one-ULP nonconstant transverse defect is never an exact projective tangent identity"
        );
    }

    #[test]
    fn original_periodic_seam_clears_a_nonrepresentable_positive_scale() {
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let source = original_tensor(
            &[
                [1.0, 0.0],
                [1.0, 1.0],
                [0.0, 1.0],
                [-1.0, 1.0],
                [-1.0, 0.0],
                [-1.0, -1.0],
                [0.0, -1.0],
                [1.0, -1.0],
                [1.0, 0.0],
            ],
            &[3.0, w, 1.0, w, 1.0, w, 1.0, w, 2.0],
        );
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let domain = [[0.0, 4.0], [0.0, 1.0]];
        let provider =
            PreparedOriginalSurface::new(&source, domain, MAX_SCRATCH_BYTES, &mut work, &cancel)
                .unwrap();
        assert!(periodic_regular_join(&provider, 0, domain, &mut work, &cancel).unwrap());
        assert!(
            !periodic_regular_join(&provider, 0, [[0.0, 3.0], domain[1]], &mut work, &cancel)
                .unwrap(),
            "the original seam receipt cannot close another selected endpoint"
        );
        let OriginalSurface::BSpline(raw) = &source else {
            unreachable!()
        };
        let mut perturbed = raw.as_ref().clone();
        perturbed.points[8][1][0] = up(1.0);
        let perturbed = OriginalSurface::BSpline(Box::new(perturbed));
        let perturbed =
            PreparedOriginalSurface::new(&perturbed, domain, MAX_SCRATCH_BYTES, &mut work, &cancel)
                .unwrap();
        assert!(
            !periodic_regular_join(&perturbed, 0, domain, &mut work, &cancel).unwrap(),
            "a one-ULP endpoint gap is not closed by a matching reference row"
        );
    }

    #[test]
    fn direct_tensor_inventory_preserves_trim_gauge_and_refuses_nested_sweeps() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let source = original_tensor(&[[1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], &[1.0; 3]);
        let trimmed = OriginalSurface::RectangularTrimmed {
            basis: Arc::new(source),
            u1: 0.25,
            u2: 0.75,
            v1: 0.0,
            v2: 1.0,
            u_sense: false,
            v_sense: true,
        };
        let domain = [[0.25, 0.75], [0.0, 1.0]];
        let provider =
            PreparedOriginalSurface::new(&trimmed, domain, MAX_SCRATCH_BYTES, &mut work, &cancel)
                .unwrap();
        let axes = direct_tensor_axes(&provider, domain, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(axes[0], (&[0.0, 0.0, 0.0, 1.0, 1.0, 1.0][..], 2, 3));
        let swept = OriginalSurface::Extrusion {
            profile: Arc::new(solidcraft_step::geom::Curve::Line {
                origin: [0.0; 3],
                direction: [1.0, 0.0, 0.0],
                magnitude: 1.0,
            }),
            direction: [0.0, 1.0, 0.0],
            magnitude: 1.0,
        };
        let provider = PreparedOriginalSurface::new(
            &swept,
            [[0.0, 1.0]; 2],
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            direct_tensor_axes(&provider, [[0.0, 1.0]; 2], &mut work, &cancel)
                .unwrap()
                .is_none(),
            "nested profile coefficients are not a root surface tensor"
        );
    }
}
