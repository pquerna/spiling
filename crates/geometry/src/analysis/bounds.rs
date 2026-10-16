// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Directional support of the closed, genuinely trimmed native boundary.
//!
//! A search cell supplies an exclusion enclosure, never a contact witness.
//! Only native boundary-curve points or surface parameters proved inside the
//! true trim supply feasible witnesses. The two endpoint brackets are kept
//! independently throughout the search.

use super::{BoundsPolicy, GeometryWorkBudget, ProjectedBounds};
use crate::{
    Definition, GeometryError,
    parametric::{self, Interval},
};
use monstertruck_modeling::{BoundedCurve, Curve, Surface};
use spiling_contracts::geometry::GeometryErrorCode;
use std::{cmp::Ordering, collections::BinaryHeap, sync::atomic::AtomicBool};

const MAX_LIVE_CELLS: usize = 131_072;
const MAX_CREATED_CELLS: u64 = 2_000_000;
const MAX_SCRATCH_ALLOCATION_BYTES: usize = 32 * 1024 * 1024;
const MAX_DEPTH: u16 = 96;

/// Enclose the minimum and maximum of normalized `direction` separately.
/// Native carriers, not inspection-family metadata, select the boundary-only
/// optimization. Every other carrier includes its complete trimmed interior.
pub fn projected_bounds(
    definition: &Definition,
    direction: [f64; 3],
    policy: BoundsPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ProjectedBounds, GeometryError> {
    projected_bounds_with_total_limit(
        definition,
        direction,
        policy,
        MAX_SCRATCH_ALLOCATION_BYTES,
        work,
        cancel,
    )
}

/// Total allowance including pinned proof, unlike the scratch-only boundary.
fn projected_bounds_with_total_limit(
    definition: &Definition,
    direction: [f64; 3],
    policy: BoundsPolicy,
    total_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ProjectedBounds, GeometryError> {
    work.charge(1, cancel)?;
    let remaining_bytes = total_bytes
        .min(MAX_SCRATCH_ALLOCATION_BYTES)
        .checked_sub(definition.retained_query_proof_bytes())
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "retained native query proof exhausts directional allocation cap",
            )
        })?;
    projected_bounds_with_scratch_limit(
        crate::section::OriginalTrimSource::standalone(definition),
        direction,
        policy,
        remaining_bytes,
        work,
        cancel,
    )
}

