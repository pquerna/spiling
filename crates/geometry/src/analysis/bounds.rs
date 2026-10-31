// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Directional support from immutable native coefficients and literal trims.
//! Chart enclosures exclude extrema; only feasible native material points supply
//! contact. Original STEP correspondence is not a material prerequisite.

use super::{
    BoundsPolicy, GeometryWorkBudget, ProjectedBounds, material_source::MaterialSourceView,
};
use crate::{
    Definition, GeometryError,
    kernel::{PreparedNativeTrimSource, PreparedTrimSurface, TrimClassification},
    parametric::{self, Interval},
};
use spiling_contracts::geometry::{DefinitionId, GeometryErrorCode};
use std::{cmp::Ordering, collections::BinaryHeap, sync::atomic::AtomicBool};

const MAX_LIVE_CELLS: usize = 131_072;
const MAX_CREATED_CELLS: u64 = 2_000_000;
pub(crate) const MAX_SCRATCH_ALLOCATION_BYTES: usize = 32 * 1024 * 1024;
const MAX_DEPTH: u16 = 96;

/// Enclose the minimum and maximum of mathematically normalized `direction`
/// on the native, literally trimmed authoring faces. This is not material proof.
pub fn projected_bounds(
    definition: &Definition,
    direction: [f64; 3],
    policy: BoundsPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ProjectedBounds, GeometryError> {
    work.charge(1, cancel)?;
    validate_query(direction, policy)?;
    let remaining = MAX_SCRATCH_ALLOCATION_BYTES
        .checked_sub(definition.retained_query_proof_bytes())
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "retained native query proof exhausts directional allocation cap",
            )
        })?;
    let source = PreparedNativeTrimSource::prepare_charts(definition, remaining, work, cancel)?;
    let remaining = remaining
        .checked_sub(source.retained_bytes())
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "retained native trim source exhausts directional allocation cap",
            )
        })?;
    projected_bounds_with_scratch_limit(&source, direction, policy, remaining, work, cancel)
}

/// Support receipt for the immutable native BREP material. The effective
/// topology tolerance is explicit and is not an original-source error bound.
#[derive(Debug, Clone)]
pub struct MaterialProjectedBounds {
    bounds: ProjectedBounds,
    definition_id: DefinitionId,
    material_realization: crate::kernel::MaterialRealizationIdentity,
    direction: [f64; 3],
    policy: BoundsPolicy,
    effective_tolerance_mm: f64,
}

impl MaterialProjectedBounds {
    pub fn bounds(&self) -> ProjectedBounds {
        self.bounds
    }

    pub fn definition_id(&self) -> &DefinitionId {
        &self.definition_id
    }

    pub fn effective_tolerance_mm(&self) -> f64 {
        self.effective_tolerance_mm
    }

    pub fn direction(&self) -> [f64; 3] {
        self.direction
    }

    pub fn policy(&self) -> BoundsPolicy {
        self.policy
    }

    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.definition_id.allocated_bytes()
    }

    pub(crate) fn material_realization_identity(
        &self,
    ) -> crate::kernel::MaterialRealizationIdentity {
        self.material_realization
    }
}

/// Enclose the native material's directional support. Import provenance remains
/// separate from the native coefficients, trims and declared effective tolerance.
pub fn material_projected_bounds(
    definition: &Definition,
    direction: [f64; 3],
    policy: BoundsPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<MaterialProjectedBounds, GeometryError> {
    work.charge(1, cancel)?;
    validate_query(direction, policy)?;
    let prepared = super::material_source::prepare_solid(
        definition,
        MAX_SCRATCH_ALLOCATION_BYTES,
        work,
        cancel,
    )?;
    prepared.material_bounds(
        direction,
        policy,
        MAX_SCRATCH_ALLOCATION_BYTES,
        work,
        cancel,
    )
}

/// `remaining_bytes` excludes the enclosing owner's retained source and every
/// coexisting allocation. An authoring-only chart owner cannot mint this receipt.
pub(super) fn material_projected_bounds_with_scratch_limit(
    source: MaterialSourceView<'_, '_>,
    direction: [f64; 3],
    policy: BoundsPolicy,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<MaterialProjectedBounds, GeometryError> {
    work.charge(1, cancel)?;
    validate_query(direction, policy)?;
    let definition = source.definition();
    // Reserve receipt storage while the search is live, before heap growth.
    let capture_bytes = std::mem::size_of::<MaterialProjectedBounds>()
        .checked_add(definition.id().as_str().len())
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "material directional capture size overflow",
            )
        })?;
    let remaining_bytes = remaining_bytes
        .min(MAX_SCRATCH_ALLOCATION_BYTES)
        .checked_sub(capture_bytes)
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "material directional capture exhausts allocation cap",
            )
        })?;
    let bounds = projected_bounds_with_scratch_limit(
        source.trim_source(),
        direction,
        policy,
        remaining_bytes,
        work,
        cancel,
    )?;
    work.charge(1, cancel)?;
    Ok(MaterialProjectedBounds {
        bounds,
        definition_id: definition.id().clone(),
        material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
        direction,
        policy,
        effective_tolerance_mm: source.effective_tolerance_mm(),
    })
}

