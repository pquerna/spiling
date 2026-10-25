// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Directional support from immutable native coefficients and literal trims.
//!
//! A chart enclosure excludes possible extrema; it never supplies a feasible
//! point. Raw support uses only certified inside-domain surface points. Material
//! support additionally consumes the complete selected source realization's
//! correction supports and feasible witnesses. Native spatial edges are not
//! assumed equal to either the literal chart trim or the corrected boundary.

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
const MAX_SCRATCH_ALLOCATION_BYTES: usize = 32 * 1024 * 1024;
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

/// Support receipt for the immutable selected source-realization image.
/// Its identity binds operands, not global filled-material or embedding proof.
/// Manufacturing consumers retain their independent material admission gate.
#[derive(Debug, Clone)]
pub struct MaterialProjectedBounds {
    bounds: ProjectedBounds,
    definition_id: DefinitionId,
    material_realization: crate::kernel::MaterialRealizationIdentity,
    source_realization_error_mm: f64,
    scalar_transport_is_identically_zero: bool,
}

impl MaterialProjectedBounds {
    pub fn bounds(&self) -> ProjectedBounds {
        self.bounds
    }

    pub fn definition_id(&self) -> &DefinitionId {
        &self.definition_id
    }

    /// Maximum proved physical error of the selected source realization over
    /// its complete correction supports, not the declared source allowance.
    pub fn source_realization_error_mm(&self) -> f64 {
        self.source_realization_error_mm
    }

    /// Complete support proves exact zero displacement along this covector.
    pub fn scalar_transport_is_identically_zero(&self) -> bool {
        self.scalar_transport_is_identically_zero
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

/// Enclose the selected source-realization image using its actual F/G/V
/// supports and feasible points. Original correspondence and completed support
/// preparation are prerequisites; this is not a global filled-M+/embedding
/// certificate, and imported tolerances cannot replace those separate proofs.
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
    let remaining = MAX_SCRATCH_ALLOCATION_BYTES
        .checked_sub(prepared.retained_bytes())
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "retained native material source exhausts directional allocation cap",
            )
        })?;
    material_projected_bounds_with_scratch_limit(
        prepared.source_view()?,
        direction,
        policy,
        remaining,
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
    source
        .trim_source()
        .require_source_correspondence(work, cancel)?;
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
    // Raw chart search and correction preparation run sequentially and share
    // the same remaining cap. No source support is skipped by an assumed zero.
    let raw_before = work.consumed();
    let raw = projected_bounds_with_scratch_limit(
        source.trim_source(),
        direction,
        BoundsPolicy {
            maximum_endpoint_uncertainty_mm: policy.maximum_endpoint_uncertainty_mm * 0.5,
        },
        remaining_bytes,
        work,
        cancel,
    )
    .map_err(|failure| {
        work.annotate_error(
            failure,
            raw_before,
            format_args!("material directional raw support search; direction={direction:?}"),
        )
    })?;
    let before = work.consumed();
    let supports = super::critical::material_link::source_transport_supports(
        source,
        normalized_direction(direction)?,
        Some((
            [
                raw.minimum[1].min(raw.maximum[0]),
                raw.minimum[1].max(raw.maximum[0]),
            ],
            policy.maximum_endpoint_uncertainty_mm * 0.125,
        )),
        remaining_bytes,
        work,
        cancel,
    )
    .map_err(|failure| {
        work.annotate_error(
            failure,
            before,
            format_args!(
                "material directional source-support preparation; direction={direction:?}"
            ),
        )
    })?;
    let (bounds, source_realization_error_mm, scalar_transport_is_identically_zero) =
        aggregate_material_supports(
            raw,
            direction,
            &supports,
            definition.provenance().uncertainty_mm,
            policy,
            work,
            cancel,
        )?;
    drop(supports);
    work.charge(1, cancel)?;
    Ok(MaterialProjectedBounds {
        bounds,
        definition_id: definition.id().clone(),
        material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
        source_realization_error_mm,
        scalar_transport_is_identically_zero,
    })
}