/// `remaining_bytes` is scratch capacity AFTER the enclosing caller reserves
/// the definition's pinned query proof and all other coexisting buffers.
pub(super) fn projected_bounds_with_scratch_limit(
    source: crate::section::OriginalTrimSource<'_, '_>,
    direction: [f64; 3],
    policy: BoundsPolicy,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ProjectedBounds, GeometryError> {
    let definition = source.definition();
    work.charge(1, cancel)?;
    let tolerance = policy.maximum_endpoint_uncertainty_mm;
    if !tolerance.is_finite() || tolerance <= 0.0 {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "directional bounds require a finite positive endpoint uncertainty",
        ));
    }
    let direction = normalized_direction(direction)?;
    let mut scratch = Scratch {
        byte_limit: remaining_bytes.min(MAX_SCRATCH_ALLOCATION_BYTES),
        ..Scratch::default()
    };
    let mut aggregate = None;
    for (face_index, face) in definition.solid.face_iter().enumerate() {
        work.charge(1, cancel)?;
        let boundary_complete = face.with_surface(boundary_support_is_complete);
        let support = if boundary_complete {
            let mut face_support = None;
            for edge in face.edge_iter() {
                work.charge(1, cancel)?;
                let support = edge.with_curve(|curve| {
                    let remaining_bytes = scratch.byte_limit - scratch.allocated_bytes;
                    let prepared = definition.prepare_boundary_curve(
                        edge.id(),
                        curve,
                        curve.range_tuple(),
                        remaining_bytes,
                        work,
                        cancel,
                    )?;
                    prepared_curve_support(
                        curve,
                        prepared,
                        direction,
                        tolerance,
                        &mut scratch,
                        work,
                        cancel,
                    )
                })?;
                combine(&mut face_support, support);
            }
            face_support.ok_or_else(|| {
                error(
                    GeometryErrorCode::InvalidGeometry,
                    "bounded analytic face has no native boundary",
                )
            })?
        } else {
            let remaining_bytes = scratch.byte_limit - scratch.allocated_bytes;
            let trim = source.face_domain(face_index, remaining_bytes, work, cancel)?;
            scratch.allocated_bytes = scratch
                .allocated_bytes
                .checked_add(trim.owned_bytes())
                .filter(|bytes| *bytes <= scratch.byte_limit)
                .ok_or_else(|| {
                    error(
                        GeometryErrorCode::ResourceLimit,
                        "native trim proof capacity exhausts directional scratch allocation",
                    )
                })?;
            // Preparation restricts this cover only by an independently proved
            // source/native carrier domain, never an assumed zero-to-one box.
            let uv = trim.uv_bounds();
            validate_domain(uv, 2)?;
            face.with_surface(|surface| {
                let boxes = parametric::surface_parameter_boxes(surface, uv, work, cancel)?;
                search_domains(
                    boxes.into_iter(),
                    2,
                    tolerance,
                    &mut scratch,
                    work,
                    cancel,
                    |cell, work, cancel| {
                        use crate::kernel::TrimClassification;
                        let class = trim.classify(cell, work, cancel)?;
                        if matches!(class, TrimClassification::Outside) {
                            return Ok(None);
                        }
                        let enclosure = parametric::surface_enclosure(surface, cell, work, cancel)?;
                        let midpoint = cell.map(|span| span[0] * 0.5 + span[1] * 0.5);
                        let point_cell = midpoint.map(|value| [value, value]);
                        let point =
                            parametric::surface_enclosure(surface, point_cell, work, cancel)?;
                        let position = project(enclosure.position, direction)?;
                        let centered =
                            surface_taylor(cell, midpoint, &enclosure, &point, direction)?;
                        let height = intersect(position, centered)?;
                        let feasible = matches!(class, TrimClassification::Inside)
                            || matches!(
                                trim.classify(point_cell, work, cancel)?,
                                TrimClassification::Inside
                            );
                        let witness = if feasible {
                            Some(project(point.position, direction)?)
                        } else {
                            None
                        };
                        let du = project(enclosure.du, direction)?.abs_upper();
                        let dv = project(enclosure.dv, direction)?.abs_upper();
                        let split_axis = if du * width(cell[0]) >= dv * width(cell[1]) {
                            0
                        } else {
                            1
                        };
                        Ok(Some(Evaluation {
                            height,
                            witness,
                            split_axis,
                        }))
                    },
                )
            })?
        };
        combine(&mut aggregate, support);
    }
    let mut result = aggregate.ok_or_else(|| {
        error(
            GeometryErrorCode::InvalidGeometry,
            "native definition has no bounded faces",
        )
    })?;
    result.achieved_uncertainty_mm = uncertainty(&result);
    if result.achieved_uncertainty_mm > tolerance {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "directional endpoint brackets do not meet the requested uncertainty",
        ));
    }
    Ok(result)
}

fn normalized_direction(direction: [f64; 3]) -> Result<[Interval; 3], GeometryError> {
    if direction.iter().any(|value| !value.is_finite()) {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "nonfinite support direction",
        ));
    }
    // Scaling avoids overflow/underflow before interval normalization. The
    // represented query is the mathematical normalization of the input vector.
    let scale = direction.iter().copied().map(f64::abs).fold(0.0, f64::max);
    if scale == 0.0 {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "zero support direction",
        ));
    }
    if direction.iter().filter(|value| **value != 0.0).count() == 1 {
        // A single nonzero component has an exactly known mathematical unit
        // direction, even when its magnitude is subnormal or nearly maximal.
        // This is structural equality, not a tolerance-based bracket collapse.
        return Ok(
            direction.map(|value| Interval::point(if value == 0.0 { 0.0 } else { value.signum() }))
        );
    }
    let scaled = [
        Interval::point(direction[0]).div(Interval::point(scale))?,
        Interval::point(direction[1]).div(Interval::point(scale))?,
        Interval::point(direction[2]).div(Interval::point(scale))?,
    ];
    let norm = scaled
        .iter()
        .fold(Interval::point(0.0), |sum, value| sum.add(value.square()))
        .sqrt()?;
    Ok([
        scaled[0].div(norm)?,
        scaled[1].div(norm)?,
        scaled[2].div(norm)?,
    ])
}

/// A linear functional on a compact plane takes its extrema on the boundary.
/// The same holds on an affine image of a cylinder: its axial dependence is
/// affine, or it is constant on a generator reaching the boundary. We require
/// a structurally exact axis-aligned profile rather than trusting rounded cross
/// products or descriptive cylinder metadata. Other cylinders use the general
/// proof path without a change in supported geometry.
fn boundary_support_is_complete(surface: &Surface) -> bool {
    match surface {
        Surface::Plane(_) => true,
        Surface::RevolutionSurface(processor) => {
            let transform = processor.transform();
            if transform.x.w != 0.0
                || transform.y.w != 0.0
                || transform.z.w != 0.0
                || transform.w.w != 1.0
            {
                return false;
            }
            let revolution = processor.entity();
            let Some((start, end)) = identity_line_basis(revolution.entity_curve()) else {
                return false;
            };
            let axis = revolution.axis();
            let components = [axis.x, axis.y, axis.z];
            let Some(axial) = components
                .iter()
                .position(|value| *value == 1.0 || *value == -1.0)
            else {
                return false;
            };
            start
                .iter()
                .chain(end.iter())
                .all(|value| value.is_finite())
                && start[axial] != end[axial]
                && (0..3).all(|i| i == axial || (components[i] == 0.0 && start[i] == end[i]))
        }
        _ => false,
    }
}

