// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Source-only comparison expressions. A proposal is not a transport receipt:
//! the original 3DC and completed composition still need the fresh whole-germ
//! proof, and none of these coefficients enter the actual B/G construction.

use super::*;

#[derive(Clone, Copy)]
pub(super) enum ComparisonCompletion {
    Linear,
    Hermite(HermiteCompletion),
}

/// Only `propose` constructs this operator binding. Its intervals enclose the
/// target minus the SAME immutable original endpoint-derivative operator; they
/// are not freely chosen replacement derivatives.
#[derive(Clone, Copy)]
pub(super) struct HermiteCompletion {
    /// A literal finite nonzero vector, not an asserted original tangent.
    target_tangent: [f64; 2],
    /// The incoming target is the exact expression -incoming_scale*T.
    incoming_scale: f64,
    derivative_correction: [Box2; 2],
}

#[derive(Clone, Copy)]
pub(super) struct CorrectionJet {
    pub(super) position: Box2,
    pub(super) derivative: Box2,
    pub(super) second_derivative: Box2,
}

/// Conservative fixed scratch for proposal coefficient and surface jets.
pub(super) const STACK_BYTES: usize = 6
    * std::mem::size_of::<crate::parametric::ChartCurveEnclosure>()
    + 2 * std::mem::size_of::<SurfaceEnclosure>()
    + 2 * std::mem::size_of::<ComparisonCompletion>()
    + 2 * std::mem::size_of::<CorrectionJet>()
    + 12 * std::mem::size_of::<Box2>()
    + 8 * std::mem::size_of::<Box3>();

impl ComparisonCompletion {
    pub(super) fn correction_jet(
        &self,
        joint_correction: [Box2; 2],
        index: usize,
        s: Interval,
    ) -> Result<CorrectionJet, GeometryError> {
        let s = source_unit_parameter(s)?;
        let delta = *joint_correction
            .get(index)
            .ok_or_else(|| refusal("comparison completion has no such germ"))?;
        let complement = source_unit_complement(s)?;
        match *self {
            Self::Linear => Ok(CorrectionJet {
                position: delta.map(|v| complement.mul(v)),
                derivative: delta.map(Interval::neg),
                second_derivative: ZERO2,
            }),
            Self::Hermite(completion) => {
                let delta_d = completion.derivative_correction[index];
                // These are polynomial identities at the literal endpoints,
                // not intersections with an assumed small endpoint error.
                let (h00, h10, d00, d10, dd00, dd10) = if s.lo == 0.0 && s.hi == 0.0 {
                    (
                        Interval::point(1.0),
                        Interval::point(0.0),
                        Interval::point(0.0),
                        Interval::point(1.0),
                        Interval::point(-6.0),
                        Interval::point(-4.0),
                    )
                } else if s.lo == 1.0 && s.hi == 1.0 {
                    (
                        Interval::point(0.0),
                        Interval::point(0.0),
                        Interval::point(0.0),
                        Interval::point(0.0),
                        Interval::point(6.0),
                        Interval::point(2.0),
                    )
                } else {
                    // h00 = 1-3s²+2s³, h10 = s-2s²+s³. Factored
                    // forms retain the zero exit position and derivative.
                    let square = complement.square();
                    (
                        square.mul(Interval::point(1.0).add(Interval::point(2.0).mul(s))),
                        s.mul(square),
                        Interval::point(-6.0).mul(s).mul(complement),
                        complement.mul(Interval::point(1.0).sub(Interval::point(3.0).mul(s))),
                        Interval::point(12.0).mul(s).sub(Interval::point(6.0)),
                        Interval::point(6.0).mul(s).sub(Interval::point(4.0)),
                    )
                };
                let combine = |a: Interval, b: Interval| {
                    std::array::from_fn(|i| a.mul(delta[i]).add(b.mul(delta_d[i])))
                };
                Ok(CorrectionJet {
                    position: combine(h00, h10),
                    derivative: combine(d00, d10),
                    second_derivative: combine(dd00, dd10),
                })
            }
        }
    }

    pub(super) fn apex_line(&self) -> Option<(Box2, Interval)> {
        match *self {
            Self::Linear => None,
            Self::Hermite(completion) => Some((
                completion.target_tangent.map(Interval::point),
                Interval::point(completion.incoming_scale),
            )),
        }
    }
}

/// Only a needed closed-apex, near-opposed candidate may be proposed. The
/// interval determinant supplies the decision, not an angular tolerance.
/// Quadratic ribbons/bulge have no first-order apex contribution; the original
/// germ jets and the actual manufactured cone still retain their own receipts.

