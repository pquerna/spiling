// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{Interval, norm_upper};
use crate::{GeometryError, GeometryWorkBudget, PlaneFrameMm};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

/// Encloses the projected Green integral of the caller's original native arc.
///
/// The caller must prove C1 continuity and bounded piecewise second derivatives
/// over the entire raw parameter span, and bind both true endpoint enclosures to
/// that same arc, gauge and millimetre frame. Endpoint order supplies orientation.
///
/// Write the projected curve as its true linear chord `l` plus `delta`. The
/// zero-endpoint interpolation kernel gives `|delta| <= M*h^2/8`; subtracting
/// the mean derivative gives `|delta'| <= M*h/2`. These bounds also hold across
/// piecewise C2 joins because C1 excludes derivative jumps. Integration by parts
/// (the endpoint delta terms vanish) gives
/// `A(C)-A(l) = integral(delta cross l') + integral(delta cross delta')/2`.
/// Thus the absolute remainder is at most `M*h^2*L/8 + M^2*h^4/32`.
pub(super) fn signed_area_from_curvature(
    frame: &PlaneFrameMm,
    reference_mm: [f64; 3],
    endpoints: &[[Interval; 3]; 2],
    second_derivative: &[Interval; 3],
    range: (f64, f64),
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[f64; 2], GeometryError> {
    work.charge(1, cancel)?;
    frame.validate()?;
    if !reference_mm.iter().all(|value| value.is_finite())
        || !range.0.is_finite()
        || !range.1.is_finite()
        || range.0 == range.1
    {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "invalid native Green integral reference or parameter span",
        ));
    }
    for value in endpoints.iter().flatten().chain(second_derivative.iter()) {
        Interval::new(value.lo, value.hi)?;
    }

    let project = |vector: &[Interval; 3], axis: [f64; 3], position: bool| {
        (0..3).fold(Interval::point(0.0), |sum, index| {
            let value = if position {
                vector[index].sub(Interval::point(reference_mm[index]))
            } else {
                vector[index]
            };
            sum.add(value.mul(Interval::point(axis[index])))
        })
    };
    let projected = endpoints.map(|point| {
        [
            project(&point, frame.x_axis, true),
            project(&point, frame.y_axis, true),
        ]
    });
    let chord_area = projected[0][0]
        .mul(projected[1][1])
        .sub(projected[0][1].mul(projected[1][0]))
        .mul(Interval::point(0.5));
    let chord_length = norm_upper([
        projected[1][0].sub(projected[0][0]),
        projected[1][1].sub(projected[0][1]),
        Interval::point(0.0),
    ])?;
    // Use the actual stored axis coefficients: frame validation allows small
    // orthonormality residuals, so an uninflated world norm need not dominate.
    let curvature = norm_upper([
        project(second_derivative, frame.x_axis, false),
        project(second_derivative, frame.y_axis, false),
        Interval::point(0.0),
    ])?;
    let span = Interval::point(range.1).sub(Interval::point(range.0));
    Interval::new(span.lo, span.hi)?;
    // Squaring encloses |delta t|^2 for either orientation. Form B first instead
    // of separately squaring M or taking h^4, which can overflow unnecessarily.
    let bending = Interval::point(curvature).mul(span.square());
    let remainder = bending
        .mul(Interval::point(chord_length))
        .mul(Interval::point(0.125))
        .add(bending.square().mul(Interval::point(0.03125)));
    Interval::new(remainder.lo, remainder.hi)?;
    let area = chord_area.add(Interval {
        lo: -remainder.hi,
        hi: remainder.hi,
    });
    Interval::new(area.lo, area.hi)?;
    Ok([area.lo, area.hi])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(posed: bool) -> PlaneFrameMm {
        if posed {
            PlaneFrameMm {
                origin_mm: [7.0, -5.0, 11.0],
                x_axis: [0.0, 0.0, 1.0],
                y_axis: [1.0, 0.0, 0.0],
                z_axis: [0.0, 1.0, 0.0],
            }
        } else {
            PlaneFrameMm {
                origin_mm: [0.0; 3],
                x_axis: [1.0, 0.0, 0.0],
                y_axis: [0.0, 1.0, 0.0],
                z_axis: [0.0, 0.0, 1.0],
            }
        }
    }

    fn world(frame: &PlaneFrameMm, point: [f64; 2]) -> [f64; 3] {
        std::array::from_fn(|axis| {
            frame.origin_mm[axis] + frame.x_axis[axis] * point[0] + frame.y_axis[axis] * point[1]
        })
    }

    // Independent antiderivative of 1/2 (x*y' - y*x') for
    // (x,y)=(cx+2*cos(theta),cy+2*sin(theta)), relative to the reference.
    fn arc_area(center: [f64; 2], reference: [f64; 2], a: f64, b: f64) -> f64 {
        0.5 * (4.0 * (b - a)
            + 2.0 * (center[0] - reference[0]) * (b.sin() - a.sin())
            + 2.0 * (center[1] - reference[1]) * (a.cos() - b.cos()))
    }

    #[test]
    fn analytic_arc_preserves_raw_gauge_orientation_and_posed_frame() {
        let angles: [f64; 2] = [0.2, 0.35];
        let reference = [1.0, 2.0];
        let center = [3.0, -1.0];
        // theta=0.2+(t-10)*0.15/4, not a unit raw parameter interval.
        let acceleration = 2.0 * (0.15_f64 / 4.0).powi(2);
        for posed in [false, true] {
            let frame = frame(posed);
            let endpoints = angles.map(|angle| {
                world(
                    &frame,
                    [center[0] + 2.0 * angle.cos(), center[1] + 2.0 * angle.sin()],
                )
                .map(|value| Interval::new(value - 1.0e-13, value + 1.0e-13).unwrap())
            });
            let second = std::array::from_fn(|axis| {
                let bound = acceleration * (frame.x_axis[axis].abs() + frame.y_axis[axis].abs());
                Interval::new(-bound, bound).unwrap()
            });
            let expected = arc_area(center, reference, angles[0], angles[1]);
            for reverse in [false, true] {
                let points = if reverse {
                    [endpoints[1], endpoints[0]]
                } else {
                    endpoints
                };
                let range = if reverse { (14.0, 10.0) } else { (10.0, 14.0) };
                let area = signed_area_from_curvature(
                    &frame,
                    world(&frame, reference),
                    &points,
                    &second,
                    range,
                    &mut GeometryWorkBudget::new(1),
                    &AtomicBool::new(false),
                )
                .unwrap();
                let truth = if reverse { -expected } else { expected };
                assert!(
                    area[0] <= truth && truth <= area[1],
                    "{area:?} misses {truth}"
                );
                assert!(area[1] - area[0] < 0.01);
                // The true arc differs from the endpoint chord: this is not
                // merely a test that an endpoint polygon encloses itself.
                let arc_minus_chord =
                    2.0 * ((angles[1] - angles[0]) - (angles[1] - angles[0]).sin());
                assert!(arc_minus_chord > 0.001);
                assert!(area[0].abs().min(area[1].abs()) > 0.1);
            }
        }
    }

    #[test]
    fn uncertain_original_endpoints_cover_translated_analytic_arcs() {
        let frame = frame(true);
        let angles: [f64; 2] = [0.2, 0.35];
        let reference = [1.0, 2.0];
        let endpoints = angles.map(|angle| {
            world(&frame, [3.0 + 2.0 * angle.cos(), -1.0 + 2.0 * angle.sin()])
                .map(|value| Interval::new(value - 0.02, value + 0.02).unwrap())
        });
        let second = [
            Interval::new(-2.0, 2.0).unwrap(),
            Interval::point(0.0),
            Interval::new(-2.0, 2.0).unwrap(),
        ];
        let area = signed_area_from_curvature(
            &frame,
            world(&frame, reference),
            &endpoints,
            &second,
            (angles[0], angles[1]),
            &mut GeometryWorkBudget::new(1),
            &AtomicBool::new(false),
        )
        .unwrap();
        for dx in [-0.01, 0.0, 0.01] {
            for dy in [-0.01, 0.0, 0.01] {
                let truth = arc_area([3.0 + dx, -1.0 + dy], reference, angles[0], angles[1]);
                assert!(area[0] <= truth && truth <= area[1]);
            }
        }
    }

    #[test]
    fn invalid_inputs_and_overflow_are_charged_failures() {
        let valid_frame = frame(false);
        let endpoints = [[Interval::point(0.0); 3], [Interval::point(1.0); 3]];
        let second = [Interval::point(1.0); 3];
        for case in 0..8 {
            let mut frame = valid_frame;
            let mut reference = [0.0; 3];
            let mut points = endpoints;
            let mut curvature = second;
            let mut range = (0.0, 1.0);
            match case {
                0 => range = (1.0, 1.0),
                1 => range.1 = f64::INFINITY,
                2 => reference[0] = f64::NAN,
                3 => points[0][0] = Interval { lo: 2.0, hi: 1.0 },
                4 => curvature[1].lo = f64::NEG_INFINITY,
                5 => frame.x_axis = [0.0; 3],
                6 => range = (-f64::MAX, f64::MAX),
                7 => curvature = [Interval::point(f64::MAX); 3],
                _ => unreachable!(),
            }
            let mut work = GeometryWorkBudget::new(1);
            assert!(
                signed_area_from_curvature(
                    &frame,
                    reference,
                    &points,
                    &curvature,
                    range,
                    &mut work,
                    &AtomicBool::new(false),
                )
                .is_err()
            );
            assert_eq!(work.consumed(), 1);
        }
    }

    #[test]
    fn cancellation_and_exhaustion_preserve_shared_budget() {
        let frame = frame(false);
        let endpoints = [[Interval::point(0.0); 3], [Interval::point(1.0); 3]];
        let second = [Interval::point(1.0); 3];
        for (maximum, cancelled, code, consumed) in [
            (0, false, GeometryErrorCode::ResourceLimit, 0),
            (1, true, GeometryErrorCode::Cancelled, 1),
        ] {
            let mut work = GeometryWorkBudget::new(maximum);
            let error = signed_area_from_curvature(
                &frame,
                [0.0; 3],
                &endpoints,
                &second,
                (0.0, 1.0),
                &mut work,
                &AtomicBool::new(cancelled),
            )
            .unwrap_err();
            assert_eq!(error.code, code);
            assert_eq!(work.consumed(), consumed);
        }
    }
}