/// Recognize bounded identity/sense wrappers without cloning or replacing the
/// native basis. Pose-changing wrappers retain the general interval proof path.
fn identity_line_basis(mut curve: &Curve) -> Option<([f64; 3], [f64; 3])> {
    for _ in 0..32 {
        match curve {
            Curve::Line(profile) => {
                return Some((
                    [profile.0.x, profile.0.y, profile.0.z],
                    [profile.1.x, profile.1.y, profile.1.z],
                ));
            }
            Curve::TrimmedCurve(processor) => {
                let identity = monstertruck_modeling::Matrix4::new(
                    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
                );
                if *processor.transform() != identity {
                    return None;
                }
                curve = processor.entity().curve().as_ref();
            }
            _ => return None,
        }
    }
    None
}

fn prepared_curve_support(
    curve: &Curve,
    prepared: parametric::PreparedCurveEnclosure<'_>,
    direction: [Interval; 3],
    tolerance: f64,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ProjectedBounds, GeometryError> {
    let range = curve.range_tuple();
    let domain = [[range.0, range.1], [0.0, 0.0]];
    validate_domain(domain, 1)?;
    // The caller binds original boundaries to the definition's retained native
    // continuation; every adaptive child consumes that same prepared source.
    // Proof storage and subdivision cells share one cumulative allocation
    // authority; dropping a previous curve's capture does not reset the charge.
    scratch.allocated_bytes = scratch
        .allocated_bytes
        .checked_add(prepared.retained_bytes())
        .filter(|bytes| *bytes <= scratch.byte_limit)
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "native curve proof capacity exhausts directional scratch allocation",
            )
        })?;
    if identity_line_basis(curve).is_some() {
        let a = prepared.jet((range.0, range.0), work, cancel)?;
        let b = prepared.jet((range.1, range.1), work, cancel)?;
        let a = project(a.position, direction)?;
        let b = project(b.position, direction)?;
        let result = ProjectedBounds {
            minimum: [a.lo.min(b.lo), a.hi.min(b.hi)],
            maximum: [a.lo.max(b.lo), a.hi.max(b.hi)],
            achieved_uncertainty_mm: 0.0,
        };
        if uncertainty(&result) > tolerance {
            return Err(error(
                GeometryErrorCode::ConditioningFailure,
                "native line evaluation cannot meet directional endpoint uncertainty",
            ));
        }
        return Ok(result);
    }
    let mut support = None;
    let mut covered_until = range.0;
    for span in prepared.parameter_intervals() {
        work.charge(1, cancel)?;
        if span.0 != covered_until || span.1 <= span.0 || span.1 > range.1 {
            return Err(error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "native curve spans do not give an ordered complete parameter cover",
            ));
        }
        covered_until = span.1;
        let result = search(
            [[span.0, span.1], [0.0, 0.0]],
            1,
            tolerance,
            scratch,
            work,
            cancel,
            |cell, work, cancel| {
                let span = (cell[0][0], cell[0][1]);
                let enclosure = prepared.jet(span, work, cancel)?;
                let midpoint = span.0 * 0.5 + span.1 * 0.5;
                let point = prepared.jet((midpoint, midpoint), work, cancel)?;
                let h = project(point.position, direction)?;
                let delta = Interval {
                    lo: parametric::down(span.0 - midpoint),
                    hi: parametric::up(span.1 - midpoint),
                };
                // Full derivative bounds remain valid across continuous spline knots;
                // a point derivative plus a piecewise second derivative does not.
                let centered = h.add(project(enclosure.derivative, direction)?.mul(delta));
                Ok(Some(Evaluation {
                    height: intersect(project(enclosure.position, direction)?, centered)?,
                    witness: Some(h),
                    split_axis: 0,
                }))
            },
        )?;
        combine(&mut support, result);
    }
    if covered_until != range.1 {
        return Err(error(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "native curve span cover ends before the boundary endpoint",
        ));
    }
    let mut result = support.ok_or_else(|| {
        error(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "native boundary has no bounded parameter spans",
        )
    })?;
    result.achieved_uncertainty_mm = uncertainty(&result);
    Ok(result)
}

