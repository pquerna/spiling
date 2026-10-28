// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use solidcore::{
    curve::curve::Curve,
    nurbs::{
        b_spline_curve::BSplineCurve,
        crv_basic,
        math::{CPoint, NOW, NOZ},
        srf_basic,
    },
};

fn curve(points: Vec<CPoint>, degree: usize, knots: Vec<f64>, dim: u64) -> Curve {
    let raw = crv_basic::Curve::new(points, degree, knots).unwrap();
    // borrowed=true preserves the supplied original knot vector, including
    // intervals smaller than native fixup tolerances.
    BSplineCurve::from_nurb(raw, dim, true).into()
}
fn rational_line() -> Curve {
    curve(
        vec![
            CPoint::new(0.0, 0.0, 0.0, 2.0),
            CPoint::new(4.0, 0.0, 0.0, 4.0),
        ],
        1,
        vec![0.0, 0.0, 1.0, 1.0],
        3,
    )
}
#[test]
fn rational_curve_uses_homogeneous_weights_and_third_quotient_derivative() {
    let c = rational_line();
    let mut work = GeometryWorkBudget::new(1_000_000);
    let cancel = AtomicBool::new(false);
    let owner =
        PreparedValidatedCurve::new(&c, [0.0, 1.0], 10.0, usize::MAX, &mut work, &cancel).unwrap();
    let j = owner.third_jet([0.5; 2], &mut work, &cancel).unwrap();
    assert!(j.jet.position[0].contains(10.0 * 2.0 / 3.0));
    assert!(j.jet.derivative[0].contains(10.0 * 8.0 / 9.0));
    assert!(j.jet.second_derivative[0].contains(-10.0 * 32.0 / 27.0));
    assert!(j.third_derivative[0].contains(10.0 * 64.0 / 27.0));
    let point = owner.evaluate(0.5, &mut work, &cancel).unwrap();
    assert!(
        Interval::point(point.position[0])
            .add(point.error[0])
            .contains(10.0 * 2.0 / 3.0)
    );
    let small = owner
        .position_bounds([0.0, 0.125], &mut work, &cancel)
        .unwrap();
    assert!(
        small[0].hi < 4.0,
        "refinement must evaluate its own coefficients, not return a cached whole-span hull"
    );
}
#[test]
fn native_uv_curve_noz_marker_is_not_a_spatial_coordinate() {
    let c = curve(
        vec![
            CPoint::new(0.0, 1.0, NOZ, NOW),
            CPoint::new(2.0, 3.0, NOZ, NOW),
        ],
        1,
        vec![0.0, 0.0, 1.0, 1.0],
        2,
    );
    let mut work = GeometryWorkBudget::new(100_000);
    let cancel = AtomicBool::new(false);
    let j = curve2d_piecewise_jet(&c, [0.25, 0.5], &mut work, &cancel).unwrap();
    assert!(j.position[0].contains(0.5) && j.position[0].contains(1.0));
    assert!(j.position[1].contains(1.5) && j.position[1].contains(2.0));
    assert!(j.derivative[0].contains(2.0));
}
#[test]
fn complete_original_narrow_knot_partition_and_closed_c1_refusal() {
    let cut = 0.5;
    let narrow = up(cut);
    let c = curve(
        [0.0, 1.0, -10.0, 0.0]
            .map(|x| CPoint::new(x, 0.0, 0.0, NOW))
            .to_vec(),
        1,
        vec![0.0, 0.0, cut, narrow, 1.0, 1.0],
        3,
    );
    let mut work = GeometryWorkBudget::new(1_000_000);
    let cancel = AtomicBool::new(false);
    let spans: Vec<_> = curve_parameter_spans(&c, [0.0, 1.0], &mut work, &cancel)
        .unwrap()
        .collect();
    assert!(spans.contains(&[cut, narrow]));
    let whole = curve_enclosure(&c, (0.0, 1.0), &mut work, &cancel).unwrap();
    assert!(whole.position[0].contains(-10.0));
    assert!(!curve_parameter_span_smooth(&c, [0.25, cut], &mut work, &cancel).unwrap());
    let prepared =
        PreparedValidatedCurve::new(&c, [0.0, 1.0], 1.0, usize::MAX, &mut work, &cancel).unwrap();
    let narrow_jet = prepared.jet([cut, narrow], &mut work, &cancel).unwrap();
    assert!(narrow_jet.position[0].contains(1.0) && narrow_jet.position[0].contains(-10.0));
    let joint = prepared.jet([cut, cut], &mut work, &cancel).unwrap();
    assert!(
        joint.derivative[0].contains(2.0),
        "closed query retains the left span"
    );
    assert!(
        joint.derivative[0].lo < -1e15,
        "closed query retains the original one-ULP right span"
    );
    assert!(
        !prepared
            .is_smooth_on([0.25, cut], &mut work, &cancel)
            .unwrap()
    );
}
#[test]
fn rational_denominator_zero_is_not_sampled_or_clipped_away() {
    let c = curve(
        vec![
            CPoint::new(0.0, 0.0, 0.0, 1.0),
            CPoint::new(1.0, 0.0, 0.0, -1.0),
        ],
        1,
        vec![0.0, 0.0, 1.0, 1.0],
        3,
    );
    let mut work = GeometryWorkBudget::new(100_000);
    let cancel = AtomicBool::new(false);
    let failure = curve_enclosure(&c, (0.0, 1.0), &mut work, &cancel).unwrap_err();
    assert_eq!(failure.code, GeometryErrorCode::ConditioningFailure);
}
#[test]
fn active_domain_prepared_binding_and_bytes_are_not_overlap_tests() {
    let c = rational_line();
    let mut work = GeometryWorkBudget::new(100_000);
    let cancel = AtomicBool::new(false);
    let owner =
        PreparedValidatedCurve::new(&c, [0.25, 0.75], 1.0, usize::MAX, &mut work, &cancel).unwrap();
    assert_eq!(
        owner.jet([0.0, 0.5], &mut work, &cancel).unwrap_err().code,
        GeometryErrorCode::ConditioningFailure
    );
    assert_eq!(
        curve_enclosure(&c, (-0.1, 0.5), &mut work, &cancel)
            .unwrap_err()
            .code,
        GeometryErrorCode::ConditioningFailure
    );
    assert_eq!(
        PreparedValidatedCurve::new(&c, [0.0, 1.0], 1.0, 0, &mut work, &cancel)
            .unwrap_err()
            .code,
        GeometryErrorCode::ResourceLimit
    );
}
#[test]
fn native_surface_derivatives_enclose_rational_tensor_and_refuse_partial_domain() {
    let raw = srf_basic::Surface::new(
        vec![
            vec![
                CPoint::new(0.0, 0.0, 0.0, 2.0),
                CPoint::new(0.0, 2.0, 0.0, 2.0),
            ],
            vec![
                CPoint::new(4.0, 0.0, 0.0, 4.0),
                CPoint::new(4.0, 4.0, 0.0, 4.0),
            ],
        ],
        1,
        1,
        vec![0.0, 0.0, 1.0, 1.0],
        vec![0.0, 0.0, 1.0, 1.0],
    )
    .unwrap();
    let mut work = GeometryWorkBudget::new(1_000_000);
    let cancel = AtomicBool::new(false);
    let j = evaluate_nurbs_surface_jet(&raw, [[0.5; 2], [0.25; 2]], &mut work, &cancel).unwrap();
    assert!(j.position[0].contains(2.0 / 3.0));
    assert!(j.position[1].contains(0.25));
    assert!(j.du[0].contains(8.0 / 9.0));
    assert!(j.dv[1].contains(1.0));
    assert!(j.duu[0].contains(-32.0 / 27.0));
    assert!(j.duv[0].contains(0.0));
    assert_eq!(
        evaluate_nurbs_surface_jet(&raw, [[-0.1, 0.25], [0.0, 1.0]], &mut work, &cancel)
            .unwrap_err()
            .code,
        GeometryErrorCode::ConditioningFailure
    );
}
fn axis_invariant_surface_net() -> Vec<Vec<CPoint>> {
    vec![
        vec![
            CPoint::new(2.0, 3.0, 0.0, 1.0),
            CPoint::new(2.0, 3.0, 1.0, 1.0),
        ],
        vec![
            CPoint::new(8.0, 10.0, 0.0, 2.0),
            CPoint::new(8.0, 10.0, 2.0, 2.0),
        ],
    ]
}

