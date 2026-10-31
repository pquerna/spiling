// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{finite, source_capacity, source_unavailable};
use crate::exact_coefficient::ExactCoefficient as E;
use crate::kernel::source::OriginalCoedgeSource;
use crate::kernel::source_operand::{PreparedOriginalCurve, preferred_source_pcurve};
use crate::parametric::{AxisSpans, CurveEvaluator, ExactAffineCurve, Interval};
use crate::{GeometryError, GeometryWorkBudget};
use solidcraft_step::geom::{Curve, Surface};
use std::sync::atomic::AtomicBool;

const SCRATCH_BYTES: usize = 32 * std::mem::size_of::<E>();

pub(crate) struct SourceAffineRestriction<'d> {
    operand: PreparedOriginalCurve<'d>,
    normalized: ExactAffineCurve,
    start: E,
    delta: E,
    squared_direction: E,
    endpoints: [Interval; 2],
    map: [Interval; 2],
    position: [Interval; 3],
    derivative: [Interval; 3],
    source_error_mm: f64,
    exact_endpoints: bool,
}

fn unavailable() -> GeometryError {
    source_unavailable("selected affine source exceeds exact coefficient conditioning")
}
fn enclosure(value: &E) -> Result<Interval, GeometryError> {
    finite(
        value.terms[..value.len]
            .iter()
            .fold(Interval::point(0.0), |sum, &term| {
                sum.add(Interval::point(term))
            }),
    )
}
fn add(a: &mut E, b: &E) -> Result<(), GeometryError> {
    for &term in &b.terms[..b.len] {
        a.push(term).ok_or_else(unavailable)?;
    }
    Ok(())
}
fn product(
    a: &E,
    b: &E,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<E, GeometryError> {
    a.product(b, work, cancel)?.ok_or_else(unavailable)
}
fn quotient(a: &E, b: &E) -> Result<Interval, GeometryError> {
    finite(enclosure(a)?.div(enclosure(b)?)?)
}
fn query(range: [f64; 2]) -> Result<Interval, GeometryError> {
    let s = Interval::new(range[0], range[1])?;
    if range[0] < 0.0 || range[1] > 1.0 {
        return Err(source_unavailable(
            "affine source query exceeds its normalized edge",
        ));
    }
    Ok(s)
}
impl<'d> SourceAffineRestriction<'d> {
    pub(super) fn try_new(
        source: &'d OriginalCoedgeSource,
        allowance: f64,
        remaining: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        Self::try_new_curve(
            source,
            source.spatial_curve.as_ref(),
            source.same_sense,
            allowance,
            remaining,
            work,
            cancel,
        )
    }
    pub(super) fn try_new_curve(
        source: &'d OriginalCoedgeSource,
        curve: &'d Curve,
        same_sense: bool,
        allowance: f64,
        remaining: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        work.charge(1, cancel)?;
        let Curve::SurfaceCurve { curve_3d, .. } = curve else {
            return Ok(None);
        };
        let preferred = preferred_source_pcurve(curve)?;
        let eligible = match preferred {
            Some(pcurve) => {
                matches!(pcurve.curve.as_ref(), Curve::Line { .. })
                    && matches!(pcurve.surface.as_ref(), Surface::Plane { .. })
            }
            None => matches!(curve_3d.as_ref(), Curve::Line { .. }),
        };
        if !eligible {
            return Ok(None);
        }
        if !allowance.is_finite() || allowance < 0.0 {
            return Err(source_unavailable(
                "affine source admission must be finite and nonnegative",
            ));
        }
        let fixed = std::mem::size_of::<Self>();
        let available = remaining
            .checked_sub(fixed)
            .and_then(|n| n.checked_sub(SCRATCH_BYTES))
            .and_then(|n| n.checked_add(std::mem::size_of::<PreparedOriginalCurve<'d>>()))
            .ok_or_else(source_capacity)?;
        let operand = PreparedOriginalCurve::new(curve, [0.0, 1.0], available, work, cancel)?;
        let Some(raw) = operand.exact_affine_coefficients(work, cancel)? else {
            return Err(unavailable());
        };
        if raw.denominator.sign() == 0 {
            return Err(unavailable());
        }
        let mut squared_direction = E::point(0.0);
        let mut numerators = [E::point(0.0), E::point(0.0)];
        for axis in 0..3 {
            add(
                &mut squared_direction,
                &product(&raw.derivative[axis], &raw.derivative[axis], work, cancel)?,
            )?;
            for (endpoint, point) in source.vertex_points_mm.iter().enumerate() {
                let mut residual = product(&E::point(point[axis]), &raw.denominator, work, cancel)?;
                residual
                    .subtract(&raw.position[axis])
                    .ok_or_else(unavailable)?;
                add(
                    &mut numerators[endpoint],
                    &product(&residual, &raw.derivative[axis], work, cancel)?,
                )?;
            }
        }
        if squared_direction.sign() <= 0 {
            return Err(unavailable());
        }
        let [start, end] = numerators;
        let mut delta = end.clone();
        delta.subtract(&start).ok_or_else(unavailable)?;
        // A preferred PCURVE has its own raw gauge; its direction can oppose
        // the spatial LINE. The trim owner proves that separate association.
        if delta.sign() == 0
            || (preferred.is_none()
                && ((same_sense && delta.sign() < 0) || (!same_sense && delta.sign() > 0)))
        {
            return Err(source_unavailable(
                "affine source endpoints contradict captured edge sense",
            ));
        }
        let endpoints = [
            quotient(&start, &squared_direction)?,
            quotient(&end, &squared_direction)?,
        ];
        let map = [endpoints[0], quotient(&delta, &squared_direction)?];
        let mut normalized = ExactAffineCurve {
            position: std::array::from_fn(|_| E::point(0.0)),
            derivative: std::array::from_fn(|_| E::point(0.0)),
            denominator: product(&raw.denominator, &squared_direction, work, cancel)?,
        };
        let denominator = enclosure(&normalized.denominator)?;
        // A nonzero exact denominator also needs a finite, sign-separated numerical enclosure.
        if denominator.lo <= 0.0 && denominator.hi >= 0.0 {
            return Err(unavailable());
        }
        for axis in 0..3 {
            normalized.position[axis] =
                product(&raw.position[axis], &squared_direction, work, cancel)?;
            add(
                &mut normalized.position[axis],
                &product(&raw.derivative[axis], &start, work, cancel)?,
            )?;
            normalized.derivative[axis] = product(&raw.derivative[axis], &delta, work, cancel)?;
        }
        let mut position = [Interval::point(0.0); 3];
        let mut derivative = position;
        for axis in 0..3 {
            position[axis] = finite(enclosure(&normalized.position[axis])?.div(denominator)?)?;
            derivative[axis] = finite(enclosure(&normalized.derivative[axis])?.div(denominator)?)?;
        }
        let mut operator = Self {
            operand,
            normalized,
            start,
            delta,
            squared_direction,
            endpoints,
            map,
            position,
            derivative,
            source_error_mm: 0.0,
            exact_endpoints: true,
        };
        for endpoint in 0..2 {
            let mut exact = true;
            for axis in 0..3 {
                exact &= operator.normalized.endpoint_coordinate_equal(
                    endpoint as f64,
                    axis,
                    source.vertex_points_mm[endpoint][axis],
                    work,
                    cancel,
                )?;
            }
            if !exact {
                operator.exact_endpoints = false;
                let jet = operator.tensor([endpoint as f64; 2], 0, work, cancel)?;
                let error = finite(
                    (0..3)
                        .fold(Interval::point(0.0), |sum, axis| {
                            sum.add(
                                jet[0][axis]
                                    .sub(Interval::point(source.vertex_points_mm[endpoint][axis]))
                                    .square(),
                            )
                        })
                        .sqrt()?,
                )?
                .hi;
                if error > allowance {
                    return Err(source_unavailable(
                        "affine source endpoint residual exceeds admission",
                    ));
                }
                operator.source_error_mm = operator.source_error_mm.max(error);
            }
        }
        Ok(Some(operator))
    }
    pub(super) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self
                .operand
                .retained_bytes()
                .saturating_sub(std::mem::size_of::<PreparedOriginalCurve<'_>>())
    }
    pub(super) fn source_error_mm(&self) -> f64 {
        self.source_error_mm
    }
    pub(super) fn exact_source_endpoints(&self) -> bool {
        self.exact_endpoints
    }
    pub(super) fn endpoint_parameters(&self) -> [Interval; 2] {
        self.endpoints
    }
    pub(super) fn parameter_derivative(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Interval, GeometryError> {
        work.charge(1, cancel)?;
        quotient(&self.delta, &self.squared_direction)
    }
    pub(super) fn parameter_image(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Interval, GeometryError> {
        let s = query(range)?;
        work.charge(1, cancel)?;
        // Preserve the exact quotient recipe rather than subtracting rounded endpoint roots.
        let mut numerator = product(&self.delta, &E::point(s.lo), work, cancel)?;
        add(&mut numerator, &self.start)?;
        let a = quotient(&numerator, &self.squared_direction)?;
        let mut numerator = product(&self.delta, &E::point(s.hi), work, cancel)?;
        add(&mut numerator, &self.start)?;
        Ok(a.hull(quotient(&numerator, &self.squared_direction)?))
    }
}
impl CurveEvaluator for SourceAffineRestriction<'_> {
    fn active_range(&self) -> [f64; 2] {
        [0.0, 1.0]
    }
    fn exact_affine_coefficients(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<ExactAffineCurve>, GeometryError> {
        work.charge(1, cancel)?;
        Ok(Some(ExactAffineCurve {
            position: self.normalized.position.clone(),
            derivative: self.normalized.derivative.clone(),
            denominator: self.normalized.denominator.clone(),
        }))
    }
    fn source_parameter_map(&self) -> Option<[Interval; 2]> {
        Some(self.map)
    }
    fn source_parameter_image(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Interval>, GeometryError> {
        self.parameter_image(range, work, cancel).map(Some)
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        let s = query(range)?;
        work.charge(1, cancel)?;
        if order > 3 {
            return Err(source_unavailable("affine source jet order exceeds three"));
        }
        let mut result = [[Interval::point(0.0); 3]; 4];
        for (axis, &position) in self.position.iter().enumerate() {
            result[0][axis] = finite(position.add(self.derivative[axis].mul(s)))?;
            if order > 0 {
                result[1][axis] = self.derivative[axis];
            }
        }
        Ok(result)
    }
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        query(range)?;
        work.charge(1, cancel)?;
        Ok(true)
    }
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.continuous(range, work, cancel)
    }
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<AxisSpans<'_>, GeometryError> {
        query(range)?;
        work.charge(1, cancel)?;
        AxisSpans::single(range)
    }
    fn original_curve(&self) -> Option<&Curve> {
        Some(self.operand.source())
    }
}
