// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Temporary actual S_original(gamma_raw(t)) operand. It owns only borrowed
//! coefficient inventories, never a generated source declaration or topology.
use super::window::PcurveWindow;
use crate::{GeometryError, GeometryWorkBudget};
use crate::kernel::source_operand::{lift, PreparedOriginalCurve, PreparedOriginalSurface};
use crate::parametric::{AxisSpans, CurveEvaluator, Interval};
use solidcraft_step::geom::Pcurve;
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

pub(super) struct LiftedPcurve<'d> {
    surface: PreparedOriginalSurface<'d>,
    chart: PreparedOriginalCurve<'d>,
}
fn capacity() -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, "temporary original PCURVE lift exceeds its live byte allowance")
}
impl<'d> LiftedPcurve<'d> {
    pub(super) fn new(pcurve: &'d Pcurve, window: &PcurveWindow, remaining: usize,
        work: &mut GeometryWorkBudget, cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        let chart = PreparedOriginalCurve::new(&pcurve.curve, window.raw, remaining, work, cancel)?;
        let available = remaining.checked_sub(chart.retained_bytes()).ok_or_else(capacity)?;
        let surface = PreparedOriginalSurface::new(&pcurve.surface, window.surface_domain, available, work, cancel)?;
        let owner = Self { surface, chart };
        if owner.retained_bytes() > remaining { return Err(capacity()); }
        Ok(owner)
    }
    pub(super) fn chart(&self) -> &PreparedOriginalCurve<'d> { &self.chart }
    pub(super) fn into_chart(self) -> PreparedOriginalCurve<'d> { self.chart }
    pub(super) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.chart.retained_bytes().saturating_sub(std::mem::size_of::<PreparedOriginalCurve<'d>>())
            + self.surface.retained_bytes().saturating_sub(std::mem::size_of::<PreparedOriginalSurface<'d>>())
    }
    fn image(&self, range: [f64; 2], work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[f64; 2]; 2], GeometryError> {
        let uv = self.chart.position_bounds(range, work, cancel)?;
        Ok([[uv[0].lo, uv[0].hi], [uv[1].lo, uv[1].hi]])
    }
}
impl CurveEvaluator for LiftedPcurve<'_> {
    fn active_range(&self) -> [f64; 2] { self.chart.active_range() }
    fn source_parameter_map(&self) -> Option<[Interval; 2]> {
        Some([Interval::point(0.0), Interval::point(1.0)])
    }
    fn tensor(&self, range: [f64; 2], order: usize, work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        if order > 3 {
            return Err(GeometryError::new(GeometryErrorCode::ConditioningFailure,
                "original PCURVE lift derivative order exceeds three"));
        }
        // Reuse the shared original chain rule at precisely the requested
        // order, without an artificial parameter-map arithmetic round trip.
        let uv = self.chart.evaluate_order(range, order, work, cancel)?;
        let domain = [[uv[0][0].lo, uv[0][0].hi], [uv[0][1].lo, uv[0][1].hi]];
        let surface = self.surface.evaluate_order(domain, order, work, cancel)?;
        lift(surface, uv, order)
    }
    fn continuous(&self, range: [f64; 2], work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        if !self.chart.span_is_continuous(range, work, cancel)? { return Ok(false); }
        self.surface.span_is_continuous(self.image(range, work, cancel)?, work, cancel)
    }
    fn smooth(&self, range: [f64; 2], work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        if !self.chart.span_is_smooth(range, work, cancel)? { return Ok(false); }
        self.surface.span_is_smooth(self.image(range, work, cancel)?, work, cancel)
    }
    fn parameter_spans(&self, range: [f64; 2], work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<AxisSpans<'_>, GeometryError> {
        // This is an evaluation cover of actual chart knots, not a claim that
        // every carrier-knot preimage has been isolated. Whole-query smoothness
        // above remains mandatory for every derivative/root consumer.
        self.chart.parameter_spans(range, work, cancel)
    }
}
