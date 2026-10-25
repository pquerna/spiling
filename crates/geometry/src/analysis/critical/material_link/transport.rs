// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Complete scalar support of the immutable source-relative realization.
//! Position bounds are not a material embedding certificate: that authority
//! additionally requires the closed radial link and complete degree proof.

use super::*;
use crate::analysis::material_source::MaterialSourceView;
use crate::analysis::source_incidence::native_coedge;
use crate::kernel::source_operand::SOURCE_OPERAND_SCRATCH_BYTES;
use crate::parametric::SurfaceEvaluator;
use solidcore::EdgeId;
use vertex_transport::SourceVertexPlan;

// Original operators may recursively evaluate their selected source carriers.
// Keep evaluator scratch disjoint from live scalar heaps and output capacity.
pub(super) const EVALUATION_SCRATCH_BYTES: usize =
    if SOURCE_OPERAND_SCRATCH_BYTES > SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES {
        SOURCE_OPERAND_SCRATCH_BYTES
    } else {
        SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES
    };

#[derive(Clone, Copy)]
pub(crate) enum SourceTransportOwner {
    OriginalCoedge { face: usize, coedge: usize },
    OriginalVertex { point_mm: [f64; 3] },
}

impl std::fmt::Display for SourceTransportOwner {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OriginalCoedge { face, coedge } => {
                write!(out, "original face {face}, coedge {coedge}")
            }
            Self::OriginalVertex { point_mm } => {
                write!(out, "original vertex at {point_mm:?} mm")
            }
        }
    }
}

impl std::fmt::Debug for SourceTransportOwner {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, out)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SourceTransportSupport {
    pub(crate) owner: SourceTransportOwner,
    pub(crate) base_interval: [f64; 2],
    pub(crate) corrected_interval: [f64; 2],
    pub(crate) displacement_interval: [f64; 2],
    /// Encloses a proved feasible scalar of the complete selected realized G,
    /// not necessarily this refined support cell. This is geometric authority:
    /// diagnostic owner coordinates and arbitrary edge samples cannot supply it.
    /// Only the normative closed cone currently proves its retained apex.
    pub(crate) realized_witness_interval: Option<[f64; 2]>,
    /// Independently feasible extrema of actual G: an upper bound on a
    /// feasible minimum and a lower bound on a feasible maximum. These need
    /// not be ordered as an interval enclosing one point.
    pub(crate) realized_endpoint_witnesses: Option<[f64; 2]>,
    /// Physical three-dimensional error, independent of the scalar covector.
    pub(crate) source_error_mm: f64,
}

pub(super) fn resource() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "source material support exhausts the existing allocation cap",
    )
}
fn unavailable(message: &'static str) -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
        message,
    )
}
pub(super) fn output_bytes(output: &Vec<SourceTransportSupport>) -> usize {
    output
        .capacity()
        .saturating_mul(std::mem::size_of::<SourceTransportSupport>())
}
pub(super) fn push(
    output: &mut Vec<SourceTransportSupport>,
    support: SourceTransportSupport,
    limit: usize,
) -> Result<(), GeometryError> {
    if output.len() == output.capacity() {
        let size = std::mem::size_of::<SourceTransportSupport>();
        let minimum = output.len().checked_add(1).ok_or_else(resource)?;
        let desired = output
            .capacity()
            .saturating_mul(2)
            .max(16)
            .min(limit / size);
        if desired < minimum {
            return Err(resource());
        }
        output
            .try_reserve_exact(desired - output.len())
            .map_err(|_| resource())?;
        if output_bytes(output) > limit {
            return Err(resource());
        }
    }
    output.push(support);
    Ok(())
}

