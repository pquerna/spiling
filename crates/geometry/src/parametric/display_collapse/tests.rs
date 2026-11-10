// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use solidcore::{
    nurbs::{b_spline_surface::BSplineSurface, math::CPoint, srf_basic},
    surface::surface::p2::SurfaceKind,
};

fn raw(upper: bool, tilt: f64) -> srf_basic::Surface {
    let controls = [1.0, 2.0, 4.0]
        .into_iter()
        .enumerate()
        .map(|(column, weight)| {
            let horizontal = column as f64 * 0.5;
            let mut rows = vec![
                CPoint::new(2.0 * weight, 2.0 * weight, 0.0, weight),
                CPoint::new(
                    (2.0 + 0.5 * horizontal) * weight,
                    2.5 * weight,
                    tilt * weight,
                    weight,
                ),
                CPoint::new((2.0 + horizontal) * weight, 3.0 * weight, weight, weight),
            ];
            if upper {
                rows.reverse();
            }
            rows
        })
        .collect();
    let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    srf_basic::Surface::new(controls, 2, 2, knots.clone(), knots).unwrap()
}

fn surface(raw: srf_basic::Surface) -> Surface {
    Surface::new(SurfaceKind::BSpline(BSplineSurface::from_nurb(raw, true)))
}

#[test]
fn exact_weighted_row_retains_lower_and_upper_one_sided_normals() {
    let cancel = AtomicBool::new(false);
    for upper in [false, true] {
        let source = surface(raw(upper, 0.0));
        let v = if upper { 1.0 } else { 0.0 };
        let mut work = GeometryWorkBudget::query();
        let position = collapsed_row_position(&source, v, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(position[0].contains(2.0) && position[1].contains(2.0));
        assert_eq!(position[2], Interval::point(0.0));
        let normal = collapsed_row_normal(&source, [0.0, 1.0], v, &mut work, &cancel)
            .unwrap_or_else(|error| panic!("upper={upper}: {error:?}"));
        assert!(normal[0].contains(0.0) && normal[1].contains(0.0));
        assert!(normal[2].contains(if upper { -1.0 } else { 1.0 }));
        assert!(normal[2].abs_lower() > 0.9999);
        assert!(
            collapsed_row_position(&source, 1.0 - v, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
        assert!(
            collapsed_row_position(&source, 0.5, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn tiny_adjacent_coefficients_remain_in_the_normal_limit() {
    let tilt = 2.22044604925e-16;
    let source = surface(raw(false, tilt));
    let normal = collapsed_row_normal(
        &source,
        [0.0, 1.0],
        0.0,
        &mut GeometryWorkBudget::query(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(normal[1].contains(-2.0 * tilt), "{normal:?}");
    assert!(normal[2].lo > 0.9999, "{normal:?}");
}

#[test]
fn complete_collapsed_strips_require_nonvanishing_inward_normal_coefficients() {
    let cancel = AtomicBool::new(false);
    for upper in [false, true] {
        let mut coefficients = raw(upper, 0.0);
        for column in 0..3 {
            for row in 0..3 {
                let inward = if upper { 2 - row } else { row } as f64 * 0.5;
                *coefficients.cpt_mut(column, row).unwrap() =
                    CPoint::new(column as f64 * 0.5 * inward, inward, 0.0, 1.0);
            }
        }
        let source = surface(coefficients.clone());
        let domain = [[0.49, 0.51], if upper { [0.99, 1.0] } else { [0.0, 0.01] }];
        assert!(strip_regular(&source, domain, &mut GeometryWorkBudget::query(), &cancel).unwrap());
        assert!(
            !strip_regular(
                &source,
                [[0.0, 1.0], [0.25, 0.5]],
                &mut GeometryWorkBudget::query(),
                &cancel
            )
            .unwrap()
        );
        for column in 0..3 {
            for row in 0..3 {
                coefficients.cpt_mut(column, row).unwrap().y = 0.0;
            }
        }
        assert!(
            !strip_regular(
                &surface(coefficients),
                domain,
                &mut GeometryWorkBudget::query(),
                &cancel
            )
            .unwrap()
        );
    }
}

#[test]
fn noncollapsed_and_one_ulp_perturbed_rows_have_no_quotient_authority() {
    let cancel = AtomicBool::new(false);
    for perturbation in [0.125, super::super::up(8.0) - 8.0] {
        let mut source = raw(false, 0.0);
        source.cpt_mut(2, 0).unwrap().x += perturbation;
        let source = surface(source);
        assert!(
            collapsed_row_position(&source, 0.0, &mut GeometryWorkBudget::query(), &cancel)
                .unwrap()
                .is_none()
        );
        assert!(
            collapsed_row_normal(
                &source,
                [0.0, 1.0],
                0.0,
                &mut GeometryWorkBudget::query(),
                &cancel,
            )
            .is_err()
        );
    }
}

#[test]
fn rounded_cartesian_equality_is_not_an_exact_rational_identity() {
    let mut source = raw(false, 0.0);
    let numerator = super::super::up(1.0);
    let weight = super::super::up(10.0);
    assert_eq!(1.0 / 10.0, numerator / weight);
    *source.cpt_mut(0, 0).unwrap() = CPoint::new(1.0, 0.0, 0.0, 10.0);
    *source.cpt_mut(1, 0).unwrap() = CPoint::new(numerator, 0.0, 0.0, weight);
    *source.cpt_mut(2, 0).unwrap() = CPoint::new(1.0, 0.0, 0.0, 10.0);
    assert!(
        collapsed_row_position(
            &surface(source),
            0.0,
            &mut GeometryWorkBudget::query(),
            &AtomicBool::new(false),
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn mixed_or_zero_row_weights_do_not_prove_a_defined_collapsed_image() {
    for weight in [0.0, -2.0] {
        let mut source = raw(false, 0.0);
        *source.cpt_mut(1, 0).unwrap() = CPoint::new(2.0 * weight, 2.0 * weight, 0.0, weight);
        assert!(
            collapsed_row_position(
                &surface(source),
                0.0,
                &mut GeometryWorkBudget::query(),
                &AtomicBool::new(false),
            )
            .unwrap()
            .is_none()
        );
    }
}

#[test]
fn singular_first_normal_coefficient_never_uses_a_nearby_normal() {
    for higher_order in [false, true] {
        let mut source = raw(false, 0.0);
        for column in 0..3 {
            let pole = source.cpt(column, 0).unwrap();
            for row in 1..3 {
                let point = source.cpt_mut(column, row).unwrap();
                if higher_order && row == 1 {
                    *point = pole;
                } else if higher_order {
                    point.z = 0.0;
                } else {
                    point.y = 2.0 * point.w;
                    point.z = 0.0;
                }
            }
        }
        let source = surface(source);
        assert!(
            collapsed_row_position(
                &source,
                0.0,
                &mut GeometryWorkBudget::query(),
                &AtomicBool::new(false),
            )
            .unwrap()
            .is_some()
        );
        let error = collapsed_row_normal(
            &source,
            [0.0, 1.0],
            0.0,
            &mut GeometryWorkBudget::query(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate);
    }
}

#[test]
fn collapsed_row_proofs_preserve_terminal_work_and_cancellation() {
    let source = surface(raw(false, 0.0));
    for (limit, cancelled, code) in [
        (0, false, GeometryErrorCode::ResourceLimit),
        (1_000_000, true, GeometryErrorCode::Cancelled),
    ] {
        let cancel = AtomicBool::new(cancelled);
        let error =
            collapsed_row_position(&source, 0.0, &mut GeometryWorkBudget::new(limit), &cancel)
                .unwrap_err();
        assert_eq!(error.code, code);
        let error = collapsed_row_normal(
            &source,
            [0.0, 1.0],
            0.0,
            &mut GeometryWorkBudget::new(limit),
            &cancel,
        )
        .unwrap_err();
        assert_eq!(error.code, code);
    }
}

#[test]
fn collapsed_control_row_without_clamped_endpoint_is_not_authority() {
    let source = raw(false, 0.0);
    let controls = (0..3)
        .map(|column| (0..3).map(|row| source.cpt(column, row).unwrap()).collect())
        .collect();
    let source = surface(
        srf_basic::Surface::new(
            controls,
            2,
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            vec![0.0, 0.0, 0.25, 0.75, 1.0, 1.0],
        )
        .unwrap(),
    );
    let v = source.natural_uv_domain_if_built().v_min();
    assert!(
        collapsed_row_position(
            &source,
            v,
            &mut GeometryWorkBudget::query(),
            &AtomicBool::new(false),
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn normal_coefficient_vanishing_inside_the_row_refuses_complete_ownership() {
    let mut source = raw(false, 0.0);
    for (column, offset) in [0.25, -0.25, 0.25].into_iter().enumerate() {
        for row in 0..3 {
            let height = row as f64 * 0.5;
            *source.cpt_mut(column, row).unwrap() =
                CPoint::new(2.0 + height * offset, 2.0 + height, 0.0, 1.0);
        }
    }
    let source = surface(source);
    let cancel = AtomicBool::new(false);
    assert!(
        collapsed_row_position(&source, 0.0, &mut GeometryWorkBudget::query(), &cancel)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        collapsed_row_normal(
            &source,
            [0.0, 1.0],
            0.0,
            &mut GeometryWorkBudget::query(),
            &cancel,
        )
        .unwrap_err()
        .code,
        GeometryErrorCode::AnalysisIndeterminate,
    );
}
