// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Whole original line/ellipse coefficients through retained source wrappers.
//! Rounded coefficient midpoints may propose charts, but only these outward
//! coefficients and native trigonometric enclosures supply proof authority.

use crate::parametric::{CurveEnclosure, Interval, native_error};
use monstertruck_io::step::load::step_geometry::{Conic3D, Curve3D};
use monstertruck_modeling::{
    ValidatedGeometryBudget, ValidatedInterval, base::*, validated_sin_cos,
};
use spiling_contracts::geometry::{GeometryError, GeometryErrorCode};
use std::sync::atomic::AtomicBool;

const MAX_DEPTH: usize = 32;
fn p(value: f64) -> Interval {
    Interval::point(value)
}
fn add(a: Interval, b: Interval) -> Interval {
    if a.lo == 0.0 && a.hi == 0.0 {
        b
    } else if b.lo == 0.0 && b.hi == 0.0 {
        a
    } else {
        a.add(b)
    }
}
fn mul(a: Interval, b: Interval) -> Interval {
    if (a.lo == 0.0 && a.hi == 0.0) || (b.lo == 0.0 && b.hi == 0.0) {
        p(0.0)
    } else {
        a.mul(b)
    }
}
fn vector(point: Point3) -> [Interval; 3] {
    [p(point.x), p(point.y), p(point.z)]
}
fn subtract(a: [Interval; 3], b: [Interval; 3]) -> [Interval; 3] {
    std::array::from_fn(|i| add(a[i], b[i].neg()))
}
fn affine(
    matrix: &Matrix4,
    value: [Interval; 3],
    point: bool,
) -> Result<[Interval; 3], GeometryError> {
    if !(0..4).all(|i| (0..4).all(|j| matrix[i][j].is_finite())) {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "nonfinite source conic affine pose",
        ));
    }
    if (0..3).any(|i| matrix[i][3] != 0.0) || matrix[3][3] != 1.0 {
        return Err(GeometryError::new(
            GeometryErrorCode::UnsupportedOperation,
            "projective source conic pose is not an affine chart",
        ));
    }
    Ok(std::array::from_fn(|i| {
        (0..3).fold(if point { p(matrix[3][i]) } else { p(0.0) }, |sum, j| {
            add(sum, mul(p(matrix[j][i]), value[j]))
        })
    }))
}

/// Ellipses use `c + u*cos(phase) + v*sin(phase)` with
/// `phase = phase_offset + orientation*t` in the actual outer parameter.
/// Lines use `c + u*(t-a)/(b-a)` over `range=(a,b)`; their coefficients already
/// include every wrapper's sense. No basis geometry or carrier is copied.
pub(super) struct SourceConic {
    pub c: [Interval; 3],
    pub u: [Interval; 3],
    pub v: [Interval; 3],
    pub range: (f64, f64),
    pub circle: bool,
    pub orientation: f64,
    pub phase_offset: Interval,
}
impl SourceConic {
    pub fn jet(
        &self,
        range: [f64; 2],
        budget: &mut ValidatedGeometryBudget,
        cancel: &AtomicBool,
    ) -> Result<CurveEnclosure, GeometryError> {
        budget.charge(1, cancel).map_err(native_error)?;
        let parameter = Interval::new(range[0], range[1])?;
        if range[0] < self.range.0 || range[1] > self.range.1 {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "source conic request escapes its captured original range",
            ));
        }
        if self.circle {
            let phase = add(
                self.phase_offset,
                if self.orientation > 0.0 {
                    parameter
                } else {
                    parameter.neg()
                },
            );
            let (sine, cosine) = validated_sin_cos(
                ValidatedInterval {
                    lower: phase.lo,
                    upper: phase.hi,
                },
                budget,
                cancel,
            )
            .map_err(native_error)?;
            let sine = Interval {
                lo: sine.lower,
                hi: sine.upper,
            };
            let cosine = Interval {
                lo: cosine.lower,
                hi: cosine.upper,
            };
            Ok(CurveEnclosure {
                position: std::array::from_fn(|i| {
                    add(self.c[i], add(mul(self.u[i], cosine), mul(self.v[i], sine)))
                }),
                derivative: std::array::from_fn(|i| {
                    let value = add(mul(self.u[i], sine).neg(), mul(self.v[i], cosine));
                    if self.orientation > 0.0 {
                        value
                    } else {
                        value.neg()
                    }
                }),
                second_derivative: std::array::from_fn(|i| {
                    add(mul(self.u[i], cosine), mul(self.v[i], sine)).neg()
                }),
            })
        } else {
            let width = p(self.range.1).sub(p(self.range.0));
            let normalized = if range == [self.range.0, self.range.0] {
                p(0.0)
            } else if range == [self.range.1, self.range.1] {
                p(1.0)
            } else {
                parameter.sub(p(self.range.0)).div(width)?
            };
            Ok(CurveEnclosure {
                position: std::array::from_fn(|i| add(self.c[i], mul(self.u[i], normalized))),
                derivative: [
                    self.u[0].div(width)?,
                    self.u[1].div(width)?,
                    self.u[2].div(width)?,
                ],
                second_derivative: [p(0.0); 3],
            })
        }
    }
}