/// Combine complete correction RANGE bounds with independently feasible
/// transported/raw and selected-realization witnesses. A BASE enclosure is
/// never clipped to the raw support or mistaken for realized contact.
fn aggregate_material_supports(
    raw: ProjectedBounds,
    direction: [f64; 3],
    supports: &[super::critical::material_link::SourceTransportSupport],
    declared_allowance_mm: Option<f64>,
    policy: BoundsPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(ProjectedBounds, f64, bool), GeometryError> {
    work.charge(1, cancel)?;
    let tolerance = policy.maximum_endpoint_uncertainty_mm;
    if !tolerance.is_finite() || tolerance <= 0.0 {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "material directional bounds require a finite positive endpoint uncertainty",
        ));
    }
    let allowance = declared_allowance_mm.unwrap_or(0.0);
    if !allowance.is_finite() || allowance < 0.0 {
        return Err(error(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "material directional bounds lack a finite nonnegative source allowance",
        ));
    }
    let mut bounds = raw;
    // Zero preserves the possibility that the original feasible point lies in
    // an unchanged region. Only BASE overlap can move that endpoint's witness.
    let mut minimum_witness_displacement = 0.0_f64;
    let mut maximum_witness_displacement = 0.0_f64;
    let mut source_error_mm = 0.0_f64;
    let mut realized_minimum_witness = f64::INFINITY;
    let mut realized_maximum_witness = f64::NEG_INFINITY;
    let mut scalar_transport_is_identically_zero = true;
    let mut winners = [None; 7];
    for (index, support) in supports.iter().enumerate() {
        work.charge(1, cancel)?;
        let base = Interval::new(support.base_interval[0], support.base_interval[1])?;
        let corrected =
            Interval::new(support.corrected_interval[0], support.corrected_interval[1])?;
        let displacement = Interval::new(
            support.displacement_interval[0],
            support.displacement_interval[1],
        )?;
        if !support.source_error_mm.is_finite()
            || support.source_error_mm < 0.0
            || support.source_error_mm > allowance
        {
            return Err(error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "material directional support exceeds the original source allowance",
            ));
        }
        if support.source_error_mm > source_error_mm {
            winners[4] = Some(index);
        }
        source_error_mm = source_error_mm.max(support.source_error_mm);
        scalar_transport_is_identically_zero &= displacement.lo == 0.0 && displacement.hi == 0.0;
        if corrected.lo < bounds.minimum[0] {
            winners[0] = Some(index);
        }
        if corrected.hi > bounds.maximum[1] {
            winners[3] = Some(index);
        }
        bounds.minimum[0] = bounds.minimum[0].min(corrected.lo);
        bounds.maximum[1] = bounds.maximum[1].max(corrected.hi);
        if base.lo <= raw.minimum[1] && raw.minimum[0] <= base.hi {
            if displacement.hi > minimum_witness_displacement {
                winners[1] = Some(index);
            }
            minimum_witness_displacement = minimum_witness_displacement.max(displacement.hi);
        }
        if base.lo <= raw.maximum[1] && raw.maximum[0] <= base.hi {
            if displacement.lo < maximum_witness_displacement {
                winners[2] = Some(index);
            }
            maximum_witness_displacement = maximum_witness_displacement.min(displacement.lo);
        }
        if let Some(witness) = support.realized_witness_interval {
            let witness = Interval::new(witness[0], witness[1])?;
            if witness.hi < realized_minimum_witness {
                winners[5] = Some(index);
                realized_minimum_witness = witness.hi;
            }
            if witness.lo > realized_maximum_witness {
                winners[6] = Some(index);
                realized_maximum_witness = witness.lo;
            }
        }
        if let Some([minimum, maximum]) = support.realized_endpoint_witnesses {
            if !minimum.is_finite() || !maximum.is_finite() {
                return Err(error(
                    GeometryErrorCode::ConditioningFailure,
                    "material directional endpoint witnesses must be finite",
                ));
            }
            if minimum < realized_minimum_witness {
                winners[5] = Some(index);
                realized_minimum_witness = minimum;
            }
            if maximum > realized_maximum_witness {
                winners[6] = Some(index);
                realized_maximum_witness = maximum;
            }
        }
    }
    // Independently proved selected-source points tighten only feasible ends,
    // never the complete outward RANGE enclosures.
    bounds.minimum[1] = Interval::point(raw.minimum[1])
        .add(Interval::point(minimum_witness_displacement))
        .hi
        .min(realized_minimum_witness);
    bounds.maximum[0] = Interval::point(raw.maximum[0])
        .add(Interval::point(maximum_witness_displacement))
        .lo
        .max(realized_maximum_witness);
    bounds.achieved_uncertainty_mm = uncertainty(&bounds);
    if bounds
        .minimum
        .iter()
        .chain(bounds.maximum.iter())
        .any(|x| !x.is_finite())
        || bounds.minimum[0] > bounds.minimum[1]
        || bounds.maximum[0] > bounds.maximum[1]
        || bounds.achieved_uncertainty_mm > tolerance
    {
        use std::fmt::Write as _;
        // Format only seven independent extremum/error owners, not the full
        // support inventory. No second traversal or coefficient clone.
        let mut message = format!(
            "material directional endpoint brackets do not meet the requested uncertainty; \
             query_direction={direction:?}; raw_min={:?}; raw_max={:?}; \
             material_min={:?}; material_max={:?}; requested_endpoint_uncertainty_mm={tolerance:?}; \
             achieved_endpoint_uncertainty_mm={:?}; maximum_physical_source_error_mm={source_error_mm:?}; \
             minHi_witness_displacement={minimum_witness_displacement:?}; \
             maxLo_witness_displacement={maximum_witness_displacement:?}",
            raw.minimum,
            raw.maximum,
            bounds.minimum,
            bounds.maximum,
            bounds.achieved_uncertainty_mm,
        );
        for (label, winner) in [
            "minLo",
            "minHi",
            "maxLo",
            "maxHi",
            "physicalError",
            "minHi_actual_G",
            "maxLo_actual_G",
        ]
        .into_iter()
        .zip(winners)
        {
            if let Some(index) = winner {
                let support = &supports[index];
                let _ = write!(
                    message,
                    "; {label}_winner=index:{index},owner:({}),BASE:{:?},G:{:?},D:{:?},physical_error_mm:{:?},actual_G_witness:{:?},actual_G_endpoint_witnesses:{:?}",
                    support.owner,
                    support.base_interval,
                    support.corrected_interval,
                    support.displacement_interval,
                    support.source_error_mm,
                    support.realized_witness_interval,
                    support.realized_endpoint_witnesses,
                );
            } else {
                let _ = write!(message, "; {label}_winner=raw_or_zero");
            }
        }
        return Err(error(GeometryErrorCode::AnalysisIndeterminate, &message));
    }
    Ok((
        bounds,
        source_error_mm,
        scalar_transport_is_identically_zero,
    ))
}