fn bilinear_axis_surface(net: Vec<Vec<CPoint>>) -> srf_basic::Surface {
    srf_basic::Surface::new(
        net,
        1,
        1,
        vec![0.0, 0.0, 1.0, 1.0],
        vec![0.0, 0.0, 1.0, 1.0],
    )
    .unwrap()
}

#[test]
fn native_surface_axis_invariance_preserves_mixed_derivatives() {
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(100_000);
    for transpose in [false, true] {
        let mut net = axis_invariant_surface_net();
        if transpose {
            let off_diagonal = net[0][1];
            net[0][1] = net[1][0];
            net[1][0] = off_diagonal;
        }
        let raw = bilinear_axis_surface(net);
        for uv in [[[0.0, 1.0]; 2], [[0.5; 2]; 2]] {
            let jet =
                super::nurbs_eval::evaluate_nurbs_surface_tensor(&raw, uv, 3, &mut work, &cancel)
                    .unwrap();
            for a in 0..=3 {
                for b in 0..=3 - a {
                    if if transpose { a > 0 } else { b > 0 } {
                        assert_eq!(jet[a][b][0], Interval::point(0.0));
                        assert_eq!(jet[a][b][1], Interval::point(0.0));
                    }
                }
            }
            if uv[0][0] == uv[0][1] {
                let varying = if transpose {
                    jet[0][1][0]
                } else {
                    jet[1][0][0]
                };
                assert!(
                    varying.lo > 0.0,
                    "the other parameter must retain its real derivative"
                );
                assert!(varying.contains(16.0 / 9.0));
            }
        }
    }
}

