// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::analysis::material_source::MaterialSourceView;
use crate::analysis::source_incidence::{boundary_loop, native_face};

pub(in crate::analysis::critical) const MAX_CORRECTION_BYTES: usize = MAX_EDGE_BYTES;

/// Error-only original use ordinals. Native allocation IDs remain private proof
/// keys, and healed native coordinates are not the original material apex.
fn vertex_diagnostic(
    definition: &Definition,
    incidence: &SourceEdgeIncidence,
    vertex: solidcore::VertexId,
) -> Option<[usize; 3]> {
    if !incidence.owns_definition(definition) {
        return None;
    }
    let id = incidence.vertex_edges(vertex).next()?;
    let owner = incidence.get(id)?.first;
    let (_, edge) = crate::analysis::source_incidence::native_coedge(
        definition,
        owner.face,
        owner.wire,
        owner.index,
    )
    .ok()?;
    if !edge.vertices.contains(&vertex) {
        return None;
    }
    Some([owner.face, owner.wire, owner.index])
}

/// A negative-witness prerequisite only: the caller supplies the original
/// carrier evaluation of this SAME UV box. Strict original trim ownership and
/// exclusion of every removed R/K_F support prove that evaluation is still F
/// in the fixed realization. This grants no positive material/topology proof.
/// `remaining_bytes` excludes the borrowed prepared domain and caller incidence/input storage.
#[allow(clippy::too_many_arguments)]
pub(in crate::analysis::critical) fn source_f_point_is_retained(
    source: MaterialSourceView<'_, '_>,
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
        + crate::kernel::source_operand::SOURCE_OPERAND_SCRATCH_BYTES
        + std::mem::size_of::<Option<vertex_transport::CompletePositionCoverFailure>>();
    let before = work.consumed();
    let mut phase = "original source/domain binding";
    let mut active_coedge = None;
    let mut active_vertex = None;
    let outcome =
        (|| {
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
            let (face_id, face) = native_face(definition, face_index)?;
            if !incidence.owns_definition(definition)
                || !std::ptr::eq(domain.definition(), definition)
                || domain.face() != face_id
                || !std::ptr::eq(
                    domain,
                    source.face_domain(face_index, available, work, cancel)?,
                )
                || face.loops.is_empty()
            {
                return Ok(false);
            }
            let orientation = domain.orientation();
            phase = "strict whole original trim Inside";
            if domain.classify_with_byte_limit(uv, available, work, cancel)?
                != TrimClassification::Inside
            {
                return Ok(false);
            }
            phase = "all complete original reference-R overcovers";
            let point_uv = uv.map(|v| Interval { lo: v[0], hi: v[1] });
            let mut flat = 0;
            for wire_index in 0..face.loops.len() {
                let wire = boundary_loop(definition, face_index, wire_index)?;
                if wire.edges.is_empty() {
                    return Ok(false);
                }
                for (index, edge) in wire.edges.iter().enumerate() {
                    work.charge(1, cancel)?;
                    active_coedge = Some((wire_index, index, flat));
                    let Some(coedge) = domain.coedges().get(flat) else {
                        return Ok(false);
                    };
                    if coedge.edge != edge.edge || coedge.forward != edge.forward {
                        return Ok(false);
                    }
                    let use_ = CoedgeUse {
                        face: face_index,
                        coedge: flat,
                        wire: wire_index,
                        index,
                        orientation: edge.forward == orientation,
                        edgeuse: coedge.edgeuse,
                    };
                    let Some(uses) = incidence.get(edge.edge) else {
                        return Ok(false);
                    };
                    if (uses.first != use_ && uses.second != Some(use_))
                        || uses.vertices != coedge.vertices
                    {
                        return Ok(false);
                    }
                    let proof = domain.original_coedge_correspondence(flat)?;
                    let Some(binding) = proof.binding() else {
                        return Ok(false);
                    };
                    if binding.face != face_id
                    || binding.edge != edge.edge
                    || binding.owner != [face_index, wire_index, index]
                    || binding.edgeuse != coedge.edgeuse
                    || binding.endpoints != coedge.vertices
                    || binding.native_interval != coedge.native_range
                    || proof.provenance()
                        == crate::kernel::trim::OriginalCorrespondenceProvenance::CompletedPoleGraph
                {
                    return Ok(false);
                }
                    let mapped = proof.uv_range();
                    if proof
                        .affine_spatial_parameter_preimage([
                            mapped[0].min(mapped[1]),
                            mapped[0].max(mapped[1]),
                        ])?
                        .is_none()
                    {
                        return Ok(false);
                    }
                    let range = proof.spatial_range();
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
            if flat != domain.coedges().len() {
                return Ok(false);
            }
            // Only a source-coefficient identity may eliminate all K_F recipes.
            if source.material_transport_is_proved_zero(work, cancel)? {
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
                        Ok((0..3)
                            .any(|i| position[i].hi < cover[i].lo || cover[i].hi < position[i].lo))
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
struct SpatialEndpointExit {
    edge: solidcore::EdgeId,
    spatial_high: bool,
    parameter: f64,
    // Present only after exact source-star qualification AND complete closed
    // actual-coordinate G rank/metric coverage for this original STEP apex.
    closed_atlas_vertex: Option<solidcore::VertexId>,
}

/// Query-local proof storage, never an alternative source representation.
/// Every nonzero source transport support is retained until a COMPLETE regular
/// fiber census has calibrated the closed boundary's total covering degree.
pub(in crate::analysis::critical) struct ClosedRadialLinks<'d> {
    definition: &'d Definition,
    supports: Vec<[Interval; 3]>,
    pub source_error_mm: f64,
    exits: Vec<SpatialEndpointExit>,
    unchanged_source: bool,
    all_vertex_supports_outside: bool,
}
impl<'d> ClosedRadialLinks<'d> {
    pub fn new(
        source: MaterialSourceView<'_, 'd>,
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

    /// The SAME complete source vertex inventory and literal spatial exits as
    /// radial coverage, qualified independently of a physical witness field.
    pub(super) fn geometry(
        source: MaterialSourceView<'_, 'd>,
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

    /// Complete endpoint inventory for the CLOSED actual-coordinate atlas.
    /// A complete original-F/added-G outside cover needs no chart authority.
    /// Otherwise exact removed-domain qualification precedes both the closed
    /// G visitor and retention of the full source/reference/G homotopy support.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn for_closed_atlas(
        source: MaterialSourceView<'_, 'd>,
        incidence: &SourceEdgeIncidence,
        coordinates: &crate::analysis::placed::ActualMaterialCoordinates<'_>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        visitor: &mut dyn FnMut(
            usize,
            [Interval; 3],
            [Interval; 3],
            bool,
            &mut GeometryWorkBudget,
            &AtomicBool,
        ) -> Result<BoundaryVisit, GeometryError>,
    ) -> Result<Option<Self>, GeometryError> {
        let mut active_before = work.consumed();
        let mut phase = "immutable source vertex plan validation";
        let mut active_vertex = None;
        let outcome = (|| {
            let definition = source.definition();
            if !incidence.owns_definition(definition) {
                return Err(GeometryError::new(
                    GeometryErrorCode::InvalidGeometry,
                    "closed radial links borrow incidence from another native definition",
                ));
            }
            if let Some(plans) = source.vertex_plans() {
                plans.validate_source(definition, work, cancel)?;
            }
            let unchanged_source = source.material_transport_is_proved_zero(work, cancel)?;
            let allowance = source_allowance(definition)?;
            let interval = coordinates.world_interval();
            let expand = |position: [Interval; 3]| {
                position.map(|value| {
                    add(
                        value,
                        Interval {
                            lo: -allowance,
                            hi: allowance,
                        },
                    )
                })
            };
            let outside = |position: [Interval; 3]| -> Result<bool, GeometryError> {
                let z = coordinates.position(position)?[2];
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
                definition,
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
                    .checked_mul(std::mem::size_of::<SpatialEndpointExit>())
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
                let Some(owner) = vertex_diagnostic(definition, incidence, vertex) else {
                    return Err(failure(
                        GeometryErrorCode::KernelFailure,
                        "original vertex inventory lost its original coedge owner",
                        0,
                        (),
                    ));
                };
                let point = source
                    .trim_source()
                    .source_vertex_point(vertex, work, cancel)?;
                active_vertex = Some((owner, point));
                if unchanged_source {
                    // A native cycle alone is not a complete projected-star
                    // theorem. Unchanged active vertices remain unavailable.
                    if !outside(expand(point.map(Interval::point)))? {
                        return Ok(None);
                    }
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
                let excluded = plan.complete_incident_positions_miss(
                    source,
                    scratch,
                    work,
                    cancel,
                    &mut failed_cover,
                    &mut |position| outside(expand(position)),
                )?;
                let support_limit = remaining_bytes
                    .checked_sub(selection.owned_bytes())
                    .ok_or_else(|| {
                        failure(
                            GeometryErrorCode::ResourceLimit,
                            "closed cylinder corner/support coexistence exceeds allowance",
                            0,
                            (),
                        )
                    })?;
                let closed_atlas_vertex = if excluded {
                    None
                } else {
                    // This method completes immutable source graph/trim/
                    // comparison-disk/cutover/separation qualification FIRST.
                    // Only its query-local subdivision proves the actual
                    // closed G field and caller's independent boundary metric.
                    phase = "exact removed-domain and closed actual G atlas";
                    let qualification_before = work.consumed();
                    let complete = plan
                        .visit_closed_atlas_geometry(
                            source,
                            coordinates,
                            scratch,
                            work,
                            cancel,
                            visitor,
                        )
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
                    if !complete {
                        return Ok(None);
                    }
                    phase = "qualified complete source/reference/G support retention";
                    plan.visit_position_supports(
                        work,
                        cancel,
                        &mut |_realized, source_and_realized| {
                            let support = expand(source_and_realized);
                            result.all_vertex_supports_outside &= outside(support)?;
                            result.record_support_with_byte_limit(support, support_limit)
                        },
                    )?;
                    Some(vertex)
                };
                phase = "literal original vertex/native-edge/germ central exits";
                plan.visit_spatial_exits(work, cancel, &mut |edge, spatial_high, parameter, work| {
                    let Some(uses) = incidence.get(edge) else {
                        return Err(failure(
                            GeometryErrorCode::ParameterCorrespondenceUnavailable,
                            "source vertex exit lost its original native edge",
                            owner[0],
                            (owner[1], owner[2]),
                        ));
                    };
                    let use_ = uses.first;
                    let domain = source.face_domain(use_.face, support_limit, work, cancel)?;
                    let proof = domain.original_coedge_correspondence(use_.coedge)?;
                    let binding = proof.binding().ok_or_else(|| GeometryError::new(
                        GeometryErrorCode::ParameterCorrespondenceUnavailable,
                        "source spatial exit lacks its selected original/native binding",
                    ))?;
                    let (_, native_edge) = crate::analysis::source_incidence::native_coedge(
                        definition, use_.face, use_.wire, use_.index,
                    )?;
                    if binding.edge != edge
                        || binding.native_interval != [native_edge.interval.min(), native_edge.interval.max()]
                    {
                        return Err(GeometryError::new(
                            GeometryErrorCode::ParameterCorrespondenceUnavailable,
                            "source spatial exit belongs to another actual native interval",
                        ));
                    }
                    if uses.vertices[binding.native_endpoint_index(spatial_high)] != vertex {
                        return Err(failure(
                            GeometryErrorCode::ParameterCorrespondenceUnavailable,
                            "source vertex exit belongs to another original endpoint germ",
                            owner[0],
                            (owner[1], owner[2], spatial_high, parameter),
                        ));
                    }
                    if result.exits.len() == result.exits.capacity()
                        && !result.exits.iter().any(|previous| {
                            previous.edge == edge && previous.spatial_high == spatial_high
                        })
                    {
                        return Err(failure(
                            GeometryErrorCode::KernelFailure,
                            "source vertex exit is absent from complete original edge inventory",
                            0,
                            (),
                        ));
                    }
                    result.record_exit_with_byte_limit(SpatialEndpointExit {
                        edge,
                        spatial_high,
                        parameter,
                        closed_atlas_vertex,
                    }, support_limit)
                })?;
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
        source: MaterialSourceView<'_, 'd>,
        incidence: &SourceEdgeIncidence,
        field: Option<(&SamplePrism, &SlabRequest)>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        visitor: &mut impl FnMut(usize, [Interval; 3], [Interval; 3]) -> Result<(), GeometryError>,
    ) -> Result<Self, GeometryError> {
        let definition = source.definition();
        if !incidence.owns_definition(definition) {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "material links borrow incidence from another native definition",
            ));
        }
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
        let unchanged_source = source.material_transport_is_proved_zero(work, cancel)?;
        let mut result = Self {
            definition,
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
            let available = remaining_bytes
                .checked_sub(result.allocated_bytes())
                .ok_or_else(|| {
                    failure(
                        GeometryErrorCode::ResourceLimit,
                        "source vertex inventory exceeds remaining scratch allowance",
                        0,
                        (),
                    )
                })?;
            let Some(selection) = vertex_transport::SourceVertexPlan::prepare(
                source, incidence, vertex, available, work, cancel,
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
            let scratch = available
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
            let support_limit = remaining_bytes
                .checked_sub(selection.owned_bytes())
                .ok_or_else(|| {
                    failure(
                        GeometryErrorCode::ResourceLimit,
                        "source vertex plan/support coexistence exceeds scratch allowance",
                        0,
                        (),
                    )
                })?;
            plan.visit_position_supports(work, cancel, &mut |_realized, homotopy| {
                result.record_support_with_byte_limit(homotopy, support_limit)
            })?;
            plan.visit_spatial_exits(work, cancel, &mut |edge, spatial_high, parameter, _work| {
                result.record_exit_with_byte_limit(
                    SpatialEndpointExit {
                        edge,
                        spatial_high,
                        parameter,
                        closed_atlas_vertex: None,
                    },
                    support_limit,
                )
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
                    .saturating_mul(std::mem::size_of::<SpatialEndpointExit>()),
            )
    }

    fn record_exit_with_byte_limit(
        &mut self,
        exit: SpatialEndpointExit,
        remaining_bytes: usize,
    ) -> Result<(), GeometryError> {
        let limit = remaining_bytes.min(MAX_CORRECTION_BYTES);
        if self.allocated_bytes() > limit {
            return Err(failure(
                GeometryErrorCode::ResourceLimit,
                "source endpoint exits exceed live scratch allowance",
                0,
                (),
            ));
        }
        if !exit.parameter.is_finite() {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "source endpoint recipe has a nonfinite native exit",
                0,
                exit.parameter,
            ));
        }
        if let Some(previous) = self.exits.iter().find(|previous| {
            previous.edge == exit.edge && previous.spatial_high == exit.spatial_high
        }) {
            if previous.parameter != exit.parameter
                || previous.closed_atlas_vertex != exit.closed_atlas_vertex
            {
                return Err(failure(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "opposed original uses disagree on immutable source-spatial endpoint exit",
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
                .saturating_add(next.saturating_mul(std::mem::size_of::<SpatialEndpointExit>()));
            if bytes > limit {
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
            if self.allocated_bytes() > limit {
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

    /// This exact vertex/native-edge/absolute-germ/exit tuple was emitted by
    /// the same immutable source plan AFTER source ownership and closed G
    /// qualification. It grants only the terminal gluing premise: the central
    /// ribbon must still complete its closed reference/G and seam proofs.
    pub(super) fn qualified_closed_cutover(
        &self,
        vertex: solidcore::VertexId,
        edge: solidcore::EdgeId,
        spatial_high: bool,
        parameter: f64,
    ) -> bool {
        self.exits.iter().any(|exit| {
            exit.edge == edge
                && exit.spatial_high == spatial_high
                && exit.parameter == parameter
                && exit.closed_atlas_vertex == Some(vertex)
        })
    }

    /// Active original closure admission borrows completed source/reference/G
    /// authority only through both exact source/native endpoint cutovers. It also
    /// requires the independent WHOLE original joint image proof; an exit by
    /// itself never discharges this obligation.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn qualified_original_joint(
        &self,
        source: MaterialSourceView<'_, '_>,
        incidence: &SourceEdgeIncidence,
        domain: &PreparedFaceTrimDomain<'_>,
        joint_index: usize,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        work.charge(1, cancel)?;
        let remaining_bytes = remaining_bytes
            .checked_sub(
                super::atlas::ORIGINAL_JOINT_POSITION_SCRATCH_BYTES
                    + 2 * std::mem::size_of::<[Interval; 3]>(),
            )
            .ok_or_else(|| {
                failure(
                    GeometryErrorCode::ResourceLimit,
                    "original joint physical-image gate exceeds scratch allowance",
                    0,
                    joint_index,
                )
            })?;
        let Some(joint) = domain.original_joints()?.get(joint_index) else {
            return Ok(false);
        };
        if !incidence.owns_definition(source.definition())
            || !std::ptr::eq(domain.definition(), source.definition())
            || !std::ptr::eq(self.definition, source.definition())
            || self.unchanged_source
            || !matches!(
                joint.kind,
                crate::kernel::trim::TrimJointKind::EndpointUncertainty
                    | crate::kernel::trim::TrimJointKind::SphericalPole { .. }
            )
            || !self
                .exits
                .iter()
                .any(|exit| exit.closed_atlas_vertex == Some(joint.vertex_id))
        {
            return Ok(false);
        }
        // The closed G apex is the captured ORIGINAL STEP VertexPoint in every
        // selected cone cell. Its retained homotopy support must cover the entire
        // allowance cube. The independent joint proof below qualifies its UV
        // and physical projection; the actual connector/world image must also
        // lie in this cube, so no admitted closure can escape the original
        // fiber census's retained-support exclusion. This is not Jordan
        // ownership for arbitrary completions.
        let allowance = source_allowance(self.definition)?;
        let vertex_cube = joint.vertex_point.map(|value| {
            add(
                Interval::point(value),
                Interval {
                    lo: -allowance,
                    hi: allowance,
                },
            )
        });
        let position =
            super::atlas::original_joint_position_enclosure(domain, joint, work, cancel)?;
        if !(0..3).all(|axis| {
            vertex_cube[axis].lo <= position[axis].lo && position[axis].hi <= vertex_cube[axis].hi
        }) {
            return Ok(false);
        }
        if !self.supports.iter().any(|support| {
            (0..3).all(|i| support[i].lo <= vertex_cube[i].lo && vertex_cube[i].hi <= support[i].hi)
        }) {
            return Ok(false);
        }
        let Some(selection) = vertex_transport::SourceVertexPlan::prepare(
            source,
            incidence,
            joint.vertex_id,
            remaining_bytes,
            work,
            cancel,
        )?
        else {
            return Ok(false);
        };
        let scratch = remaining_bytes
            .checked_sub(selection.owned_bytes())
            .ok_or_else(|| {
                failure(
                    GeometryErrorCode::ResourceLimit,
                    "original joint/source star coexistence exceeds scratch allowance",
                    0,
                    joint_index,
                )
            })?;
        selection.plan().qualified_original_joint_image(
            source,
            domain,
            joint_index,
            scratch,
            work,
            cancel,
            &|edge, spatial_high, parameter| {
                self.qualified_closed_cutover(joint.vertex_id, edge, spatial_high, parameter)
            },
        )
    }

    /// Both literal limits come from the same source-only corner recipes used
    /// by the active-star proof in original SOURCE spatial gauge. A closed edge
    /// retains its two distinct source endpoint germs even when vertex IDs agree.
    pub fn central_spatial_range(
        &self,
        edge: solidcore::EdgeId,
        original: [f64; 2],
        source_owner: [usize; 3],
    ) -> Result<[f64; 2], GeometryError> {
        if self.unchanged_source {
            return Ok(original);
        }
        let find = |spatial_high| {
            self.exits
                .iter()
                .find(|exit| exit.edge == edge && exit.spatial_high == spatial_high)
                .map(|exit| exit.parameter)
        };
        let (Some(front), Some(back)) = (find(false), find(true)) else {
            return Err(failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "source central collar lacks complete spatial endpoint ownership",
                source_owner[0],
                (source_owner[1], source_owner[2], original),
            ));
        };
        if front <= original[0] || back >= original[1] || front >= back {
            return Err(failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "source endpoint exits do not partition the original spatial curve",
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
                                    .saturating_mul(std::mem::size_of::<SpatialEndpointExit>()),
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

    /// Complete original F (including declared joint/lift images), central G
    /// displaced by its sealed whole-source forward bound, and all retained
    /// source-corner/G supports. A lowered native import AABB supplies none of
    /// this authority.
    fn complete_source_position_box(
        &self,
        source: MaterialSourceView<'_, '_>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 3], GeometryError> {
        const STACK_BYTES: usize = crate::kernel::source_operand::SOURCE_OPERAND_SCRATCH_BYTES
            + crate::parametric::SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES
            + 3 * std::mem::size_of::<[Interval; 3]>()
            + std::mem::size_of::<crate::kernel::trim::PreparedTrimSurface<'_, '_>>()
            + super::atlas::ORIGINAL_JOINT_POSITION_SCRATCH_BYTES;
        let before = work.consumed();
        let mut active_face = None;
        let outcome = (|| {
            work.charge(1, cancel)?;
            if !std::ptr::eq(self.definition, source.definition()) {
                return Err(GeometryError::new(
                    GeometryErrorCode::InvalidGeometry,
                    "whole-source cap bound borrows another source definition",
                ));
            }
            let available = remaining_bytes.checked_sub(STACK_BYTES).ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "whole-original/source-G cap bound exceeds remaining scratch",
                )
            })?;
            let mut cover: Option<[Interval; 3]> = None;
            let mut include = |position: [Interval; 3]| -> Result<(), GeometryError> {
                for value in position {
                    Interval::new(value.lo, value.hi)?;
                }
                cover = Some(match cover {
                    Some(previous) => {
                        std::array::from_fn(|axis| previous[axis].hull(position[axis]))
                    }
                    None => position,
                });
                Ok(())
            };
            let mut forward_padding_mm = 0.0_f64;
            for face in 0..source.definition().face_ids().len() {
                active_face = Some(face);
                let domain = source.face_domain(face, available, work, cancel)?;
                let surface = domain.surface_operand();
                include(crate::parametric::surface_position_enclosure(
                    &surface,
                    domain.uv_domain(),
                    work,
                    cancel,
                )?)?;
                for joint in domain.original_joints()? {
                    include(super::atlas::original_joint_position_enclosure(
                        domain, joint, work, cancel,
                    )?)?;
                }
                for coedge in 0..domain.coedges().len() {
                    work.charge(1, cancel)?;
                    let error = domain
                        .original_coedge_correspondence(coedge)?
                        .position_error_mm();
                    if !error.is_finite() || error < 0.0 {
                        return Err(GeometryError::new(
                            GeometryErrorCode::ConditioningFailure,
                            "whole-source cap bound has an invalid original forward receipt",
                        ));
                    }
                    forward_padding_mm = forward_padding_mm.max(error);
                }
            }
            let original = cover.ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::InvalidGeometry,
                    "whole-source cap bound has no original face inventory",
                )
            })?;
            // R is owned in the original carrier domains above, and every
            // central G point is F(U)+lambda*(1-w)*delta with |delta| bounded
            // by the selected whole-source forward receipt. This is an
            // overcover, not equality or source-error acceptance.
            let padding = Interval::new(-forward_padding_mm, forward_padding_mm)?;
            let mut result = original.map(|value| value.add(padding));
            for support in &self.supports {
                work.charge(1, cancel)?;
                for value in *support {
                    Interval::new(value.lo, value.hi)?;
                }
                result = std::array::from_fn(|axis| result[axis].hull(support[axis]));
            }
            Ok(result)
        })();
        outcome.map_err(|error| {
            work.annotate_error(
                error,
                before,
                format_args!("degree-one whole-original/source-G cap bound face={active_face:?}",),
            )
        })
    }

    pub fn degree_one(
        &self,
        source: MaterialSourceView<'_, '_>,
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
        let source_box =
            self.complete_source_position_box(source, remaining_bytes, work, cancel)?;
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
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!("../../../../../../fixtures/geometry/box-mm.step"),
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut links = ClosedRadialLinks {
            definition: &definition,
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