/// Uncorrected support with borrowed, already admitted chart domains and their
/// selected coefficient operators. Native spatial edges never supply witnesses.
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
    let mut aggregate = None;
    for ordinal in 0..definition.face_ids().len() {
        work.charge(1, cancel)?;
        let trim = source.face_domain(ordinal).ok_or_else(|| {
            error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "native directional source omitted an original face trim domain",
            )
        })?;
        // The admitted domain selects its own coefficient carrier and chart
        // units. Original STEP radians must not be evaluated on a native
        // degree-parameterized Surface::Step by looking up only its FaceId.
        let surface = trim.surface_operand();
        let before = work.consumed();
        let uv = trim.uv_domain();
        let native_boxes = match &surface {
            PreparedTrimSurface::Native(native) => Some(parametric::surface_parameter_boxes(
                *native, uv, work, cancel,
            )?),
            PreparedTrimSurface::Original(_) => None,
        };
        // Original position bounds cover the complete source expression,
        // including its knots. They need no inferred native partition/gauge.
        let original_box = native_boxes.is_none().then_some(uv);
        let boxes = native_boxes.into_iter().flatten().chain(original_box);
        // Prior faces supply independently feasible points of this same source
        // under the same covector. The query needs global support, not tight
        // extrema of every noncontact face. Each new face still has its complete
        // coefficient/trim cover searched; the seed never clips its enclosures
        // or supplies a feasible point on that particular face.
        let support = search_domains(
            boxes, 2, tolerance, aggregate, &mut scratch, work, cancel,
            |cell, work, cancel| {
                let class = trim.classify(cell, work, cancel)?;
                if matches!(class, TrimClassification::Outside) {
                    return Ok(None);
                }
                let middle = cell.map(|span| span[0] * 0.5 + span[1] * 0.5);
                let point_cell = middle.map(|value| [value, value]);
                let (height, point_height, split_axis) = match &surface {
                    PreparedTrimSurface::Native(native) => {
                        // Whole continuous first bounds suffice for a mean-value
                        // enclosure without manufacturing C1 at native knots.
                        let enclosure = parametric::surface_piecewise_first_enclosure(
                            *native, cell, work, cancel,
                        )?;
                        let point = parametric::surface_position_enclosure(
                            *native, point_cell, work, cancel,
                        )?;
                        let point_height = project(point, direction)?;
                        let delta = [0, 1].map(|axis| Interval {
                            lo: parametric::down(cell[axis][0] - middle[axis]),
                            hi: parametric::up(cell[axis][1] - middle[axis]),
                        });
                        let du = project(enclosure.du, direction)?;
                        let dv = project(enclosure.dv, direction)?;
                        let centered = point_height.add(du.mul(delta[0])).add(dv.mul(delta[1]));
                        (
                            intersect(project(enclosure.position, direction)?, centered)?,
                            point_height,
                            usize::from(du.abs_upper() * width(cell[0])
                                < dv.abs_upper() * width(cell[1])),
                        )
                    }
                    PreparedTrimSurface::Original(original) => {
                        // Literal source position bounds remain valid across
                        // source knot joins. Without separate continuity proof,
                        // no derivative/mean-value enclosure is used here.
                        let height = project(original.position_bounds(cell, work, cancel)?, direction)?;
                        let point_height = project(
                            original.position_bounds(point_cell, work, cancel)?, direction,
                        )?;
                        // Prioritize actual projected variation, not unrelated
                        // UV span lengths. These C0 cross-sections choose only
                        // refinement order; the full-cell bound above and the
                        // independent feasible witness remain the authority.
                        let u_height = project(
                            original.position_bounds([cell[0], point_cell[1]], work, cancel)?,
                            direction,
                        )?;
                        let v_height = project(
                            original.position_bounds([point_cell[0], cell[1]], work, cancel)?,
                            direction,
                        )?;
                        let variation = [
                            width([u_height.lo, u_height.hi]),
                            width([v_height.lo, v_height.hi]),
                        ];
                        let split_axis = if variation == [0.0; 2] {
                            usize::from(width(cell[0]) < width(cell[1]))
                        } else {
                            usize::from(variation[0] < variation[1])
                        };
                        (height, point_height, split_axis)
                    }
                };
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

/// Complete directed extrema of a selected one-parameter source recipe.
/// Only point enclosures of that same recipe supply feasible witnesses.
pub(super) fn source_recipe_curve_bounds(
    range: [f64; 2],
    tolerance: f64,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut evaluate: impl FnMut(
        [f64; 2],
        &mut GeometryWorkBudget,
        &AtomicBool,
        usize,
    ) -> Result<(Interval, Interval), GeometryError>,
) -> Result<ProjectedBounds, GeometryError> {
    let mut scratch = Scratch {
        byte_limit: remaining_bytes.min(MAX_SCRATCH_ALLOCATION_BYTES),
        ..Scratch::default()
    };
    search_domains_with_query_scratch(
        std::iter::once([range, [0.0, 0.0]]),
        1,
        tolerance,
        None,
        &mut scratch,
        work,
        cancel,
        |cell, work, cancel, remaining_bytes| {
            let (height, witness) = evaluate(cell[0], work, cancel, remaining_bytes)?;
            Ok(Some(Evaluation {
                height,
                witness: Some(witness),
                split_axis: 0,
            }))
        },
    )
}

/// Bound the selected source recipe's parameter rectangle, not a coarse UV
/// image. Raw endpoint references select sufficient RANGE proofs; they never
/// clip BASE or become feasible witnesses of the selected realization.
pub(super) fn source_recipe_scalar_support(
    domain: [[f64; 2]; 2],
    endpoint_reference: [f64; 2],
    tolerance: f64,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut evaluate: impl FnMut(
        [[f64; 2]; 2],
        &mut GeometryWorkBudget,
        &AtomicBool,
    ) -> Result<(Interval, Option<Interval>, usize), GeometryError>,
) -> Result<Interval, GeometryError> {
    validate_domain(domain, 2)?;
    let target = Interval::new(endpoint_reference[0], endpoint_reference[1])?.add(Interval {
        lo: -tolerance,
        hi: tolerance,
    });
    let mut scratch = Scratch {
        byte_limit: remaining_bytes.min(MAX_SCRATCH_ALLOCATION_BYTES),
        ..Scratch::default()
    };
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
    let mut feasible_min = f64::INFINITY;
    let mut feasible_max = f64::NEG_INFINITY;
    let mut outer_min = f64::INFINITY;
    let mut outer_max = f64::NEG_INFINITY;
    while let Some(cell) = scratch.cells.pop() {
        work.charge(1, cancel)?;
        let (height, suggested_axis, conditioning) = match evaluate(cell.uv, work, cancel) {
            Ok((height, witness, split_axis)) => {
                let height = intersect(height, cell.ancestor)?;
                if let Some(witness) = witness {
                    feasible_min = feasible_min.min(witness.hi);
                    feasible_max = feasible_max.max(witness.lo);
                }
                // A collar far from global extrema does not require tightly
                // resolved local extrema or a vanishing fixed source margin.
                let inside_target = target.lo <= height.lo && height.hi <= target.hi;
                let resolved_local_extrema = feasible_min.is_finite()
                    && feasible_max.is_finite()
                    && parametric::up(feasible_min - height.lo) <= tolerance
                    && parametric::up(height.hi - feasible_max) <= tolerance;
                if inside_target || resolved_local_extrema {
                    // Retain the complete coefficient/mean-value enclosure,
                    // including every proved value outside the raw reference.
                    outer_min = outer_min.min(height.lo);
                    outer_max = outer_max.max(height.hi);
                    continue;
                }
                (height, split_axis, false)
            }
            Err(failure) if failure.code == GeometryErrorCode::ConditioningFailure => (
                cell.ancestor,
                usize::from(width(cell.uv[1]) > width(cell.uv[0])),
                true,
            ),
            Err(failure) => return Err(failure),
        };
        if cell.depth >= MAX_DEPTH {
            return Err(error(
                if conditioning {
                    GeometryErrorCode::ConditioningFailure
                } else {
                    GeometryErrorCode::AnalysisIndeterminate
                },
                "source recipe subdivision cannot resolve scalar range uncertainty",
            ));
        }
        let axis = if suggested_axis < 2 {
            suggested_axis
        } else {
            0
        };
        let axis = if midpoint(cell.uv[axis]).is_some() {
            axis
        } else if midpoint(cell.uv[1 - axis]).is_some() {
            1 - axis
        } else {
            return Err(error(
                if conditioning {
                    GeometryErrorCode::ConditioningFailure
                } else {
                    GeometryErrorCode::AnalysisIndeterminate
                },
                "source recipe parameters cannot resolve scalar range uncertainty",
            ));
        };
        let middle = midpoint(cell.uv[axis]).expect("splittable source recipe interval");
        let mut left = cell.uv;
        let mut right = cell.uv;
        left[axis][1] = middle;
        right[axis][0] = middle;
        let priority = (target.lo - height.lo).max(height.hi - target.hi);
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
    if !outer_min.is_finite() || !outer_max.is_finite() {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "source recipe has no finite complete scalar range enclosure",
        ));
    }
    Interval::new(outer_min, outer_max)
}

/// Search a complete cover, sharing independently feasible extrema across all
/// spans. `seed` must itself consist of feasible subset support brackets; its
/// outer endpoints cover only that subset, never the unevaluated chart.
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

    fn exact_scalar_bounds() -> ProjectedBounds {
        ProjectedBounds {
            minimum: [0.0, 0.0],
            maximum: [10.0, 10.0],
            achieved_uncertainty_mm: 0.0,
        }
    }

    fn scalar_transport(
        base_interval: [f64; 2],
        corrected_interval: [f64; 2],
        displacement_interval: [f64; 2],
        source_error_mm: f64,
    ) -> super::super::critical::material_link::SourceTransportSupport {
        super::super::critical::material_link::SourceTransportSupport {
            owner: super::super::critical::material_link::SourceTransportOwner::OriginalCoedge {
                face: 0,
                coedge: 0,
            },
            base_interval,
            corrected_interval,
            displacement_interval,
            realized_witness_interval: None,
            realized_endpoint_witnesses: None,
            source_error_mm,
        }
    }

    fn scalar_material_bounds(
        raw: ProjectedBounds,
        supports: &[super::super::critical::material_link::SourceTransportSupport],
        tolerance: f64,
    ) -> Result<(ProjectedBounds, f64), GeometryError> {
        aggregate_material_supports(
            raw,
            [0.0, 0.0, 1.0],
            supports,
            Some(1.0),
            BoundsPolicy {
                maximum_endpoint_uncertainty_mm: tolerance,
            },
            &mut GeometryWorkBudget::new(100),
            &AtomicBool::new(false),
        )
        .map(|(bounds, source_error_mm, _)| (bounds, source_error_mm))
    }

    #[test]
    fn scalar_identity_requires_every_complete_support_displacement_to_be_exact_zero() {
        for (supports, expected) in [
            (vec![], true),
            (
                vec![scalar_transport([5.0, 6.0], [5.0, 6.0], [0.0, 0.0], 0.25)],
                true,
            ),
            (
                vec![scalar_transport([5.0, 6.0], [5.0, 6.25], [0.0, 0.25], 0.25)],
                false,
            ),
            (
                vec![scalar_transport(
                    [5.0, 6.0],
                    [5.0, 6.0],
                    [-f64::from_bits(1), f64::from_bits(1)],
                    0.25,
                )],
                false,
            ),
        ] {
            let (_, _, scalar_identity) = aggregate_material_supports(
                exact_scalar_bounds(),
                [0.0, 0.0, 1.0],
                &supports,
                Some(1.0),
                BoundsPolicy {
                    maximum_endpoint_uncertainty_mm: 0.00001,
                },
                &mut GeometryWorkBudget::new(100),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(scalar_identity, expected);
        }
    }

    #[test]
    fn high_localized_transport_does_not_move_lower_support_or_its_witness() {
        let raw = exact_scalar_bounds();
        let support = scalar_transport([5.0, 6.0], [5.0, 6.25], [0.0, 0.25], 0.25);
        let (bounds, source_error) = scalar_material_bounds(raw, &[support], 0.00001).unwrap();
        assert_eq!(bounds.minimum, raw.minimum);
        assert_eq!(bounds.maximum, raw.maximum);
        assert_eq!(source_error, 0.25);
    }

    #[test]
    fn directed_bottom_and_new_top_supports_are_included_without_global_expansion() {
        let supports = [
            scalar_transport([0.0, 0.1], [-0.05, 0.1], [-0.05, 0.0], 0.05),
            scalar_transport([9.9, 10.0], [9.9, 10.2], [0.0, 0.2], 0.2),
        ];
        let (bounds, source_error) =
            scalar_material_bounds(exact_scalar_bounds(), &supports, 0.25).unwrap();
        assert_eq!(bounds.minimum, [-0.05, 0.0]);
        assert_eq!(bounds.maximum, [10.0, 10.2]);
        assert_eq!(source_error, 0.2);
    }

    #[test]
    fn removed_raw_extremal_witness_is_not_reused_as_realized_contact() {
        let supports = [
            scalar_transport([0.0, 0.0], [0.2, 0.2], [0.2, 0.2], 0.2),
            scalar_transport([10.0, 10.0], [9.7, 9.7], [-0.3, -0.3], 0.3),
        ];
        let (bounds, _) = scalar_material_bounds(exact_scalar_bounds(), &supports, 0.31).unwrap();
        assert_eq!(bounds.minimum, [0.0, 0.2]);
        assert!(bounds.maximum[0] <= 9.7 && bounds.maximum[0] > 9.69);
        assert_eq!(bounds.maximum[1], 10.0);
        assert!(bounds.achieved_uncertainty_mm > 0.29);
    }

    #[test]
    fn retained_realized_points_resolve_endpoints_despite_conservative_displacement() {
        let raw = exact_scalar_bounds();
        let mut supports = [
            scalar_transport([0.0, 2.0], [0.0, 2.0], [-0.25, 0.25], 0.25),
            scalar_transport([8.0, 10.0], [8.0, 10.0], [-0.25, 0.25], 0.25),
        ];
        // Diagnostic vertex labels alone are not feasible-point authority.
        for (support, z) in supports.iter_mut().zip([0.0, 10.0]) {
            support.owner =
                super::super::critical::material_link::SourceTransportOwner::OriginalVertex {
                    point_mm: [0.0, 0.0, z],
                };
        }
        let failure = scalar_material_bounds(raw, &supports, 1.0e-6).unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
        supports[0].realized_witness_interval = Some([0.0, 2.0e-8]);
        supports[1].realized_witness_interval = Some([10.0 - 2.0e-8, 10.0]);
        let (bounds, source_error) = scalar_material_bounds(raw, &supports, 1.0e-6).unwrap();
        assert_eq!(bounds.minimum[0], raw.minimum[0]);
        assert_eq!(bounds.maximum[1], raw.maximum[1]);
        assert!(0.0 <= bounds.minimum[1] && bounds.minimum[1] <= 2.0e-8);
        assert!(10.0 - 2.0e-8 <= bounds.maximum[0] && bounds.maximum[0] <= 10.0);
        assert!(bounds.achieved_uncertainty_mm <= 1.0e-6);
        assert_eq!(source_error, 0.25);
        // Feasible points cannot erase the complete outward RANGE.
        supports[0].corrected_interval[0] = -2.0e-5;
        supports[1].corrected_interval[1] = 10.0 + 2.0e-5;
        let failure = scalar_material_bounds(raw, &supports, 1.0e-6).unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
    }

    #[test]
    fn realized_endpoint_witnesses_resolve_minimum_and_maximum_independently() {
        let mut support = scalar_transport([0.0, 10.0], [0.0, 10.0], [-0.25, 0.25], 0.25);
        support.realized_endpoint_witnesses = Some([2.0e-8, 10.0 - 2.0e-8]);
        let (bounds, _) =
            scalar_material_bounds(exact_scalar_bounds(), &[support], 1.0e-6).unwrap();
        assert_eq!(bounds.minimum, [0.0, 2.0e-8]);
        assert_eq!(bounds.maximum, [10.0 - 2.0e-8, 10.0]);
        for endpoints in [[f64::NAN, 10.0], [0.0, f64::INFINITY]] {
            support.realized_endpoint_witnesses = Some(endpoints);
            let failure =
                scalar_material_bounds(exact_scalar_bounds(), &[support], 1.0e-6).unwrap_err();
            assert_eq!(failure.code, GeometryErrorCode::ConditioningFailure);
        }
    }

    #[test]
    fn material_feasible_witness_intervals_must_be_finite_and_ordered() {
        for witness in [
            [f64::NAN, 0.0],
            [0.0, f64::NAN],
            [f64::NEG_INFINITY, 0.0],
            [0.0, f64::INFINITY],
            [1.0, 0.0],
        ] {
            let mut support = scalar_transport([4.0, 6.0], [4.0, 6.0], [0.0, 0.0], 0.0);
            support.realized_witness_interval = Some(witness);
            let failure =
                scalar_material_bounds(exact_scalar_bounds(), &[support], 1.0e-6).unwrap_err();
            assert_eq!(failure.code, GeometryErrorCode::ConditioningFailure);
        }
    }

    #[test]
    fn base_overlap_uses_the_entire_raw_endpoint_bracket() {
        let mut raw = exact_scalar_bounds();
        raw.minimum = [0.0, 0.1];
        raw.maximum = [9.9, 10.0];
        let supports = [
            scalar_transport([0.025, 0.075], [0.225, 0.275], [0.2, 0.2], 0.2),
            scalar_transport([9.925, 9.975], [9.725, 9.775], [-0.2, -0.2], 0.2),
        ];
        let (bounds, _) = scalar_material_bounds(raw, &supports, 0.31).unwrap();
        assert!(bounds.minimum[1] >= 0.3);
        assert!(bounds.maximum[0] <= 9.7);
    }

    #[test]
    fn material_support_cannot_relax_policy_or_declared_source_allowance() {
        let support = scalar_transport([0.0, 0.1], [-0.05, 0.1], [-0.05, 0.0], 0.05);
        let failure = scalar_material_bounds(exact_scalar_bounds(), &[support], 0.001).unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
        let diagnostic = failure.to_string();
        for detail in [
            "query_direction=[0.0, 0.0, 1.0]",
            "raw_min=",
            "raw_max=",
            "material_min=",
            "material_max=",
            "requested_endpoint_uncertainty_mm=0.001",
            "maximum_physical_source_error_mm=0.05",
            "minLo_winner=index:0,owner:(original face 0, coedge 0)",
            "minHi_winner=raw_or_zero",
            "maxLo_winner=raw_or_zero",
            "maxHi_winner=raw_or_zero",
            "physicalError_winner=index:0",
            "BASE:[0.0, 0.1],G:[-0.05, 0.1],D:[-0.05, 0.0]",
        ] {
            assert!(diagnostic.contains(detail), "{diagnostic}");
        }
        let failure = aggregate_material_supports(
            exact_scalar_bounds(),
            [0.0, 0.0, 1.0],
            &[scalar_transport([5.0, 6.0], [5.0, 6.05], [0.0, 0.05], 0.05)],
            Some(0.01),
            BoundsPolicy {
                maximum_endpoint_uncertainty_mm: 0.001,
            },
            &mut GeometryWorkBudget::new(100),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(
            failure.code,
            GeometryErrorCode::ParameterCorrespondenceUnavailable
        );
    }

    #[test]
    fn material_scalar_aggregation_preserves_terminal_work_and_cancel_errors() {
        for (maximum_work, cancelled, expected) in [
            (0, false, GeometryErrorCode::ResourceLimit),
            (100, true, GeometryErrorCode::Cancelled),
        ] {
            let failure = aggregate_material_supports(
                exact_scalar_bounds(),
                [0.0, 0.0, 1.0],
                &[],
                None,
                BoundsPolicy {
                    maximum_endpoint_uncertainty_mm: 0.001,
                },
                &mut GeometryWorkBudget::new(maximum_work),
                &AtomicBool::new(cancelled),
            )
            .unwrap_err();
            assert_eq!(failure.code, expected);
        }
    }

    #[test]
    fn unproved_base_outside_raw_support_is_not_clipped_to_raw_extrema() {
        let support = scalar_transport([-0.01, 10.01], [-0.01, 10.01], [0.0, 0.0], 0.0);
        let failure = scalar_material_bounds(exact_scalar_bounds(), &[support], 0.001).unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
        assert!(failure.message.contains("BASE:[-0.01, 10.01]"));
    }

    #[test]
    fn source_recipe_refinement_targets_never_clip_an_unproved_native_base() {
        let mut work = GeometryWorkBudget::new(100_000);
        let cancel = AtomicBool::new(false);
        // The selected affine recipe really extends beyond both raw references.
        let base = source_recipe_scalar_support(
            [[0.0, 1.0]; 2],
            [0.0, 10.0],
            0.0001,
            MAX_SCRATCH_ALLOCATION_BYTES,
            &mut work,
            &cancel,
            |uv, _, _| {
                let evaluate =
                    |u: Interval| Interval::point(-0.01).add(Interval::point(10.02).mul(u));
                Ok((
                    evaluate(Interval::new(uv[0][0], uv[0][1])?),
                    Some(evaluate(Interval::point(uv[0][0] * 0.5 + uv[0][1] * 0.5))),
                    0,
                ))
            },
        )
        .unwrap();
        assert!(base.lo <= -0.01 && 10.01 <= base.hi, "{base:?}");
        let support = scalar_transport([base.lo, base.hi], [base.lo, base.hi], [0.0, 0.0], 0.0);
        let failure = scalar_material_bounds(exact_scalar_bounds(), &[support], 0.001).unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
        assert!(failure.message.contains("BASE:"));
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
    fn prior_source_witnesses_do_not_bypass_terminal_face_errors() {
        for code in [GeometryErrorCode::ResourceLimit, GeometryErrorCode::Cancelled] {
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