/// Fold the covector into original homogeneous coefficients before bounding
/// the scalar ratio. Piecewise first jets also supply the mean-value theorem.
pub(super) fn scalar_surface_jet(
    surface: &(impl SurfaceEvaluator + ?Sized),
    uv: [[f64; 2]; 2],
    direction: [Interval; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(Interval, [Interval; 2]), GeometryError> {
    let [jet] = crate::parametric::surface_functional_enclosures(
        surface,
        uv,
        &[crate::parametric::SurfaceFunctional {
            direction,
            origin: [Interval::point(0.0); 3],
            offset: Interval::point(0.0),
        }],
        work,
        cancel,
    )?;
    Ok((jet.position, [jet.du, jet.dv]))
}

fn collar_scalar_support(
    collar: &OriginalCollar<'_, '_>,
    surface: &(impl SurfaceEvaluator + ?Sized),
    range: [f64; 2],
    direction: [Interval; 3],
    refinement: Option<([f64; 2], f64)>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Interval, GeometryError> {
    let CarrierTransport::Collar { width, inward } = collar.transport else {
        unreachable!()
    };
    let mapped = |cell: [[f64; 2]; 2],
                  work: &mut GeometryWorkBudget,
                  cancel: &AtomicBool|
     -> Result<_, GeometryError> {
        let uv = chart_jet(collar.domain, collar.use_.coedge, cell[0], work, cancel)?;
        let (beta, beta_t) = collar.bubble(cell[0])?;
        let across = Interval::new(cell[1][0], cell[1][1])?;
        let transverse = rotate(uv.derivative, inward);
        let offset = transverse.map(|value| mul(mul(Interval::point(width), beta), value));
        let at = std::array::from_fn(|axis| add(uv.position[axis], mul(across, offset[axis])));
        let (height, gradient) = scalar_surface_jet(
            surface,
            uv_box(add_margin(at, collar.margin)),
            direction,
            work,
            cancel,
        )?;
        if refinement.is_none() || cell.iter().all(|span| span[0] == span[1]) {
            return Ok((height, [Interval::point(0.0); 2], 0));
        }
        let transverse_t = rotate(uv.second_derivative, inward);
        let offset_t: [Interval; 2] = std::array::from_fn(|axis| {
            mul(
                Interval::point(width),
                add(mul(beta_t, transverse[axis]), mul(beta, transverse_t[axis])),
            )
        });
        let along: [Interval; 2] =
            std::array::from_fn(|axis| add(uv.derivative[axis], mul(across, offset_t[axis])));
        let project_uv =
            |vector: [Interval; 2]| add(mul(gradient[0], vector[0]), mul(gradient[1], vector[1]));
        let derivative = [project_uv(along), project_uv(offset)];
        let motion: [f64; 2] = std::array::from_fn(|axis| {
            derivative[axis].abs_upper() * (cell[axis][1] - cell[axis][0])
        });
        Ok((height, derivative, usize::from(motion[1] > motion[0])))
    };
    let Some((endpoint_reference, tolerance)) = refinement else {
        return mapped([range, [0.0, 1.0]], work, cancel).map(|(height, _, _)| height);
    };
    let target = Interval::new(endpoint_reference[0], endpoint_reference[1])?.add(Interval {
        lo: -tolerance,
        hi: tolerance,
    });
    crate::analysis::bounds::source_recipe_scalar_support(
        [range, [0.0, 1.0]],
        endpoint_reference,
        tolerance,
        remaining_bytes,
        work,
        cancel,
        |cell, work, cancel| {
            let (height, derivative, split_axis) = mapped(cell, work, cancel)?;
            if target.lo <= height.lo && height.hi <= target.hi {
                return Ok((height, None, split_axis));
            }
            let middle = cell.map(|span| span[0] * 0.5 + span[1] * 0.5);
            let point = middle.map(|parameter| [parameter; 2]);
            let (witness, _, _) = mapped(point, work, cancel)?;
            // Hold the SAME margin operand fixed along each recipe path.
            // The chain rule retains the actual correlation between the two
            // UV coordinates, rather than Taylor-expanding their image box.
            let mut centered = witness;
            for axis in 0..2 {
                let delta =
                    Interval::new(cell[axis][0], cell[axis][1])?.sub(Interval::point(middle[axis]));
                centered = add(centered, mul(derivative[axis], delta));
            }
            let height = intersection(height, centered).ok_or_else(|| {
                unavailable("source collar coefficient and recipe mean-value enclosures disagree")
            })?;
            Ok((height, Some(witness), split_axis))
        },
    )
}

/// Optional scalar refinement supplies global endpoint search references and
/// a local endpoint tolerance. It changes enclosures only, never the selected
/// source recipe, original domain, displacement grid or physical error proof.
pub(crate) fn source_transport_supports(
    source: MaterialSourceView<'_, '_>,
    direction: [Interval; 3],
    scalar_refinement: Option<([f64; 2], f64)>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Vec<SourceTransportSupport>, GeometryError> {
    work.charge(1, cancel)?;
    if scalar_refinement.is_some_and(|(reference, tolerance)| {
        !tolerance.is_finite()
            || tolerance <= 0.0
            || reference.iter().any(|value| !value.is_finite())
            || reference[0] > reference[1]
    }) {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "source support requires finite ordered scalar endpoint references and positive uncertainty",
        ));
    }
    if direction
        .iter()
        .any(|v| !v.lo.is_finite() || !v.hi.is_finite() || v.lo > v.hi)
    {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "source material support requires a finite scalar covector",
        ));
    }
    let definition = source.definition();
    if let Some(plans) = source.vertex_plans() {
        plans.validate_source(definition, work, cancel)?;
    }
    if source.material_transport_is_proved_zero(work, cancel)? {
        return Ok(Vec::new());
    }
    let allowance = source_allowance(definition)?;
    let before = work.consumed();
    let mut incidence = SourceEdgeIncidence::new(definition, remaining_bytes, work, cancel)
        .map_err(|error| {
            work.annotate_error(
                error,
                before,
                "source support opposed-coedge inventory preparation",
            )
        })?;
    let before = work.consumed();
    incidence
        .prepare_vertices(remaining_bytes, work, cancel)
        .map_err(|error| {
            work.annotate_error(
                error,
                before,
                "source support original-vertex inventory preparation",
            )
        })?;
    let available = remaining_bytes
        .checked_sub(incidence.owned_bytes())
        .ok_or_else(resource)?;
    let mut output = Vec::new();
    // Every actual central ribbon is a restriction of this full original-edge
    // formula. Including the unused endpoint portions is conservative; the
    // selected source-only corner replacements are enclosed independently below.
    for uses in incidence.values() {
        let second = uses.second.ok_or_else(|| {
            unavailable("source material support requires two opposed original coedge uses")
        })?;
        for use_ in [uses.first, second] {
            work.charge(1, cancel)?;
            let face_limit = available
                .checked_sub(output_bytes(&output))
                .ok_or_else(resource)?;
            let before = work.consumed();
            let domain = source
                .face_domain(use_.face, face_limit, work, cancel)
                .map_err(|error| {
                    work.annotate_error(
                        error,
                        before,
                        format_args!(
                            "source support face-domain preparation; face={}, coedge={}",
                            use_.face, use_.coedge
                        ),
                    )
                })?;
            let working = face_limit;
            let before = work.consumed();
            let Some(collar) = OriginalCollar::for_enclosure_with_byte_limit(
                domain, use_, working, work, cancel,
            )
                .map_err(|error| work.annotate_error(error, before,
                    format_args!("source support central-coedge owner preparation; face={}, coedge={}, spatial_range={:?}",
                        use_.face, use_.coedge, domain.original_coedge_correspondence(use_.coedge).ok().map(|proof| proof.spatial_range()))))? else {
                return Err(failure(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "source material collar has no complete original coefficient recipe",
                    use_.face,
                    (use_.wire, use_.index, use_.coedge),
                ));
            };
            if matches!(collar.transport, CarrierTransport::Exact) {
                continue;
            }
            let (edge_use, edge) = native_coedge(definition, use_.face, use_.wire, use_.index)?;
            let coedge = domain.coedges().get(use_.coedge).ok_or_else(|| {
                unavailable("source support lost its original coedge binding")
            })?;
            if coedge.edge != edge_use.edge || coedge.edgeuse != use_.edgeuse {
                return Err(unavailable("source support belongs to another original native use"));
            }
            let correspondence = domain.original_coedge_correspondence(use_.coedge)?;
            let binding = correspondence.binding().ok_or_else(|| {
                unavailable("source support lost its original native binding")
            })?;
            if binding.edge != edge_use.edge
                || binding.owner != [use_.face, use_.wire, use_.index]
                || binding.endpoints != edge.vertices
                || binding.native_interval != [edge.interval.min(), edge.interval.max()]
                || correspondence.spatial_range() != collar.range
            {
                return Err(unavailable("source support collar has a different original spatial/native binding"));
            }
            let before = work.consumed();
            let surface = domain.surface_operand();
            domain.with_original_spatial_curve(
                use_.coedge, working, work, cancel,
                |prepared, work, cancel| {
                    // The selected source provider owns this gauge and its literal
                    // coefficients. Native edge intervals above bind identity only.
                    if !crate::parametric::curve_parameter_span_continuous(
                        prepared, collar.range, work, cancel,
                    )? {
                        return Err(unavailable("source support spatial provider has no whole-branch continuity proof"));
                    }
                    crate::parametric::curve_enclosure(
                        prepared, (collar.range[0], collar.range[1]), work, cancel,
                    )?;
                    for range in crate::parametric::curve_parameter_spans(
                        prepared, collar.range, work, cancel,
                    )? {
                        work.charge(1, cancel)?;
                        let spatial = crate::parametric::curve_enclosure(
                            prepared, (range[0], range[1]), work, cancel,
                        )?;
                        let uv = chart_jet(domain, use_.coedge, range, work, cancel)?;
                        let before = work.consumed();
                        let outcome = (|| {
                            let boundary = crate::parametric::surface_piecewise_enclosure(&surface,
                                uv_box(add_margin(uv.position, collar.margin)), work, cancel)?;
                            let components: [Option<Interval>; 3] = std::array::from_fn(|axis| intersection(
                                exact_difference(spatial.position[axis], boundary.position[axis]),
                                Interval { lo: -collar.position_error_mm, hi: collar.position_error_mm }));
                            let [Some(dx), Some(dy), Some(dz)] = components else {
                                return Err(unavailable("source material support contradicts its complete forward correspondence"));
                            };
                            let displacement = [dx, dy, dz];
                            let error = length(displacement)?.hi.min(collar.position_error_mm);
                            if !error.is_finite() || error > allowance {
                                return Err(unavailable("source material ribbon exceeds the frozen source allowance"));
                            }
                            let scalar_scratch = available.checked_sub(output_bytes(&output))
                                .and_then(|bytes| bytes.checked_sub(EVALUATION_SCRATCH_BYTES))
                                .ok_or_else(resource)?;
                            let scalar_base = collar_scalar_support(&collar, &surface, range, direction,
                                scalar_refinement, scalar_scratch, work, cancel)?;
                            let scalar_delta = mul(Interval { lo: 0.0, hi: 1.0 }, dot(direction, displacement));
                            let scalar_corrected = add(scalar_base, scalar_delta);
                            // Retain the complete lambda homotopy, including raw and
                            // corrected images at both CLOSED height cuts.
                            let support = SourceTransportSupport {
                                owner: SourceTransportOwner::OriginalCoedge { face: use_.face, coedge: use_.coedge },
                                base_interval: [scalar_base.lo, scalar_base.hi],
                                corrected_interval: [scalar_corrected.lo, scalar_corrected.hi],
                                displacement_interval: [scalar_delta.lo, scalar_delta.hi],
                                realized_witness_interval: None,
                                realized_endpoint_witnesses: None,
                                source_error_mm: error,
                            };
                            let output_limit = available.checked_sub(EVALUATION_SCRATCH_BYTES)
                                .ok_or_else(resource)?;
                            push(&mut output, support, output_limit)
                        })();
                        outcome.map_err(|error| work.annotate_error(error, before,
                            format_args!("source support central-coedge scalar enclosure; face={}, coedge={}, spatial_range={range:?}",
                                use_.face, use_.coedge)))?;
                    }
                    Ok(())
                },
            ).map_err(|error| work.annotate_error(error, before,
                format_args!("source support central-coedge curve preparation; face={}, coedge={}, spatial_range={:?}",
                    use_.face, use_.coedge, collar.range)))?;
        }
    }
    let vertices = incidence.vertices().ok_or_else(|| {
        unavailable("source material support is missing its complete original vertex inventory")
    })?;
    for (vertex_ordinal, vertex) in vertices.enumerate() {
        work.charge(1, cancel)?;
        let plan_limit = available
            .checked_sub(output_bytes(&output))
            .ok_or_else(resource)?;
        let before = work.consumed();
        let Some(selection) = SourceVertexPlan::prepare(
            source, &incidence, vertex, plan_limit, work, cancel,
        ).map_err(|error| work.annotate_error(error, before,
            format_args!("source support corner-owner preparation; source_vertex_ordinal={vertex_ordinal}")))?
        else {
            return Err(failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "source material corner has no evaluatable source-only realization",
                0,
                format_args!("source_vertex_ordinal={vertex_ordinal}"),
            ));
        };
        let plan = selection.plan();
        if plan.source_error_mm() > allowance {
            return Err(failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "source material corner exceeds the frozen source allowance",
                0,
                (vertex_ordinal, plan.source_error_mm(), allowance),
            ));
        }
        let output_limit = available
            .checked_sub(selection.owned_bytes())
            .ok_or_else(resource)?;
        let before = work.consumed();
        if let Some((reference, tolerance)) = scalar_refinement {
            plan.visit_endpoint_supports(
                source,
                direction,
                reference,
                tolerance,
                output_limit,
                work,
                cancel,
                &mut output,
            )
        } else {
            plan.visit_supports(direction, work, cancel, &mut |support| {
                push(&mut output, support, output_limit)
            })
        }
        .map_err(|error| {
            work.annotate_error(
                error,
                before,
                format_args!(
                    "source support corner scalar enclosure; source_vertex_ordinal={vertex_ordinal}"
                ),
            )
        })?;
    }
    Ok(output)
}

