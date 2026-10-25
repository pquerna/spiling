// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{GeometryError, GeometryWorkBudget};
use crate::kernel::source::OriginalCoedgeSource;
use crate::parametric::{down, up, Interval};
use solidcraft_step::geom::{BSplineCurve, Curve, PreferredRep, Trim, TrimPreference};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

pub(super) struct SourceSelection {
    pub(super) active: [f64; 2],
    pub(super) search: [f64; 2],
    pub(super) declared_endpoints: Option<[f64; 2]>,
    pub(super) endpoint_targets: Option<[[f64; 3]; 2]>,
    pub(super) declared_turns: [i32; 2],
    pub(super) periodic: Option<Interval>,
    pub(super) increasing: bool,
}

fn unavailable(message: &'static str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ParameterCorrespondenceUnavailable, message)
}
fn invalid(message: &'static str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::InvalidGeometry, message)
}
fn resource() -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, "original selector inventory exceeds its bounded census")
}

// A nonpreferred PARAMETER_VALUE is not an endpoint expression. In particular,
// retaining it alongside a Cartesian selector does not authorize its use.
fn parameter(selectors: &[Trim], preference: TrimPreference,
    work: &mut GeometryWorkBudget, cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    let mut parameter = None;
    let mut point = false;
    for selector in selectors {
        work.charge(1, cancel)?;
        match selector {
            Trim::Parameter(value) => {
                Interval::new(*value, *value)?;
                if parameter.is_some_and(|old| old != *value) {
                    return Err(unavailable("contradictory original parameter trim selectors"));
                }
                parameter = Some(*value);
            }
            Trim::Point(value) => {
                for coordinate in value { Interval::new(*coordinate, *coordinate)?; }
                point = true;
            }
        }
    }
    if point && preference != TrimPreference::Parameter {
        return Err(unavailable("selected original Cartesian trim requires an original point-to-parameter branch provider"));
    }
    parameter.ok_or_else(|| unavailable("original trim has no selected parameter endpoint; Cartesian pullback provider is unavailable"))
}

