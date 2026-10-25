// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::{Admission, indeterminate};
use crate::{GeometryError, GeometryWorkBudget, parametric::Interval};
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, Debug)]
pub(super) struct Root { pub parameter: Interval, pub derivative: Interval }
#[derive(Clone, Copy)]
pub(super) struct ScalarJet { pub value: Interval, pub derivative: Interval }
pub(super) fn sign(x: Interval) -> i8 { if x.lo > 0.0 { 1 } else if x.hi < 0.0 { -1 } else { 0 } }

/// Every initial knot span is exhausted by exclusion or a monotone root proof.
/// A subdivision threshold is only a refusal boundary, never root authority.
pub(super) fn isolate(
    spans: impl Iterator<Item = [f64; 2]>,
    evaluate: &mut impl FnMut([f64; 2], &mut GeometryWorkBudget, &AtomicBool) -> Result<ScalarJet, GeometryError>,
    parameter_accuracy: f64, admission: &mut Admission,
    work: &mut GeometryWorkBudget, cancel: &AtomicBool,
) -> Result<Vec<Root>, GeometryError> {
    let mut roots = Vec::new();
    for span in spans {
        visit(span, 0, evaluate, parameter_accuracy, &mut roots, admission, work, cancel)?;
    }
    roots.sort_by(|a, b| a.parameter.lo.total_cmp(&b.parameter.lo));
    let mut keep = 0;
    for i in 0..roots.len() {
        let root = roots[i];
        if keep > 0 && roots[keep - 1].parameter.intersection(root.parameter).is_some() {
            // Both intervals already prove one root. Only an exact shared knot
            // is a proved duplicate; overlapping candidate boxes are not enough.
            let old = roots[keep - 1].parameter;
            if old.is_point() && root.parameter.is_point() && old == root.parameter { continue; }
            return Err(indeterminate("native root isolators overlap without an exact shared-knot identity"));
        }
        roots[keep] = root;
        keep += 1;
    }
    roots.truncate(keep);
    Ok(roots)
}
fn visit(
    range: [f64; 2], depth: u8,
    evaluate: &mut impl FnMut([f64; 2], &mut GeometryWorkBudget, &AtomicBool) -> Result<ScalarJet, GeometryError>,
    accuracy: f64, roots: &mut Vec<Root>, admission: &mut Admission,
    work: &mut GeometryWorkBudget, cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    let jet = evaluate(range, work, cancel)?;
    if sign(jet.value) != 0 { return Ok(()); }
    let first = evaluate([range[0]; 2], work, cancel)?.value;
    let last = evaluate([range[1]; 2], work, cancel)?.value;
    let ds = sign(jet.derivative);
    if ds != 0 {
        let a = sign(first);
        let b = sign(last);
        if a != 0 && a == b { return Ok(()); }
        let exact_a = first.is_point() && first.lo == 0.0;
        let exact_b = last.is_point() && last.lo == 0.0;
        let mid = range[0] * 0.5 + range[1] * 0.5;
        let inclusion = Interval::point(mid).sub(evaluate([mid; 2], work, cancel)?.value.div(jet.derivative)?);
        let interior = inclusion.lo > range[0] && inclusion.hi < range[1];
        if exact_a || exact_b || (a != 0 && b != 0 && a != b) || interior {
            let mut root = if exact_a { Interval::point(range[0]) }
                else if exact_b { Interval::point(range[1]) }
                else if interior { inclusion }
                else { Interval::new(range[0], range[1])? };
            for _ in 0..96 {
                work.charge(1, cancel)?;
                if root.width() <= accuracy {
                    return admission.push(roots, Root { parameter: root, derivative: jet.derivative });
                }
                let mid = root.mid();
                let value = evaluate([mid; 2], work, cancel)?.value;
                let derivative = evaluate([root.lo, root.hi], work, cancel)?.derivative;
                if sign(derivative) == 0 { return Err(indeterminate("root refinement lost its complete derivative sign")); }
                let newton = Interval::point(mid).sub(value.div(derivative)?);
                let next = root.intersect(newton)?;
                if next == root { break; }
                root = next;
            }
            return Err(indeterminate("native root exists uniquely but could not be enclosed to the requested accuracy"));
        }
    }
    if depth >= 40 {
        return Err(indeterminate("native plane root may be tangent, coplanar or multiple; complete isolation is unqualified"));
    }
    let mid = range[0] * 0.5 + range[1] * 0.5;
    if mid <= range[0] || mid >= range[1] {
        return Err(indeterminate("native root isolation exhausted representable chart subdivision"));
    }
    visit([range[0], mid], depth + 1, evaluate, accuracy, roots, admission, work, cancel)?;
    visit([mid, range[1]], depth + 1, evaluate, accuracy, roots, admission, work, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transverse_root_requires_existence_not_a_small_candidate_box() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000);
        let mut admission = Admission { limit: usize::MAX, used: 0 };
        let roots = isolate(std::iter::once([-1.0, 1.0]), &mut |r, _, _| Ok(ScalarJet {
            value: Interval::new(r[0], r[1])?, derivative: Interval::point(1.0),
        }), 1e-10, &mut admission, &mut work, &cancel).unwrap();
        assert_eq!(roots.len(), 1);
        assert!(roots[0].parameter.contains(0.0));
    }
    #[test]
    fn unresolved_tangent_is_not_an_empty_section() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000);
        let mut admission = Admission { limit: usize::MAX, used: 0 };
        let result = isolate(std::iter::once([-1.0, 1.0]), &mut |r, _, _| {
            let t = Interval::new(r[0], r[1])?;
            Ok(ScalarJet { value: t.square(), derivative: t.mul(Interval::point(2.0)) })
        }, 1e-10, &mut admission, &mut work, &cancel);
        assert!(result.is_err());
    }
}