/// Query-local C0 enclosure only. The seed covers retained original F; added
/// cells are evaluated on the literal selected G, never on a source proxy.
pub(super) struct SourceProjectionCover<'a> {
    rows: &'a [[Interval; 3]; 3],
    translation: [f64; 3],
    interval: [f64; 2],
    xy: [[f64; 2]; 2],
    tolerance: f64,
}

impl SourceProjectionCover<'_> {
    pub(super) fn include(&mut self, position: [Interval; 3]) -> Result<bool, GeometryError> {
        let world: [Interval; 3] = std::array::from_fn(|axis| {
            add(
                dot(self.rows[axis], position),
                Interval::point(self.translation[axis]),
            )
        });
        for value in world {
            Interval::new(value.lo, value.hi)?;
        }
        if world[2].hi < self.interval[0] || world[2].lo > self.interval[1] {
            return Ok(true);
        }
        if (0..2)
            .all(|axis| self.xy[axis][0] <= world[axis].lo && world[axis].hi <= self.xy[axis][1])
        {
            return Ok(true);
        }
        if (0..2).any(|axis| world[axis].sub(Interval::point(world[axis].lo)).hi > self.tolerance) {
            return Ok(false);
        }
        for axis in 0..2 {
            self.xy[axis][0] = self.xy[axis][0].min(world[axis].lo);
            self.xy[axis][1] = self.xy[axis][1].max(world[axis].hi);
        }
        Ok(true)
    }

    pub(super) fn split(
        &self,
        parameter: [[f64; 2]; 2],
        depth: u8,
        face: usize,
        first: [Interval; 3],
        second: [Interval; 3],
    ) -> Result<[[[f64; 2]; 2]; 2], GeometryError> {
        let first: [Interval; 3] = std::array::from_fn(|axis| dot(self.rows[axis], first));
        let second: [Interval; 3] = std::array::from_fn(|axis| dot(self.rows[axis], second));
        // Include world height in the motion metric: an excluded cut-crosser
        // must refine in its actual G recipe direction, not a source-F proxy.
        crate::analysis::critical::split_metric_derivatives(parameter, depth, face, &first, &second)
    }
}

