// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::{Admission, indeterminate};
use crate::{GeometryError, GeometryWorkBudget, parametric::Interval};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

const MAX_ROOT_DEPTH: u8 = 40;

#[derive(Clone, Copy, Debug)]
pub(super) struct Root {
    pub parameter: Interval,
    pub derivative: Interval,
}
#[derive(Clone, Copy)]
pub(super) struct ScalarJet {
    pub value: Interval,
    pub derivative: Interval,
}
pub(super) fn sign(x: Interval) -> i8 {
    if x.lo > 0.0 {
        1
    } else if x.hi < 0.0 {
        -1
    } else {
        0
    }
}

/// Every initial knot span is exhausted by exclusion or a monotone root proof.
/// A subdivision threshold is only a refusal boundary, never root authority.
pub(super) fn isolate(
    spans: impl Iterator<Item = [f64; 2]>,
    evaluate: &mut impl FnMut(
        [f64; 2],
        &mut GeometryWorkBudget,
        &AtomicBool,
    ) -> Result<ScalarJet, GeometryError>,
    parameter_accuracy: f64,
    admission: &mut Admission,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Vec<Root>, GeometryError> {
    let mut roots = Vec::new();
    for span in spans {
        visit(
            span,
            0,
            evaluate,
            parameter_accuracy,
            &mut roots,
            admission,
            work,
            cancel,
        )?;
    }
    roots.sort_by(|a, b| a.parameter.lo.total_cmp(&b.parameter.lo));
    let mut keep = 0;
    for i in 0..roots.len() {
        let root = roots[i];
        if keep > 0
            && roots[keep - 1]
                .parameter
                .intersection(root.parameter)
                .is_some()
        {
            // Both intervals already prove one root. Only an exact shared knot
            // is a proved duplicate; overlapping candidate boxes are not enough.
            let old = roots[keep - 1].parameter;
            if old.is_point() && root.parameter.is_point() && old == root.parameter {
                continue;
            }
            return Err(indeterminate(
                "native root isolators overlap without an exact shared-knot identity",
            ));
        }
        roots[keep] = root;
        keep += 1;
    }
    roots.truncate(keep);
    Ok(roots)
}
/// A strict derivative bound may need several closed pieces even when the
/// continuous parent has one monotone root. Exhaust BOTH children and retain
/// one common strict sign; a partial cover or an unresolved piece proves
/// nothing. The same depth and caller work allowance bound this optional proof.
fn strict_derivative_cover(
    range: [f64; 2],
    derivative: Interval,
    depth: u8,
    evaluate: &mut impl FnMut(
        [f64; 2],
        &mut GeometryWorkBudget,
        &AtomicBool,
    ) -> Result<ScalarJet, GeometryError>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    work.charge(1, cancel)?;
    if sign(derivative) != 0 {
        return Ok(Some(derivative));
    }
    if derivative.is_point() || depth >= MAX_ROOT_DEPTH {
        return Ok(None);
    }
    let mid = range[0] * 0.5 + range[1] * 0.5;
    if mid <= range[0] || mid >= range[1] {
        return Ok(None);
    }
    let left = [range[0], mid];
    let left_derivative = evaluate(left, work, cancel)?.derivative;
    let Some(left_derivative) =
        strict_derivative_cover(left, left_derivative, depth + 1, evaluate, work, cancel)?
    else {
        return Ok(None);
    };
    let right = [mid, range[1]];
    let right_derivative = evaluate(right, work, cancel)?.derivative;
    if sign(right_derivative) != 0 && sign(left_derivative) != sign(right_derivative) {
        return Ok(None);
    }
    let Some(right_derivative) =
        strict_derivative_cover(right, right_derivative, depth + 1, evaluate, work, cancel)?
    else {
        return Ok(None);
    };
    Ok((sign(left_derivative) == sign(right_derivative))
        .then(|| left_derivative.hull(right_derivative)))
}

fn visit(
    range: [f64; 2],
    depth: u8,
    evaluate: &mut impl FnMut(
        [f64; 2],
        &mut GeometryWorkBudget,
        &AtomicBool,
    ) -> Result<ScalarJet, GeometryError>,
    accuracy: f64,
    roots: &mut Vec<Root>,
    admission: &mut Admission,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    let jet = evaluate(range, work, cancel)?;
    if sign(jet.value) != 0 {
        return Ok(());
    }
    let first = evaluate([range[0]; 2], work, cancel)?.value;
    let last = evaluate([range[1]; 2], work, cancel)?.value;
    let a = sign(first);
    let b = sign(last);
    let mut complete_derivative = jet.derivative;
    if sign(complete_derivative) == 0 && a != 0 && b != 0 && a != b {
        // Opposite outer signs already prove existence on this SAME continuous
        // parent. Before assigning an uncertain split-point value to either
        // child, try to prove uniqueness by a complete monotone cover.
        let covered = match strict_derivative_cover(
            range,
            complete_derivative,
            depth,
            evaluate,
            work,
            cancel,
        ) {
            Ok(covered) => covered,
            Err(failure)
                if matches!(
                    failure.code,
                    GeometryErrorCode::AnalysisIndeterminate
                        | GeometryErrorCode::ConditioningFailure
                        | GeometryErrorCode::SingularGeometry
                        | GeometryErrorCode::UnsupportedOperation
                ) =>
            {
                None
            }
            Err(failure) => return Err(failure),
        };
        if let Some(covered) = covered {
            complete_derivative = complete_derivative.intersect(covered)?;
        }
    }
    let ds = sign(complete_derivative);
    if ds != 0 {
        if a != 0 && a == b {
            return Ok(());
        }
        let exact_a = first.is_point() && first.lo == 0.0;
        let exact_b = last.is_point() && last.lo == 0.0;
        let mid = range[0] * 0.5 + range[1] * 0.5;
        let inclusion = Interval::point(mid).sub(
            evaluate([mid; 2], work, cancel)?
                .value
                .div(complete_derivative)?,
        );
        let interior = inclusion.lo > range[0] && inclusion.hi < range[1];
        if exact_a || exact_b || (a != 0 && b != 0 && a != b) || interior {
            let mut root = if exact_a {
                Interval::point(range[0])
            } else if exact_b {
                Interval::point(range[1])
            } else if interior {
                inclusion
            } else {
                Interval::new(range[0], range[1])?
            };
            for _ in 0..96 {
                work.charge(1, cancel)?;
                if root.width() <= accuracy {
                    return admission.push(
                        roots,
                        Root {
                            parameter: root,
                            derivative: complete_derivative,
                        },
                    );
                }
                let mid = root.mid();
                let value = evaluate([mid; 2], work, cancel)?.value;
                let derivative = evaluate([root.lo, root.hi], work, cancel)?
                    .derivative
                    .intersect(complete_derivative)?;
                if sign(derivative) == 0 {
                    return Err(indeterminate(
                        "root refinement lost its complete derivative sign",
                    ));
                }
                let newton = Interval::point(mid).sub(value.div(derivative)?);
                let next = root.intersect(newton)?;
                if next == root {
                    break;
                }
                root = next;
            }
            return Err(indeterminate(
                "native root exists uniquely but could not be enclosed to the requested accuracy",
            ));
        }
    }
    if depth >= MAX_ROOT_DEPTH {
        return Err(indeterminate(format!(
            "parameter={range:?} value={:?} derivative={:?} endpoints={first:?},{last:?} depth={depth}: native plane root may be tangent, coplanar or multiple; complete isolation is unqualified",
            jet.value, jet.derivative,
        )));
    }
    let mid = range[0] * 0.5 + range[1] * 0.5;
    if mid <= range[0] || mid >= range[1] {
        return Err(indeterminate(
            "native root isolation exhausted representable chart subdivision",
        ));
    }
    visit(
        [range[0], mid],
        depth + 1,
        evaluate,
        accuracy,
        roots,
        admission,
        work,
        cancel,
    )?;
    visit(
        [mid, range[1]],
        depth + 1,
        evaluate,
        accuracy,
        roots,
        admission,
        work,
        cancel,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transverse_root_requires_existence_not_a_small_candidate_box() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000);
        let mut admission = Admission {
            limit: usize::MAX,
            used: 0,
        };
        let roots = isolate(
            std::iter::once([-1.0, 1.0]),
            &mut |r, _, _| {
                Ok(ScalarJet {
                    value: Interval::new(r[0], r[1])?,
                    derivative: Interval::point(1.0),
                })
            },
            1e-10,
            &mut admission,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(roots.len(), 1);
        assert!(roots[0].parameter.contains(0.0));
    }
    #[test]
    fn uncertain_dyadic_split_has_one_complete_monotone_root() {
        let cancel = AtomicBool::new(false);
        for root in [0.5, 0.75] {
            for slope in [-8.0, 8.0] {
                let mut work = GeometryWorkBudget::new(10_000);
                let mut admission = Admission {
                    limit: usize::MAX,
                    used: 0,
                };
                let roots = isolate(
                    std::iter::once([0.0, 1.0]),
                    &mut |range, _, _| {
                        let parameter = Interval::new(range[0], range[1])?;
                        Ok(ScalarJet {
                            // Valid point enclosures deliberately do not turn
                            // the dyadic root into a literal zero endpoint.
                            value: parameter
                                .sub(Interval::point(root))
                                .mul(Interval::point(slope))
                                .add(Interval::new(-1e-13, 1e-13)?),
                            // Each small piece proves the true same slope.
                            // A broad enclosure alone cannot prove monotonicity.
                            derivative: if range[1] - range[0] > 0.125 {
                                Interval::new(-16.0, 16.0)?
                            } else {
                                Interval::point(slope)
                            },
                        })
                    },
                    1e-10,
                    &mut admission,
                    &mut work,
                    &cancel,
                )
                .unwrap();
                assert_eq!(roots.len(), 1);
                assert!(roots[0].parameter.contains(root));
                assert!(roots[0].parameter.width() <= 1e-10);
                assert_eq!(sign(roots[0].derivative), if slope > 0.0 { 1 } else { -1 });
            }
        }
    }

    #[test]
    fn zero_or_mixed_derivatives_do_not_supply_complete_monotonicity() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000);
        let zero = strict_derivative_cover(
            [-1.0, 1.0],
            Interval::point(0.0),
            0,
            &mut |_, _, _| {
                Ok(ScalarJet {
                    value: Interval::point(0.0),
                    derivative: Interval::point(0.0),
                })
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(zero.is_none());
        // t³-t/4 has opposite outer signs but THREE roots. A positive
        // derivative on an initial subset cannot certify the whole parent.
        let mixed = strict_derivative_cover(
            [-1.0, 1.0],
            Interval::new(-0.25, 2.75).unwrap(),
            0,
            &mut |range, _, _| {
                let t = Interval::new(range[0], range[1])?;
                Ok(ScalarJet {
                    value: t.mul(t.square().sub(Interval::point(0.25))),
                    derivative: t
                        .square()
                        .mul(Interval::point(3.0))
                        .sub(Interval::point(0.25)),
                })
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(mixed.is_none());
    }

    #[test]
    fn optional_monotone_cover_preserves_resource_and_cancellation() {
        let cancel = AtomicBool::new(false);
        for code in [
            GeometryErrorCode::ResourceLimit,
            GeometryErrorCode::Cancelled,
        ] {
            let mut work = GeometryWorkBudget::new(10_000);
            let mut admission = Admission {
                limit: usize::MAX,
                used: 0,
            };
            let mut stopped = false;
            let failure = isolate(
                std::iter::once([0.0, 1.0]),
                &mut |range, _, _| {
                    assert!(
                        !stopped,
                        "a fatal native query must not fall back or evaluate again"
                    );
                    if range != [0.0, 1.0] && range[0] != range[1] {
                        stopped = true;
                        return Err(GeometryError::new(code, "native scalar query stopped"));
                    }
                    Ok(ScalarJet {
                        value: Interval::new(range[0], range[1])?
                            .sub(Interval::point(0.5))
                            .mul(Interval::point(8.0)),
                        derivative: Interval::new(-16.0, 16.0)?,
                    })
                },
                1e-10,
                &mut admission,
                &mut work,
                &cancel,
            )
            .unwrap_err();
            assert_eq!(failure.code, code);
            assert!(stopped);
        }
    }

    #[test]
    fn unresolved_tangent_is_not_an_empty_section() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000);
        let mut admission = Admission {
            limit: usize::MAX,
            used: 0,
        };
        let result = isolate(
            std::iter::once([-1.0, 1.0]),
            &mut |r, _, _| {
                let t = Interval::new(r[0], r[1])?;
                Ok(ScalarJet {
                    value: t.square(),
                    derivative: t.mul(Interval::point(2.0)),
                })
            },
            1e-10,
            &mut admission,
            &mut work,
            &cancel,
        );
        assert!(result.is_err());
    }
}