#[test]
fn native_surface_axis_invariance_rejects_perturbed_controls() {
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(100_000);
    let mut net = axis_invariant_surface_net();
    net[1][1].x += 0.25;
    let jet = super::nurbs_eval::evaluate_nurbs_surface_tensor(
        &bilinear_axis_surface(net),
        [[0.5; 2]; 2],
        2,
        &mut work,
        &cancel,
    )
    .unwrap();
    assert!(jet[0][1][0].lo > 0.0 && jet[0][1][0].contains(1.0 / 12.0));
    assert!(jet[1][1][0].lo > 0.0 && jet[1][1][0].contains(1.0 / 9.0));
    assert_eq!(jet[0][1][1], Interval::point(0.0));
}

#[test]
fn native_surface_axis_invariance_keeps_varying_weight_terms() {
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(100_000);
    // Hx is literally constant, but W=(1+u)(1+v). Recentered Hx' therefore
    // retains -origin*W'; Cartesian x=2/W has nonzero pure/mixed derivatives.
    let net = vec![
        vec![
            CPoint::new(2.0, 0.0, 0.0, 1.0),
            CPoint::new(2.0, 0.0, 2.0, 2.0),
        ],
        vec![
            CPoint::new(2.0, 2.0, 0.0, 2.0),
            CPoint::new(2.0, 4.0, 4.0, 4.0),
        ],
    ];
    let jet = super::nurbs_eval::evaluate_nurbs_surface_tensor(
        &bilinear_axis_surface(net),
        [[0.5; 2]; 2],
        3,
        &mut work,
        &cancel,
    )
    .unwrap();
    assert!(jet[0][1][0].hi < 0.0 && jet[0][1][0].contains(-16.0 / 27.0));
    assert!(jet[1][1][0].lo > 0.0 && jet[1][1][0].contains(32.0 / 81.0));
    assert!(jet[0][2][0].lo > 0.0 && jet[0][2][0].contains(64.0 / 81.0));
    assert!(jet[1][2][0].hi < 0.0 && jet[1][2][0].contains(-128.0 / 243.0));
}

#[test]
fn native_surface_axis_invariance_hulls_both_closed_knot_sides() {
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(100_000);
    let raw = srf_basic::Surface::new(
        vec![
            vec![
                CPoint::new(2.0, 0.0, 0.0, NOW),
                CPoint::new(2.0, 0.0, 1.0, NOW),
                CPoint::new(2.0, 0.0, 2.0, NOW),
            ],
            vec![
                CPoint::new(4.0, 1.0, 0.0, NOW),
                CPoint::new(4.0, 1.0, 1.0, NOW),
                CPoint::new(5.0, 1.0, 2.0, NOW),
            ],
        ],
        1,
        1,
        vec![0.0, 0.0, 1.0, 1.0],
        vec![0.0, 0.0, 1.0, 2.0, 2.0],
    )
    .unwrap();
    let left = super::nurbs_eval::evaluate_nurbs_surface_tensor(
        &raw,
        [[0.5; 2], [0.25, 0.75]],
        2,
        &mut work,
        &cancel,
    )
    .unwrap();
    assert_eq!(left[0][1][0], Interval::point(0.0));
    let knot = super::nurbs_eval::evaluate_nurbs_surface_tensor(
        &raw,
        [[0.5; 2], [1.0; 2]],
        2,
        &mut work,
        &cancel,
    )
    .unwrap();
    assert!(knot[0][1][0].contains(0.0) && knot[0][1][0].contains(0.5));
    assert!(knot[1][1][0].contains(0.0) && knot[1][1][0].contains(1.0));
    assert!(
        !super::continuity::surface_is_c1(&raw, [[0.25, 0.75], [0.75, 1.25]], &mut work, &cancel,)
            .unwrap(),
        "coefficient annihilation must not grant C1 across a real derivative jump"
    );
}

