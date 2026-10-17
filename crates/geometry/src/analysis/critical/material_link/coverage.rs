// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

pub(in crate::analysis::critical) const MAX_CORRECTION_BYTES: usize = MAX_EDGE_BYTES;

/// Error-only, source-local witness. Native allocation IDs remain private
/// proof keys and must never become an IPC diagnostic identifier.
fn vertex_diagnostic(
    definition: &Definition,
    incidence: &SourceEdgeIncidence,
    vertex: monstertruck_topology::VertexId<monstertruck_modeling::Point3>,
) -> Option<([usize; 3], [f64; 3])> {
    let id = incidence.vertex_edges(vertex).next()?;
    let owner = incidence.get(id)?.first;
    let face = definition.solid.face_iter().nth(owner.face)?;
    let edge = face
        .absolute_boundaries()
        .get(owner.wire)?
        .get(owner.index)?;
    if edge.id() != id {
        return None;
    }
    let point = if edge.absolute_front().id() == vertex {
        edge.absolute_front().point()
    } else if edge.absolute_back().id() == vertex {
        edge.absolute_back().point()
    } else {
        return None;
    };
    Some((
        [owner.face, owner.wire, owner.index],
        [point.x, point.y, point.z],
    ))
}

/// A negative-witness prerequisite only: the caller supplies the original
/// carrier evaluation of this SAME UV box. Strict original trim ownership and
/// exclusion of every removed R/K_F support prove that evaluation is still F
/// in the fixed realization. This grants no positive material/topology proof.
/// `remaining_bytes` excludes caller-owned domain/incidence/input storage.
#[allow(clippy::too_many_arguments)]
pub(in crate::analysis::critical) fn source_f_point_is_retained(
    source: OriginalTrimSource<'_, '_>,
    domain: &PreparedFaceTrimDomain<'_>,
    face_index: usize,
    uv: [[f64; 2]; 2],
    position: [Interval; 3],
    incidence: &SourceEdgeIncidence,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    const FIXED_BYTES: usize = std::mem::size_of::<ChartCurveEnclosure>()
        + std::mem::size_of::<[[f64; 2]; 2]>()
        + 2 * std::mem::size_of::<[Interval; 3]>()
        + std::mem::size_of::<Option<vertex_transport::CompletePositionCoverFailure>>();
    let before = work.consumed();
    let mut phase = "original source/domain binding";
    let mut active_coedge = None;
    let mut active_vertex = None;
    let outcome = (|| {
        work.charge(1, cancel)?;
        let available = remaining_bytes
            .min(MAX_SCRATCH_BYTES)
            .checked_sub(FIXED_BYTES)
            .ok_or_else(|| {
                failure(
                    GeometryErrorCode::ResourceLimit,
                    "retained original-F witness exceeds remaining scratch",
                    face_index,
                    (),
                )
            })?;
        if uv
            .iter()
            .any(|v| !v[0].is_finite() || !v[1].is_finite() || v[0] > v[1])
            || position
                .iter()
                .any(|v| !v.lo.is_finite() || !v.hi.is_finite() || v.lo > v.hi)
        {
            return Ok(false);
        }
        let definition = source.definition();
        let Some(face) = definition.solid.face_iter().nth(face_index) else {
            return Ok(false);
        };
        let Some(original) = definition.face_boundaries.get(face_index) else {
            return Ok(false);
        };
        let boundaries = face.absolute_boundaries();
        if domain.native().face().id() != face.id()
            || original.face != face.id()
            || original.wires.is_empty()
            || original.wires.len() != boundaries.len()
        {
            return Ok(false);
        }
        let Some(first) = original.wires.iter().flatten().next() else {
            return Ok(false);
        };
        // Bind the literal immutable metadata owner, not a matching source ID.
        // This is an input guard; actual trim/curve bounds below grant proof.
        if !std::ptr::eq(domain.original_coedge(0)?, first) {
            return Ok(false);
        }
        phase = "strict whole original trim Inside";
        let inclusive = available
            .checked_add(domain.retained_bytes())
            .ok_or_else(|| {
                failure(
                    GeometryErrorCode::ResourceLimit,
                    "retained original-F classifier storage overflow",
                    face_index,
                    (),
                )
            })?
            .min(MAX_SCRATCH_BYTES);
        if domain.classify_with_byte_limit(uv, inclusive, work, cancel)?
            != TrimClassification::Inside
        {
            return Ok(false);
        }
        phase = "all complete original reference-R overcovers";
        let point_uv = uv.map(|v| Interval { lo: v[0], hi: v[1] });
        let mut flat = 0;
        for (wire_index, (wire, references)) in boundaries.iter().zip(&original.wires).enumerate() {
            if wire.is_empty() || wire.len() != references.len() {
                return Ok(false);
            }
            for (index, (edge, reference)) in wire.iter().zip(references).enumerate() {
                work.charge(1, cancel)?;
                active_coedge = Some((wire_index, index, flat));
                if !std::ptr::eq(domain.original_coedge(flat)?, reference)
                    || reference.edge != edge.id()
                    || reference.orientation != edge.orientation()
                {
                    return Ok(false);
                }
                let use_ = CoedgeUse {
                    face: face_index,
                    coedge: flat,
                    wire: wire_index,
                    index,
                    orientation: edge.orientation() == face.orientation(),
                };
                let Some(uses) = incidence.get(edge.id()) else {
                    return Ok(false);
                };
                if (uses.first != use_ && uses.second != Some(use_))
                    || uses.vertices != [edge.absolute_front().id(), edge.absolute_back().id()]
                {
                    return Ok(false);
                }
                let proof = domain.original_coedge_correspondence(flat)?;
                let Some(binding) = proof.binding() else {
                    return Ok(false);
                };
                if binding.face != face.id()
                    || binding.edge != edge.id()
                    || binding.owner != [face_index, wire_index, index]
                    || binding.endpoints != [edge.front().id(), edge.back().id()]
                    || proof.provenance()
                        == crate::kernel::OriginalCorrespondenceProvenance::CompletedPoleGraph
                {
                    return Ok(false);
                }
                let mapped = proof.uv_range();
                if proof
                    .affine_native_parameter_preimage([
                        mapped[0].min(mapped[1]),
                        mapped[0].max(mapped[1]),
                    ])?
                    .is_none()
                {
                    return Ok(false);
                }
                let range = proof.native_range();
                let width = (range[1] - range[0])
                    * domain.material_realization().collar_parameter_fraction();
                if !width.is_finite() || width <= 0.0 {
                    return Ok(false);
                }
                // beta∈[0,1/4], w∈[0,1], and BOTH possible inward signs.
                // The complete original piecewise first jet includes knots
                // and closed endpoints; no gamma C1/embedding is inferred.
                let jet = domain.original_coedge_uv_piecewise_jet(flat, range, work, cancel)?;
                let radius = mul(Interval::point(width), Interval::point(0.25)).hi;
                let rotated = rotate(jet.derivative, 1.0);
                let tube = add_margin(
                    std::array::from_fn(|i| {
                        add(
                            jet.position[i],
                            mul(
                                rotated[i],
                                Interval {
                                    lo: -radius,
                                    hi: radius,
                                },
                            ),
                        )
                    }),
                    proof.margin_uv(),
                );
                if !(0..2).any(|i| point_uv[i].hi < tube[i].lo || tube[i].hi < point_uv[i].lo) {
                    return Ok(false);
                }
                flat += 1;
            }
        }
        if flat != domain.native().coedges().len() {
            return Ok(false);
        }
        // Only a source-coefficient identity may eliminate all K_F recipes.
        if definition.material_transport_is_proved_zero(work, cancel)? {
            return Ok(true);
        }
        let Some(vertices) = incidence.vertices() else {
            return Ok(false);
        };
        let mut found_owner = false;
        phase = "all owning original reference-K_F position overcovers";
        for vertex in vertices {
            work.charge(1, cancel)?;
            let mut owns_face = false;
            for edge in incidence.vertex_edges(vertex) {
                work.charge(1, cancel)?;
                let Some(uses) = incidence.get(edge) else {
                    return Ok(false);
                };
                if uses.first.face == face_index
                    || uses.second.is_some_and(|use_| use_.face == face_index)
                {
                    owns_face = true;
                    break;
                }
            }
            if !owns_face {
                continue;
            }
            found_owner = true;
            active_vertex = vertex_diagnostic(definition, incidence, vertex);
            let Some(selection) = vertex_transport::SourceVertexPlan::prepare(
                source, incidence, vertex, available, work, cancel,
            )?
            else {
                return Ok(false);
            };
            let scratch = available
                .checked_sub(selection.owned_bytes())
                .ok_or_else(|| {
                    failure(
                        GeometryErrorCode::ResourceLimit,
                        "retained original-F corner selection exceeds scratch",
                        face_index,
                        (),
                    )
                })?;
            let mut failed_cover = None;
            if !selection.plan().complete_incident_positions_miss(
                source,
                scratch,
                work,
                cancel,
                &mut failed_cover,
                &mut |cover| {
                    Ok(
                        (0..3)
                            .any(|i| position[i].hi < cover[i].lo || cover[i].hi < position[i].lo),
                    )
                },
            )? {
                // No promotion of cell.source and no implicit empty support.
                return Ok(false);
            }
        }
        Ok(found_owner)
    })();
    outcome.map_err(|error| work.annotate_error(error, before, format_args!(
        "retained original-F negative witness face={face_index} uv={uv:?} phase={phase} coedge={active_coedge:?} vertex_owner_mm={active_vertex:?}"
    )))
}

