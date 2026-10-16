// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A second radial factor for the zero-offset Middle comparison only. This is
//! local differential evidence, not boundary winding or relative-trim authority.

use super::*;

// Fixed, allocation-free coefficient scratch, including both completed C2 jets.
// The owner reserves this in addition to the native evaluator's own scratch.
pub(super) const STACK_BYTES: usize =
    32 * std::mem::size_of::<Box2>() + 16 * std::mem::size_of::<Interval>();

fn exact_zero(value: Box2) -> bool {
    value.iter().all(|v| v.lo == 0.0 && v.hi == 0.0)
}

fn finite(value: Interval) -> bool {
    value.lo.is_finite() && value.hi.is_finite() && value.lo <= value.hi
}

fn strict(value: Interval, sign: f64) -> bool {
    finite(value)
        && if sign == 1.0 {
            value.lo > 0.0
        } else {
            value.hi < 0.0
        }
}

fn qualified_line(line: Option<(Box2, Interval)>) -> Option<(Box2, Interval)> {
    let (tangent, kappa) = line?;
    // These are literal operators, not a near-opposition tolerance or an
    // independently rounded incoming vector. B'(0) is exactly -kappa*T.
    if !finite(kappa)
        || kappa.lo <= 0.0
        || kappa.lo != kappa.hi
        || tangent.iter().any(|v| !finite(*v) || v.lo != v.hi)
        || !tangent.iter().any(|v| v.lo > 0.0 || v.hi < 0.0)
    {
        return None;
    }
    Some((tangent, kappa))
}

/// Proves the extra-s angular and Jacobian coefficients on a CLOSED apex
/// restriction. Call only for Piece::Middle. The original same-NativeV quotient
/// retains s=0; no positive-radius hole or clamped normal is introduced here.
#[allow(clippy::too_many_arguments)]
pub(super) fn proves(
    domain: &PreparedFaceTrimDomain<'_>,
    recipe: &CornerRecipe,
    comparison: &UvJointComparison,
    s: Interval,
    theta: Interval,
    sign: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    comparison.validate(domain, recipe)?;
    let s = source_unit_parameter(s)?;
    let theta = source_unit_parameter(theta)?;
    if s.lo != 0.0
        || (sign != 1.0 && sign != -1.0)
        || !exact_zero(recipe.outgoing.offset)
        || !exact_zero(recipe.incoming.offset)
        || !exact_zero(recipe.bulge)
    {
        return Ok(false);
    }
    let Some((tangent, kappa)) = qualified_line(comparison.apex_line()) else {
        return Ok(false);
    };
    let whole = Interval { lo: 0.0, hi: s.hi };
    // A C0 jet refusal is propagated, not converted into C2 authority by
    // sampling, endpoint data, or subdivision across its unqualified knot.
    let a = comparison.completed_end_jet(domain, recipe, 0, whole, work, cancel)?;
    let b = comparison.completed_end_jet(domain, recipe, 1, whole, work, cancel)?;
    work.charge(1, cancel)?;
    Ok(coefficients_strict(
        tangent,
        kappa,
        a.second_derivative,
        b.second_derivative,
        s,
        theta,
        sign,
    ))
}

#[allow(clippy::too_many_arguments)]
fn coefficients_strict(
    tangent: Box2,
    kappa: Interval,
    dda: Box2,
    ddb: Box2,
    s: Interval,
    theta: Interval,
    sign: f64,
) -> bool {
    if (sign != 1.0 && sign != -1.0)
        || qualified_line(Some((tangent, kappa))).is_none()
        || dda.into_iter().chain(ddb).any(|v| !finite(v))
    {
        return false;
    }
    // Integral Taylor identities use only C2 enclosures on [0,s.hi]:
    // gamma_a = joint + s*T + s²*Ra,
    // gamma_b = joint - s*kappa*T + s²*Rb,
    // gamma_a' = T + s*Pa, gamma_b' = -kappa*T + s*Pb.
    // Ra = integral (1-v) gamma_a''(sv)dv lies in dda/2;
    // Pa = integral gamma_a''(sv)dv lies in dda (and likewise b).
    // Ra and Pa are NOT asserted equal to one another or to endpoint samples.
    let ra = dda.map(|v| v.mul(Interval::point(0.5)));
    let rb = ddb.map(|v| v.mul(Interval::point(0.5)));
    let delta: Box2 = std::array::from_fn(|i| rb[i].sub(ra[i]));
    let rblend = mix(ra, rb, theta);
    let pblend = mix(dda, ddb, theta);
    let one_plus_kappa = Interval::point(1.0).add(kappa);
    let a = Interval::point(1.0).sub(one_plus_kappa.mul(theta));
    let angular = determinant(tangent, rb)
        .add(kappa.mul(determinant(tangent, ra)))
        .add(s.mul(determinant(ra, rb)));
    let jacobian = a
        .mul(determinant(tangent, delta))
        .add(one_plus_kappa.mul(determinant(tangent, pblend)))
        .add(s.mul(determinant(pblend, delta)));
    // Q = a(theta)*T + s*Rblend. For every s>0, this strict
    // determinant proves Q != 0, including a(theta)=0. At s=0 the
    // collapsed same-NativeV quotient is retained by the parent consumer.
    let normal_remainder = determinant(tangent, rblend);
    strict(angular, sign)
        && strict(jacobian, sign)
        && finite(normal_remainder)
        && (normal_remainder.lo > 0.0 || normal_remainder.hi < 0.0)
}