#[test]
fn native_surface_axis_invariance_preserves_weight_and_work_refusals() {
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(100_000);
    for weights in [[[0.0; 2]; 2], [[1.0, -1.0]; 2]] {
        let net = weights
            .into_iter()
            .enumerate()
            .map(|(u, row)| {
                row.into_iter()
                    .enumerate()
                    .map(|(v, w)| CPoint::new(2.0, u as f64, v as f64, w))
                    .collect()
            })
            .collect();
        let failure = super::nurbs_eval::evaluate_nurbs_surface_tensor(
            &bilinear_axis_surface(net),
            [[0.0, 1.0]; 2],
            2,
            &mut work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::ConditioningFailure);
    }
    let raw = bilinear_axis_surface(axis_invariant_surface_net());
    assert_eq!(
        super::nurbs_eval::evaluate_nurbs_surface_tensor(
            &raw,
            [[0.0, 1.0]; 2],
            2,
            &mut GeometryWorkBudget::new(0),
            &cancel,
        )
        .unwrap_err()
        .code,
        GeometryErrorCode::ResourceLimit
    );
    assert_eq!(
        super::nurbs_eval::evaluate_nurbs_surface_tensor(
            &raw,
            [[0.0, 1.0]; 2],
            2,
            &mut work,
            &AtomicBool::new(true),
        )
        .unwrap_err()
        .code,
        GeometryErrorCode::Cancelled
    );
}

#[test]
fn cancellation_and_cumulative_failed_queries_do_not_reset_work() {
    let c = rational_line();
    let mut work = GeometryWorkBudget::new(10_000);
    assert_eq!(
        curve_enclosure(&c, (0.0, 1.0), &mut work, &AtomicBool::new(true))
            .unwrap_err()
            .code,
        GeometryErrorCode::Cancelled
    );
    assert!(work.consumed() > 0);
    let before = work.consumed();
    assert_eq!(
        curve_enclosure(&c, (-1.0, 1.0), &mut work, &AtomicBool::new(false))
            .unwrap_err()
            .code,
        GeometryErrorCode::ConditioningFailure
    );
    assert!(work.consumed() > before);
    let mut exhausted = GeometryWorkBudget::new(1);
    assert_eq!(
        curve_enclosure(&c, (0.0, 1.0), &mut exhausted, &AtomicBool::new(false))
            .unwrap_err()
            .code,
        GeometryErrorCode::ResourceLimit
    );
    assert_eq!(exhausted.remaining(), 0);
}

#[test]
fn rational_c1_is_an_exact_join_not_multiplicity_or_near_tangent() {
    let w = std::f64::consts::FRAC_1_SQRT_2;
    let controls = vec![
        CPoint::new(1.0, 0.0, 0.0, 1.0),
        CPoint::new(w, w, 0.0, w),
        CPoint::new(0.0, 1.0, 0.0, 1.0),
        CPoint::new(-w, w, 0.0, w),
        CPoint::new(-1.0, 0.0, 0.0, 1.0),
    ];
    let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 2.0];
    let c = curve(controls.clone(), 2, knots.clone(), 3);
    let mut work = GeometryWorkBudget::new(1_000_000);
    let cancel = AtomicBool::new(false);
    assert!(curve_parameter_span_smooth(&c, [0.5, 1.0], &mut work, &cancel).unwrap());
    let mut perturbed = controls.clone();
    perturbed[3].x = -up(w);
    let bad = curve(perturbed, 2, knots.clone(), 3);
    assert!(!curve_parameter_span_smooth(&bad, [0.5, 1.5], &mut work, &cancel).unwrap());
    let net = controls
        .into_iter()
        .map(|p| vec![p, CPoint::new(p.x, p.y, p.w, p.w)])
        .collect();
    let raw = srf_basic::Surface::new(net, 2, 1, knots, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
    assert!(
        super::continuity::surface_is_c1(&raw, [[0.5, 1.5], [0.0, 1.0]], &mut work, &cancel)
            .unwrap()
    );
}

#[test]
fn uv_domain_containment_uses_exact_homogeneous_inequalities() {
    let mut work = GeometryWorkBudget::new(100_000);
    let cancel = AtomicBool::new(false);
    let c = curve(
        vec![
            CPoint::new(0.0, 0.0, NOZ, 2.0),
            CPoint::new(4.0, 4.0, NOZ, 4.0),
        ],
        1,
        vec![0.0, 0.0, 1.0, 1.0],
        2,
    );
    assert!(curve2d_image_within_box(&c, [0.0, 1.0], [[0.0, 1.0]; 2], &mut work, &cancel).unwrap());
    let outside = curve(
        vec![
            CPoint::new(0.0, 0.0, NOZ, 2.0),
            CPoint::new(up(4.0), 4.0, NOZ, 4.0),
        ],
        1,
        vec![0.0, 0.0, 1.0, 1.0],
        2,
    );
    assert!(
        !curve2d_image_within_box(&outside, [0.0, 1.0], [[0.0, 1.0]; 2], &mut work, &cancel)
            .unwrap()
    );
    let jump = curve(
        vec![
            CPoint::new(-1.0, 0.0, 0.0, NOW),
            CPoint::new(-0.5, 0.0, 0.0, NOW),
            CPoint::new(0.5, 0.0, 0.0, NOW),
            CPoint::new(1.0, 0.0, 0.0, NOW),
        ],
        1,
        vec![0.0, 0.0, 0.5, 0.5, 1.0, 1.0],
        3,
    );
    assert!(!curve_parameter_span_continuous(&jump, [0.0, 1.0], &mut work, &cancel).unwrap());
}