#[derive(Clone, Copy)]
struct NativeEndpointExit {
    edge: monstertruck_topology::EdgeId<Curve>,
    absolute_back: bool,
    parameter: f64,
}

/// Query-local proof storage, never an alternative source representation.
/// Every nonzero source transport support is retained until a COMPLETE regular
/// fiber census has calibrated the closed boundary's total covering degree.
pub(in crate::analysis::critical) struct ClosedRadialLinks {
    supports: Vec<[Interval; 3]>,
    pub source_error_mm: f64,
    exits: Vec<NativeEndpointExit>,
    unchanged_source: bool,
    all_vertex_supports_outside: bool,
}
impl ClosedRadialLinks {
    pub fn new(
        source: OriginalTrimSource<'_, '_>,
        incidence: &SourceEdgeIncidence,
        prism: &SamplePrism,
        request: &SlabRequest,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        Self::prepare(
            source,
            incidence,
            Some((prism, request)),
            remaining_bytes,
            work,
            cancel,
            &mut |_, _, _| Ok(()),
        )
    }

    /// The SAME complete source vertex inventory and literal native exits as
    /// radial coverage, qualified independently of a physical witness field.
    pub(super) fn geometry(
        source: OriginalTrimSource<'_, '_>,
        incidence: &SourceEdgeIncidence,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        visitor: &mut impl FnMut(usize, [Interval; 3], [Interval; 3]) -> Result<(), GeometryError>,
    ) -> Result<Self, GeometryError> {
        Self::prepare(
            source,
            incidence,
            None,
            remaining_bytes,
            work,
            cancel,
            visitor,
        )
    }