fn surface_taylor(
    cell: [[f64; 2]; 2],
    midpoint: [f64; 2],
    enclosure: &parametric::SurfaceEnclosure,
    point: &parametric::SurfaceEnclosure,
    direction: [Interval; 3],
) -> Result<Interval, GeometryError> {
    let delta = [0, 1].map(|i| Interval {
        lo: parametric::down(cell[i][0] - midpoint[i]),
        hi: parametric::up(cell[i][1] - midpoint[i]),
    });
    Ok(project(point.position, direction)?
        // The mean-value enclosure uses derivatives over the entire box. This
        // includes piecewise smooth knot spans without assuming C1 continuity.
        .add(project(enclosure.du, direction)?.mul(delta[0]))
        .add(project(enclosure.dv, direction)?.mul(delta[1])))
}

fn project(position: [Interval; 3], direction: [Interval; 3]) -> Result<Interval, GeometryError> {
    let value = (0..3).fold(Interval::point(0.0), |sum, i| {
        sum.add(position[i].mul(direction[i]))
    });
    Interval::new(value.lo, value.hi)
}

fn intersect(a: Interval, b: Interval) -> Result<Interval, GeometryError> {
    Interval::new(a.lo.max(b.lo), a.hi.min(b.hi)).map_err(|_| {
        error(
            GeometryErrorCode::ConditioningFailure,
            "native support enclosures disagree",
        )
    })
}

#[derive(Clone, Copy)]
struct Evaluation {
    height: Interval,
    witness: Option<Interval>,
    split_axis: usize,
}

#[derive(Clone, Copy)]
struct Cell {
    uv: [[f64; 2]; 2],
    ancestor: Interval,
    priority: f64,
    depth: u16,
}
impl PartialEq for Cell {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority
    }
}
impl Eq for Cell {}
impl PartialOrd for Cell {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Cell {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority.total_cmp(&other.priority)
    }
}

struct Scratch {
    cells: BinaryHeap<Cell>,
    created: u64,
    allocated_bytes: usize,
    byte_limit: usize,
}
impl Default for Scratch {
    fn default() -> Self {
        Self {
            cells: BinaryHeap::new(),
            created: 0,
            allocated_bytes: 0,
            byte_limit: MAX_SCRATCH_ALLOCATION_BYTES,
        }
    }
}
impl Scratch {
    fn push(
        &mut self,
        cell: Cell,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        if self.created >= MAX_CREATED_CELLS || self.cells.len() >= MAX_LIVE_CELLS {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "directional subdivision cell cap exhausted",
            ));
        }
        if self.cells.len() == self.cells.capacity() {
            let additional = self
                .cells
                .capacity()
                .max(16)
                .min(MAX_LIVE_CELLS - self.cells.len());
            let bytes = additional
                .checked_mul(std::mem::size_of::<Cell>())
                .and_then(|bytes| self.allocated_bytes.checked_add(bytes))
                .ok_or_else(|| {
                    error(
                        GeometryErrorCode::ResourceLimit,
                        "directional scratch size overflow",
                    )
                })?;
            if bytes > self.byte_limit {
                return Err(error(
                    GeometryErrorCode::ResourceLimit,
                    "cumulative directional scratch allocation cap exhausted",
                ));
            }
            let before = self.cells.capacity();
            self.cells.try_reserve_exact(additional).map_err(|_| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "directional scratch allocation failed",
                )
            })?;
            self.allocated_bytes += (self.cells.capacity() - before) * std::mem::size_of::<Cell>();
            if self.allocated_bytes > self.byte_limit {
                return Err(error(
                    GeometryErrorCode::ResourceLimit,
                    "directional allocator capacity exceeds cap",
                ));
            }
        }
        self.created += 1;
        self.cells.push(cell);
        Ok(())
    }
}

fn search(
    domain: [[f64; 2]; 2],
    dimensions: usize,
    tolerance: f64,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    evaluate: impl FnMut(
        [[f64; 2]; 2],
        &mut GeometryWorkBudget,
        &AtomicBool,
    ) -> Result<Option<Evaluation>, GeometryError>,
) -> Result<ProjectedBounds, GeometryError> {
    search_domains(
        std::iter::once(domain),
        dimensions,
        tolerance,
        scratch,
        work,
        cancel,
        evaluate,
    )
}