/// Support from admitted native trim domains, coefficient surfaces and spatial
/// boundary witnesses; original STEP finish operands are not material carriers.
pub(super) fn projected_bounds_with_scratch_limit(
    source: &PreparedNativeTrimSource<'_>,
    direction: [f64; 3],
    policy: BoundsPolicy,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ProjectedBounds, GeometryError> {
    native_projected_bounds(source, direction, policy, remaining_bytes, work, cancel)
}

fn native_projected_bounds(
    source: &PreparedNativeTrimSource<'_>,
    direction: [f64; 3],
    policy: BoundsPolicy,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ProjectedBounds, GeometryError> {
    work.charge(1, cancel)?;
    let direction = validate_query(direction, policy)?;
    let tolerance = policy.maximum_endpoint_uncertainty_mm;
    // Coefficient evaluation uses fixed stack scratch, never a per-cell heap.
    // Reserve it together with the bounded live heap under the unchanged cap.
    let evaluator_bytes = parametric::SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES
        .max(crate::kernel::source_operand::SOURCE_OPERAND_SCRATCH_BYTES);
    let byte_limit = remaining_bytes.min(MAX_SCRATCH_ALLOCATION_BYTES);
    if evaluator_bytes > byte_limit {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "native coefficient scratch exhausts directional allocation cap",
        ));
    }
    let mut scratch = Scratch {
        allocated_bytes: evaluator_bytes,
        byte_limit,
        ..Scratch::default()
    };
    let definition = source.definition();
    let mut aggregate: Option<ProjectedBounds> = None;
    for ordinal in 0..definition.face_ids().len() {
        work.charge(1, cancel)?;
        let trim = source.face_domain(ordinal).ok_or_else(|| {
            error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "native directional source omitted a face trim domain",
            )
        })?;
        if trim.is_native_material() {
            for (index, coedge) in trim.coedges().iter().enumerate() {
                work.charge(1, cancel)?;
                // Native topology owns its spatial boundary and vertices.
                // They are feasible material points, not original-STEP witnesses.
                for vertex in coedge.vertices {
                    let height = project(
                        source
                            .native_vertex_point(vertex, work, cancel)?
                            .map(Interval::point),
                        direction,
                    )?;
                    let point = ProjectedBounds {
                        minimum: [height.lo, height.hi],
                        maximum: [height.lo, height.hi],
                        achieved_uncertainty_mm: height.hi - height.lo,
                    };
                    aggregate = Some(match aggregate {
                        Some(previous) => ProjectedBounds {
                            minimum: [
                                previous.minimum[0].min(point.minimum[0]),
                                previous.minimum[1].min(point.minimum[1]),
                            ],
                            maximum: [
                                previous.maximum[0].max(point.maximum[0]),
                                previous.maximum[1].max(point.maximum[1]),
                            ],
                            achieved_uncertainty_mm: 0.0,
                        },
                        None => point,
                    });
                }
                let range = coedge.oriented_native_range();
                let range = [range[0].min(range[1]), range[0].max(range[1])];
                // The borrowed native adapter allocates no heap. Its evaluator
                // shares the fixed reservation already included in Scratch.
                let curve_before = work.consumed();
                let support = trim.with_native_spatial_curve(
                    index, evaluator_bytes, work, cancel, |curve, work, cancel| {
                        let control_support = parametric::native_curve_functional_control_bounds(
                            curve, range, direction, Interval::point(0.0), work, cancel,
                        )?;
                        search_domains(
                            std::iter::once([range, [0.0, 0.0]]),
                            1, tolerance, aggregate, &mut scratch, work, cancel,
                            |cell, work, cancel| {
                                let jet = curve.tensor(cell[0], 1, work, cancel)?;
                                let height = project(jet[0], direction)?;
                                let height = match control_support {
                                    Some(control) => intersect(height, control)?,
                                    None => height,
                                };
                                let middle = cell[0][0] * 0.5 + cell[0][1] * 0.5;
                                let witness = project(curve.tensor([middle, middle], 0, work, cancel)?[0], direction)?;
                                let delta = Interval {
                                    lo: parametric::down(cell[0][0] - middle),
                                    hi: parametric::up(cell[0][1] - middle),
                                };
                                let centered = witness.add(project(jet[1], direction)?.mul(delta));
                                Ok(Some(Evaluation {
                                    height: intersect(height, centered)?,
                                    witness: Some(witness),
                                    split_axis: 0,
                                }))
                            },
                        )
                    },
                ).map_err(|error| work.annotate_error(
                    error, curve_before,
                    format_args!("native directional edge; face={ordinal}; coedge={index}; edge={:?}; range={range:?}; direction={direction:?}", coedge.edge),
                ))?;
                aggregate = Some(support);
            }
        }
        let PreparedTrimSurface::Native(surface) = trim.surface_operand() else {
            return Err(error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "native directional source requires a native trim surface",
            ));
        };
        let before = work.consumed();
        let uv = trim.support_uv_cover(work, cancel)?;
        let boxes = parametric::surface_parameter_boxes(surface, uv, work, cancel)?;
        // Prior faces supply independently feasible points of this same source
        // under the same covector. The query needs global support, not tight
        // extrema of every noncontact face. Each new face still has its complete
        // coefficient/trim cover searched; the seed never clips its enclosures
        // or supplies a feasible point on that particular face.
        let support = search_domains(
            boxes, 2, tolerance, aggregate, &mut scratch, work, cancel,
            |cell, work, cancel| {
                // Enclose the complete coefficient sheet before asking whether
                // its trimmed subset can furnish contact. Prior source points
                // may already exclude the whole sheet from global extrema.
                // Native first bounds retain complete knot/denominator validation.
                let [enclosure] = parametric::surface_functional_enclosures(
                    surface, cell,
                    &[parametric::SurfaceFunctional {
                        direction,
                        origin: [Interval::point(0.0); 3],
                        offset: Interval::point(0.0),
                    }],
                    work, cancel,
                )?;
                let height = enclosure.position;
                if aggregate.is_some_and(|seed| {
                    resolved_by_feasible_extrema(
                        height, seed.minimum[1], seed.maximum[0], tolerance,
                    )
                }) {
                    return Ok(Some(Evaluation {
                        height,
                        witness: None,
                        split_axis: 0,
                    }));
                }
                let class = trim.classify(cell, work, cancel)?;
                if matches!(class, TrimClassification::Outside) {
                    return Ok(None);
                }
                let middle = cell.map(|span| span[0] * 0.5 + span[1] * 0.5);
                let point_cell = middle.map(|value| [value, value]);
                let [point] = parametric::surface_functional_enclosures(
                    surface, point_cell,
                    &[parametric::SurfaceFunctional {
                        direction,
                        origin: [Interval::point(0.0); 3],
                        offset: Interval::point(0.0),
                    }],
                    work, cancel,
                )?;
                let point_height = point.position;
                let delta = [0, 1].map(|axis| Interval {
                    lo: parametric::down(cell[axis][0] - middle[axis]),
                    hi: parametric::up(cell[axis][1] - middle[axis]),
                });
                let du = enclosure.du;
                let dv = enclosure.dv;
                let centered = point_height.add(du.mul(delta[0])).add(dv.mul(delta[1]));
                let height = intersect(height, centered)?;
                let split_axis = usize::from(du.abs_upper() * width(cell[0])
                    < dv.abs_upper() * width(cell[1]));
                let feasible = matches!(class, TrimClassification::Inside)
                    || matches!(trim.classify(point_cell, work, cancel)?, TrimClassification::Inside);
                Ok(Some(Evaluation {
                    height,
                    witness: feasible.then_some(point_height),
                    split_axis,
                }))
            },
        ).map_err(|failure| work.annotate_error(failure, before, format_args!(
            "native directional trimmed face; source_face={ordinal}; uv_domain={uv:?}; direction={direction:?}"
        )))?;
        aggregate = Some(support);
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
    work.charge(1, cancel)?;
    Ok(result)
}