#[cfg(test)]
mod properties {
    use super::*;
    use monstertruck_modeling::{
        BsplineCurve, Conic2D, Curve2D, KnotVector, Matrix3, Point2, Processor, TrimmedCurve,
        UnitCircle, ValidatedGeometryBudget, validated_curve2d_jet,
    };

    fn point(x: f64, y: f64) -> Box2 {
        [Interval::point(x), Interval::point(y)]
    }

    fn quadratic(speed: f64, curvature: f64) -> Curve2D {
        // The actual Bernstein coefficients give gamma=(speed*s,curvature*s²).
        Curve2D::BsplineCurve(BsplineCurve::new(
            KnotVector::from(vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(speed * 0.5, 0.0),
                Point2::new(speed, curvature),
            ],
        ))
    }

    fn second(curve: &Curve2D, hi: f64) -> Box2 {
        let jet = validated_curve2d_jet(
            curve,
            [0.0, hi],
            &mut ValidatedGeometryBudget::new(1_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        jet.second.map(|v| Interval {
            lo: v.lower,
            hi: v.upper,
        })
    }

    #[test]
    fn actual_quadratic_smooth_split_has_a_true_second_radial_factor() {
        for kappa in [0.5, 1.0, 2.0] {
            for curvature in [0.125, 1.0, 8.0] {
                let s = Interval { lo: 0.0, hi: 0.125 };
                let a = second(&quadratic(1.0, curvature), s.hi);
                let b = second(&quadratic(-kappa, curvature), s.hi);
                assert!(coefficients_strict(
                    point(1.0, 0.0),
                    Interval::point(kappa),
                    a,
                    b,
                    s,
                    UNIT,
                    1.0,
                ));
                // The old first-factor enclosure contains zero at the closed
                // apex, but the extra coefficient remains strictly positive.
                let angular = s.mul(Interval::point((1.0 + kappa) * curvature));
                assert!(angular.lo <= 0.0 && angular.hi > 0.0);
                assert!(!coefficients_strict(
                    point(1.0, 0.0),
                    Interval::point(kappa),
                    a,
                    b,
                    s,
                    UNIT,
                    -1.0,
                ));
            }
        }
    }

    #[test]
    fn actual_circular_split_has_nonzero_normal_remainder() {
        let circle = |direction: f64| {
            // gamma=(direction*sin(s),1-cos(s)); both literal endpoints
            // are s=0, not rounded PI/TAU endpoint substitutions.
            Curve2D::Conic(Conic2D::Ellipse(Processor::with_transform(
                TrimmedCurve::new(UnitCircle::<Point2>::new(), (0.0, 1.0)),
                Matrix3::new(0.0, -1.0, 0.0, direction, 0.0, 0.0, 0.0, 1.0, 1.0),
            )))
        };
        let s = Interval {
            lo: 0.0,
            hi: 0.03125,
        };
        assert!(coefficients_strict(
            point(1.0, 0.0),
            Interval::point(1.0),
            second(&circle(1.0), s.hi),
            second(&circle(-1.0), s.hi),
            s,
            UNIT,
            1.0,
        ));
    }

    #[test]
    fn actual_curvature_fold_and_zero_normal_remainder_refuse() {
        let s = Interval { lo: 0.0, hi: 0.125 };
        let a = second(&quadratic(1.0, 1.0), s.hi);
        let fold = second(&quadratic(-1.0, -1.0), s.hi);
        let flat_a = second(&quadratic(1.0, 0.0), s.hi);
        let flat_b = second(&quadratic(-1.0, 0.0), s.hi);
        for (a, b) in [(a, fold), (flat_a, flat_b)] {
            for sign in [-1.0, 1.0] {
                assert!(!coefficients_strict(
                    point(1.0, 0.0),
                    Interval::point(1.0),
                    a,
                    b,
                    s,
                    UNIT,
                    sign,
                ));
            }
        }
        assert!(
            qualified_line(None).is_none(),
            "no opposed-completion authority"
        );
        assert!(qualified_line(Some((point(0.0, 0.0), Interval::point(1.0)))).is_none());
        assert!(qualified_line(Some((point(1.0, 0.0), Interval::point(0.0)))).is_none());
        assert!(qualified_line(Some((point(1.0, 0.0), Interval { lo: 0.9, hi: 1.1 }))).is_none());
        assert!(!exact_zero([
            Interval {
                lo: 0.0,
                hi: f64::MIN_POSITIVE
            },
            Interval::point(0.0)
        ]));
    }
}