struct ClosedSourceExit {
    edge: EdgeId,
    /// High endpoint in the original spatial provider gauge, not native back.
    spatial_high: bool,
    parameter: f64,
}

/// Overcover the added actual-G pieces intersecting the CLOSED world slab.
/// No physical Original-F/G error, material membership or void is asserted.
#[allow(clippy::too_many_arguments)]
pub(crate) fn source_transport_projection_cover(
    source: MaterialSourceView<'_, '_>,
    matrix: [[Interval; 3]; 3],
    pose: spiling_contracts::geometry::RigidPoseMm,
    interval: [f64; 2],
    original_xy: [[f64; 2]; 2],
    tolerance: f64,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[f64; 2]; 2], GeometryError> {
    type Pending = [([[f64; 2]; 2], u8); MAX_DEPTH as usize + 1];
    const FIXED_BYTES: usize = EVALUATION_SCRATCH_BYTES
        + std::mem::size_of::<Pending>()
        + std::mem::size_of::<SourceProjectionCover<'static>>()
        + std::mem::size_of::<OriginalCollar<'static, 'static>>()
        + 2 * std::mem::size_of::<SurfaceFirstEnclosure>()
        + std::mem::size_of::<ChartCurveEnclosure>()
        + std::mem::size_of::<CurveEnclosure>()
        + std::mem::size_of::<Vec<ClosedSourceExit>>();
    work.charge(1, cancel)?;
    let definition = source.definition();
    if let Some(plans) = source.vertex_plans() {
        plans.validate_source(definition, work, cancel)?;
    }
    if source.material_transport_is_proved_zero(work, cancel)? {
        return Ok(original_xy);
    }
    let mut cover = SourceProjectionCover {
        rows: &matrix,
        translation: pose.translation_mm,
        interval,
        xy: original_xy,
        tolerance,
    };
    let available = remaining_bytes
        .checked_sub(FIXED_BYTES)
        .ok_or_else(resource)?;
    let mut incidence = SourceEdgeIncidence::new(definition, available, work, cancel)?;
    incidence.prepare_vertices(available, work, cancel)?;
    let available = available
        .checked_sub(incidence.owned_bytes())
        .ok_or_else(resource)?;
    let mut exit_count = 0usize;
    for _ in incidence.values() {
        work.charge(1, cancel)?;
        exit_count = exit_count.checked_add(2).ok_or_else(resource)?;
    }
    let requested = exit_count
        .checked_mul(std::mem::size_of::<ClosedSourceExit>())
        .filter(|bytes| *bytes <= available)
        .ok_or_else(resource)?;
    let mut exits: Vec<ClosedSourceExit> = Vec::new();
    exits
        .try_reserve_exact(exit_count)
        .map_err(|_| resource())?;
    let exit_bytes = exits
        .capacity()
        .checked_mul(std::mem::size_of::<ClosedSourceExit>())
        .filter(|bytes| requested <= *bytes && *bytes <= available)
        .ok_or_else(resource)?;
    let available = available.checked_sub(exit_bytes).ok_or_else(resource)?;
    work.charge(exit_bytes as u64, cancel)?;
    let vertices = incidence.vertices().ok_or_else(|| {
        unavailable("actual-G cover lacks the complete original vertex inventory")
    })?;
    for (ordinal, vertex) in vertices.enumerate() {
        work.charge(1, cancel)?;
        let before = work.consumed();
        let outcome = (|| {
            let selection =
                SourceVertexPlan::prepare(source, &incidence, vertex, available, work, cancel)?
                    .ok_or_else(|| {
                        unavailable("actual-G cover lacks a selected original corner")
                    })?;
            let scratch = available
                .checked_sub(selection.owned_bytes())
                .ok_or_else(resource)?;
            let plan = selection.plan();
            plan.visit_projected_position_supports(source, &mut cover, scratch, work, cancel)?;
            plan.visit_spatial_exits(work, cancel, &mut |edge, spatial_high, parameter, work| {
                if !parameter.is_finite() || incidence.get(edge).is_none() {
                    return Err(unavailable(
                        "actual-G corner has a foreign or nonfinite original spatial exit",
                    ));
                }
                for previous in &exits {
                    work.charge(1, cancel)?;
                    if previous.edge == edge && previous.spatial_high == spatial_high {
                        return if previous.parameter == parameter {
                            Ok(())
                        } else {
                            Err(unavailable(
                                "actual-G corners disagree on their literal original spatial exit",
                            ))
                        };
                    }
                }
                if exits.len() >= exit_count {
                    return Err(unavailable(
                        "actual-G corners exceed the original endpoint inventory",
                    ));
                }
                exits.push(ClosedSourceExit {
                    edge,
                    spatial_high,
                    parameter,
                });
                Ok(())
            })
        })();
        outcome.map_err(|error| {
            work.annotate_error(
                error,
                before,
                format_args!("actual-G C0 corner cover; source_vertex_ordinal={ordinal}"),
            )
        })?;
    }
    if exits.len() != exit_count {
        return Err(unavailable(
            "actual-G cover lacks complete literal original spatial endpoint exits",
        ));
    }
    let mut stack: Pending = [([[0.0; 2]; 2], 0); MAX_DEPTH as usize + 1];
    for uses in incidence.values() {
        let second = uses.second.ok_or_else(|| {
            unavailable("actual-G cover requires two opposed original coedge uses")
        })?;
        for use_ in [uses.first, second] {
            work.charge(1, cancel)?;
            let before = work.consumed();
            let outcome = (|| {
                let domain = source.face_domain(use_.face, available, work, cancel)?;
                let scratch = available;
                let collar = OriginalCollar::for_enclosure_with_byte_limit(
                    domain,
                    use_,
                    scratch,
                    work,
                    cancel,
                )?
                .ok_or_else(|| unavailable("actual-G ribbon has no complete original recipe"))?;
                if matches!(collar.transport, CarrierTransport::Exact) {
                    // No added ribbon: its original F is already in the seed.
                    return Ok(());
                }
                let (edge_use, edge) = native_coedge(definition, use_.face, use_.wire, use_.index)?;
                let coedge = domain.coedges().get(use_.coedge).ok_or_else(|| {
                    unavailable("actual-G ribbon lost its original coedge binding")
                })?;
                if coedge.edge != edge_use.edge || coedge.edgeuse != use_.edgeuse {
                    return Err(unavailable("actual-G ribbon belongs to another original native use"));
                }
                let mut central = [None; 2];
                for exit in &exits {
                    work.charge(1, cancel)?;
                    if exit.edge == edge_use.edge {
                        central[usize::from(exit.spatial_high)] = Some(exit.parameter);
                    }
                }
                let [Some(front), Some(back)] = central else {
                    return Err(unavailable(
                        "actual-G ribbon lacks both literal original spatial endpoint exits",
                    ));
                };
                if collar.range[0] >= front || front >= back || back >= collar.range[1] {
                    return Err(unavailable(
                        "actual-G exits do not partition the original spatial ribbon",
                    ));
                }
                let correspondence = domain.original_coedge_correspondence(use_.coedge)?;
                let binding = correspondence.binding().ok_or_else(|| {
                    unavailable("actual-G ribbon lost its original native binding")
                })?;
                if binding.edge != edge_use.edge
                    || binding.owner != [use_.face, use_.wire, use_.index]
                    || binding.endpoints != edge.vertices
                    || binding.native_interval != [edge.interval.min(), edge.interval.max()]
                    || correspondence.spatial_range() != collar.range
                {
                    return Err(unavailable("actual-G ribbon has a different original spatial/native binding"));
                }
                let provider_query_bytes = scratch.checked_add(EVALUATION_SCRATCH_BYTES).ok_or_else(resource)?;
                domain.with_original_spatial_curve(
                    use_.coedge, provider_query_bytes, work, cancel,
                    |prepared, work, cancel| {
                        if !crate::parametric::curve_parameter_span_continuous(
                            prepared, collar.range, work, cancel,
                        )? {
                            return Err(unavailable("actual-G spatial provider has no whole-branch continuity proof"));
                        }
                        crate::parametric::curve_enclosure(
                            prepared, (front, back), work, cancel,
                        )?;
                        let mut covered = front;
                        for span in crate::parametric::curve_parameter_spans(
                            prepared, [front, back], work, cancel,
                        )? {
                            work.charge(1, cancel)?;
                            let range = [span[0].max(front), span[1].min(back)];
                            if range[0] > range[1] {
                                continue;
                            }
                            if range[0] > covered {
                                return Err(unavailable(
                                    "actual-G coefficient spans leave an uncovered original spatial interval",
                                ));
                            }
                            covered = covered.max(range[1]);
                            stack[0] = ([range, [0.0, 1.0]], 0);
                            let mut pending = 1;
                            while pending != 0 {
                                pending -= 1;
                                let (parameter, depth) = stack[pending];
                                work.charge(1, cancel)?;
                                let spatial = crate::parametric::curve_enclosure(
                                    prepared, (parameter[0][0], parameter[0][1]), work, cancel,
                                )?;
                                let jet =
                                    collar.position_first_jet(parameter, &spatial, work, cancel)?;
                                if cover.include(jet.0)? {
                                    continue;
                                }
                                let children =
                                    cover.split(parameter, depth, use_.face, jet.1, jet.2)?;
                                if pending + children.len() > stack.len() {
                                    return Err(resource());
                                }
                                for child in children.into_iter().rev() {
                                    stack[pending] = (child, depth + 1);
                                    pending += 1;
                                }
                            }
                        }
                        if covered < back {
                            return Err(unavailable(
                                "actual-G coefficient spans omit its closed original spatial endpoint",
                            ));
                        }
                        Ok(())
                    },
                )
            })();
            outcome.map_err(|error| {
                work.annotate_error(
                    error,
                    before,
                    format_args!(
                        "actual-G C0 central ribbon cover; face={}, coedge={}",
                        use_.face, use_.coedge,
                    ),
                )
            })?;
        }
    }
    Ok(cover.xy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::source_operand::{
        PreparedOriginalSurface, SOURCE_OPERAND_SCRATCH_BYTES,
    };

    #[test]
    fn source_scalar_support_preserves_original_radian_surface_law() {
        let source = solidcraft_step::geom::Surface::Cylinder {
            position: solidcraft_step::geom::Placement {
                origin: [3.0, -4.0, 5.0],
                axis: [0.0, 0.0, 1.0],
                ref_direction: [1.0, 0.0, 0.0],
            },
            radius: 2.0,
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let prepared = PreparedOriginalSurface::new(
            &source,
            [[0.0, std::f64::consts::TAU], [-1.0, 1.0]],
            SOURCE_OPERAND_SCRATCH_BYTES + 64 * 1024,
            &mut work,
            &cancel,
        ).unwrap();
        let angle = 0.73_f64;
        let (height, derivative) = scalar_surface_jet(
            &prepared,
            [[angle; 2], [0.2; 2]],
            [1.0, 0.0, 0.0].map(Interval::point),
            &mut work,
            &cancel,
        ).unwrap();
        let expected = 3.0 + 2.0 * angle.cos();
        let expected_du = -2.0 * angle.sin();
        assert!(height.lo <= expected && expected <= height.hi);
        assert!(derivative[0].lo <= expected_du && expected_du <= derivative[0].hi);
        assert!(derivative[1].contains_zero());
        // The native analytic degree chart at the same number has x near 5
        // and a much smaller derivative; neither is this original operand.
        assert!(height.hi < 4.6);
        assert!(derivative[0].hi < -1.0);
    }
}