fn validate_query(
    direction: [f64; 3],
    policy: BoundsPolicy,
) -> Result<[Interval; 3], GeometryError> {
    if !policy.maximum_endpoint_uncertainty_mm.is_finite()
        || policy.maximum_endpoint_uncertainty_mm <= 0.0
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "directional bounds require a finite positive endpoint uncertainty",
        ));
    }
    normalized_direction(direction)
}

fn normalized_direction(direction: [f64; 3]) -> Result<[Interval; 3], GeometryError> {
    if direction.iter().any(|value| !value.is_finite()) {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "nonfinite support direction",
        ));
    }
    // Scaling avoids overflow/underflow before outward interval normalization.
    let scale = direction.iter().copied().map(f64::abs).fold(0.0, f64::max);
    if scale == 0.0 {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "zero support direction",
        ));
    }
    if direction.iter().filter(|value| **value != 0.0).count() == 1 {
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

fn project(position: [Interval; 3], direction: [Interval; 3]) -> Result<Interval, GeometryError> {
    let value = (0..3).fold(Interval::point(0.0), |sum, axis| {
        sum.add(position[axis].mul(direction[axis]))
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

/// Search a complete cover, sharing independently feasible extrema across all
/// spans. `seed` must itself consist of feasible subset support brackets; its
/// outer endpoints cover only that subset, never the unevaluated chart.
#[expect(
    clippy::too_many_arguments,
    reason = "Keep candidate domains, dimension and tolerance distinct from the search scratch, evaluator and cancellation budget"
)]
fn search_domains(
    domains: impl Iterator<Item = [[f64; 2]; 2]>,
    dimensions: usize,
    tolerance: f64,
    seed: Option<ProjectedBounds>,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut evaluate: impl FnMut(
        [[f64; 2]; 2],
        &mut GeometryWorkBudget,
        &AtomicBool,
    ) -> Result<Option<Evaluation>, GeometryError>,
) -> Result<ProjectedBounds, GeometryError> {
    search_domains_with_query_scratch(
        domains,
        dimensions,
        tolerance,
        seed,
        scratch,
        work,
        cancel,
        |cell, work, cancel, _| evaluate(cell, work, cancel),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "The evaluator receives its independently admitted scratch while the search retains its own domains, seed and convergence policy"
)]
fn search_domains_with_query_scratch(
    domains: impl Iterator<Item = [[f64; 2]; 2]>,
    dimensions: usize,
    tolerance: f64,
    seed: Option<ProjectedBounds>,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut evaluate: impl FnMut(
        [[f64; 2]; 2],
        &mut GeometryWorkBudget,
        &AtomicBool,
        usize,
    ) -> Result<Option<Evaluation>, GeometryError>,
) -> Result<ProjectedBounds, GeometryError> {
    scratch.cells.clear();
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
    let mut feasible_min = seed.map_or(f64::INFINITY, |value| value.minimum[1]);
    let mut feasible_max = seed.map_or(f64::NEG_INFINITY, |value| value.maximum[0]);
    let mut outer_min = seed.map_or(f64::INFINITY, |value| value.minimum[0]);
    let mut outer_max = seed.map_or(f64::NEG_INFINITY, |value| value.maximum[1]);
    while let Some(cell) = scratch.cells.pop() {
        work.charge(1, cancel)?;
        let mut conditioning = false;
        let evaluation = match evaluate(
            cell.uv,
            work,
            cancel,
            scratch.byte_limit - scratch.allocated_bytes,
        ) {
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
            if resolved_by_feasible_extrema(
                evaluation.height,
                feasible_min,
                feasible_max,
                tolerance,
            ) {
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

/// Only independent feasible endpoints justify stopping a complete range proof.
/// Outward subtraction is shared by early sheet exclusion and final cell search.
fn resolved_by_feasible_extrema(
    height: Interval,
    feasible_min: f64,
    feasible_max: f64,
    tolerance: f64,
) -> bool {
    feasible_min.is_finite()
        && feasible_max.is_finite()
        && parametric::up(feasible_min - height.lo) <= tolerance
        && parametric::up(height.hi - feasible_max) <= tolerance
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

    fn exact_scalar_bounds() -> ProjectedBounds {
        ProjectedBounds {
            minimum: [0.0, 0.0],
            maximum: [10.0, 10.0],
            achieved_uncertainty_mm: 0.0,
        }
    }

    #[test]
    fn original_plane_without_pcurves_has_tight_support_within_placement_work_limit() {
        let definition = crate::from_step_bytes(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/geometry/box-mm.step"
        )))
        .unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(20_000_000);
        let precision = 0.00001;
        let receipt = material_projected_bounds(
            &definition,
            [1.0, 0.0, 0.0],
            BoundsPolicy {
                maximum_endpoint_uncertainty_mm: precision,
            },
            &mut work,
            &cancel,
        )
        .expect("original plane support must fit the placement work allowance");
        let bounds = receipt.bounds();
        assert!(bounds.minimum[0] <= 0.0 && 0.0 <= bounds.minimum[1]);
        assert!(bounds.maximum[0] <= 20.0 && 20.0 <= bounds.maximum[1]);
        assert!(width(bounds.minimum) <= precision);
        assert!(width(bounds.maximum) <= precision);
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
            None,
            scratch,
            work,
            cancel,
            evaluate,
        )
    }

    #[test]
    fn interior_extremum_is_not_replaced_by_boundary_or_control_box_contact() {
        let result = search(
            [[-1.0, 1.0]; 2],
            2,
            0.001,
            &mut Scratch::default(),
            &mut GeometryWorkBudget::new(100_000),
            &AtomicBool::new(false),
            |uv, _, _| {
                let u = Interval::new(uv[0][0], uv[0][1])?;
                let v = Interval::new(uv[1][0], uv[1][1])?;
                Ok(Some(Evaluation {
                    height: Interval::point(2.0).sub(u.square()).sub(v.square()),
                    witness: Some(
                        Interval::point(2.0)
                            .sub(Interval::point(u.mid()).square())
                            .sub(Interval::point(v.mid()).square()),
                    ),
                    split_axis: if u.width() >= v.width() { 0 } else { 1 },
                }))
            },
        )
        .unwrap();
        assert!(result.minimum[0] <= 0.0 && result.minimum[1] >= 0.0);
        assert!(result.maximum[0] <= 2.0 && result.maximum[1] >= 2.0);
        assert!(result.achieved_uncertainty_mm <= 0.001);
    }

    #[test]
    fn outside_trim_never_supplies_a_witness() {
        let result = search(
            [[0.0, 1.0], [0.0, 0.0]],
            1,
            0.001,
            &mut Scratch::default(),
            &mut GeometryWorkBudget::new(10_000),
            &AtomicBool::new(false),
            |uv, _, _| {
                if uv[0][1] < 0.75 {
                    return Ok(None);
                }
                let height = Interval::new(uv[0][0], uv[0][1])?;
                let middle = height.mid();
                Ok(Some(Evaluation {
                    height,
                    witness: (middle >= 0.75).then_some(Interval::point(middle)),
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
    fn chart_exclusion_alone_never_becomes_a_feasible_witness() {
        let failure = search(
            [[0.0, 1.0], [0.0, 0.0]],
            1,
            0.001,
            &mut Scratch::default(),
            &mut GeometryWorkBudget::new(100),
            &AtomicBool::new(false),
            |_, _, _| {
                Ok(Some(Evaluation {
                    height: Interval::point(0.0),
                    witness: None,
                    split_axis: 0,
                }))
            },
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::ResourceLimit);
    }

    #[test]
    fn trim_seam_span_shares_only_genuine_feasible_witnesses() {
        let domains = [[[-1.0, 0.0], [0.0, 0.0]], [[0.0, 1.0], [0.0, 0.0]]];
        let result = search_domains(
            domains.into_iter(),
            1,
            0.001,
            None,
            &mut Scratch::default(),
            &mut GeometryWorkBudget::new(10_000),
            &AtomicBool::new(false),
            |uv, _, _| {
                if uv[0][1] < 0.0 {
                    return Ok(None);
                }
                let height = Interval::new(uv[0][0], uv[0][1])?;
                let middle = height.mid();
                Ok(Some(Evaluation {
                    height,
                    witness: (middle > 0.0).then_some(Interval::point(middle)),
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
    fn feasible_seed_does_not_clip_an_unevaluated_interior_extremum() {
        let seed = ProjectedBounds {
            minimum: [0.0, 0.0],
            maximum: [0.0, 0.0],
            achieved_uncertainty_mm: 0.0,
        };
        let result = search_domains(
            std::iter::once([[-1.0, 1.0], [0.0, 0.0]]),
            1,
            0.001,
            Some(seed),
            &mut Scratch::default(),
            &mut GeometryWorkBudget::new(100_000),
            &AtomicBool::new(false),
            |uv, _, _| {
                let u = Interval::new(uv[0][0], uv[0][1])?;
                Ok(Some(Evaluation {
                    height: Interval::point(1.0).sub(u.square()),
                    witness: Some(Interval::point(1.0).sub(Interval::point(u.mid()).square())),
                    split_axis: 0,
                }))
            },
        )
        .unwrap();
        assert!(result.minimum[0] <= 0.0 && result.minimum[1] >= 0.0);
        assert!(result.maximum[0] <= 1.0 && result.maximum[1] >= 1.0);
    }

    #[test]
    fn prior_source_faces_resolve_noncontact_faces_without_local_witnesses() {
        let seed = exact_scalar_bounds();
        let mut evaluated = 0;
        let result = search_domains(
            std::iter::once([[0.0, 1.0]; 2]),
            2,
            0.001,
            Some(seed),
            &mut Scratch::default(),
            &mut GeometryWorkBudget::new(100),
            &AtomicBool::new(false),
            |_, _, _| {
                evaluated += 1;
                // The complete new face is strictly between the independently
                // feasible extrema on prior faces. It need not provide a local
                // extremum or turn its enclosure into a contact witness.
                Ok(Some(Evaluation {
                    height: Interval::new(2.0, 8.0)?,
                    witness: None,
                    split_axis: 0,
                }))
            },
        )
        .unwrap();
        assert_eq!(evaluated, 1);
        assert_eq!(result.minimum, seed.minimum);
        assert_eq!(result.maximum, seed.maximum);
    }

    #[test]
    fn noncontact_exclusion_uses_outward_endpoint_uncertainty() {
        let tolerance = 0.125;
        assert!(resolved_by_feasible_extrema(
            Interval {
                lo: -parametric::down(tolerance),
                hi: 10.0
            },
            0.0,
            10.0,
            tolerance,
        ));
        assert!(!resolved_by_feasible_extrema(
            Interval {
                lo: -tolerance,
                hi: 10.0
            },
            0.0,
            10.0,
            tolerance,
        ));
        assert!(resolved_by_feasible_extrema(
            Interval {
                lo: 0.0,
                hi: parametric::down(10.0 + tolerance)
            },
            0.0,
            10.0,
            tolerance,
        ));
        assert!(!resolved_by_feasible_extrema(
            Interval {
                lo: 0.0,
                hi: 10.0 + tolerance
            },
            0.0,
            10.0,
            tolerance,
        ));
        for endpoints in [
            [f64::INFINITY, 10.0],
            [0.0, f64::NEG_INFINITY],
            [f64::NAN, 10.0],
            [0.0, f64::NAN],
        ] {
            assert!(!resolved_by_feasible_extrema(
                Interval { lo: 2.0, hi: 8.0 },
                endpoints[0],
                endpoints[1],
                tolerance,
            ));
        }
    }

    #[test]
    fn prior_source_witnesses_do_not_bypass_terminal_face_errors() {
        for code in [
            GeometryErrorCode::ResourceLimit,
            GeometryErrorCode::Cancelled,
        ] {
            let failure = search_domains(
                std::iter::once([[0.0, 1.0]; 2]),
                2,
                0.001,
                Some(exact_scalar_bounds()),
                &mut Scratch::default(),
                &mut GeometryWorkBudget::new(100),
                &AtomicBool::new(false),
                |_, _, _| Err(error(code, "new source face query failed")),
            )
            .unwrap_err();
            assert_eq!(failure.code, code);
        }
    }

    #[test]
    fn resource_and_cancellation_are_terminal_not_partial_bounds() {
        for (maximum_work, cancelled, expected) in [
            (0, false, GeometryErrorCode::ResourceLimit),
            (100, true, GeometryErrorCode::Cancelled),
        ] {
            let failure = search(
                [[0.0, 1.0], [0.0, 0.0]],
                1,
                0.001,
                &mut Scratch::default(),
                &mut GeometryWorkBudget::new(maximum_work),
                &AtomicBool::new(cancelled),
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
        let failure = search(
            [[0.0, 1.0], [0.0, 0.0]],
            1,
            0.001,
            &mut scratch,
            &mut GeometryWorkBudget::new(100),
            &AtomicBool::new(false),
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
        let failure = search(
            [[0.0, f64::from_bits(1)], [0.0, 0.0]],
            1,
            0.001,
            &mut Scratch::default(),
            &mut GeometryWorkBudget::new(1_000),
            &AtomicBool::new(false),
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