pub(super) fn capture<'a>(
    original: &'a Curve3D,
    budget: &mut ValidatedGeometryBudget,
    cancel: &AtomicBool,
) -> Result<Option<SourceConic>, GeometryError> {
    capture_kind(original, budget, cancel, false)
}

/// Guide coefficients propose a chart only. They never establish membership of
/// the projected intersection: that requires the retained continuation proof.
pub(super) fn capture_guide_candidate<'a>(
    original: &'a Curve3D,
    budget: &mut ValidatedGeometryBudget,
    cancel: &AtomicBool,
) -> Result<Option<SourceConic>, GeometryError> {
    capture_kind(original, budget, cancel, true)
}
fn capture_kind<'a>(
    original: &'a Curve3D,
    budget: &mut ValidatedGeometryBudget,
    cancel: &AtomicBool,
    guide_candidate: bool,
) -> Result<Option<SourceConic>, GeometryError> {
    budget.charge(1, cancel).map_err(native_error)?;
    // Bound a declared SurfaceCurve prefix before asking native range methods
    // to recurse through it. All later trim ranges are stored directly.
    let mut curve = original;
    let mut depth = 0;
    while let Curve3D::SurfaceCurve(source) = curve {
        budget.charge(1, cancel).map_err(native_error)?;
        if depth == MAX_DEPTH {
            return Err(depth_error());
        }
        depth += 1;
        curve = source.leader();
    }
    if !guide_candidate && matches!(curve, Curve3D::IntersectionCurve(_)) {
        return Ok(None);
    }
    let Some(range) = curve.try_range_tuple() else {
        return Ok(None);
    };
    Interval::new(range.0, range.1)?;
    if range.0 == range.1 {
        return Ok(None);
    }
    let mut active = Interval::new(range.0, range.1)?;
    let mut transforms: [Option<&'a Matrix4>; MAX_DEPTH] = [None; MAX_DEPTH];
    let mut transform_count = 0;
    let mut phase_offset = p(0.0);
    let mut orientation = 1.0;
    loop {
        budget.charge(1, cancel).map_err(native_error)?;
        match curve {
            Curve3D::SurfaceCurve(source) => {
                if depth == MAX_DEPTH {
                    return Err(depth_error());
                }
                depth += 1;
                curve = source.leader();
            }
            // A true intersection leader does not establish the root curve.
            Curve3D::IntersectionCurve(source) if guide_candidate => {
                if depth == MAX_DEPTH {
                    return Err(depth_error());
                }
                depth += 1;
                curve = source.leader().as_ref();
            }
            Curve3D::IntersectionCurve(_) => return Ok(None),
            Curve3D::TrimmedCurve(source) => {
                if depth == MAX_DEPTH {
                    return Err(depth_error());
                }
                depth += 1;
                let Some((a, b)) = source.try_range_tuple() else {
                    return Ok(None);
                };
                Interval::new(a, b)?;
                if active.lo < a || active.hi > b {
                    return Ok(None);
                }
                transforms[transform_count] = Some(source.transform());
                transform_count += 1;
                if !source.orientation() {
                    let anchor = p(a).add(p(b));
                    phase_offset = anchor.sub(phase_offset);
                    orientation = -orientation;
                    let image = anchor.sub(active);
                    // Reflection maps its own real closed interval to itself.
                    active = Interval::new(image.lo.max(a), image.hi.min(b))?;
                }
                curve = source.entity().curve().as_ref();
            }
            _ => break,
        }
    }
    let (mut c, mut u, mut v, circle) = match curve {
        Curve3D::Conic(Conic3D::Ellipse(source)) => {
            let matrix = source.transform();
            // Applying to canonical coefficients validates the complete affine
            // matrix without forming a rounded composite matrix.
            let c = affine(matrix, [p(0.0); 3], true)?;
            let u = affine(matrix, [p(1.0), p(0.0), p(0.0)], false)?;
            let v = affine(matrix, [p(0.0), p(1.0), p(0.0)], false)?;
            if !source.orientation() {
                let (a, b) = source.range_tuple();
                phase_offset = p(a).add(p(b)).sub(phase_offset);
                orientation = -orientation;
            }
            (c, u, v, true)
        }
        Curve3D::Line(source) => (
            vector(source.0),
            subtract(vector(source.1), vector(source.0)),
            [p(0.0); 3],
            false,
        ),
        _ => return Ok(None),
    };
    for matrix in transforms[..transform_count].iter().rev().flatten() {
        budget.charge(3, cancel).map_err(native_error)?;
        c = affine(matrix, c, true)?;
        u = affine(matrix, u, false)?;
        v = affine(matrix, v, false)?;
    }
    if !circle {
        let at = |t: f64| {
            let parameter = add(
                phase_offset,
                if orientation > 0.0 { p(t) } else { p(t).neg() },
            );
            std::array::from_fn(|i| add(c[i], mul(u[i], parameter)))
        };
        let start = at(range.0);
        let end = at(range.1);
        c = start;
        u = subtract(end, start);
    }
    for component in c.into_iter().chain(u).chain(v) {
        Interval::new(component.lo, component.hi)?;
    }
    Ok(Some(SourceConic {
        c,
        u,
        v,
        range,
        circle,
        orientation,
        phase_offset,
    }))
}
fn depth_error() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "retained source conic wrapper nesting exceeds cap",
    )
}