fn search_domains(
    domains: impl Iterator<Item = [[f64; 2]; 2]>,
    dimensions: usize,
    tolerance: f64,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut evaluate: impl FnMut(
        [[f64; 2]; 2],
        &mut GeometryWorkBudget,
        &AtomicBool,
    ) -> Result<Option<Evaluation>, GeometryError>,
) -> Result<ProjectedBounds, GeometryError> {
    scratch.cells.clear();
    // All smooth source spans share feasible extrema. A span that only touches
    // a trim seam need not independently contain an interior witness.
    for domain in domains {
        validate_domain(domain, dimensions)?;
        scratch.push(
            Cell {
                uv: domain,
                ancestor: Interval {
                    lo: f64::NEG_INFINITY,
                    hi: f64::INFINITY,
                },
                priority: f64::INFINITY,
                depth: 0,
            },
            work,
            cancel,
        )?;
    }
    let mut feasible_min = f64::INFINITY;
    let mut feasible_max = f64::NEG_INFINITY;
    let mut outer_min = f64::INFINITY;
    let mut outer_max = f64::NEG_INFINITY;
    while let Some(cell) = scratch.cells.pop() {
        work.charge(1, cancel)?;
        let mut conditioning = false;
        let evaluation = match evaluate(cell.uv, work, cancel) {
            Ok(None) => continue,
            Ok(Some(mut value)) => {
                value.height = intersect(value.height, cell.ancestor)?;
                Some(value)
            }
            Err(failure) if failure.code == GeometryErrorCode::ConditioningFailure => {
                conditioning = true;
                None
            }
            Err(failure) => return Err(failure),
        };
        let (height, suggested_axis) = if let Some(evaluation) = evaluation {
            if let Some(witness) = evaluation.witness {
                feasible_min = feasible_min.min(witness.hi);
                feasible_max = feasible_max.max(witness.lo);
            }
            if feasible_min.is_finite()
                && feasible_max.is_finite()
                && parametric::up(feasible_min - evaluation.height.lo) <= tolerance
                && parametric::up(evaluation.height.hi - feasible_max) <= tolerance
            {
                outer_min = outer_min.min(evaluation.height.lo);
                outer_max = outer_max.max(evaluation.height.hi);
                continue;
            }
            (evaluation.height, evaluation.split_axis)
        } else {
            (
                cell.ancestor,
                if width(cell.uv[0]) >= width(cell.uv[1]) {
                    0
                } else {
                    1
                },
            )
        };
        if cell.depth >= MAX_DEPTH {
            return Err(error(
                if conditioning {
                    GeometryErrorCode::ConditioningFailure
                } else {
                    GeometryErrorCode::AnalysisIndeterminate
                },
                "native directional subdivision depth cannot resolve endpoint uncertainty",
            ));
        }
        let axis = if suggested_axis < dimensions {
            suggested_axis
        } else {
            0
        };
        let axis = if midpoint(cell.uv[axis]).is_some() {
            axis
        } else if dimensions == 2 && midpoint(cell.uv[1 - axis]).is_some() {
            1 - axis
        } else {
            return Err(error(
                if conditioning {
                    GeometryErrorCode::ConditioningFailure
                } else {
                    GeometryErrorCode::AnalysisIndeterminate
                },
                "native directional parameters cannot resolve endpoint uncertainty",
            ));
        };
        let middle = midpoint(cell.uv[axis]).expect("splittable parameter interval");
        let mut left = cell.uv;
        let mut right = cell.uv;
        left[axis][1] = middle;
        right[axis][0] = middle;
        let priority = if height.lo.is_finite() && height.hi.is_finite() {
            height.width()
        } else {
            f64::INFINITY
        };
        for uv in [left, right] {
            scratch.push(
                Cell {
                    uv,
                    ancestor: height,
                    priority,
                    depth: cell.depth + 1,
                },
                work,
                cancel,
            )?;
        }
    }
    if !feasible_min.is_finite()
        || !feasible_max.is_finite()
        || !outer_min.is_finite()
        || !outer_max.is_finite()
    {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "true native trimmed domain has no certified feasible support witness",
        ));
    }
    let mut result = ProjectedBounds {
        minimum: [outer_min, feasible_min],
        maximum: [feasible_max, outer_max],
        achieved_uncertainty_mm: 0.0,
    };
    result.achieved_uncertainty_mm = uncertainty(&result);
    if result.minimum[0] > result.minimum[1]
        || result.maximum[0] > result.maximum[1]
        || result.achieved_uncertainty_mm > tolerance
    {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "native feasible witnesses disagree with directional exclusion bounds",
        ));
    }
    Ok(result)
}