#[test]
fn uv_containment_refines_the_whole_curve_not_its_untrimmed_control_hull() {
    let cancel = AtomicBool::new(false);
    for (control, contained) in [(1.2, true), (3.0, false)] {
        let c = curve(
            vec![
                CPoint::new(0.0, 0.5, NOZ, 1.0),
                CPoint::new(control, 0.5, NOZ, 1.0),
                CPoint::new(0.8, 0.5, NOZ, 1.0),
            ],
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            2,
        );
        let mut work = GeometryWorkBudget::new(100_000);
        // The first curve touches the exact lower chart boundary, but its
        // complete quadratic image stays in the unit chart despite an outlying control.
        // The second has a real interior excursion with the SAME endpoints.
        assert_eq!(
            curve2d_image_within_box(&c, [0.0, 1.0], [[0.0, 1.0]; 2], &mut work, &cancel).unwrap(),
            contained,
        );
    }
}

#[test]
fn scalar_functionals_keep_actual_coefficient_cancellation_before_division() {
    let raw = srf_basic::Surface::new(
        vec![
            vec![
                CPoint::new(1.0e12, 1.0e12, 0.0, 2.0),
                CPoint::new(2.0e12, 2.0e12, 0.0, 3.0),
            ],
            vec![
                CPoint::new(-1.0e12, -1.0e12, 0.0, 4.0),
                CPoint::new(3.0e12, 3.0e12, 0.0, 5.0),
            ],
        ],
        1,
        1,
        vec![0.0, 0.0, 1.0, 1.0],
        vec![0.0, 0.0, 1.0, 1.0],
    )
    .unwrap();
    let mut work = GeometryWorkBudget::new(100_000);
    let cancel = AtomicBool::new(false);
    let rows = [SurfaceFunctional {
        direction: [1.0, -1.0, 0.0].map(Interval::point),
        origin: [Interval::point(0.0); 3],
        offset: Interval::point(0.0),
    }];
    let [j] = super::nurbs_eval::evaluate_nurbs_surface_functionals(
        &raw,
        [[0.0, 1.0]; 2],
        &rows,
        &mut work,
        &cancel,
    )
    .unwrap();
    assert_eq!(j.position, Interval::point(0.0));
    assert_eq!(j.du, Interval::point(0.0));
    assert_eq!(j.dv, Interval::point(0.0));
}

#[test]
fn nan_parameter_cannot_be_normalized_into_an_admitted_endpoint() {
    let c = rational_line();
    let mut work = GeometryWorkBudget::new(100_000);
    let cancel = AtomicBool::new(false);
    assert_eq!(
        curve_enclosure(&c, (f64::NAN, 0.5), &mut work, &cancel)
            .unwrap_err()
            .code,
        GeometryErrorCode::ConditioningFailure
    );
    assert_eq!(
        PreparedValidatedCurve::new(&c, [0.5, f64::NAN], 1.0, usize::MAX, &mut work, &cancel)
            .unwrap_err()
            .code,
        GeometryErrorCode::ConditioningFailure
    );
}