/// Bounded nominal candidate only. Import precharges the fixed wrapper walk.
/// This performs no validated capture, fresh work budget or geometry allocation;
/// the independent `capture` coefficient/angle proof must authorize any chart.
pub(super) fn nominal_boundary(
    original: &Curve3D,
    scale: f64,
) -> Result<Option<super::BoundaryCurveMm>, GeometryError> {
    let mut curve = original;
    let mut depth = 0;
    while let Curve3D::SurfaceCurve(source) = curve {
        if depth == MAX_DEPTH {
            return Err(depth_error());
        }
        depth += 1;
        curve = source.leader();
    }
    if matches!(curve, Curve3D::IntersectionCurve(_)) {
        return Ok(None);
    }
    let Some(range) = curve.try_range_tuple() else {
        return Ok(None);
    };
    if !range.0.is_finite() || !range.1.is_finite() || range.0 >= range.1 {
        return Ok(None);
    }
    let mut matrices: [Option<&Matrix4>; MAX_DEPTH] = [None; MAX_DEPTH];
    let mut count = 0;
    let mut offset = 0.0;
    let mut sense = 1.0;
    loop {
        match curve {
            Curve3D::SurfaceCurve(source) => {
                if depth == MAX_DEPTH {
                    return Err(depth_error());
                }
                depth += 1;
                curve = source.leader();
            }
            Curve3D::IntersectionCurve(_) => return Ok(None),
            Curve3D::TrimmedCurve(source) => {
                if depth == MAX_DEPTH {
                    return Err(depth_error());
                }
                depth += 1;
                let Some((a, b)) = source.try_range_tuple() else {
                    return Ok(None);
                };
                matrices[count] = Some(source.transform());
                count += 1;
                if !source.orientation() {
                    offset = a + b - offset;
                    sense = -sense;
                }
                curve = source.entity().curve().as_ref();
            }
            _ => break,
        }
    }
    match curve {
        Curve3D::Line(_) => {
            let start = original.subs(range.0).to_vec() * scale;
            let end = original.subs(range.1).to_vec() * scale;
            let start = [start.x, start.y, start.z];
            let end = [end.x, end.y, end.z];
            if start.into_iter().chain(end).any(|value| !value.is_finite()) {
                return Ok(None);
            }
            Ok(Some(super::BoundaryCurveMm::Line {
                start_mm: start,
                end_mm: end,
            }))
        }
        Curve3D::Conic(Conic3D::Ellipse(source)) => {
            let matrix = source.transform();
            nominal_affine(matrix)?;
            let mut center = matrix.transform_point(Point3::origin());
            let mut u = matrix.transform_vector(Vector3::unit_x());
            let mut v = matrix.transform_vector(Vector3::unit_y());
            if !source.orientation() {
                let (a, b) = source.range_tuple();
                offset = a + b - offset;
                sense = -sense;
            }
            for matrix in matrices[..count].iter().rev().flatten() {
                nominal_affine(matrix)?;
                center = matrix.transform_point(center);
                u = matrix.transform_vector(u);
                v = matrix.transform_vector(v);
            }
            super::circular_witness(
                center,
                u,
                v,
                offset + sense * range.0,
                offset + sense * range.1,
                scale,
            )
            .map(Some)
        }
        _ => Ok(None),
    }
}
fn nominal_affine(matrix: &Matrix4) -> Result<(), GeometryError> {
    if !(0..4).all(|i| (0..4).all(|j| matrix[i][j].is_finite()))
        || (0..3).any(|i| matrix[i][3] != 0.0)
        || matrix[3][3] != 1.0
    {
        return Err(GeometryError::new(
            GeometryErrorCode::UnsupportedOperation,
            "nominal source conic candidate requires a finite affine pose",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use monstertruck_io::step::load::step_geometry::SourceTrimmedCurve;
    use monstertruck_modeling::{Line, Processor, TrimmedCurve, UnitCircle};

    #[test]
    fn retained_arc_range_pose_and_nested_sense_keep_original_phase() {
        let conic = Curve3D::Conic(Conic3D::Ellipse(Processor::<_, Matrix4>::new(
            TrimmedCurve::new(UnitCircle::<Point3>::new(), (0.0, std::f64::consts::TAU)),
        )));
        let mut trim = Processor::with_transform(
            TrimmedCurve::new(Box::new(conic), (0.25, 1.25)),
            Matrix4::from_translation(Vector3::new(2.0, 3.0, 4.0)),
        );
        trim.invert();
        let source = Curve3D::TrimmedCurve(SourceTrimmedCurve::internal(trim));
        let cancel = AtomicBool::new(false);
        let mut budget = ValidatedGeometryBudget::new(100_000);
        let captured = capture(&source, &mut budget, &cancel).unwrap().unwrap();
        assert_eq!(captured.range, (0.25, 1.25));
        assert_eq!(captured.orientation, -1.0);
        assert!(captured.phase_offset.lo <= 1.5 && captured.phase_offset.hi >= 1.5);
        let nominal = nominal_boundary(&source, 1.0).unwrap().unwrap();
        let super::super::BoundaryCurveMm::CircleArc {
            start_angle,
            end_angle,
            ..
        } = nominal
        else {
            panic!("ellipse candidate lost");
        };
        assert_eq!((start_angle, end_angle), (1.25, 0.25));
        let jet = captured.jet([0.25, 1.25], &mut budget, &cancel).unwrap();
        for t in [0.25, 0.75, 1.25] {
            let point = source.subs(t);
            for axis in 0..3 {
                assert!(
                    jet.position[axis].lo <= point[axis] && jet.position[axis].hi >= point[axis]
                );
            }
        }
    }

    #[test]
    fn retained_unbounded_line_uses_its_declared_range_not_unit_endpoints() {
        let source = Curve3D::TrimmedCurve(SourceTrimmedCurve::internal(
            Processor::<_, Matrix4>::new(TrimmedCurve::new(
                Box::new(Curve3D::Line(Line(
                    Point3::origin(),
                    Point3::new(2.0, 0.0, 0.0),
                ))),
                (4.0, 6.0),
            )),
        ));
        let cancel = AtomicBool::new(false);
        let mut budget = ValidatedGeometryBudget::new(100_000);
        let captured = capture(&source, &mut budget, &cancel).unwrap().unwrap();
        assert_eq!(captured.range, (4.0, 6.0));
        assert!(captured.c[0].lo <= 8.0 && captured.c[0].hi >= 8.0);
        let end = captured
            .jet([6.0, 6.0], &mut budget, &cancel)
            .unwrap()
            .position[0];
        assert!(end.lo <= 12.0 && end.hi >= 12.0);
    }
}