fn combine(aggregate: &mut Option<ProjectedBounds>, next: ProjectedBounds) {
    if let Some(value) = aggregate {
        for i in 0..2 {
            value.minimum[i] = value.minimum[i].min(next.minimum[i]);
            value.maximum[i] = value.maximum[i].max(next.maximum[i]);
        }
    } else {
        *aggregate = Some(next);
    }
}
fn uncertainty(value: &ProjectedBounds) -> f64 {
    parametric::up(value.minimum[1] - value.minimum[0])
        .max(parametric::up(value.maximum[1] - value.maximum[0]))
}
fn width(span: [f64; 2]) -> f64 {
    parametric::up(span[1] - span[0])
}
fn midpoint(span: [f64; 2]) -> Option<f64> {
    let value = span[0] * 0.5 + span[1] * 0.5;
    (span[0] < value && value < span[1]).then_some(value)
}
fn validate_domain(domain: [[f64; 2]; 2], dimensions: usize) -> Result<(), GeometryError> {
    if domain[..dimensions]
        .iter()
        .any(|span| !span[0].is_finite() || !span[1].is_finite() || span[0] >= span[1])
    {
        return Err(error(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "native trimmed support domain is not finite and ordered",
        ));
    }
    Ok(())
}
fn error(code: GeometryErrorCode, message: &str) -> GeometryError {
    GeometryError::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_implicit_proof_is_pinned_before_directional_scratch() {
        let source = crate::corpus_fixtures::implicit_intersections(
            include_str!("../../../../fixtures/geometry/box-mm.step"),
            "implicit.step",
        );
        let cancel = AtomicBool::new(false);
        let definition = crate::import_step(source.as_bytes(), &cancel).unwrap();
        assert!(definition.boundary_continuations.iter().flatten().count() > 0);
        let pinned = definition.retained_query_proof_bytes();
        assert!(pinned > 0);
        let mut work = GeometryWorkBudget::new(20_000_000);
        let policy = BoundsPolicy {
            maximum_endpoint_uncertainty_mm: 0.001,
        };
        let failure = projected_bounds_with_total_limit(
            &definition,
            [0., 0., 1.],
            policy,
            pinned - 1,
            &mut work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::ResourceLimit);
        assert!(failure.message.contains("retained native query proof"));
        let bounds =
            projected_bounds(&definition, [0., 0., 1.], policy, &mut work, &cancel).unwrap();
        assert!(bounds.maximum[0] <= 8. && bounds.maximum[1] >= 8.);
    }

    fn polynomial_search(tolerance: f64) -> ProjectedBounds {
        let mut scratch = Scratch::default();
        let mut work = GeometryWorkBudget::new(100_000);
        let cancel = AtomicBool::new(false);
        search(
            [[-1.0, 1.0], [-1.0, 1.0]],
            2,
            tolerance,
            &mut scratch,
            &mut work,
            &cancel,
            |uv, _, _| {
                let u = Interval::new(uv[0][0], uv[0][1])?;
                let v = Interval::new(uv[1][0], uv[1][1])?;
                let height = Interval::point(2.0).sub(u.square()).sub(v.square());
                let witness = Interval::point(2.0)
                    .sub(Interval::point(u.mid()).square())
                    .sub(Interval::point(v.mid()).square());
                Ok(Some(Evaluation {
                    height,
                    witness: Some(witness),
                    split_axis: if u.width() >= v.width() { 0 } else { 1 },
                }))
            },
        )
        .unwrap()
    }

    #[test]
    fn interior_extremum_is_not_replaced_by_boundary_or_control_box_contact() {
        let result = polynomial_search(0.001);
        assert!(result.minimum[0] <= 0.0 && result.minimum[1] >= 0.0);
        assert!(result.maximum[0] <= 2.0 && result.maximum[1] >= 2.0);
        assert!(result.achieved_uncertainty_mm <= 0.001);
        assert!(result.minimum[1] > result.minimum[0]);
    }

    #[test]
    fn native_boundary_corner_is_covered_without_crossing_a_nonsmooth_knot() {
        use monstertruck_modeling::{BsplineCurve, KnotVector, Point3};
        let curve = Curve::BsplineCurve(BsplineCurve::new(
            KnotVector::from(vec![0.0, 0.0, 0.5, 1.0, 1.0]),
            vec![
                Point3::new(-1.0, 0.0, 0.0),
                Point3::new(0.0, 0.0, 2.0),
                Point3::new(1.0, 0.0, 0.0),
            ],
        ));
        let mut scratch = Scratch::default();
        let mut work = GeometryWorkBudget::new(100_000);
        let cancel = AtomicBool::new(false);
        let prepared = parametric::prepare_curve_enclosure(
            &curve,
            curve.range_tuple(),
            scratch.byte_limit - scratch.allocated_bytes,
            &mut work,
            &cancel,
        )
        .unwrap();
        let result = prepared_curve_support(
            &curve,
            prepared,
            [
                Interval::point(0.0),
                Interval::point(0.0),
                Interval::point(1.0),
            ],
            0.001,
            &mut scratch,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(result.minimum[0] <= 0.0 && result.minimum[1] >= 0.0);
        assert!(result.maximum[0] <= 2.0 && result.maximum[1] >= 2.0);
        assert!(result.achieved_uncertainty_mm <= 0.001);
    }

    #[test]
    fn native_quadratic_spline_includes_true_interior_support_not_control_hull() {
        use monstertruck_modeling::{BsplineSurface, KnotVector, Point3};
        let knots = KnotVector::bezier_knot(2);
        let heights = [0.0, 2.0, 0.0];
        let controls = (0..3)
            .map(|u| {
                (0..3)
                    .map(|v| Point3::new(u as f64 * 0.5, v as f64 * 0.5, heights[u] + heights[v]))
                    .collect()
            })
            .collect();
        let surface =
            Surface::BsplineSurface(BsplineSurface::new((knots.clone(), knots), controls));
        let direction = [
            Interval::point(0.0),
            Interval::point(0.0),
            Interval::point(1.0),
        ];
        let mut scratch = Scratch::default();
        let mut work = GeometryWorkBudget::new(500_000);
        let cancel = AtomicBool::new(false);
        // This face's actual trim is the entire closed Bezier rectangle.
        let result = search(
            [[0.0, 1.0], [0.0, 1.0]],
            2,
            0.002,
            &mut scratch,
            &mut work,
            &cancel,
            |uv, work, cancel| {
                let enclosure = parametric::surface_enclosure(&surface, uv, work, cancel)?;
                let midpoint = uv.map(|span| span[0] * 0.5 + span[1] * 0.5);
                let point = parametric::surface_enclosure(
                    &surface,
                    midpoint.map(|x| [x, x]),
                    work,
                    cancel,
                )?;
                Ok(Some(Evaluation {
                    height: intersect(
                        project(enclosure.position, direction)?,
                        surface_taylor(uv, midpoint, &enclosure, &point, direction)?,
                    )?,
                    witness: Some(project(point.position, direction)?),
                    split_axis: if width(uv[0]) >= width(uv[1]) { 0 } else { 1 },
                }))
            },
        )
        .unwrap();
        assert!(result.minimum[0] <= 0.0 && result.minimum[1] >= 0.0);
        assert!(result.maximum[0] <= 2.0 && result.maximum[1] >= 2.0);
        assert!(
            result.maximum[1] < 2.01,
            "the control hull reaches four, not two"
        );
        assert!(result.achieved_uncertainty_mm <= 0.002);
    }

    #[test]
    fn native_imported_box_and_cylinder_preserve_separate_endpoint_brackets() {
        let cancel = AtomicBool::new(false);
        for (name, direction, expected) in [
            ("box-mm.step", [1.0, 1.0, 0.0], [0.0, 30.0 / 2.0_f64.sqrt()]),
            (
                "cylinder.step",
                [1.0, 1.0, 1.0],
                [
                    -5.0 * (2.0_f64 / 3.0).sqrt(),
                    8.0 / 3.0_f64.sqrt() + 5.0 * (2.0_f64 / 3.0).sqrt(),
                ],
            ),
        ] {
            let bytes = std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../fixtures/geometry")
                    .join(name),
            )
            .unwrap();
            let definition = crate::import_step(&bytes, &cancel).unwrap();
            let mut work = GeometryWorkBudget::new(2_000_000);
            let support = projected_bounds(
                &definition,
                direction,
                BoundsPolicy {
                    maximum_endpoint_uncertainty_mm: 0.00001,
                },
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(support.minimum[0] <= expected[0] && expected[0] <= support.minimum[1]);
            assert!(support.maximum[0] <= expected[1] && expected[1] <= support.maximum[1]);
            assert!(support.achieved_uncertainty_mm <= 0.00001);
            assert!(work.consumed() > 0);
        }
    }

    #[test]
    fn outside_trim_never_supplies_a_witness() {
        let mut scratch = Scratch::default();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000);
        let result = search(
            [[0.0, 1.0], [0.0, 0.0]],
            1,
            0.001,
            &mut scratch,
            &mut work,
            &cancel,
            |uv, _, _| {
                if uv[0][1] < 0.75 {
                    return Ok(None);
                }
                let height = Interval::new(uv[0][0], uv[0][1])?;
                let mid = height.mid();
                Ok(Some(Evaluation {
                    height,
                    witness: (mid >= 0.75).then_some(Interval::point(mid)),
                    split_axis: 0,
                }))
            },
        )
        .unwrap();
        assert!(result.minimum[0] <= 0.75 && result.minimum[1] >= 0.75);
        assert!(result.minimum[0] > 0.74);
        assert!(result.maximum[0] <= 1.0 && result.maximum[1] >= 1.0);
    }

    #[test]
    fn trim_seam_only_span_uses_feasible_witnesses_from_other_source_spans() {
        let mut scratch = Scratch::default();
        let mut work = GeometryWorkBudget::new(10_000);
        let cancel = AtomicBool::new(false);
        let domains = [[[-1.0, 0.0], [0.0, 0.0]], [[0.0, 1.0], [0.0, 0.0]]];
        let result = search_domains(
            domains.into_iter(),
            1,
            0.001,
            &mut scratch,
            &mut work,
            &cancel,
            |uv, _, _| {
                if uv[0][1] < 0.0 {
                    return Ok(None);
                }
                let height = Interval::new(uv[0][0], uv[0][1])?;
                let midpoint = height.mid();
                Ok(Some(Evaluation {
                    height,
                    witness: (midpoint > 0.0).then_some(Interval::point(midpoint)),
                    split_axis: 0,
                }))
            },
        )
        .unwrap();
        assert!(result.minimum[0] <= 0.0 && result.minimum[1] >= 0.0);
        assert!(result.maximum[0] <= 1.0 && result.maximum[1] >= 1.0);
        assert!(result.achieved_uncertainty_mm <= 0.001);
    }

    #[test]
    fn resource_and_cancellation_are_terminal_not_partial_bounds() {
        for (maximum_work, cancelled, expected) in [
            (0, false, GeometryErrorCode::ResourceLimit),
            (100, true, GeometryErrorCode::Cancelled),
        ] {
            let mut scratch = Scratch::default();
            let mut work = GeometryWorkBudget::new(maximum_work);
            let cancel = AtomicBool::new(cancelled);
            let failure = search(
                [[0.0, 1.0], [0.0, 0.0]],
                1,
                0.001,
                &mut scratch,
                &mut work,
                &cancel,
                |_, _, _| {
                    Ok(Some(Evaluation {
                        height: Interval::point(0.0),
                        witness: Some(Interval::point(0.0)),
                        split_axis: 0,
                    }))
                },
            )
            .unwrap_err();
            assert_eq!(failure.code, expected);
        }
    }

    #[test]
    fn enclosing_job_capacity_narrows_search_allocation_before_growth() {
        let mut scratch = Scratch {
            byte_limit: 1,
            ..Scratch::default()
        };
        let mut work = GeometryWorkBudget::new(100);
        let cancel = AtomicBool::new(false);
        let failure = search(
            [[0.0, 1.0], [0.0, 0.0]],
            1,
            0.001,
            &mut scratch,
            &mut work,
            &cancel,
            |_, _, _| {
                Ok(Some(Evaluation {
                    height: Interval::point(0.0),
                    witness: Some(Interval::point(0.0)),
                    split_axis: 0,
                }))
            },
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::ResourceLimit);
        assert_eq!(scratch.allocated_bytes, 0);
    }

    #[test]
    fn unresolved_rational_conditioning_remains_a_typed_failure() {
        let mut scratch = Scratch::default();
        let mut work = GeometryWorkBudget::new(1_000);
        let cancel = AtomicBool::new(false);
        // The smallest representable interval cannot be split; a denominator
        // containing zero is not turned into a finite sampled support value.
        let failure = search(
            [[0.0, f64::from_bits(1)], [0.0, 0.0]],
            1,
            0.001,
            &mut scratch,
            &mut work,
            &cancel,
            |_, _, _| {
                Err(error(
                    GeometryErrorCode::ConditioningFailure,
                    "denominator contains zero",
                ))
            },
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::ConditioningFailure);
    }

    #[test]
    fn direction_normalization_is_enclosed_and_rejects_invalid_inputs() {
        let d = normalized_direction([3.0, 4.0, 0.0]).unwrap();
        assert!(d[0].lo <= 0.6 && d[0].hi >= 0.6);
        assert!(d[1].lo <= 0.8 && d[1].hi >= 0.8);
        for magnitude in [f64::from_bits(1), 1.0, f64::MAX] {
            for sign in [-1.0, 1.0] {
                let axis = normalized_direction([0.0, sign * magnitude, -0.0]).unwrap();
                assert_eq!([axis[0].lo, axis[0].hi], [0.0, 0.0]);
                assert_eq!([axis[1].lo, axis[1].hi], [sign, sign]);
                assert_eq!([axis[2].lo, axis[2].hi], [0.0, 0.0]);
            }
        }
        assert!(normalized_direction([0.0; 3]).is_err());
        assert!(normalized_direction([f64::NAN, 0.0, 1.0]).is_err());
    }
}