// Locate U[p] and U[n+1] without allocating the expanded knot vector. Knot
// multiplicities, not distinct-knot indices, define the natural domain.
fn spline_domain(spline: &BSplineCurve, work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[f64; 2], GeometryError> {
    if spline.points.len() <= spline.degree || spline.knots.len() != spline.multiplicities.len() {
        return Err(invalid("original spline has an invalid coefficient or knot census"));
    }
    let expected = spline.points.len().checked_add(spline.degree)
        .and_then(|n| n.checked_add(1)).ok_or_else(resource)?;
    let mut count = 0usize;
    let mut previous = None;
    let mut ends = [None; 2];
    for (&knot, &multiplicity) in spline.knots.iter().zip(&spline.multiplicities) {
        work.charge(1, cancel)?;
        Interval::new(knot, knot)?;
        if multiplicity == 0 || previous.is_some_and(|old| old >= knot) {
            return Err(invalid("original spline knots are not a strictly ordered positive-multiplicity census"));
        }
        let next = count.checked_add(multiplicity).ok_or_else(resource)?;
        for (slot, index) in [spline.degree, spline.points.len()].into_iter().enumerate() {
            if count <= index && index < next { ends[slot] = Some(knot); }
        }
        count = next;
        previous = Some(knot);
    }
    if count != expected { return Err(invalid("original spline expanded knot census disagrees with its coefficients")); }
    let [Some(a), Some(b)] = ends else { return Err(invalid("original spline natural domain is absent")); };
    if a >= b { return Err(invalid("original spline natural domain has no positive span")); }
    Ok([a, b])
}

pub(super) fn select(source: &OriginalCoedgeSource, work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SourceSelection, GeometryError> {
    select_curve(&source.spatial_curve, source.same_sense, work, cancel)
}

/// Inventory a literal reference operand in its own raw parameter gauge.
pub(super) fn select_curve(source: &Curve, same_sense: bool, work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SourceSelection, GeometryError> {
    let mut curve = source;
    let mut increasing = same_sense;
    let mut declared_endpoints = None;
    let mut restriction: Option<[f64; 2]> = None;
    let mut endpoint_targets = None;
    let mut declared_turns = [0; 2];
    let mut lifted = false;
    let mut declaring_period = None;
    let mut selected_pcurve = false;
    for depth in 0..64 {
        work.charge(1, cancel)?;
        match curve {
            Curve::Trimmed { basis, trim_1, trim_2, sense, preference } => {
                let selected_point = |selectors: &[Trim]| -> Result<Option<[f64; 3]>, GeometryError> {
                    let mut point = None;
                    for selector in selectors {
                        if let Trim::Point(value) = selector {
                            for coordinate in value { Interval::new(*coordinate, *coordinate)?; }
                            if point.is_some_and(|old| old != *value) {
                                return Err(unavailable("contradictory Cartesian trim selectors"));
                            }
                            point = Some(*value);
                        }
                    }
                    Ok(point)
                };
                work.charge(trim_1.len() as u64, cancel)?;
                work.charge(trim_2.len() as u64, cancel)?;
                let points = [selected_point(trim_1)?, selected_point(trim_2)?];
                let parameters_present = [trim_1, trim_2].into_iter()
                    .all(|selectors| selectors.iter().any(|value| matches!(value, Trim::Parameter(_))));
                if *preference != TrimPreference::Parameter || !parameters_present {
                    if let [Some(a), Some(b)] = points {
                        if *preference == TrimPreference::Unspecified && parameters_present {
                            return Err(unavailable("unspecified dual trim selectors require proved parameter/Cartesian equivalence"));
                        }
                        let targets = [a, b];
                        if declared_endpoints.is_some() || endpoint_targets.is_some_and(|old| old != targets) {
                            return Err(unavailable("nested selected Cartesian restrictions require endpoint containment correspondence"));
                        }
                        endpoint_targets = Some(targets);
                        increasing = increasing == *sense;
                        curve = basis;
                        if depth == 63 { return Err(resource()); }
                        continue;
                    }
                }
                let endpoints = [parameter(trim_1, *preference, work, cancel)?,
                    parameter(trim_2, *preference, work, cancel)?];
                let wraps = (*sense && endpoints[0] >= endpoints[1])
                    || (!*sense && endpoints[0] <= endpoints[1]);
                if wraps {
                    if restriction.is_some() || endpoint_targets.is_some() {
                        return Err(unavailable("nested periodic trim lifting requires inherited branch containment correspondence"));
                    }
                    declaring_period = Some(crate::kernel::source_operand::original_curve_parameter_period(
                        basis, work, cancel,
                    )?.ok_or_else(|| unavailable("selected trim basis has no parameter period authorizing its covering turn"))?);
                    declared_turns[1] = if *sense { 1 } else { -1 };
                    lifted = true;
                } else if lifted {
                    return Err(unavailable("nested periodic trim lifting requires inherited branch containment correspondence"));
                }
                let bounds = [endpoints[0].min(endpoints[1]), endpoints[0].max(endpoints[1])];
                if declared_endpoints.is_none() && endpoint_targets.is_none() { declared_endpoints = Some(endpoints); }
                if let Some(outer) = restriction {
                    if outer[0] < bounds[0] || outer[1] > bounds[1] {
                        return Err(unavailable("outer original trim exceeds its inherited basis restriction"));
                    }
                } else { restriction = Some(bounds); }
                increasing = increasing == *sense;
                curve = basis;
            }
            Curve::SurfaceCurve { curve_3d, master, associated, .. } => {
                curve = match master {
                    PreferredRep::Curve3d => curve_3d,
                    PreferredRep::PcurveS1 | PreferredRep::PcurveS2 => {
                        let index = if *master == PreferredRep::PcurveS1 { 0 } else { 1 };
                        match associated.get(index) {
                            Some(solidcraft_step::geom::Associated::Pcurve(pcurve)) => {
                                if selected_pcurve {
                                    return Err(unavailable("nested preferred PCURVE compositions require a composed domain inventory"));
                                }
                                selected_pcurve = true;
                                &pcurve.curve
                            }
                            _ => return Err(unavailable("preferred original PCURVE association is not a retained curve operand")),
                        }
                    }
                };
            }
            _ => break,
        }
        if depth == 63 { return Err(resource()); }
    }
    let periodic = crate::kernel::source_operand::original_curve_parameter_period(
        source, work, cancel,
    )?;
    if let Some(period) = periodic {
        Interval::new(period.lo, period.hi)?;
        if period.lo <= 0.0 {
            return Err(unavailable("original periodic master has no finite positive covering period"));
        }
    }
    if lifted {
        let period = declaring_period.ok_or_else(|| unavailable("nonperiodic original trim endpoints disagree with its declared sense"))?;
        let endpoints = declared_endpoints.ok_or_else(|| unavailable("lifted trim lacks selected literal endpoints"))?;
        let shifted = Interval::point(endpoints[1])
            .add(period.mul(Interval::point(f64::from(declared_turns[1]))));
        let bounds = [endpoints[0].min(shifted.lo), endpoints[0].max(shifted.hi)];
        if (declared_turns[1] > 0 && shifted.lo <= endpoints[0])
            || (declared_turns[1] < 0 && shifted.hi >= endpoints[0]) {
            return Err(unavailable("original periodic trim requires more than one proved covering turn"));
        }
        return Ok(SourceSelection { active: bounds, search: bounds, declared_endpoints,
            endpoint_targets, declared_turns, periodic: Some(period), increasing });
    }
    let natural = match curve {
        Curve::BSpline(spline) => Some(spline_domain(spline, work, cancel)?),
        Curve::Polyline(points) => {
            work.charge(points.len() as u64, cancel)?;
            if points.len() < 2 || points.len() > (1usize << 53) {
                return Err(invalid("original polyline has no exactly indexed finite domain"));
            }
            Some([0.0, (points.len() - 1) as f64])
        }
        _ => None,
    };
    if let Some(bounds) = restriction {
        if natural.is_some_and(|domain| bounds[0] < domain[0] || bounds[1] > domain[1]) {
            return Err(unavailable("original trim exceeds the natural source coefficient domain"));
        }
        return Ok(SourceSelection { active: bounds, search: bounds, declared_endpoints,
            endpoint_targets, declared_turns, periodic, increasing });
    }
    if let Some(domain) = natural {
        return Ok(SourceSelection { active: domain, search: domain, declared_endpoints,
            endpoint_targets, declared_turns, periodic: None, increasing });
    }
    if let Some(period) = periodic {
        // Cover one full turn with overlap at both seams; reserve two turns
        // either side for endpoint recipes lifted by an exact period enclosure.
        let margin = period.hi / 16.0;
        let search = [-margin, up(period.hi + margin)];
        let active = [down(-2.0 * period.hi), up(3.0 * period.hi)];
        return Ok(SourceSelection { active, search, declared_endpoints, endpoint_targets,
            declared_turns, periodic: Some(period), increasing });
    }
    Err(unavailable("unbounded original curve has no finite selected parameter window; literal LINE requires the source vertex projection restriction"))
}

/// Return the actual spatial declaration for a preferred-PCURVE source. Outer
/// trim senses compose with EDGE_CURVE sense, but their numeric selectors stay
/// in the selected master gauge and are not transplanted onto curve_3d.
pub(super) fn original_3d_reference<'a>(source: &'a OriginalCoedgeSource,
    work: &mut GeometryWorkBudget, cancel: &AtomicBool,
) -> Result<Option<(&'a Curve, bool)>, GeometryError> {
    original_3d_reference_curve(source.spatial_curve.as_ref(), source.same_sense, work, cancel)
}

pub(super) fn original_3d_reference_curve<'a>(source: &'a Curve, same_sense: bool,
    work: &mut GeometryWorkBudget, cancel: &AtomicBool,
) -> Result<Option<(&'a Curve, bool)>, GeometryError> {
    let mut curve = source;
    let mut increasing = same_sense;
    for _ in 0..64 {
        work.charge(1, cancel)?;
        match curve {
            Curve::Trimmed { basis, sense, .. } => {
                increasing = increasing == *sense;
                curve = basis;
            }
            Curve::SurfaceCurve { curve_3d, master, .. } => {
                if *master != PreferredRep::Curve3d {
                    return Ok(Some((curve_3d.as_ref(), increasing)));
                }
                curve = curve_3d;
            }
            _ => return Ok(None),
        }
    }
    Err(resource())
}