#[test]
fn explicit_native_step_wrapper_retains_degrees_height_and_parameter_orientation() {
    use solidcore::{
        Point3, Vec3,
        surface::{
            cone::Cone,
            surface::{Surface, p1::StepSurface, p2::SurfaceKind},
        },
    };
    let cone = Cone::new_cylinder_bounded(
        Point3::new(2.0, 3.0, 4.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        5.0,
        0.0,
        180.0,
        10.0,
        true,
        true,
        false,
    );
    let native = Surface::new(SurfaceKind::Cone(Box::new(cone)));
    let wrapped = Surface::new(SurfaceKind::Step(StepSurface::new(native, true).unwrap()));
    let mut work = GeometryWorkBudget::new(100_000);
    let cancel = AtomicBool::new(false);
    let j = surface_enclosure(&wrapped, [[90.0; 2], [3.0; 2]], &mut work, &cancel).unwrap();
    for (got, expected) in j.position.into_iter().zip([2.0, 8.0, 7.0]) {
        assert!(got.contains(expected));
    }
    assert!(j.du[0].contains(-5.0 * std::f64::consts::PI / 180.0));
    assert!(j.dv[2].contains(1.0));
    assert!(j.duu[1].contains(-5.0 * (std::f64::consts::PI / 180.0).powi(2)));
    // inside_out/swap_uv describe the *NURBS* map and must not reverse this
    // explicitly bound original STEP chart, nor apply owning face orientation.
    assert!(j.du[0].hi < 0.0 && j.dv[2].lo > 0.0);
}

#[test]
fn nonlinear_surface_composition_preserves_negative_affine_map_through_third_order() {
    use solidcore::{
        nurbs::b_spline_surface::BSplineSurface,
        surface::surface::{Surface, p2::SurfaceKind},
    };
    let net = [0.0, 0.0, 1.0]
        .map(|u2| {
            vec![
                CPoint::new(u2, 0.0, 0.0, NOW),
                CPoint::new(u2, 1.0, 0.0, NOW),
            ]
        })
        .to_vec();
    let raw = srf_basic::Surface::new(
        net,
        2,
        1,
        vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        vec![0.0, 0.0, 1.0, 1.0],
    )
    .unwrap();
    let surface = Surface::new(SurfaceKind::BSpline(BSplineSurface::from_nurb(raw, true)));
    let chart = rational_line();
    let mut work = GeometryWorkBudget::new(1_000_000);
    let cancel = AtomicBool::new(false);
    let j = curve_on_surface_enclosure(&surface, &chart, [0.5; 2], [1.0, -1.0], &mut work, &cancel)
        .unwrap();
    assert!(j.jet.position[0].contains(4.0 / 9.0));
    assert!(j.jet.derivative[0].contains(-32.0 / 27.0));
    assert!(j.jet.second_derivative[0].contains(0.0));
    assert!(j.third_derivative[0].contains(256.0 / 81.0));
    assert!(
        curve_on_surface_span_smooth(
            &surface,
            &chart,
            [0.25, 0.75],
            [1.0, -1.0],
            &mut work,
            &cancel
        )
        .unwrap()
    );
    assert_eq!(
        curve_on_surface_enclosure(&surface, &chart, [0.0, 1.0], [2.0, 1.0], &mut work, &cancel)
            .unwrap_err()
            .code,
        GeometryErrorCode::ConditioningFailure
    );
}

#[test]
fn repeated_knot_actual_homogeneous_c0_does_not_grant_c1() {
    use solidcore::{
        nurbs::b_spline_surface::BSplineSurface,
        surface::surface::{Surface, p2::SurfaceKind},
    };
    let points = vec![
        CPoint::new(0.0, 0.0, 0.0, 1.0),
        CPoint::new(1.0, 0.0, 0.0, 2.0),
        CPoint::new(2.0, 0.0, 0.0, 4.0),
        CPoint::new(4.0, 0.0, 0.0, 4.0),
    ];
    let knots = vec![0.0, 0.0, 0.5, 0.5, 1.0, 1.0];
    let c = curve(points.clone(), 1, knots.clone(), 3);
    let mut work = GeometryWorkBudget::new(1_000_000);
    let cancel = AtomicBool::new(false);
    assert!(curve_parameter_span_continuous(&c, [0.0, 1.0], &mut work, &cancel).unwrap());
    assert!(!curve_parameter_span_smooth(&c, [0.0, 1.0], &mut work, &cancel).unwrap());
    let net = points
        .into_iter()
        .map(|p| vec![p, CPoint::new(p.x, p.w, 0.0, p.w)])
        .collect();
    let raw = srf_basic::Surface::new(net, 1, 1, knots, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
    let s = Surface::new(SurfaceKind::BSpline(BSplineSurface::from_nurb(raw, true)));
    assert!(surface_parameter_span_continuous(&s, [[0.0, 1.0]; 2], &mut work, &cancel).unwrap());
    assert!(!surface_parameter_span_smooth(&s, [[0.0, 1.0]; 2], &mut work, &cancel).unwrap());
    let j = surface_piecewise_first_enclosure(&s, [[0.0, 1.0]; 2], &mut work, &cancel).unwrap();
    assert!(j.position[0].contains(0.5));
}

#[test]
fn explicit_step_sphere_has_a_coefficient_proved_pole_normal_not_a_nearby_sample() {
    use solidcore::{
        geom::axis2_placement::Axis2Placement,
        surface::{
            sphere::Sphere,
            surface::{Surface, p1::StepSurface, p2::SurfaceKind},
        },
    };
    let kernel = crate::make_kernel_context();
    let sphere = Sphere::create_canonical(&kernel, &Axis2Placement::IDENTITY, 2.0).unwrap();
    let native = Surface::new(SurfaceKind::Sphere(Box::new(sphere)));
    let mut work = GeometryWorkBudget::new(1_000_000);
    let cancel = AtomicBool::new(false);
    let domain = native.natural_uv_domain();
    let corner = domain.min();
    assert_eq!(
        surface_geometric_normal_enclosure(
            &native,
            [[corner.x; 2], [corner.y; 2]],
            &mut work,
            &cancel
        )
        .unwrap_err()
        .code,
        GeometryErrorCode::SingularGeometry,
        "stored NURBS sphere must not borrow an analytic implicit normal"
    );
    let wrapped = Surface::new(SurfaceKind::Step(StepSurface::new(native, true).unwrap()));
    let north =
        surface_geometric_normal_enclosure(&wrapped, [[0.0, 360.0], [90.0; 2]], &mut work, &cancel)
            .unwrap();
    assert!(north[0].contains(0.0) && north[1].contains(0.0) && north[2].contains(1.0));
    assert!(north[2].lo > 0.99);
    let south = surface_geometric_normal_enclosure(
        &wrapped,
        [[0.0, 360.0], [-90.0; 2]],
        &mut work,
        &cancel,
    )
    .unwrap();
    assert!(south[2].contains(-1.0) && south[2].hi < -0.99);
}

#[test]
fn endpoint_coordinate_identity_does_not_promote_a_nearby_dyadic() {
    let c = rational_line();
    let mut work = GeometryWorkBudget::new(100_000);
    let cancel = AtomicBool::new(false);
    assert!(curve_endpoint_coordinate_equal(&c, 1.0, 0, 1.0, &mut work, &cancel).unwrap());
    assert!(!curve_endpoint_coordinate_equal(&c, 1.0, 0, up(1.0), &mut work, &cancel).unwrap());
    assert!(curve_endpoint_coordinate_equal(&c, 0.5, 1, 0.0, &mut work, &cancel).unwrap());
    assert!(!curve_endpoint_coordinate_equal(&c, 0.5, 0, 2.0 / 3.0, &mut work, &cancel).unwrap());
}

#[test]
fn subnormal_homogeneous_ratios_do_not_overflow_an_unneeded_reciprocal() {
    let w = 1.0e-320;
    let c = curve(
        vec![
            CPoint::new(0.0, 0.0, 0.0, w),
            CPoint::new(2.0 * w, 0.0, 0.0, 2.0 * w),
        ],
        1,
        vec![0.0, 0.0, 1.0, 1.0],
        3,
    );
    let mut work = GeometryWorkBudget::new(100_000);
    let cancel = AtomicBool::new(false);
    let j = curve_enclosure(&c, (0.5, 0.5), &mut work, &cancel).unwrap();
    assert!(j.position[0].contains(2.0 / 3.0));
    assert!(j.derivative[0].contains(8.0 / 9.0));
    assert!(
        j.position
            .iter()
            .chain(j.derivative.iter())
            .all(|v| v.lo.is_finite() && v.hi.is_finite())
    );
}

#[test]
fn shared_c1_knot_has_one_transverse_root_receipt() {
    use solidcore::{
        Brep, Point2, Point3,
        geom::{extent1d::Extent1d, extent2d::Extent2d},
        nurbs::b_spline_surface::BSplineSurface,
        surface::surface::{Surface, p2::SurfaceKind},
        topology::{Edge, Face, Shell, Vertex},
    };
    let knots = vec![-1.0, -1.0, 0.0, 1.0, 1.0];
    let spatial = curve(
        [-1.0, 0.0, 1.0]
            .map(|t| CPoint::new(0.0, t, 0.0, NOW))
            .to_vec(),
        1,
        knots.clone(),
        3,
    );
    let chart = curve(
        [-1.0, 0.0, 1.0]
            .map(|t| CPoint::new(0.0, t, NOZ, NOW))
            .to_vec(),
        1,
        knots,
        2,
    );
    let raw = srf_basic::Surface::new(
        vec![
            vec![
                CPoint::new(0.0, -1.0, 0.0, NOW),
                CPoint::new(0.0, 1.0, 0.0, NOW),
            ],
            vec![
                CPoint::new(1.0, -1.0, 0.0, NOW),
                CPoint::new(1.0, 1.0, 0.0, NOW),
            ],
        ],
        1,
        1,
        vec![0.0, 0.0, 1.0, 1.0],
        vec![-1.0, -1.0, 1.0, 1.0],
    )
    .unwrap();
    let surface = Surface::new(SurfaceKind::BSpline(BSplineSurface::from_nurb(raw, true)));
    let mut brep = Brep::new();
    let shell = brep.add_shell_record(Shell::default()).unwrap();
    let mut face = Face::new(shell);
    face.surface = Some(surface);
    face.uv_domain = Extent2d::from_points(Point2::new(0.0, -1.0), Point2::new(1.0, 1.0));
    let face = brep.add_face_record(face).unwrap();
    let a = brep
        .add_vertex_record(Vertex {
            point: Point3::new(0.0, -1.0, 0.0),
            ..Vertex::default()
        })
        .unwrap();
    let z = brep
        .add_vertex_record(Vertex {
            point: Point3::new(0.0, 1.0, 0.0),
            ..Vertex::default()
        })
        .unwrap();
    let edge = brep
        .add_edge_record(Edge {
            vertices: [a, z],
            edgeuses: Vec::new(),
            interval: Extent1d::from_min_max(-1.0, 1.0),
            tolerance: 0.0,
            curve: Some(spatial),
        })
        .unwrap();
    let mut work = GeometryWorkBudget::new(5_000_000);
    let cancel = AtomicBool::new(false);
    let spatial = PreparedValidatedCurve::new(
        brep.edge(edge).unwrap().curve.as_ref().unwrap(),
        [-1.0, 1.0],
        1.0,
        usize::MAX,
        &mut work,
        &cancel,
    )
    .unwrap();
    let chart =
        PreparedValidatedCurve::new(&chart, [-1.0, 1.0], 1.0, usize::MAX, &mut work, &cancel)
            .unwrap();
    let bindings = [PreparedBoundaryBinding {
        coedge_index: 0,
        original_edge_id: edge,
        curve: &spatial,
        chart: &chart,
        parameter_range: [-1.0, 1.0],
        chart_parameter_map: [0.0, 1.0],
    }];
    let roots = PreparedFacePlaneRoots::new(
        face,
        brep.face(face).unwrap().surface.as_ref().unwrap(),
        true,
        [[0.0, 1.0], [-1.0, 1.0]],
        &crate::PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 1.0, 0.0],
        },
        &bindings,
        32 * 1024 * 1024,
        &mut work,
        &cancel,
    )
    .unwrap();
    assert_eq!(roots.relation(), FacePlaneRelation::Transverse);
    let branch = roots.boundary_roots[0].as_ref().unwrap();
    assert_eq!(
        branch.roots.len(),
        1,
        "adjacent original spans must not duplicate one geometric intersection"
    );
    assert_eq!(branch.roots[0].parameter_range, [0.0; 2]);
    assert!(
        roots
            .root_chart_for_cut(0, 0.25, &mut work, &cancel)
            .is_err(),
        "an unproved root chart must not be fabricated"
    );
}