pub(super) fn propose(
    domain: &PreparedFaceTrimDomain<'_>,
    recipe: &CornerRecipe,
    joint_correction: [Box2; 2],
    allowance: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ComparisonCompletion>, GeometryError> {
    work.charge(1, cancel)?;
    if !allowance.is_finite() || allowance <= 0.0 {
        return Ok(None);
    }
    let ends = [recipe.outgoing, recipe.incoming];
    let a = end_curve(domain, ends[0], Interval::point(0.0), work, cancel)?.derivative;
    let b = end_curve(domain, ends[1], Interval::point(0.0), work, cancel)?.derivative;
    let linear_a = std::array::from_fn(|i| a[i].sub(joint_correction[0][i]));
    let linear_b = std::array::from_fn(|i| b[i].sub(joint_correction[1][i]));
    let det = determinant(linear_a, linear_b);
    if dot2(linear_a, linear_b).hi >= 0.0 {
        return Ok(None);
    }
    let middle_a = end_curve(domain, ends[0], Interval::point(0.5), work, cancel)?;
    let middle_b = end_curve(domain, ends[1], Interval::point(0.5), work, cancel)?;
    let middle_position_a: Box2 = std::array::from_fn(|i| {
        middle_a.position[i].add(Interval::point(0.5).mul(joint_correction[0][i]))
    });
    let middle_position_b: Box2 = std::array::from_fn(|i| {
        middle_b.position[i].add(Interval::point(0.5).mul(joint_correction[1][i]))
    });
    let offsets = convex_mix(
        recipe.outgoing.offset,
        recipe.incoming.offset,
        Interval::point(0.5),
    )?;
    let radial: Box2 = mix(
        std::array::from_fn(|i| middle_a.derivative[i].sub(joint_correction[0][i])),
        std::array::from_fn(|i| middle_b.derivative[i].sub(joint_correction[1][i])),
        Interval::point(0.5),
    );
    let middle_radial: Box2 = std::array::from_fn(|i| {
        radial[i].add(offsets[i]).add(Interval::point(0.25).mul(recipe.bulge[i]))
    });
    let middle_angular: Box2 = std::array::from_fn(|i| {
        middle_position_b[i].sub(middle_position_a[i]).add(
            Interval::point(0.25)
                .mul(recipe.incoming.offset[i].sub(recipe.outgoing.offset[i])),
        )
    });
    let middle_sign = determinant(middle_radial, middle_angular);
    // A tiny but genuine opposite-sign corner needs a different proved
    // reference as well. Nonzero is not sufficient: it must agree with the
    // interior orientation. This is no near-zero angular threshold.
    if det.lo > 0.0 && middle_sign.lo > 0.0 || det.hi < 0.0 && middle_sign.hi < 0.0 {
        return Ok(None);
    }
    // Rounded midpoint/projection arithmetic ONLY chooses literal constants.
    // All subsequent identities and costs use outward interval arithmetic on
    // those constants and the SAME original coefficient operators.
    let target_tangent = a.map(|v| v.lo * 0.5 + v.hi * 0.5);
    let incoming_midpoint = b.map(|v| v.lo * 0.5 + v.hi * 0.5);
    let target_speed = target_tangent[0].hypot(target_tangent[1]);
    let incoming_scale = incoming_midpoint[0].hypot(incoming_midpoint[1]) / target_speed;
    if target_tangent.iter().any(|v| !v.is_finite())
        || !target_speed.is_finite()
        || target_speed <= 0.0
        || !incoming_scale.is_finite()
        || incoming_scale <= 0.0
    {
        return Ok(None);
    }
    let target = target_tangent.map(Interval::point);
    let incoming_target = target.map(|v| Interval::point(incoming_scale).neg().mul(v));
    let completion = ComparisonCompletion::Hermite(HermiteCompletion {
        target_tangent,
        incoming_scale,
        derivative_correction: [
            std::array::from_fn(|i| target[i].sub(a[i])),
            std::array::from_fn(|i| incoming_target[i].sub(b[i])),
        ],
    });
    for (index, end) in ends.into_iter().enumerate() {
        work.charge(1, cancel)?;
        let original = end_curve(domain, end, UNIT, work, cancel)?;
        let correction = completion.correction_jet(joint_correction, index, UNIT)?;
        let homotopy =
            std::array::from_fn(|i| original.position[i].add(UNIT.mul(correction.position[i])));
        let proof = domain.original_coedge_correspondence(end.use_.coedge)?;
        let cheap_enough = {
            (|| {
                let jet = domain.original_carrier_jet(uv_box(homotopy), work, cancel)?;
                let position = compose_first(&jet.du, &jet.dv, correction.position);
                // Integral Hessian identity bounds the change of the surface first
                // jet along the WHOLE UV homotopy. Include the derivative correction
                // itself, not just the endpoint positional displacement.
                let derivative: Box3 = std::array::from_fn(|axis| {
                    jet.du[axis]
                        .mul(correction.derivative[0])
                        .add(jet.dv[axis].mul(correction.derivative[1]))
                        .add(
                            jet.duu[axis]
                                .mul(correction.position[0])
                                .mul(original.derivative[0]),
                        )
                        .add(
                            jet.duv[axis].mul(
                                correction.position[0]
                                    .mul(original.derivative[1])
                                    .add(correction.position[1].mul(original.derivative[0])),
                            ),
                        )
                        .add(
                            jet.dvv[axis]
                                .mul(correction.position[1])
                                .mul(original.derivative[1]),
                        )
                });
                let cost = Interval::point(proof.position_error_mm())
                    .add(Interval::point(length(position)?.hi))
                    .add(Interval::point(length(derivative)?.hi));
                Ok::<_, GeometryError>(cost.hi.is_finite() && cost.hi < allowance)
            })()
        }?;
        if !cheap_enough {
            return Ok(None);
        }
    }
    Ok(Some(completion))
}

#[cfg(test)]
mod properties {
    use super::*;

    fn polynomial_fixture() -> ComparisonCompletion {
        // Polynomial algebra fixture only: production Hermite bindings are
        // constructed solely by propose from immutable original source jets.
        ComparisonCompletion::Hermite(HermiteCompletion {
            target_tangent: [0.1, 0.3],
            incoming_scale: 1.3,
            derivative_correction: [
                [Interval::point(2.0), Interval::point(-4.0)],
                [Interval::point(-8.0), Interval::point(16.0)],
            ],
        })
    }

    #[test]
    fn hermite_correction_preserves_both_exit_jets_and_literal_apex_data() {
        let completion = polynomial_fixture();
        let joint = [
            [Interval::point(1.0), Interval::point(2.0)],
            [Interval::point(4.0), Interval::point(-2.0)],
        ];
        let ComparisonCompletion::Hermite(data) = completion else {
            unreachable!()
        };
        for index in 0..2 {
            let apex = completion
                .correction_jet(joint, index, Interval::point(0.0))
                .unwrap();
            assert!(equal_box(apex.position, joint[index]));
            assert!(equal_box(
                apex.derivative,
                data.derivative_correction[index]
            ));
            let exit = completion
                .correction_jet(joint, index, Interval::point(1.0))
                .unwrap();
            assert!(equal_box(exit.position, ZERO2));
            assert!(equal_box(exit.derivative, ZERO2));
            for axis in 0..2 {
                let expected =
                    6.0 * joint[index][axis].lo + 2.0 * data.derivative_correction[index][axis].lo;
                assert!(exit.second_derivative[axis].lo <= expected);
                assert!(exit.second_derivative[axis].hi >= expected);
            }
        }
        let (target, scale) = completion.apex_line().unwrap();
        assert!(equal_box(target, data.target_tangent.map(Interval::point)));
        assert_eq!(scale.lo, data.incoming_scale);
        assert_eq!(scale.hi, data.incoming_scale);
        // The incoming expression is NOT a separately rounded point vector.
        let incoming = target.map(|v| scale.neg().mul(v));
        assert!(incoming.iter().all(|v| v.lo < v.hi));
    }

    #[test]
    fn hermite_full_polynomial_jet_encloses_known_interior_values() {
        let completion = polynomial_fixture();
        let joint = [[Interval::point(1.0), Interval::point(2.0)]; 2];
        let jet = completion
            .correction_jet(joint, 0, Interval::point(0.5))
            .unwrap();
        for (actual, expected) in [
            (jet.position, [0.75, 0.5]),
            (jet.derivative, [-2.0, -2.0]),
            (jet.second_derivative, [-2.0, 4.0]),
        ] {
            for axis in 0..2 {
                assert!(actual[axis].lo <= expected[axis]);
                assert!(actual[axis].hi >= expected[axis]);
            }
        }
        assert!(
            completion
                .correction_jet(joint, 2, Interval::point(0.5))
                .is_err()
        );
    }

    #[test]
    fn linear_completion_retains_original_position_and_derivative_polynomial() {
        let joint = [[Interval::point(2.0), Interval::point(-4.0)]; 2];
        let linear = ComparisonCompletion::Linear;
        let midpoint = linear
            .correction_jet(joint, 1, Interval::point(0.5))
            .unwrap();
        assert!(contains(
            midpoint.position,
            [Interval::point(1.0), Interval::point(-2.0)]
        ));
        assert!(equal_box(midpoint.derivative, joint[1].map(Interval::neg)));
        assert!(equal_box(midpoint.second_derivative, ZERO2));
        assert!(linear.apex_line().is_none());
    }

    #[test]
    fn general_completion_does_not_inherit_a_quarter_derivative_receipt() {
        let source = [
            Interval::point(1.0),
            Interval::point(0.0),
            Interval::point(0.0),
        ];
        for target in [-1.0, 0.5, 0.75] {
            // Exact coefficient fixture gamma_a'=(1,0), T=(target,0).
            // This is deliberately too expensive for a microscopic proposal.
            let completion = ComparisonCompletion::Hermite(HermiteCompletion {
                target_tangent: [target, 0.0],
                incoming_scale: 1.0,
                derivative_correction: [
                    [Interval::point(target - 1.0), Interval::point(0.0)],
                    ZERO2,
                ],
            });
            let correction = completion
                .correction_jet([ZERO2; 2], 0, Interval::point(0.0))
                .unwrap();
            let completed = [
                source[0].add(correction.derivative[0]),
                source[1].add(correction.derivative[1]),
                source[2],
            ];
            assert!(
                comparison_transport::relative_error(source, completed)
                    .unwrap()
                    .is_none(),
                "opposed, half-speed and exact quarter-error targets must fail"
            );
        }
    }
}