    /// Source-only endpoint inventory for a CLOSED cylinder atlas. Neither an
    /// open-band exclusion nor an actual-cone-only box can discharge this:
    /// every original vertex and every SOURCE/reference/realized star support
    /// must miss the two cuts strictly. No vertex chart is admitted here.
    pub(super) fn outside_closed_scope(
        source: OriginalTrimSource<'_, '_>,
        incidence: &SourceEdgeIncidence,
        coordinates: &crate::analysis::placed::ActualMaterialCoordinates<'_>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        let mut active_before = work.consumed();
        let mut phase = "immutable source vertex plan validation";
        let mut active_vertex = None;
        let outcome = (|| {
            let definition = source.definition();
            if let Some(plans) = source.vertex_plans() {
                plans.validate_source(definition, work, cancel)?;
            }
            let unchanged_source = definition.material_transport_is_proved_zero(work, cancel)?;
            let allowance = source_allowance(definition)?;
            let interval = coordinates.world_interval();
            let outside = |position: [Interval; 3]| -> Result<bool, GeometryError> {
                let expanded = position.map(|value| {
                    add(
                        value,
                        Interval {
                            lo: -allowance,
                            hi: allowance,
                        },
                    )
                });
                let z = coordinates.position(expanded)?[2];
                Ok(z.hi < interval[0] || z.lo > interval[1])
            };
            let vertices = incidence.vertices().ok_or_else(|| {
                failure(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "closed cylinder atlas lacks the original vertex inventory",
                    0,
                    (),
                )
            })?;
            let mut result = Self {
                supports: Vec::new(),
                source_error_mm: 0.0,
                exits: Vec::new(),
                unchanged_source,
                all_vertex_supports_outside: true,
            };
            if !unchanged_source {
                let count = incidence.values().count().checked_mul(2).ok_or_else(|| {
                    failure(
                        GeometryErrorCode::ResourceLimit,
                        "closed cylinder endpoint capacity overflow",
                        0,
                        (),
                    )
                })?;
                let bytes = count
                    .checked_mul(std::mem::size_of::<NativeEndpointExit>())
                    .ok_or_else(|| {
                        failure(
                            GeometryErrorCode::ResourceLimit,
                            "closed cylinder endpoint storage overflow",
                            0,
                            (),
                        )
                    })?;
                if bytes > remaining_bytes.min(MAX_CORRECTION_BYTES) {
                    return Err(failure(
                        GeometryErrorCode::ResourceLimit,
                        "closed cylinder endpoint capacity exceeds scratch allowance",
                        0,
                        count,
                    ));
                }
                work.charge(count as u64, cancel)?;
                result.exits.try_reserve_exact(count).map_err(|_| {
                    failure(
                        GeometryErrorCode::ResourceLimit,
                        "closed cylinder endpoint allocation refused",
                        0,
                        count,
                    )
                })?;
                if result.allocated_bytes() > remaining_bytes.min(MAX_CORRECTION_BYTES) {
                    return Err(failure(
                        GeometryErrorCode::ResourceLimit,
                        "closed cylinder endpoint allocator exceeded allowance",
                        0,
                        count,
                    ));
                }
            }
            for vertex in vertices {
                active_before = work.consumed();
                active_vertex = None;
                phase = "literal original incident cycle";
                work.charge(1, cancel)?;
                if !incidence.vertex_link_is_cycle(definition, vertex, work, cancel)? {
                    return Ok(None);
                }
                let Some((owner, point)) = vertex_diagnostic(definition, incidence, vertex) else {
                    return Err(failure(
                        GeometryErrorCode::KernelFailure,
                        "original vertex inventory lost its literal point",
                        0,
                        (),
                    ));
                };
                active_vertex = Some((owner, point));
                phase = "closed original vertex position exclusion";
                let original = point.map(Interval::point);
                if !outside(original)? {
                    return Ok(None);
                }
                if unchanged_source {
                    continue;
                }
                let available = remaining_bytes
                    .checked_sub(result.allocated_bytes())
                    .ok_or_else(|| {
                        failure(
                            GeometryErrorCode::ResourceLimit,
                            "closed cylinder endpoint inventory exceeds scratch allowance",
                            0,
                            (),
                        )
                    })?;
                phase = "immutable source corner plan preparation";
                let Some(selection) = vertex_transport::SourceVertexPlan::prepare(
                    source, incidence, vertex, available, work, cancel,
                )?
                else {
                    return Ok(None);
                };
                let plan = selection.plan();
                let scratch = available
                    .checked_sub(selection.owned_bytes())
                    .ok_or_else(|| {
                        failure(
                            GeometryErrorCode::ResourceLimit,
                            "closed cylinder corner qualification exceeds scratch allowance",
                            0,
                            (),
                        )
                    })?;
                phase = "complete original incident-face/added-G position cover";
                let mut failed_cover = None;
                if !plan.complete_incident_positions_miss(
                    source,
                    scratch,
                    work,
                    cancel,
                    &mut failed_cover,
                    &mut |position| outside(position),
                )? {
                    // A failed conservative cover grants nothing. Preserve the
                    // complete exact removed-domain qualification before using
                    // the tighter reference-star/comparison source support.
                    phase = "complete removed-corner domain/active-cone qualification";
                    let qualification_before = work.consumed();
                    plan.visit_qualified_geometry(source, scratch, work, cancel, &mut |_, _, _| {
                        Ok(())
                    })
                    .map_err(|error| {
                        work.annotate_error(
                            error,
                            qualification_before,
                            format_args!(
                                "failed complete corner cover phase_face_source_box={:?}",
                                failed_cover.map(|cover| (
                                    cover.phase,
                                    cover.source_face,
                                    cover.position
                                ))
                            ),
                        )
                    })?;
                    phase = "complete source/reference/G support exclusion";
                    let mut excluded = true;
                    plan.visit_position_supports(
                        work,
                        cancel,
                        &mut |_realized, source_and_realized| {
                            excluded &= outside(source_and_realized)?;
                            Ok(())
                        },
                    )?;
                    if !excluded {
                        return Ok(None);
                    }
                }
                phase = "literal native central exits";
                plan.visit_native_exits(work, cancel, &mut |edge, absolute_back, parameter| {
                    if result.exits.len() == result.exits.capacity()
                        && !result.exits.iter().any(|previous| {
                            previous.edge == edge && previous.absolute_back == absolute_back
                        })
                    {
                        return Err(failure(
                            GeometryErrorCode::KernelFailure,
                            "source vertex exit is absent from complete original edge inventory",
                            0,
                            (),
                        ));
                    }
                    result.record_exit(NativeEndpointExit {
                        edge,
                        absolute_back,
                        parameter,
                    })
                })?;
                if result
                    .allocated_bytes()
                    .saturating_add(selection.owned_bytes())
                    > remaining_bytes
                {
                    return Err(failure(
                        GeometryErrorCode::ResourceLimit,
                        "closed cylinder endpoint exits exceed live scratch allowance",
                        0,
                        (),
                    ));
                }
                result.source_error_mm = result.source_error_mm.max(plan.source_error_mm());
            }
            Ok(Some(result))
        })();
        outcome.map_err(|error| {
            work.annotate_error(
                error,
                active_before,
                format_args!(
                    "closed radial corner support vertex_owner_mm={active_vertex:?} phase={phase}"
                ),
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare(
        source: OriginalTrimSource<'_, '_>,
        incidence: &SourceEdgeIncidence,
        field: Option<(&SamplePrism, &SlabRequest)>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        visitor: &mut impl FnMut(usize, [Interval; 3], [Interval; 3]) -> Result<(), GeometryError>,
    ) -> Result<Self, GeometryError> {
        let definition = source.definition();
        if let Some(plans) = source.vertex_plans() {
            plans.validate_source(definition, work, cancel)?;
        }
        let allowance = source_allowance(definition)?;
        let vertices = incidence.vertices().ok_or_else(|| {
            failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "material link vertex inventory was not prepared",
                0,
                (),
            )
        })?;
        let unchanged_source = definition.material_transport_is_proved_zero(work, cancel)?;
        let mut result = Self {
            supports: Vec::new(),
            source_error_mm: 0.0,
            exits: Vec::new(),
            unchanged_source,
            all_vertex_supports_outside: true,
        };
        for vertex in vertices {
            if unchanged_source {
                let Some((prism, request)) = field else {
                    if !incidence.vertex_link_is_cycle(definition, vertex, work, cancel)? {
                        return Err(failure(
                            GeometryErrorCode::AnalysisIndeterminate,
                            "source vertex has no complete original incident cycle",
                            0,
                            vertex_diagnostic(definition, incidence, vertex),
                        ));
                    }
                    continue;
                };
                let proof = vertex_fan::original_vertex_fan(
                    definition,
                    source,
                    incidence,
                    vertex,
                    prism,
                    request,
                    allowance,
                    remaining_bytes,
                    work,
                    cancel,
                )?;
                result.all_vertex_supports_outside &=
                    matches!(proof, vertex_fan::OriginalVertexFanProof::OutsideClosedSlab);
                continue;
            }
            let Some(selection) = vertex_transport::SourceVertexPlan::prepare(
                source,
                incidence,
                vertex,
                remaining_bytes,
                work,
                cancel,
            )?
            else {
                return Err(failure(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "source vertex has no evaluatable immutable conical recipe",
                    0,
                    vertex_diagnostic(definition, incidence, vertex),
                ));
            };
            let plan = selection.plan();
            let scratch = remaining_bytes
                .checked_sub(selection.owned_bytes())
                .ok_or_else(|| {
                    failure(
                        GeometryErrorCode::ResourceLimit,
                        "local source vertex plan exceeds query scratch allowance",
                        0,
                        vertex_diagnostic(definition, incidence, vertex),
                    )
                })?;
            if let Some((prism, request)) = field {
                let Some(proof) =
                    plan.prove_radial(source, prism, request, scratch, work, cancel)?
                else {
                    return Err(failure(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "actual source conical star has no complete positive closed-slab radial link",
                        0,
                        (
                            vertex_diagnostic(definition, incidence, vertex),
                            request.height_interval_mm,
                        ),
                    ));
                };
                let proved_request = proof.request();
                if proved_request.frame != request.frame
                    || proved_request.height_interval_mm != request.height_interval_mm
                    || proved_request.sample_height_mm != request.sample_height_mm
                    || proved_request.native_shape_error_mm != request.native_shape_error_mm
                {
                    return Err(failure(
                        GeometryErrorCode::InvalidGeometry,
                        "source conical radial receipt belongs to another closed-slab query",
                        0,
                        vertex_diagnostic(definition, incidence, vertex),
                    ));
                }
                result.all_vertex_supports_outside &= proof.outside_closed_slab();
                result.source_error_mm = result.source_error_mm.max(proof.source_error_mm());
            } else {
                plan.visit_qualified_geometry(source, scratch, work, cancel, visitor)?;
                result.source_error_mm = result.source_error_mm.max(plan.source_error_mm());
            }
            plan.visit_position_supports(work, cancel, &mut |_realized, homotopy| {
                result.record_support(homotopy)
            })?;
            plan.visit_native_exits(work, cancel, &mut |edge, absolute_back, parameter| {
                result.record_exit(NativeEndpointExit {
                    edge,
                    absolute_back,
                    parameter,
                })
            })?;
        }
        Ok(result)
    }

    pub fn all_vertex_supports_outside_closed_slab(&self) -> bool {
        self.all_vertex_supports_outside
    }

    /// Before central cells are recorded, these are precisely the retained
    /// whole source-only vertex completion/homotopy supports.
    pub fn vertex_supports(&self) -> impl Iterator<Item = &[Interval; 3]> {
        self.supports.iter()
    }

    pub(super) fn allocated_bytes(&self) -> usize {
        self.supports
            .capacity()
            .saturating_mul(std::mem::size_of::<[Interval; 3]>())
            .saturating_add(
                self.exits
                    .capacity()
                    .saturating_mul(std::mem::size_of::<NativeEndpointExit>()),
            )
    }

    fn record_exit(&mut self, exit: NativeEndpointExit) -> Result<(), GeometryError> {
        if !exit.parameter.is_finite() {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "source endpoint recipe has a nonfinite native exit",
                0,
                exit.parameter,
            ));
        }
        if let Some(previous) = self.exits.iter().find(|previous| {
            previous.edge == exit.edge && previous.absolute_back == exit.absolute_back
        }) {
            if previous.parameter != exit.parameter {
                return Err(failure(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "opposed original uses disagree on immutable native endpoint exit",
                    0,
                    (previous.parameter, exit.parameter),
                ));
            }
            return Ok(());
        }
        if self.exits.len() == self.exits.capacity() {
            let next = self.exits.capacity().saturating_mul(2).max(32);
            let bytes = self
                .supports
                .capacity()
                .saturating_mul(std::mem::size_of::<[Interval; 3]>())
                .saturating_add(next.saturating_mul(std::mem::size_of::<NativeEndpointExit>()));
            if bytes > MAX_CORRECTION_BYTES {
                return Err(failure(
                    GeometryErrorCode::ResourceLimit,
                    "source endpoint exits exhaust correction proof storage",
                    0,
                    bytes,
                ));
            }
            self.exits
                .try_reserve_exact(next - self.exits.len())
                .map_err(|_| {
                    failure(
                        GeometryErrorCode::ResourceLimit,
                        "source endpoint exit allocation refused",
                        0,
                        next,
                    )
                })?;
            if self.allocated_bytes() > MAX_CORRECTION_BYTES {
                return Err(failure(
                    GeometryErrorCode::ResourceLimit,
                    "source endpoint exit allocator exceeds correction proof cap",
                    0,
                    self.allocated_bytes(),
                ));
            }
        }
        self.exits.push(exit);
        Ok(())
    }

    /// Both literal limits come from the same source-only corner recipes used
    /// by the active-star proof. A closed edge retains its two distinct germs.
    pub fn central_native_range(
        &self,
        edge: monstertruck_topology::EdgeId<Curve>,
        original: [f64; 2],
        source_owner: [usize; 3],
    ) -> Result<[f64; 2], GeometryError> {
        if self.unchanged_source {
            return Ok(original);
        }
        let find = |back| {
            self.exits
                .iter()
                .find(|exit| exit.edge == edge && exit.absolute_back == back)
                .map(|exit| exit.parameter)
        };
        let (Some(front), Some(back)) = (find(false), find(true)) else {
            return Err(failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "source central collar lacks complete native endpoint ownership",
                source_owner[0],
                (source_owner[1], source_owner[2], original),
            ));
        };
        if front <= original[0] || back >= original[1] || front >= back {
            return Err(failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "source endpoint exits do not partition the original native edge",
                source_owner[0],
                (source_owner[1], source_owner[2], original, [front, back]),
            ));
        }
        Ok([front, back])
    }

    pub fn record(
        &mut self,
        cell: &MaterialLinkCell,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        self.source_error_mm = self.source_error_mm.max(cell.source_error_mm);
        if cell.source_error_mm == 0.0 {
            return Ok(());
        }
        self.record_support(cell.position)
    }

    fn record_support(&mut self, position: [Interval; 3]) -> Result<(), GeometryError> {
        self.record_support_with_byte_limit(position, MAX_CORRECTION_BYTES)
    }

    pub(super) fn record_support_with_byte_limit(
        &mut self,
        position: [Interval; 3],
        remaining_bytes: usize,
    ) -> Result<(), GeometryError> {
        let limit = remaining_bytes.min(MAX_CORRECTION_BYTES);
        if self.allocated_bytes() > limit {
            return Err(failure(
                GeometryErrorCode::ResourceLimit,
                "material correction support exceeds live scratch allowance",
                0,
                (),
            ));
        }
        if self.supports.len() == self.supports.capacity() {
            let capacity = self
                .supports
                .capacity()
                .max(32)
                .checked_mul(2)
                .filter(|capacity| {
                    capacity
                        .checked_mul(std::mem::size_of::<[Interval; 3]>())
                        .and_then(|bytes| {
                            bytes.checked_add(
                                self.exits
                                    .capacity()
                                    .saturating_mul(std::mem::size_of::<NativeEndpointExit>()),
                            )
                        })
                        .is_some_and(|bytes| bytes <= limit)
                })
                .ok_or_else(|| {
                    failure(
                        GeometryErrorCode::ResourceLimit,
                        "material correction support capacity exhausted",
                        0,
                        self.supports.len(),
                    )
                })?;
            self.supports
                .try_reserve_exact(capacity - self.supports.len())
                .map_err(|_| {
                    failure(
                        GeometryErrorCode::ResourceLimit,
                        "material correction support allocation refused",
                        0,
                        capacity,
                    )
                })?;
            if self.allocated_bytes() > limit {
                return Err(failure(
                    GeometryErrorCode::ResourceLimit,
                    "material correction support allocator capacity exceeded",
                    0,
                    self.supports.capacity(),
                ));
            }
        }
        self.supports.push(position);
        Ok(())
    }

    pub fn degree_one(
        &self,
        source: OriginalTrimSource<'_, '_>,
        prism: &SamplePrism,
        request: &SlabRequest,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        // Cap exclusion is proved against the complete corrected source AABB.
        // The exact meridian relation is pz=beta*(rho-R), with R=0 for a
        // disk. The same whole-source bound below keeps tilted disk rays and
        // core meridians strictly between BOTH closed cuts.
        if prism.frame.z_axis != request.frame.z_axis {
            return Err(failure(
                GeometryErrorCode::AnalysisIndeterminate,
                "radial calibration needs an independently bound source cap fiber",
                0,
                (),
            ));
        }
        let center_height = prism
            .source_height
            .height(prism.center.map(Interval::point));
        if center_height.lo <= request.height_interval_mm[0]
            || center_height.hi >= request.height_interval_mm[1]
        {
            return Err(failure(
                GeometryErrorCode::AnalysisIndeterminate,
                "radial calibration fiber meets a closed cap",
                0,
                center_height,
            ));
        }
        let x = prism.frame.x_axis.map(Interval::point);
        let y = prism.frame.y_axis.map(Interval::point);
        let z = prism.frame.z_axis.map(Interval::point);
        let allowance = source_allowance(source.definition())?;
        let source_box = std::array::from_fn(|axis| Interval {
            lo: down(source.definition().bounds.min[axis] - allowance),
            hi: up(source.definition().bounds.max[axis] + allowance),
        });
        let p = project(source_box, prism.frame);
        let rho_max = length([p[0], p[1], Interval::point(0.0)])?.hi;
        let cap_margin = down(
            (center_height.lo - request.height_interval_mm[0])
                .min(request.height_interval_mm[1] - center_height.hi),
        );
        let meridian_extent = up(rho_max + prism.core_radius.unwrap_or(0.0));
        let height_rows = prism.dual.map(|column| prism.source_height.dot(column));
        let height_meridian_extent = mul(
            Interval::point(height_rows[2].abs_upper()),
            Interval::point(meridian_extent),
        )
        .hi;
        // Candidate ordering is not geometry authority. Every successful
        // candidate exhaustively excludes all correction supports and counts
        // all original roots. No sampled intersection, proximity deduplication
        // or fixture-specific direction can construct the receipt.
        for alpha in [0.0, 1.0, -1.0, 0.5, -0.5, 2.0, -2.0, 0.2718281828459045] {
            let angular =
                std::array::from_fn(|axis| y[axis].sub(mul(Interval::point(alpha), x[axis])));
            let normalization = add(Interval::point(1.0), square(Interval::point(alpha))).sqrt()?;
            // On this fiber py=alpha*px. Account actual cut-height drift in
            // the native frame rather than silently identifying its Z row.
            let transverse_height = mul(
                Interval::point(
                    add(height_rows[0], mul(Interval::point(alpha), height_rows[1])).abs_upper(),
                ),
                Interval::point(p[0].abs_upper()),
            )
            .hi;
            let remaining_margin = Interval::point(cap_margin)
                .sub(Interval::point(transverse_height))
                .lo;
            if remaining_margin <= 0.0 || height_meridian_extent <= 0.0 {
                continue;
            }
            let beta_bound = Interval::point(remaining_margin)
                .div(Interval::point(height_meridian_extent))?
                .lo
                * 0.25;
            for beta in [
                0.0,
                beta_bound,
                -beta_bound,
                beta_bound * 0.5,
                -beta_bound * 0.5,
            ] {
                work.charge(1, cancel)?;
                if add(
                    Interval::point(transverse_height),
                    mul(
                        Interval::point(beta.abs()),
                        Interval::point(height_meridian_extent),
                    ),
                )
                .hi >= cap_margin
                {
                    continue;
                }
                let meridian = std::array::from_fn(|axis| {
                    z[axis].sub(mul(mul(Interval::point(beta), normalization), x[axis]))
                });
                let rows = [angular, meridian];
                let offsets = [
                    Interval::point(0.0),
                    Interval::point(prism.core_radius.unwrap_or(0.0)).mul(Interval::point(beta)),
                ];
                let forward = cross(rows[0], rows[1]);
                if length(forward)?.lo <= 0.0 {
                    continue;
                }
                let fiber = fiber::OriginalFiber {
                    center: prism.center,
                    rows,
                    offsets,
                    forward,
                    forward_offset: Interval::point(0.0),
                    min_forward: 0.0,
                    core: prism.core_radius.map(|radius| fiber::FiberCore {
                        frame: prism.frame,
                        radius,
                    }),
                };
                if !self.misses_all_supports(&fiber, work, cancel)? {
                    continue;
                }
                if fiber::original_fiber_degree_one(
                    source,
                    &fiber,
                    request,
                    prism.source_height,
                    remaining_bytes,
                    work,
                    cancel,
                )?
                .is_some()
                {
                    // A proper local homeomorphism of the complete closed
                    // boundary to S²/T² has constant total fiber cardinality.
                    // One exhaustive regular fiber therefore proves ONE global
                    // sheet, including disconnected-component and torus-cycle
                    // multiplicity. This does not assert material convexity.
                    return Ok(());
                }
            }
        }
        Err(failure(
            GeometryErrorCode::AnalysisIndeterminate,
            "closed material radial map has no complete degree-one regular fiber",
            0,
            request.height_interval_mm,
        ))
    }

    pub(super) fn misses_all_supports(
        &self,
        fiber: &fiber::OriginalFiber,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        for position in &self.supports {
            work.charge(1, cancel)?;
            let relative =
                std::array::from_fn(|axis| position[axis].sub(Interval::point(fiber.center[axis])));
            if (0..2)
                .any(|row| !add(dot(fiber.rows[row], relative), fiber.offsets[row]).contains_zero())
            {
                continue;
            }
            if let Some(core) = &fiber.core {
                let p = project(*position, core.frame);
                if p[0].hi <= 0.0 || length([p[0], p[1], Interval::point(0.0)])?.hi <= core.radius {
                    continue;
                }
            } else if fiber::fiber_forward(fiber, *position).hi < fiber.min_forward {
                continue;
            }
            return Ok(false);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod closed_support_tests {
    use super::*;

    #[test]
    fn support_growth_is_refused_before_a_smaller_caller_budget_allocates() {
        let mut links = ClosedRadialLinks {
            supports: Vec::new(),
            exits: Vec::new(),
            source_error_mm: 0.0,
            unchanged_source: false,
            all_vertex_supports_outside: true,
        };
        let support = [Interval::point(1.0); 3];
        let requested_capacity_bytes = 64 * std::mem::size_of::<[Interval; 3]>();
        let error = links
            .record_support_with_byte_limit(support, requested_capacity_bytes - 1)
            .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::ResourceLimit);
        assert_eq!(links.allocated_bytes(), 0);
        assert!(links.supports.is_empty());
        links
            .record_support_with_byte_limit(support, requested_capacity_bytes)
            .unwrap();
        assert_eq!(links.supports.len(), 1);
        let retained = links.allocated_bytes();
        let error = links
            .record_support_with_byte_limit(support, retained - 1)
            .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::ResourceLimit);
        assert_eq!(links.allocated_bytes(), retained);
        assert_eq!(links.supports.len(), 1);
    }
}