#[test]
fn uncertain_knot_cover_preserves_the_whole_band_and_validates_coverage() {
    let lo = f64::from_bits(0.5_f64.to_bits() - 1);
    let hi = f64::from_bits(0.5_f64.to_bits() + 1);
    let cuts = [0.0, lo, hi, 1.0];
    let mut work = GeometryWorkBudget::new(1_000);
    let cancel = AtomicBool::new(false);
    let cover: Vec<_> = AxisSpans::from_cover_cuts(&cuts, [0.0, 1.0], &mut work, &cancel)
        .unwrap()
        .collect();
    assert_eq!(cover, vec![[0.0, lo], [lo, hi], [hi, 1.0]]);
    let reversed: Vec<_> = AxisSpans::from_cover_cuts(&cuts, [0.0, 1.0], &mut work, &cancel)
        .unwrap()
        .rev()
        .collect();
    assert_eq!(reversed, cover.into_iter().rev().collect::<Vec<_>>());
    assert!(
        AxisSpans::from_cover_cuts(&cuts, [-f64::MIN_POSITIVE, 1.0], &mut work, &cancel).is_err()
    );
    assert!(
        AxisSpans::from_cover_cuts(&[0.0, hi, lo, 1.0], [0.0, 1.0], &mut work, &cancel).is_err()
    );
    assert!(
        AxisSpans::from_cover_cuts(&[0.0, f64::NAN, 1.0], [0.0, 1.0], &mut work, &cancel).is_err()
    );
    assert!(
        AxisSpans::from_cover_cuts(&cuts, [0.0, 1.0], &mut work, &AtomicBool::new(true)).is_err()
    );
    let mut exhausted = GeometryWorkBudget::new(1);
    assert!(AxisSpans::from_cover_cuts(&cuts, [0.0, 1.0], &mut exhausted, &cancel).is_err());
    assert_eq!(
        AxisSpans::from_cover_cuts(&[0.5, 0.5], [0.5; 2], &mut work, &cancel)
            .unwrap()
            .collect::<Vec<_>>(),
        vec![[0.5; 2]]
    );
}
