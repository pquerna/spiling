// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Complete scalar support of the immutable source-relative realization.
//! Position bounds are not a material embedding certificate: that authority
//! additionally requires the closed radial link and complete degree proof.

use super::*;
use vertex_transport::SourceVertexTransportPlan;

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
    /// Physical three-dimensional error, independent of the scalar covector.
    pub(crate) source_error_mm: f64,
}

fn resource() -> GeometryError {
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
fn output_bytes(output: &Vec<SourceTransportSupport>) -> usize {
    output
        .capacity()
        .saturating_mul(std::mem::size_of::<SourceTransportSupport>())
}
fn push(
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
fn scalar_surface_jet(
    surface: &monstertruck_modeling::Surface,
    uv: [[f64; 2]; 2],
    direction: [Interval; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(Interval, [Interval; 2]), GeometryError> {
    use monstertruck_modeling::{
        ValidatedGeometryBudget, ValidatedInterval, ValidatedSurfaceFunctional,
    };
    const ZERO: ValidatedInterval = ValidatedInterval {
        lower: 0.0,
        upper: 0.0,
    };
    let mut native =
        ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
    let result = monstertruck_modeling::validated_surface_functional_jets(
        surface,
        uv,
        [ValidatedSurfaceFunctional {
            direction: direction.map(|value| ValidatedInterval {
                lower: value.lo,
                upper: value.hi,
            }),
            origin: [ZERO; 3],
            offset: ZERO,
        }],
        &mut native,
        cancel,
    );
    crate::parametric::debit_native_work(work, native.consumed(), cancel)?;
    let [jet] = result.map_err(crate::parametric::native_error)?;
    let interval = |value: ValidatedInterval| Interval {
        lo: value.lower,
        hi: value.upper,
    };
    Ok((interval(jet.position), [interval(jet.du), interval(jet.dv)]))
}

fn collar_scalar_support(
    collar: &OriginalCollar<'_, '_>,
    surface: &monstertruck_modeling::Surface,
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
    source: OriginalTrimSource<'_, '_>,
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
    if definition.material_transport_is_proved_zero(work, cancel)? {
        return Ok(Vec::new());
    }
    let allowance = source_allowance(definition)?;
    // Bounds and other standalone consumers share the same original domain
    // owner as height queries. Prepared sources already lend and charge it.
    let before = work.consumed();
    let originals = source
        .prepare_domains_if_missing(remaining_bytes, work, cancel)
        .map_err(|error| {
            work.annotate_error(
                error,
                before,
                "source support original-domain owner preparation",
            )
        })?;
    let source = source.borrowing_domains(originals.as_ref());
    let remaining_bytes = remaining_bytes
        .checked_sub(originals.as_ref().map_or(0, |owner| owner.retained_bytes()))
        .ok_or_else(resource)?;
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
            let before = work.consumed();
            let Some(collar) = OriginalCollar::for_enclosure(&domain, use_, work, cancel)
                .map_err(|error| work.annotate_error(error, before,
                    format_args!("source support central-coedge owner preparation; face={}, coedge={}, native_range={:?}",
                        use_.face, use_.coedge, domain.original_coedge_correspondence(use_.coedge).ok().map(|proof| proof.native_range()))))? else {
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
            let working = face_limit
                .checked_sub(domain.owned_bytes())
                .ok_or_else(resource)?;
            let face = domain.native().face();
            let edge = &face.absolute_boundaries()[use_.wire][use_.index];
            let before = work.consumed();
            edge.with_curve(|curve| -> Result<(), GeometryError> {
                let before = work.consumed();
                let prepared = definition.prepare_boundary_curve(edge.id(), curve,
                    (collar.range[0], collar.range[1]), working, work, cancel)
                    .map_err(|error| work.annotate_error(error, before,
                        format_args!("source support central-coedge curve preparation; face={}, coedge={}, native_range={:?}",
                            use_.face, use_.coedge, collar.range)))?;
                for span in prepared.parameter_intervals() {
                    work.charge(1, cancel)?;
                    let range = [span.0, span.1];
                    let spatial = prepared.jet(span, work, cancel)?;
                    let uv = chart_jet(&domain, use_.coedge, range, work, cancel)?;
                    let before = work.consumed();
                    face.with_surface(|surface| -> Result<(), GeometryError> {
                        let boundary = crate::parametric::surface_piecewise_enclosure(surface,
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
                        let scalar_scratch = available.checked_sub(domain.owned_bytes())
                            .and_then(|bytes| bytes.checked_sub(prepared.retained_bytes()))
                            .and_then(|bytes| bytes.checked_sub(output_bytes(&output))).ok_or_else(resource)?;
                        let scalar_base = collar_scalar_support(&collar, surface, range, direction,
                            scalar_refinement, scalar_scratch, work, cancel)?;
                        let scalar_delta = mul(Interval { lo: 0.0, hi: 1.0 }, dot(direction, displacement));
                        let scalar_corrected = add(scalar_base, scalar_delta);
                        // The whole lambda homotopy is retained, including raw
                        // and corrected images at both CLOSED height cuts.
                        let support = SourceTransportSupport {
                            owner: SourceTransportOwner::OriginalCoedge { face: use_.face, coedge: use_.coedge },
                            base_interval: [scalar_base.lo, scalar_base.hi],
                            corrected_interval: [scalar_corrected.lo, scalar_corrected.hi],
                            displacement_interval: [scalar_delta.lo, scalar_delta.hi],
                            source_error_mm: error,
                        };
                        let output_limit = available.checked_sub(domain.owned_bytes())
                            .and_then(|v| v.checked_sub(prepared.retained_bytes())).ok_or_else(resource)?;
                        push(&mut output, support, output_limit)
                    }).map_err(|error| work.annotate_error(error, before,
                        format_args!("source support central-coedge scalar enclosure; face={}, coedge={}, native_range={range:?}",
                            use_.face, use_.coedge)))?;
                }
                Ok(())
            }).map_err(|error| work.annotate_error(error, before,
                format_args!("source support central-coedge preparation; face={}, coedge={}, native_range={:?}",
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
        let Some(plan) = SourceVertexTransportPlan::prepare(
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
        if plan.source_error_mm() > allowance {
            return Err(failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "source material corner exceeds the frozen source allowance",
                0,
                (vertex_ordinal, plan.source_error_mm(), allowance),
            ));
        }
        let output_limit = available
            .checked_sub(plan.retained_bytes())
            .ok_or_else(resource)?;
        let before = work.consumed();
        plan.visit_supports(direction, work, cancel, &mut |support| {
            push(&mut output, support, output_limit)
        })
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
