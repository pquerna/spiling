// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{dot, kernel_error, parallel, signed_distance};
use crate::{Definition, GeometryError, PlaneMm, check_cancel, kernel::BoundaryCurveMm};
use spiling_contracts::geometry::{FaceCarrier, GeometryErrorCode, SECTION_BOOLEAN_TOLERANCE_MM};
use std::{
    f64::consts::{PI, TAU},
    sync::atomic::AtomicBool,
};

#[derive(Debug)]
struct Extrema {
    min: f64,
    max: f64,
    degenerate: bool,
}

impl Extrema {
    fn new() -> Self {
        Self {
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            degenerate: false,
        }
    }

    fn include(
        &mut self,
        distance: f64,
        is_vertex_or_stationary: bool,
    ) -> Result<(), GeometryError> {
        if !distance.is_finite() {
            return Err(kernel_error("nonfinite analytic section boundary witness"));
        }
        self.min = self.min.min(distance);
        self.max = self.max.max(distance);
        self.degenerate |=
            is_vertex_or_stationary && distance.abs() <= SECTION_BOOLEAN_TOLERANCE_MM;
        Ok(())
    }

    fn include_curve(
        &mut self,
        curve: &BoundaryCurveMm,
        plane: &PlaneMm,
    ) -> Result<(), GeometryError> {
        match curve {
            BoundaryCurveMm::Line { start_mm, end_mm } => {
                // Zero-length lines intentionally preserve original native
                // vertices; ordinary line extrema occur at these endpoints too.
                self.include(signed_distance(*start_mm, plane)?, true)?;
                self.include(signed_distance(*end_mm, plane)?, true)?;
            }
            BoundaryCurveMm::CircleArc {
                center_mm,
                u_mm,
                v_mm,
                start_angle,
                end_angle,
            } => {
                let (lo, hi) = (start_angle.min(*end_angle), start_angle.max(*end_angle));
                if !lo.is_finite() || !hi.is_finite() || hi - lo > TAU + 1e-10 {
                    return Err(kernel_error("invalid bounded native circle-arc witness"));
                }
                let offset = signed_distance(*center_mm, plane)?;
                let a = dot(*u_mm, plane.normal);
                let b = dot(*v_mm, plane.normal);
                if !a.is_finite() || !b.is_finite() {
                    return Err(kernel_error("nonfinite native circle projection"));
                }
                let evaluate = |angle: f64| offset + a * angle.cos() + b * angle.sin();
                self.include(evaluate(lo), true)?;
                self.include(evaluate(hi), true)?;
                // f(theta)=offset+a*cos(theta)+b*sin(theta). These are
                // exact stationary phases, not extrema inferred from samples.
                if a != 0.0 || b != 0.0 {
                    let phase = b.atan2(a);
                    for stationary in [phase, phase + PI] {
                        if let Some(angle) = angle_in_arc(stationary, lo, hi) {
                            self.include(evaluate(angle), true)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Finds a congruent stationary angle in this bounded arc, in either orientation.
fn angle_in_arc(phase: f64, lo: f64, hi: f64) -> Option<f64> {
    let angle = lo + (phase - lo).rem_euclid(TAU);
    if angle <= hi { Some(angle) } else { None }
}

pub(super) fn crosses_plane(
    definition: &Definition,
    plane: &PlaneMm,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let mut extrema = Extrema::new();
    for curve in &definition.boundary_curves {
        check_cancel(cancel)?;
        extrema.include_curve(curve, plane)?;
    }
    if !extrema.min.is_finite() || !extrema.max.is_finite() {
        return Err(kernel_error(
            "native definition has no finite boundary witnesses",
        ));
    }
    // A separated solid is empty on either side, without invoking the boolean.
    // A plane within the degeneracy tolerance of its boundary is NOT separated.
    if extrema.min > SECTION_BOOLEAN_TOLERANCE_MM || extrema.max < -SECTION_BOOLEAN_TOLERANCE_MM {
        return Ok(false);
    }
    for face in &definition.faces {
        check_cancel(cancel)?;
        if let FaceCarrier::Plane { origin_mm, normal } = &face.carrier
            && parallel(*normal, plane.normal)
            && signed_distance(*origin_mm, plane)?.abs() <= SECTION_BOOLEAN_TOLERANCE_MM
        {
            extrema.degenerate = true;
        }
    }
    if extrema.degenerate {
        return Err(GeometryError::new(
            GeometryErrorCode::DegenerateSection,
            "section plane contains a native vertex/edge/face or is tangent to a native boundary arc",
        ));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arc(start: f64, end: f64) -> BoundaryCurveMm {
        BoundaryCurveMm::CircleArc {
            center_mm: [0.0; 3],
            u_mm: [5.0, 0.0, 0.0],
            v_mm: [0.0, 5.0, 0.0],
            start_angle: start,
            end_angle: end,
        }
    }

    #[test]
    fn arc_extrema_include_unsampled_interior_stationary_points() {
        let plane = PlaneMm {
            origin_mm: [4.0, 0.0, 0.0],
            normal: [1.0, 0.0, 0.0],
        };
        for (start, end) in [(-0.7, 0.8), (0.8, -0.7), (TAU - 0.7, TAU + 0.8)] {
            let mut result = Extrema::new();
            result.include_curve(&arc(start, end), &plane).unwrap();
            assert!((result.max - 1.0).abs() < 1e-12);
            assert!((result.min - (5.0 * 0.8_f64.cos() - 4.0)).abs() < 1e-12);
            assert!(!result.degenerate);
        }
    }

    #[test]
    fn excluded_stationary_angle_does_not_expand_trimmed_arc() {
        let plane = PlaneMm {
            origin_mm: [0.0; 3],
            normal: [1.0, 0.0, 0.0],
        };
        let mut result = Extrema::new();
        result.include_curve(&arc(0.2, 0.8), &plane).unwrap();
        assert!((result.max - 5.0 * 0.2_f64.cos()).abs() < 1e-12);
        assert!((result.min - 5.0 * 0.8_f64.cos()).abs() < 1e-12);
    }

    #[test]
    fn tangent_and_contained_arcs_are_degenerate_without_sampling() {
        let mut tangent = Extrema::new();
        tangent
            .include_curve(
                &arc(-0.7, 0.8),
                &PlaneMm {
                    origin_mm: [5.0, 0.0, 0.0],
                    normal: [1.0, 0.0, 0.0],
                },
            )
            .unwrap();
        assert!(tangent.degenerate);
        let mut contained = Extrema::new();
        contained
            .include_curve(
                &arc(0.2, 0.8),
                &PlaneMm {
                    origin_mm: [0.0; 3],
                    normal: [0.0, 0.0, 1.0],
                },
            )
            .unwrap();
        assert!(contained.degenerate);
    }
}
