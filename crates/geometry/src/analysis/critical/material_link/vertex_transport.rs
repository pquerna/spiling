// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Source-only finite-boundary conical vertex realization. The boundary is
//! evaluated on the existing paired collar realization and unchanged carrier;
//! neither a query radial direction nor a fitted tangent plane chooses it.
//!
//! Carrier and trim evaluations borrow the domain-selected coefficient
//! operands. Original spatial/UV provider gauges remain separate from native
//! edge intervals, linked only by the retained source correspondence. Vertex
//! and edge IDs belong to the same borrowed definition as the incidence
//! inventory; arena-ID equality alone is not ownership. Native loop traversal,
//! material shell sense, and source-provider direction remain distinct.

use super::*;
use crate::analysis::material_source::MaterialSourceView;
use crate::analysis::source_incidence::CoedgeUse;
use crate::kernel::trim::{NativeCoedgeIdentity, TrimJointKind};
use solidcore::topology::{EdgeId, VertexId};
use std::sync::atomic::Ordering;

mod prepared;
mod uv_ownership;
pub(crate) use prepared::PreparedSourceVertexPlans;
pub(super) use prepared::SourceVertexPlan;
use uv_ownership::{
    OriginalCornerComparison, SourcePositionOvercover, UvComparisonPreparation, UvPrepareStage,
};

type Box3 = [Interval; 3];
type Box2 = [Interval; 2];
const CONICAL_STACK_BYTES: usize = 4
    * std::mem::size_of::<[([f64; 2], u8); MAX_DEPTH as usize + 1]>()
    + std::mem::size_of::<[([[f64; 2]; 2], u8); MAX_DEPTH as usize + 1]>()
    + UV_EMBEDDING_STACK_BYTES
    + std::mem::size_of::<UvRibbonEmbedding<'static, 'static>>()
    + uv_ownership::OWNERSHIP_STACK_BYTES;

#[derive(Clone, Copy)]
struct Corner {
    outgoing: CoedgeUse,
    incoming: CoedgeUse,
}

#[derive(Clone, Copy)]
struct ConeCell {
    source: Box3,
    realized: Box3,
    displacement: Box3,
    normal: Box3,
    error_mm: f64,
    recipe: usize,
    piece: Piece,
    parameter: [f64; 2],
}

/// Fixed-size failure context only; it grants no positional or corner proof.
#[derive(Debug, Clone, Copy)]
pub(super) struct CompletePositionCoverFailure {
    pub(super) phase: &'static str,
    pub(super) source_face: usize,
    pub(super) position: Box3,
}

/// A complete immutable source star, with the actual finite outer boundary
/// and a coefficient-bounded source correspondence on every cone sector.
/// All retained cells belong to one original definition and NativeV.
pub(super) struct SourceVertexTransportPlan<'d> {
    definition: &'d Definition,
    vertex: VertexId,
    point: [f64; 3],
    fraction: f64,
    rows: [Box3; 2],
    recipes: Vec<CornerRecipe>,
    cells: Vec<ConeCell>,
    source_error_mm: f64,
    source_geometry_qualified: AtomicBool,
}

/// Opaque successful geometry authority includes a source-complement cut for
/// EVERY immutable recipe. Cached acquisition reuses only that source theorem,
/// never a slab, projection, signed census, metric or degree conclusion.
struct QualifiedSourceGeometry<'a, 'd> {
    plan: &'a SourceVertexTransportPlan<'d>,
}

impl QualifiedSourceGeometry<'_, '_> {
    fn owns_source_cut(&self, recipe: &CornerRecipe) -> bool {
        self.plan
            .recipes
            .iter()
            .any(|owned| std::ptr::eq(owned, recipe))
    }
}

/// Original relative-cut authority only. Issued before borrowing a carrier.
/// Its connection check evaluates UV laws but never a native surface or a
/// realization's geometry qualification. It grants no root or posed authority.
pub(in crate::analysis) struct OriginalFaceCornerAttachments<'a, 'd> {
    definition: &'d Definition,
    domain: &'a PreparedFaceTrimDomain<'d>,
    plans: &'a PreparedSourceVertexPlans<'d>,
}

impl<'a, 'd> OriginalFaceCornerAttachments<'a, 'd> {
    pub(in crate::analysis) const CONNECTION_SCRATCH_BYTES: usize =
        uv_ownership::OWNERSHIP_STACK_BYTES;

    pub(in crate::analysis) fn prepare(
        source: MaterialSourceView<'a, 'd>,
        domain: &'a PreparedFaceTrimDomain<'d>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        work.charge(1, cancel)?;
        let Some(plans) = source.vertex_plans() else {
            return Ok(None);
        };
        plans.validate_source(source.definition(), work, cancel)?;
        if source.material_transport_is_proved_zero(work, cancel)? {
            return Ok(None);
        }
        let scratch = remaining_bytes
            .checked_sub(std::mem::size_of::<Self>())
            .ok_or_else(super::transport::resource)?;
        for (joint_index, joint) in domain.original_joints()?.iter().enumerate() {
            work.charge(1, cancel)?;
            if !matches!(joint.kind, TrimJointKind::EndpointUncertainty) {
                continue;
            }
            let plan = plans.plan(source.definition(), joint.vertex_id, work, cancel)?;
            let Some(recipe) = plan.original_joint_recipe(domain, joint_index)? else {
                continue;
            };
            let geometry = plan.qualify_geometry(source, scratch, work, cancel)?;
            if !geometry.owns_source_cut(recipe) {
                return Ok(None);
            }
        }
        Ok(Some(Self {
            definition: source.definition(),
            domain,
            plans,
        }))
    }

    pub(in crate::analysis) fn qualifies_original_connection(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        joint_index: usize,
        vertex: VertexId,
        connection: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        work.charge(1, cancel)?;
        if !std::ptr::eq(self.domain, domain) {
            return Ok(false);
        }
        let Some(joint) = domain.original_joints()?.get(joint_index) else {
            return Ok(false);
        };
        if joint.vertex_id != vertex || !matches!(joint.kind, TrimJointKind::EndpointUncertainty) {
            return Ok(false);
        }
        let plan = self.plans.plan(self.definition, vertex, work, cancel)?;
        if !plan.source_geometry_qualified.load(Ordering::Acquire) {
            return Ok(false);
        }
        let Some(recipe) = plan.original_joint_recipe(domain, joint_index)? else {
            return Ok(false);
        };
        // The successful all-recipe cache is immutable and source-only. This
        // original attachment query reuses it without rerunning qualification.
        if !(QualifiedSourceGeometry { plan }).owns_source_cut(recipe) {
            return Ok(false);
        }
        let Some(OriginalCornerComparison::NativeUv(comparison)) = &recipe.comparison else {
            return Ok(false);
        };
        let connection = [
            Interval::new(connection[0][0], connection[0][1])?,
            Interval::new(connection[1][0], connection[1][1])?,
        ];
        comparison.connection_stays_on_replaced_side(domain, recipe, connection, work, cancel)
    }
}

/// Query authority is separate from the geometry-only realization choice.
pub(super) struct SourceVertexRadialProof<'a, 'd> {
    plan: &'a SourceVertexTransportPlan<'d>,
    request: SlabRequest,
    outside_closed_slab: bool,
}

impl SourceVertexRadialProof<'_, '_> {
    pub(super) fn source_error_mm(&self) -> f64 {
        self.plan.source_error_mm
    }
    pub(super) fn outside_closed_slab(&self) -> bool {
        self.outside_closed_slab
    }
    pub(super) fn request(&self) -> &SlabRequest {
        &self.request
    }
}

fn refusal(message: impl AsRef<str>) -> GeometryError {
    GeometryError::new(GeometryErrorCode::AnalysisIndeterminate, message)
}
fn hull3(a: Box3, b: Box3) -> Box3 {
    std::array::from_fn(|i| a[i].hull(b[i]))
}
fn hull2(a: Box2, b: Box2) -> Box2 {
    std::array::from_fn(|i| a[i].hull(b[i]))
}
fn relative(a: Box3, b: [f64; 3]) -> Box3 {
    std::array::from_fn(|i| a[i].sub(Interval::point(b[i])))
}
fn mix<const N: usize>(a: [Interval; N], b: [Interval; N], t: Interval) -> [Interval; N] {
    std::array::from_fn(|i| a[i].add(t.mul(b[i].sub(a[i]))))
}

/// A normalized recipe coordinate is validated before its independently known
/// image [0,1] can tighten outward arithmetic. Unproved inputs are never clipped.
fn source_unit_parameter(value: Interval) -> Result<Interval, GeometryError> {
    if !value.lo.is_finite()
        || !value.hi.is_finite()
        || value.lo < 0.0
        || value.hi > 1.0
        || value.lo > value.hi
    {
        return Err(refusal(
            "source recipe coordinate is not proved inside the closed unit domain",
        ));
    }
    Ok(value)
}

fn source_unit_complement(value: Interval) -> Result<Interval, GeometryError> {
    let value = source_unit_parameter(value)?;
    intersection(
        Interval::point(1.0).sub(value),
        Interval { lo: 0.0, hi: 1.0 },
    )
    .ok_or_else(|| refusal("source unit reflection has no validated domain image"))
}

/// Error from one fixed cone chord on a restricted radial interval. Its
/// acceleration argument must cover the complete original ray, not just s.
fn source_chord_displacement(
    at_joint: Box3,
    at_boundary: Box3,
    acceleration: Box3,
    s: Interval,
) -> Result<Box3, GeometryError> {
    let complement = source_unit_complement(s)?;
    let weight = intersection(
        s.mul(complement).mul(Interval::point(0.5)),
        Interval { lo: 0.0, hi: 0.125 },
    )
    .ok_or_else(|| refusal("source chord remainder has no validated unit image"))?;
    let mut result = [Interval::point(0.0); 3];
    for i in 0..3 {
        let endpoint = intersection(
            complement.mul(at_joint[i]).add(s.mul(at_boundary[i])),
            at_joint[i].hull(at_boundary[i]),
        )
        .ok_or_else(|| refusal("source chord endpoint enclosures disagree"))?;
        let remainder = up(acceleration[i].abs_upper() * weight.hi);
        result[i] = endpoint.add(Interval {
            lo: -remainder,
            hi: remainder,
        });
    }
    Ok(result)
}
fn disjoint<const N: usize>(a: [Interval; N], b: [Interval; N]) -> bool {
    (0..N).any(|i| a[i].hi < b[i].lo || b[i].hi < a[i].lo)
}
fn contains<const N: usize>(a: [Interval; N], b: [Interval; N]) -> bool {
    (0..N).all(|i| a[i].lo <= b[i].lo && b[i].hi <= a[i].hi)
}
fn projected(a: Box3, rows: [Box3; 2]) -> Box2 {
    rows.map(|row| dot(row, a))
}

#[derive(Clone, Copy)]
struct End {
    use_: CoedgeUse,
    edge: EdgeId,
    spatial_high: bool,
    endpoint: f64,
    exit: f64,
    delta_t: Interval,
    joint_uv: Box2,
    offset: Box2,
    collar: bool,
    spatial: CurveEnclosure,
}

#[derive(Clone, Copy)]
struct CornerRecipe {
    outgoing: End,
    incoming: End,
    bulge: Box2,
    comparison: Option<OriginalCornerComparison>,
    comparison_refusal: Option<UvPrepareStage>,
}

#[derive(Clone, Copy)]
enum Piece {
    Outgoing,
    Middle,
    Incoming,
}

#[derive(Clone, Copy)]
struct BoundaryJet {
    position: Box3,
    derivative: Box3,
    displacement: Box3,
    uv: Box2,
}

struct ClosedConeJet {
    position: Box3,
    ds: Box3,
    dt: Box3,
    normal: Box3,
}

/// The very same cone selected by cover_piece: G(s,t)=V+s(B(t)-V).
/// Factor the nonnegative s out of its area normal BEFORE normalization. This
/// encloses all one-sided normalized limits at s=0 for the complete t interval;
/// the zero derivative of the collapsed parametrization is not its normal.
fn closed_cone_jet(
    point: [f64; 3],
    boundary: BoundaryJet,
    radial_range: [f64; 2],
) -> Result<ClosedConeJet, GeometryError> {
    let s = source_unit_parameter(Interval::new(radial_range[0], radial_range[1])?)?;
    let origin = point.map(Interval::point);
    let ds = relative(boundary.position, point);
    let mut position = mix(origin, boundary.position, s);
    let convex = hull3(origin, boundary.position);
    for i in 0..3 {
        position[i] = intersection(position[i], convex[i])
            .ok_or_else(|| refusal("selected closed cone interpolation enclosures disagree"))?;
    }
    Ok(ClosedConeJet {
        position,
        ds,
        dt: boundary.derivative.map(|value| s.mul(value)),
        // cover_piece proves this orientation agrees with the complete
        // original outward graph. Never flip it from a sampled/reference normal.
        normal: cross(ds, boundary.derivative),
    })
}

struct StarJet {
    uv: Box2,
    ds: Box2,
    dss: Box2,
}

impl<'d> SourceVertexTransportPlan<'d> {
    /// Select one source-only endpoint fraction for the COMPLETE original
    /// corner cycle. A failed gate never falls back to sampled tangent rays.
    pub(super) fn prepare(
        source: MaterialSourceView<'_, 'd>,
        incidence: &SourceEdgeIncidence,
        vertex: VertexId,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        let before = work.consumed();
        let mut stage = "corner inventory";
        let mut source_point: Option<[f64; 3]> = None;
        let mut face_coedges: Option<(usize, [usize; 2])> = None;
        let mut current_fraction = None;
        let mut current_candidate = None;
        let result = (|| {
            work.charge(1, cancel)?;
            if !incidence.owns_definition(source.definition()) {
                return Ok(None);
            }
            let remaining_bytes = remaining_bytes
                .checked_sub(
                    CONICAL_STACK_BYTES
                        .saturating_add(std::mem::size_of::<Self>())
                        .saturating_add(std::mem::size_of::<Vec<Corner>>())
                        .saturating_add(std::mem::size_of::<Vec<Box2>>()),
                )
                .ok_or_else(|| {
                    GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "source conical fixed scratch exceeds remaining allowance",
                    )
                })?;
            let allowance = source_allowance(source.definition())?;
            let Some((corners, point)) =
                corners(source, incidence, vertex, remaining_bytes, work, cancel)?
            else {
                return Ok(None);
            };
            source_point = Some(point);
            face_coedges = Some((
                corners[0].outgoing.face,
                [corners[0].outgoing.coedge, corners[0].incoming.coedge],
            ));
            stage = "source_frame";
            let corner_bytes = corners
                .capacity()
                .saturating_mul(std::mem::size_of::<Corner>());
            let scratch = remaining_bytes.checked_sub(corner_bytes).ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "source vertex corner inventory exceeds remaining scratch allowance",
                )
            })?;
            let Some(rows) = source_frame(source, &corners, scratch, work, cancel)? else {
                return Err(refusal(format!(
                    "source conical plane has no qualified physical incident-normal frame: definition={} vertex_mm={point:?} source_face_index={}",
                    source.definition().id().as_str(),
                    corners[0].outgoing.face,
                )));
            };
            stage = "endpoint support fraction";
            let mut fraction = source
                .face_domain(corners[0].outgoing.face, scratch, work, cancel)?
                .material_realization()
                .endpoint_support_fraction();
            let mut last_exit: Option<(usize, [usize; 2], [f64; 2], f64)> = None;
            for _ in 0..=MAX_DEPTH {
                current_fraction = Some(fraction);
                current_candidate = None;
                stage = "fraction attempt";
                work.charge(1, cancel)?;
                if !fraction.is_finite() || fraction <= 0.0 {
                    return Ok(None);
                }
                let mut cells = Vec::new();
                let mut rays = Vec::new();
                let mut recipes: Vec<CornerRecipe> = Vec::new();
                let recipe_bytes = corners
                    .len()
                    .checked_mul(std::mem::size_of::<CornerRecipe>())
                    .filter(|bytes| *bytes <= scratch)
                    .ok_or_else(|| {
                        GeometryError::new(
                            GeometryErrorCode::ResourceLimit,
                            "source conical recipes exceed scratch allowance",
                        )
                    })?;
                recipes.try_reserve_exact(corners.len()).map_err(|_| {
                    GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "source conical recipe allocation refused",
                    )
                })?;
                let recipe_bytes = recipe_bytes.max(
                    recipes
                        .capacity()
                        .saturating_mul(std::mem::size_of::<CornerRecipe>()),
                );
                let mut accepted = true;
                for corner in &corners {
                    face_coedges = Some((
                        corner.outgoing.face,
                        [corner.outgoing.coedge, corner.incoming.coedge],
                    ));
                    stage = "coedge source domain";
                    work.charge(1, cancel)?;
                    let available = scratch.checked_sub(recipe_bytes).ok_or_else(|| {
                        GeometryError::new(
                            GeometryErrorCode::ResourceLimit,
                            "source conical star exceeds scratch allowance",
                        )
                    })?;
                    let domain =
                        source.face_domain(corner.outgoing.face, available, work, cancel)?;
                    stage = "outgoing end evaluation";
                    let Some(outgoing) = prepare_end(
                        &domain,
                        corner.outgoing,
                        false,
                        fraction,
                        recipes.last().map(|recipe| recipe.incoming),
                        work,
                        cancel,
                    )?
                    else {
                        accepted = false;
                        break;
                    };
                    stage = "incoming end evaluation";
                    let Some(incoming) = prepare_end(
                        &domain,
                        corner.incoming,
                        true,
                        fraction,
                        recipes.first().map(|recipe| recipe.outgoing),
                        work,
                        cancel,
                    )?
                    else {
                        accepted = false;
                        break;
                    };
                    last_exit = Some((
                        corner.outgoing.face,
                        [corner.outgoing.coedge, corner.incoming.coedge],
                        [outgoing.exit, incoming.exit],
                        fraction,
                    ));
                    stage = "source generator radius refutation";
                    if source_radius_refuted(&domain, outgoing, point, allowance, work, cancel)?
                        || source_radius_refuted(&domain, incoming, point, allowance, work, cancel)?
                    {
                        // These full-width generators are shared by EVERY bulge.
                        // A failed point bound rejects the radius, never just one
                        // bulge, and the existing finer-fraction schedule continues.
                        accepted = false;
                        break;
                    }
                    recipes.push(CornerRecipe {
                        outgoing,
                        incoming,
                        bulge: [Interval::point(0.0); 2],
                        comparison: None,
                        comparison_refusal: None,
                    });
                }
                // Finish all radius refutations before starting ANY theta cover.
                // Retain the same prepared NativeE endpoint recipes for acceptance.
                if accepted {
                    for recipe_index in 0..recipes.len() {
                        let CornerRecipe {
                            outgoing, incoming, ..
                        } = recipes[recipe_index];
                        face_coedges = Some((
                            outgoing.use_.face,
                            [outgoing.use_.coedge, incoming.use_.coedge],
                        ));
                        stage = "finite B source domain";
                        work.charge(1, cancel)?;
                        let used = cells
                            .capacity()
                            .saturating_mul(std::mem::size_of::<ConeCell>())
                            .saturating_add(
                                rays.capacity().saturating_mul(std::mem::size_of::<Box2>()),
                            )
                            .saturating_add(recipe_bytes);
                        let available = scratch.checked_sub(used).ok_or_else(|| {
                            GeometryError::new(
                                GeometryErrorCode::ResourceLimit,
                                "source conical star exceeds scratch allowance",
                            )
                        })?;
                        let domain =
                            source.face_domain(outgoing.use_.face, available, work, cancel)?;
                        stage = "outgoing end tangent";
                        let tangent =
                            end_curve(&domain, outgoing, Interval::point(0.0), work, cancel)?
                                .derivative;
                        let mut corner_accepted = false;
                        for candidate in 0..7 {
                            current_candidate = Some(candidate);
                            stage = "finite B cover";
                            let bulge_scale = if candidate == 0 {
                                0.0
                            } else {
                                [1.0, 2.0, 4.0][(candidate - 1) / 2]
                                    * if candidate % 2 == 1 { 1.0 } else { -1.0 }
                            };
                            let bulge = if bulge_scale == 0.0 {
                                [Interval::point(0.0); 2]
                            } else {
                                rotate(tangent, bulge_scale)
                            };
                            let cell_start = cells.len();
                            let ray_start = rays.len();
                            let cell_limit =
                                scratch.checked_sub(recipe_bytes).ok_or_else(|| {
                                    GeometryError::new(
                                        GeometryErrorCode::ResourceLimit,
                                        "source conical recipes and cells exceed scratch allowance",
                                    )
                                })?;
                            let mut candidate_accepted = true;
                            for piece in [Piece::Outgoing, Piece::Middle, Piece::Incoming] {
                                if matches!(piece, Piece::Outgoing) && !outgoing.collar
                                    || matches!(piece, Piece::Incoming) && !incoming.collar
                                {
                                    continue;
                                }
                                if !cover_piece(
                                    &domain,
                                    outgoing,
                                    incoming,
                                    recipe_index,
                                    piece,
                                    bulge,
                                    point,
                                    rows,
                                    allowance,
                                    cell_limit,
                                    &mut cells,
                                    &mut rays,
                                    work,
                                    cancel,
                                )? {
                                    candidate_accepted = false;
                                    break;
                                }
                            }
                            if candidate_accepted {
                                recipes[recipe_index].bulge = bulge;
                                corner_accepted = true;
                                break;
                            }
                            cells.truncate(cell_start);
                            rays.truncate(ray_start);
                        }
                        if !corner_accepted {
                            accepted = false;
                            break;
                        }
                    }
                }
                if accepted {
                    stage = "opposed spatial exits";
                    validate_spatial_exits(&recipes, work, cancel)?;
                }
                stage = "projected vertex fan";
                if accepted
                    && !cells.is_empty()
                    && super::vertex_fan::projected_vertex_fan(&rays, work, cancel)?
                {
                    drop(rays);
                    let retained = recipes
                        .capacity()
                        .saturating_mul(std::mem::size_of::<CornerRecipe>())
                        .saturating_add(
                            cells
                                .capacity()
                                .saturating_mul(std::mem::size_of::<ConeCell>()),
                        );
                    let comparison_scratch = scratch.checked_sub(retained).ok_or_else(|| {
                        GeometryError::new(
                            GeometryErrorCode::ResourceLimit,
                            "fixed source comparison exceeds coexistent geometry capacity",
                        )
                    })?;
                    stage = "fixed realization source correspondence";
                    qualify_fixed_source_costs(
                        source,
                        vertex,
                        point,
                        allowance,
                        comparison_scratch,
                        &mut recipes,
                        &mut cells,
                        work,
                        cancel,
                    )?;
                    let source_error_mm =
                        cells.iter().map(|cell| cell.error_mm).fold(0.0, f64::max);
                    return Ok(Some(Self {
                        definition: source.definition(),
                        vertex,
                        point,
                        fraction,
                        rows,
                        recipes,
                        cells,
                        source_error_mm,
                        source_geometry_qualified: AtomicBool::new(false),
                    }));
                }
                let next = fraction * 0.5;
                if next == fraction || next <= 0.0 {
                    return Ok(None);
                }
                fraction = next;
            }
            Err(refusal(format!(
                "source conical finite outer trace cannot prove projected star and admitted-error bound: definition={} vertex_mm={point:?} last_face_coedges_exits_fraction={last_exit:?} admitted_source_error_mm={allowance}",
                source.definition().id().as_str(),
            )))
        })();
        result.map_err(|error| work.annotate_error(error, before, format_args!(
            "SourceVertexTransportPlan stage={stage} vertex_mm={source_point:?} source_face_coedges={face_coedges:?} fraction={current_fraction:?} candidate={current_candidate:?}"
        )))
    }

    pub(super) fn source_error_mm(&self) -> f64 {
        self.source_error_mm
    }

    /// C0 cover of the literal selected CLOSED cones. No correspondence to
    /// an auxiliary carrier star or removed original-F disk is asserted.
    /// Material admission remains the separate source-cut/embedding theorem.
    pub(super) fn visit_projected_position_supports(
        &self,
        source: MaterialSourceView<'_, 'd>,
        cover: &mut super::transport::SourceProjectionCover<'_>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        type Pending = [([[f64; 2]; 2], u8, Option<BoundaryJet>); MAX_DEPTH as usize + 1];
        let scratch = remaining_bytes
            .checked_sub(std::mem::size_of::<Pending>())
            .and_then(|bytes| {
                bytes.checked_sub(
                    2 * std::mem::size_of::<BoundaryJet>()
                        + std::mem::size_of::<ClosedConeJet>()
                        + std::mem::size_of::<OriginalCollar<'_, '_>>()
                        + std::mem::size_of::<CollarBoundaryJet>()
                        + 3 * std::mem::size_of::<ChartCurveEnclosure>()
                        + SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES
                        + crate::parametric::nurbs_eval::SURFACE_EVALUATION_SCRATCH_BYTES
                        + 3 * std::mem::size_of::<crate::parametric::SurfaceEnclosure>(),
                )
            })
            .ok_or_else(super::transport::resource)?;
        let mut stack: Pending = [([[0.0; 2]; 2], 0, None); MAX_DEPTH as usize + 1];
        for cell in &self.cells {
            work.charge(1, cancel)?;
            if cover.include(cell.realized)? {
                continue;
            }
            let recipe = &self.recipes[cell.recipe];
            let face = recipe.outgoing.use_.face;
            let domain = source.face_domain(face, scratch, work, cancel)?;
            let classifier_bytes = scratch;
            stack[0] = ([[0.0, 1.0], cell.parameter], 0, None);
            let mut pending = 1;
            while pending != 0 {
                pending -= 1;
                let (parameter, depth, boundary) = stack[pending];
                work.charge(1, cancel)?;
                let boundary = match boundary {
                    Some(boundary) => boundary,
                    None => boundary_jet_with_byte_limit(
                        &domain,
                        recipe.outgoing,
                        recipe.incoming,
                        cell.piece,
                        Interval::new(parameter[1][0], parameter[1][1])?,
                        recipe.bulge,
                        classifier_bytes,
                        work,
                        cancel,
                    )?,
                };
                let jet = closed_cone_jet(self.point, boundary, parameter[0])?;
                if cover.include(jet.position)? {
                    continue;
                }
                let children = cover.split(parameter, depth, face, jet.ds, jet.dt)?;
                if pending + children.len() > stack.len() {
                    return Err(super::transport::resource());
                }
                let boundary = if children[0][1] == parameter[1] {
                    Some(boundary)
                } else {
                    None
                };
                for child in children.into_iter().rev() {
                    stack[pending] = (child, depth + 1, boundary);
                    pending += 1;
                }
            }
        }
        Ok(())
    }

    fn original_joint_recipe(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        joint_index: usize,
    ) -> Result<Option<&CornerRecipe>, GeometryError> {
        let Some(joint) = domain.original_joints()?.get(joint_index) else {
            return Ok(None);
        };
        if joint.vertex_id != self.vertex
            || [
                joint.vertex_point[0],
                joint.vertex_point[1],
                joint.vertex_point[2],
            ] != self.point
        {
            return Ok(None);
        }
        let face = domain.face();
        let matches_use = |native: NativeCoedgeIdentity, end: End| {
            native.face_id == face
                && native.wire_index == end.use_.wire
                && native.coedge_index == end.use_.index
                && native.edge_id == end.edge
                && native.orientation == (end.use_.orientation == domain.native_orientation())
                && native.edgeuse == end.use_.edgeuse
        };
        let mut recipes = self.recipes.iter().filter(|recipe| {
            matches_use(joint.incoming, recipe.incoming)
                && matches_use(joint.outgoing, recipe.outgoing)
        });
        let Some(recipe) = recipes.next() else {
            return Ok(None);
        };
        if recipes.next().is_some() {
            return Ok(None);
        }
        let Some(comparison) = &recipe.comparison else {
            return Ok(None);
        };
        Ok(comparison
            .validates_original_joint(domain, recipe, self.vertex, joint_index)?
            .then_some(recipe))
    }
    pub(super) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(
                self.cells
                    .capacity()
                    .saturating_mul(std::mem::size_of::<ConeCell>()),
            )
            .saturating_add(
                self.recipes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<CornerRecipe>()),
            )
    }

    /// One literal source-provider exit for each original endpoint germ.
    /// Closed edges retain both source parameter germs. Adjacent opposed uses
    /// share these SAME parameters, separately from the native edge interval.
    pub(super) fn visit_spatial_exits(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        visitor: &mut dyn FnMut(
            EdgeId,
            bool,
            f64,
            &mut GeometryWorkBudget,
        ) -> Result<(), GeometryError>,
    ) -> Result<(), GeometryError> {
        for recipe in &self.recipes {
            work.charge(1, cancel)?;
            visitor(
                recipe.outgoing.edge,
                recipe.outgoing.spatial_high,
                recipe.outgoing.exit,
                work,
            )?;
        }
        Ok(())
    }

    /// Bind one original closure envelope to the already completed CLOSED
    /// atlas, not merely to an incident cycle or a nearby selected cone.
    /// The whole joint image is independently bounded against NativeV and the
    /// SAME source projection. It is never asserted to lie in the comparison
    /// disk: the caller must still enclose the entire original F carrier.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn qualified_original_joint_image(
        &self,
        source: MaterialSourceView<'_, 'd>,
        domain: &PreparedFaceTrimDomain<'_>,
        joint_index: usize,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        has_cutover: &dyn Fn(EdgeId, bool, f64) -> bool,
    ) -> Result<bool, GeometryError> {
        work.charge(1, cancel)?;
        if !std::ptr::eq(source.definition(), self.definition) {
            return Ok(false);
        }
        let Some(joint) = domain.original_joints()?.get(joint_index) else {
            return Ok(false);
        };
        let Some(recipe) = self.original_joint_recipe(domain, joint_index)? else {
            return Ok(false);
        };
        if !has_cutover(
            recipe.incoming.edge,
            recipe.incoming.spatial_high,
            recipe.incoming.exit,
        ) || !has_cutover(
            recipe.outgoing.edge,
            recipe.outgoing.spatial_high,
            recipe.outgoing.exit,
        ) {
            return Ok(false);
        }
        let geometry = self.qualify_geometry(source, remaining_bytes, work, cancel)?;
        // The opaque all-recipe authority contains this exact source cut,
        // even if cell qualification used the convex-affine lane. It was
        // issued only after the matching relative-complement or genuine pole
        // quotient proof. No native connector is chosen.
        if !geometry.owns_source_cut(recipe) {
            return Ok(false);
        }
        let scratch = crate::parametric::SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES
            .saturating_add(crate::parametric::surface_functional_scratch_bytes::<3>())
            .saturating_add(std::mem::size_of::<crate::parametric::SurfaceFirstEnclosure>())
            .saturating_add(4 * std::mem::size_of::<Box3>());
        let pole_scratch = if matches!(joint.kind, TrimJointKind::SphericalPole { .. }) {
            // SphereChart admission calls the native whole-pole strip proof.
            // Its chart, native jets/operands and returned receipt coexist
            // with the outer image/functional evaluation storage.
            (2 * std::mem::size_of::<source_chart::SphereChart>())
                .saturating_add(std::mem::size_of::<Option<source_chart::SphereChart>>())
                .saturating_add(crate::parametric::curve_prepared::CURVE_EVALUATION_SCRATCH_BYTES)
                .saturating_add(std::mem::size_of::<crate::parametric::ChartCurveEnclosure>())
                .saturating_add(crate::parametric::nurbs_eval::SURFACE_EVALUATION_SCRATCH_BYTES)
                .saturating_add(2 * std::mem::size_of::<TrimJointKind>())
                .saturating_add(8 * std::mem::size_of::<Box3>())
        } else {
            0
        };
        let required = scratch
            .saturating_add(pole_scratch)
            .max(super::atlas::ORIGINAL_JOINT_POSITION_SCRATCH_BYTES)
            .saturating_add(std::mem::size_of::<Box3>() + 3 * std::mem::size_of::<Box2>());
        if required > remaining_bytes {
            return Err(super::transport::resource());
        }
        let allowance = source_allowance(self.definition)?.min(domain.source_chart_error_mm());
        if !allowance.is_finite() || allowance <= 0.0 {
            return Ok(false);
        }
        let physical =
            super::atlas::original_joint_position_enclosure(domain, joint, work, cancel)?;
        if length(relative(physical, self.point))?.hi > allowance {
            return Ok(false);
        }
        let connector = crate::parametric::CurveEvaluator::tensor(
            &joint.connector,
            [0.0, 1.0],
            0,
            work,
            cancel,
        )?;
        let mut uv = hull2(joint.enclosure, [connector[0][0], connector[0][1]]);
        for endpoint in joint.endpoint_uv {
            uv = hull2(
                uv,
                endpoint.map(|range| Interval {
                    lo: range[0],
                    hi: range[1],
                }),
            );
        }
        if let TrimJointKind::SphericalPole { connector, .. } = &joint.kind {
            let connector =
                crate::parametric::CurveEvaluator::tensor(connector, [0.0, 1.0], 0, work, cancel)?;
            uv = hull2(uv, [connector[0][0], connector[0][1]]);
        }
        let surface = &domain.surface_operand();
        (|| {
            // This independent chart uses the SAME borrowed carrier/domain,
            // never a family label or the rank-zero covering-chart normal.
            let sphere_chart = if matches!(joint.kind, TrimJointKind::SphericalPole { .. }) {
                let Some(chart) = source_chart::SphereChart::new(surface, domain, work, cancel)?
                else {
                    return Ok(false);
                };
                Some(chart)
            } else {
                None
            };
            if let Some(chart) = &sphere_chart {
                let normal = chart.normal(physical)?;
                if dot(normal, cross(self.rows[0], self.rows[1])).lo <= 0.0 {
                    return Ok(false);
                }
            }
            whole_original_joint_image_fits(
                surface,
                uv,
                self.point,
                cross(self.rows[0], self.rows[1]),
                domain.orientation(),
                sphere_chart.as_ref(),
                allowance,
                work,
                cancel,
            )
        })()
    }

    /// Conservative complete-set exclusion, not corner ownership. Removed F
    /// is an exact subset of each original incident face, irrespective of how
    /// a reference star parametrizes that subset. Bound the WHOLE original
    /// carrier domain first; if needed, restrict to the Cartesian box of the
    /// complete literal reference boundary. Then check every added cone G cell
    /// with its full radial parameter. Neither cover asserts corner ownership
    /// or promotes a `cell.source` image to domain authority.
    pub(super) fn complete_incident_positions_miss(
        &self,
        source: MaterialSourceView<'_, 'd>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        failed_cover: &mut Option<CompletePositionCoverFailure>,
        misses: &mut dyn FnMut(Box3) -> Result<bool, GeometryError>,
    ) -> Result<bool, GeometryError> {
        let before = work.consumed();
        let mut phase = "immutable original definition binding";
        let mut active_face = None;
        *failed_cover = None;
        let outcome = (|| {
            work.charge(1, cancel)?;
            if !std::ptr::eq(source.definition(), self.definition) {
                return Err(refusal(
                    "source corner position cover lost its original definition owner",
                ));
            }
            let scratch = remaining_bytes
                .checked_sub(CONICAL_STACK_BYTES)
                .ok_or_else(|| {
                    GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "complete incident-face position cover exceeds scratch allowance",
                    )
                })?;
            for recipe in &self.recipes {
                phase = "complete original incident carrier domain";
                let face = recipe.outgoing.use_.face;
                active_face = Some(face);
                work.charge(1, cancel)?;
                let domain = source.face_domain(face, scratch, work, cancel)?;
                let position = {
                    let surface = &domain.surface_operand();
                    (|| {
                        crate::parametric::surface_piecewise_enclosure(
                            surface,
                            domain.uv_domain(),
                            work,
                            cancel,
                        )
                        .map(|jet| jet.position)
                    })()
                }?;
                if misses(position)? {
                    continue;
                }
                *failed_cover = Some(CompletePositionCoverFailure {
                    phase: "whole incident carrier overlaps",
                    source_face: face,
                    position,
                });
                phase = "complete original comparison source-position overcover";
                let Some(comparison) = recipe.comparison.as_ref() else {
                    *failed_cover = Some(CompletePositionCoverFailure {
                        phase: "whole carrier overlaps; original comparison unavailable",
                        source_face: face,
                        position,
                    });
                    return Ok(false);
                };
                match comparison.source_position_overcover(
                    &domain,
                    recipe,
                    self.vertex,
                    work,
                    cancel,
                )? {
                    SourcePositionOvercover::Unavailable(reason) => {
                        *failed_cover = Some(CompletePositionCoverFailure {
                            phase: reason,
                            source_face: face,
                            position,
                        });
                        return Ok(false);
                    }
                    SourcePositionOvercover::EmptyOriginalDomain => continue,
                    SourcePositionOvercover::Bounded(position) => {
                        if !misses(position)? {
                            *failed_cover = Some(CompletePositionCoverFailure {
                                phase: "complete original comparison source-position box overlaps",
                                source_face: face,
                                position,
                            });
                            return Ok(false);
                        }
                    }
                }
            }
            phase = "complete added cone G radial supports";
            for cell in &self.cells {
                work.charge(1, cancel)?;
                if !misses(cell.realized)? {
                    *failed_cover = Some(CompletePositionCoverFailure {
                        phase: "complete added cone G support overlaps",
                        source_face: self.recipes[cell.recipe].outgoing.use_.face,
                        position: cell.realized,
                    });
                    return Ok(false);
                }
            }
            Ok(true)
        })();
        outcome.map_err(|error| work.annotate_error(error, before, format_args!(
            "complete source corner position cover vertex_mm={:?} source_face={active_face:?} phase={phase}",
            self.point
        )))
    }

    pub(super) fn visit_position_supports(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        visitor: &mut dyn FnMut(Box3, Box3) -> Result<(), GeometryError>,
    ) -> Result<(), GeometryError> {
        for cell in &self.cells {
            work.charge(1, cancel)?;
            visitor(cell.realized, hull3(cell.source, cell.realized))?;
        }
        Ok(())
    }

    /// The normative selected cone is G(s,t)=V+s(B(t)-V), including s=0.
    /// Thus its literal V is an actual point of the complete G even when an
    /// individual refined support cell excludes s=0. Owner labels play no role.
    fn apex_scalar_witness(&self, direction: Box3) -> Result<Interval, GeometryError> {
        if direction
            .into_iter()
            .any(|v| !v.lo.is_finite() || !v.hi.is_finite() || v.lo > v.hi)
        {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "source conical support direction is nonfinite",
            ));
        }
        let value = dot(direction, self.point.map(Interval::point));
        Interval::new(value.lo, value.hi)
    }

    pub(super) fn visit_supports(
        &self,
        direction: [Interval; 3],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        visitor: &mut dyn FnMut(SourceTransportSupport) -> Result<(), GeometryError>,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        let witness = self.apex_scalar_witness(direction)?;
        let row = direction;
        for cell in &self.cells {
            work.charge(1, cancel)?;
            let base = dot(row, cell.source);
            let corrected = dot(row, cell.realized);
            let displacement = dot(row, cell.displacement);
            visitor(SourceTransportSupport {
                owner: SourceTransportOwner::OriginalVertex {
                    point_mm: self.point,
                },
                base_interval: [base.lo, base.hi],
                corrected_interval: [corrected.lo, corrected.hi],
                displacement_interval: [displacement.lo, displacement.hi],
                realized_witness_interval: Some([witness.lo, witness.hi]),
                realized_endpoint_witnesses: None,
                source_error_mm: cell.error_mm,
            })?;
        }
        Ok(())
    }

    /// G(s,t)=V+s(B(t)-V) is affine in s. Its complete scalar extrema are
    /// exactly the extrema of V and B, without a radial subdivision or a
    /// query-dependent OriginalF/G correspondence. Keep the immutable BASE,
    /// comparison and displacement inventory while refining actual G only.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn visit_endpoint_supports(
        &self,
        source: MaterialSourceView<'_, 'd>,
        direction: [Interval; 3],
        reference: [f64; 2],
        tolerance: f64,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        output: &mut Vec<SourceTransportSupport>,
    ) -> Result<(), GeometryError> {
        use super::transport::{output_bytes, push};
        let fixed_bytes = 2 * std::mem::size_of::<BoundaryJet>()
            + std::mem::size_of::<OriginalCollar<'_, '_>>()
            + std::mem::size_of::<CollarBoundaryJet>()
            + 3 * std::mem::size_of::<ChartCurveEnclosure>()
            + crate::parametric::SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES
            + crate::parametric::nurbs_eval::SURFACE_EVALUATION_SCRATCH_BYTES;
        let available = remaining_bytes
            .checked_sub(fixed_bytes)
            .ok_or_else(super::transport::resource)?;
        if output_bytes(output) > available {
            return Err(super::transport::resource());
        }
        work.charge(1, cancel)?;
        let witness = self.apex_scalar_witness(direction)?;
        // Boundary scratch and the adaptive scalar queue share this cap with
        // the retained domain and all previously produced support records.
        for cell in &self.cells {
            work.charge(1, cancel)?;
            let coarse = SourceTransportSupport {
                owner: SourceTransportOwner::OriginalVertex {
                    point_mm: self.point,
                },
                base_interval: {
                    let value = dot(direction, cell.source);
                    [value.lo, value.hi]
                },
                corrected_interval: {
                    let value = dot(direction, cell.realized);
                    [value.lo, value.hi]
                },
                displacement_interval: {
                    let value = dot(direction, cell.displacement);
                    [value.lo, value.hi]
                },
                realized_witness_interval: Some([witness.lo, witness.hi]),
                realized_endpoint_witnesses: None,
                source_error_mm: cell.error_mm,
            };
            if !endpoint_support_needs_refinement(&coarse, reference, tolerance) {
                push(output, coarse, available)?;
                continue;
            }
            let recipe = &self.recipes[cell.recipe];
            let domain_limit = available
                .checked_sub(output_bytes(output))
                .ok_or_else(super::transport::resource)?;
            let domain =
                source.face_domain(recipe.outgoing.use_.face, domain_limit, work, cancel)?;
            let output_limit = available;
            let query_bytes = output_limit
                .checked_sub(output_bytes(output))
                .ok_or_else(super::transport::resource)?;
            let boundary = crate::analysis::bounds::source_recipe_curve_bounds(
                cell.parameter,
                tolerance,
                query_bytes,
                work,
                cancel,
                |range, work, cancel, remaining_bytes| {
                    let (height, derivative) = cone_boundary_scalar(
                        &domain,
                        recipe,
                        cell.piece,
                        range,
                        direction,
                        remaining_bytes,
                        work,
                        cancel,
                    )?;
                    let middle = range[0] * 0.5 + range[1] * 0.5;
                    let (point, _) = cone_boundary_scalar(
                        &domain,
                        recipe,
                        cell.piece,
                        [middle; 2],
                        direction,
                        remaining_bytes,
                        work,
                        cancel,
                    )?;
                    let centered = point.add(
                        derivative
                            .mul(Interval::new(range[0], range[1])?.sub(Interval::point(middle))),
                    );
                    let height = intersection(height, centered).ok_or_else(|| {
                        refusal("source cone boundary scalar and mean-value enclosures disagree")
                    })?;
                    Ok((height, point))
                },
            )?;
            let corrected = witness.hull(Interval::new(boundary.minimum[0], boundary.maximum[1])?);
            let corrected = intersection(
                corrected,
                Interval::new(coarse.corrected_interval[0], coarse.corrected_interval[1])?,
            )
            .ok_or_else(|| refusal("source cone complete scalar enclosures disagree"))?;
            push(
                output,
                SourceTransportSupport {
                    corrected_interval: [corrected.lo, corrected.hi],
                    realized_endpoint_witnesses: Some([
                        witness.hi.min(boundary.minimum[1]),
                        witness.lo.max(boundary.maximum[0]),
                    ]),
                    ..coarse
                },
                output_limit,
            )?;
        }
        Ok(())
    }

    /// Visit the actual selected cone cells only after qualifying the COMPLETE
    /// incident star: source graph, trim ownership, cutovers and separation.
    /// The normal has its nonnegative radial parameter factored out; field
    /// consumers must retain the endpoint fan and prove their own projection.
    /// `remaining_bytes` is scratch after any locally owned plan reservation;
    /// retained source plans are charged only by their immutable source owner.
    pub(super) fn visit_qualified_geometry(
        &self,
        source: MaterialSourceView<'_, 'd>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        visitor: &mut dyn FnMut(usize, Box3, Box3) -> Result<(), GeometryError>,
    ) -> Result<(), GeometryError> {
        self.qualify_geometry(source, remaining_bytes, work, cancel)?;
        for cell in &self.cells {
            work.charge(1, cancel)?;
            visitor(
                self.recipes[cell.recipe].outgoing.use_.face,
                cell.realized,
                cell.normal,
            )?;
        }
        Ok(())
    }

    /// Query-only cover of the immutable selected G, after the complete source
    /// disk, graph, cutover and embedding theorem. The radial coordinate is
    /// CLOSED, including the collapsed apex and the complete outer cutover.
    /// Neither subdivision nor the visitor can change the selected recipe.
    /// Callback positions and normals are SOURCE-coordinate selected G, as
    /// required by BoundaryCell. Actual coordinates bind activity/scheduling.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn visit_closed_atlas_geometry(
        &self,
        source: MaterialSourceView<'_, 'd>,
        coordinates: &crate::analysis::placed::ActualMaterialCoordinates<'_>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        visitor: &mut dyn FnMut(
            usize,
            Box3,
            Box3,
            bool,
            &mut GeometryWorkBudget,
            &AtomicBool,
        )
            -> Result<super::radial_product::BoundaryVisit, GeometryError>,
    ) -> Result<bool, GeometryError> {
        use super::radial_product::BoundaryVisit;
        type Pending = [([[f64; 2]; 2], u8, Option<BoundaryJet>); MAX_DEPTH as usize + 1];
        self.qualify_geometry(source, remaining_bytes, work, cancel)?;
        // Pending radial siblings, the live boundary jet, the original domain,
        // and both first/full native evaluation scratch coexist. In particular
        // the Middle boundary uses the full carrier jet, not a fabricated Hessian.
        let scratch = remaining_bytes
            .checked_sub(std::mem::size_of::<Pending>())
            .and_then(|bytes| {
                bytes.checked_sub(
                    2 * std::mem::size_of::<BoundaryJet>()
                        + std::mem::size_of::<ClosedConeJet>()
                        + std::mem::size_of::<OriginalCollar<'_, '_>>()
                        + std::mem::size_of::<CollarBoundaryJet>()
                        + 3 * std::mem::size_of::<crate::parametric::ChartCurveEnclosure>(),
                )
            })
            .and_then(|bytes| {
                bytes.checked_sub(crate::parametric::SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES)
            })
            .and_then(|bytes| {
                bytes.checked_sub(
                    crate::parametric::nurbs_eval::SURFACE_EVALUATION_SCRATCH_BYTES
                        + 3 * std::mem::size_of::<crate::parametric::SurfaceEnclosure>(),
                )
            })
            .ok_or_else(super::transport::resource)?;
        let before = work.consumed();
        let mut active = None;
        let outcome = (|| {
            let mut stack: Pending = [([[0.0; 2]; 2], 0, None); MAX_DEPTH as usize + 1];
            for cell in &self.cells {
                work.charge(1, cancel)?;
                let recipe = &self.recipes[cell.recipe];
                let face = recipe.outgoing.use_.face;
                active = Some((face, cell.recipe, [[0.0, 1.0], cell.parameter]));
                let interval = coordinates.world_interval();
                let height = coordinates.position(cell.realized)?[2];
                if height.hi < interval[0] || height.lo > interval[1] {
                    continue;
                }
                // The retained source-only cell already encloses the SAME
                // complete closed cone. Do not repeat native carrier queries
                // when its existing range proves every query obligation.
                // Area normals with the nonnegative s factor removed match the
                // F/G inventory. Unit normalization would destroy correlation
                // between a component and its own magnitude enclosure.
                if visitor(face, cell.realized, cell.normal, true, work, cancel)?
                    == BoundaryVisit::Accepted
                {
                    continue;
                }
                let domain = source.face_domain(face, scratch, work, cancel)?;
                stack[0] = ([[0.0, 1.0], cell.parameter], 0, None);
                let mut pending = 1;
                while pending != 0 {
                    pending -= 1;
                    let (parameter, depth, boundary) = stack[pending];
                    active = Some((face, cell.recipe, parameter));
                    work.charge(1, cancel)?;
                    let boundary = match boundary {
                        Some(boundary) => boundary,
                        None => boundary_jet(
                            &domain,
                            recipe.outgoing,
                            recipe.incoming,
                            cell.piece,
                            Interval::new(parameter[1][0], parameter[1][1])?,
                            recipe.bulge,
                            work,
                            cancel,
                        )?,
                    };
                    let jet = closed_cone_jet(self.point, boundary, parameter[0])?;
                    let position = coordinates.position_relative_to_center_family(jet.position)?;
                    let interval = coordinates.world_interval();
                    // Strict exclusion only: equality with either cut remains
                    // an active closed-cell obligation, including s=0 and s=1.
                    if position[2].hi < interval[0] || position[2].lo > interval[1] {
                        continue;
                    }
                    let can_refine = depth < MAX_DEPTH
                        && parameter.iter().any(|range| {
                            let mid = range[0] * 0.5 + range[1] * 0.5;
                            range[0] < mid && mid < range[1]
                        });
                    let included =
                        visitor(face, jet.position, jet.normal, can_refine, work, cancel)?;
                    if included == BoundaryVisit::Accepted {
                        continue;
                    }
                    if !can_refine {
                        return Ok(false);
                    }
                    let children = if included == BoundaryVisit::RefineActualXy {
                        let rows = coordinates.affine_rows();
                        let ds = [dot(rows[0], jet.ds), dot(rows[1], jet.ds)];
                        let dt = [dot(rows[0], jet.dt), dot(rows[1], jet.dt)];
                        crate::analysis::critical::split_metric_derivatives(
                            parameter, depth, face, &ds, &dt,
                        )?
                    } else {
                        // The limiting normal fan varies with t even at s=0;
                        // use unattenuated angular motion for spatial/rank work.
                        crate::analysis::critical::split_metric_derivatives(
                            parameter,
                            depth,
                            face,
                            &jet.ds,
                            &boundary.derivative,
                        )?
                    };
                    if pending + children.len() > stack.len() {
                        return Err(super::transport::resource());
                    }
                    // Only a radial split may reuse B(t). A t split always
                    // reevaluates the identical authoritative boundary recipe.
                    let boundary = if children[0][1] == parameter[1] {
                        Some(boundary)
                    } else {
                        None
                    };
                    for child in children.into_iter().rev() {
                        stack[pending] = (child, depth + 1, boundary);
                        pending += 1;
                    }
                }
            }
            Ok(true)
        })();
        outcome.map_err(|error| {
            work.annotate_error(
                error,
                before,
                format_args!(
                    "closed source cone atlas vertex_mm={:?} face_recipe_parameter={active:?}",
                    self.point,
                ),
            )
        })
    }

    /// Retain only a successful qualification of this immutable source star.
    /// No slab, pose, radial field or metric participates in that theorem.
    /// Fixed inline storage avoids retained growth. The cache records only a
    /// successful source theorem; all uncached coefficient work remains charged
    /// to its caller.
    fn qualify_geometry(
        &self,
        source: MaterialSourceView<'_, 'd>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<QualifiedSourceGeometry<'_, 'd>, GeometryError> {
        work.charge(1, cancel)?;
        if !std::ptr::eq(source.definition(), self.definition) {
            return Err(refusal(
                "qualified source star lost its immutable definition owner",
            ));
        }
        if self.source_geometry_qualified.load(Ordering::Acquire) {
            return Ok(QualifiedSourceGeometry { plan: self });
        }
        self.qualify_geometry_fresh(source, remaining_bytes, work, cancel)?;
        self.source_geometry_qualified
            .store(true, Ordering::Release);
        Ok(QualifiedSourceGeometry { plan: self })
    }

    fn qualify_geometry_fresh(
        &self,
        source: MaterialSourceView<'_, 'd>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        let before = work.consumed();
        let mut phase = "immutable original definition binding";
        let mut active_recipe = None;
        let mut active_cell = None;
        let outcome = (|| {
            if !std::ptr::eq(source.definition(), self.definition) {
                return Err(refusal(
                    "source conical geometry lost its original definition owner",
                ));
            }
            let allowance = source_allowance(self.definition)?;
            let scratch = remaining_bytes
                .checked_sub(CONICAL_STACK_BYTES)
                .ok_or_else(|| {
                    GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "active source conical qualification exceeds scratch allowance",
                    )
                })?;
            for (index, recipe) in self.recipes.iter().enumerate() {
                active_recipe = Some((
                    index,
                    recipe.outgoing.use_.face,
                    [recipe.outgoing.use_.coedge, recipe.incoming.use_.coedge],
                ));
                active_cell = None;
                phase = "source corner face domain";
                work.charge(1, cancel)?;
                let face = recipe.outgoing.use_.face;
                let domain = source.face_domain(face, scratch, work, cancel)?;
                let owned_scratch = scratch;
                phase = "active collar source-P graph transversality";
                if !collar_graph_transverse(&domain, recipe.outgoing, self.rows, work, cancel)?
                    || !collar_graph_transverse(&domain, recipe.incoming, self.rows, work, cancel)?
                {
                    return Err(self.gate_failure(
                        face,
                        "active source vertex outside-G collar is not a proved source-P graph",
                    ));
                }
                phase = "convex affine reference-star ownership";
                let convex_owned = convex_affine_star_inside(
                    &domain,
                    recipe.outgoing,
                    recipe.incoming,
                    recipe.bulge,
                    work,
                    cancel,
                )?;
                // Even the convex-affine lane must qualify the actual native
                // source complement; being Inside proves no boundary surgery.
                let comparison_owned = if let Some(comparison) = &recipe.comparison {
                    phase = match comparison {
                        OriginalCornerComparison::NativeUv(_) => {
                            "original NativeUv source-face relative crosscut"
                        }
                        OriginalCornerComparison::SphericalPole(_) => {
                            "original SphericalPole source-face quotient cut"
                        }
                    };
                    comparison.validates(&domain, recipe, self.vertex)?;
                    let Some(cut) =
                        comparison.source_cut(&domain, recipe, owned_scratch, work, cancel)?
                    else {
                        return Err(self.gate_failure(
                            face,
                            "original source-face complement has no qualified cut",
                        ));
                    };
                    if !cut.binds(&domain, recipe) {
                        return Err(self.gate_failure(
                            face,
                            "original source-face cut belongs to different immutable operands",
                        ));
                    }
                    !convex_owned
                } else {
                    phase = "original comparison ownership refusal diagnostic";
                    // Reconstruct full finite diagnostics ONLY on this refusal
                    // path; successful plans retain no large diagnostic payload.
                    match OriginalCornerComparison::prepare(
                        &domain,
                        recipe,
                        self.vertex,
                        allowance,
                        owned_scratch,
                        work,
                        cancel,
                    )? {
                        UvComparisonPreparation::Refused(reason) => {
                            let stable = recipe.comparison_refusal == Some(reason.stage);
                            return Err(refusal(format!(
                                "{reason}; immutable_admission_stage_matches={stable} definition={} source_endpoint_fraction={}",
                                self.definition.id().as_str(),
                                self.fraction,
                            )));
                        }
                        UvComparisonPreparation::Prepared(_) => {
                            return Err(self.gate_failure(
                                face,
                                "fixed source comparison was not retained after proved admission",
                            ));
                        }
                    }
                };
                if !convex_owned && !comparison_owned {
                    return Err(self.gate_failure(
                        face,
                        "relative source UV disk did not retain complete ownership authority",
                    ));
                }
                phase = "source sphere-chart qualification";
                let sphere_chart = {
                    let surface = &domain.surface_operand();
                    (|| source_chart::SphereChart::new(surface, &domain, work, cancel))()
                }?;
                for cell in self.cells.iter().filter(|cell| cell.recipe == index) {
                    active_cell = Some((cell.parameter, cell.recipe));
                    phase = "source corner cell";
                    work.charge(1, cancel)?;
                    let t = Interval {
                        lo: cell.parameter[0],
                        hi: cell.parameter[1],
                    };
                    if !comparison_owned {
                        phase = "exact outer source/G cutover ownership";
                        let boundary = boundary_jet(
                            &domain,
                            recipe.outgoing,
                            recipe.incoming,
                            cell.piece,
                            t,
                            recipe.bulge,
                            work,
                            cancel,
                        )?;
                        if !outer_cutover_owned(
                            &domain,
                            recipe.outgoing,
                            recipe.incoming,
                            cell.piece,
                            recipe.bulge,
                            &boundary,
                            work,
                            cancel,
                        )? {
                            return Err(self.gate_failure(face,
                            "active source vertex outer trace lacks unchanged/paired-G collar cutover ownership"));
                        }
                    }
                    phase = "active source-P graph normal transversality";
                    let transverse = if let Some(chart) = &sphere_chart {
                        // This is a whole physical SOURCE enclosure, including the
                        // comparison carrier homotopy. Its ambient box need not lie
                        // on-carrier; the original quadric gradient bounds all
                        // source normals inside it. Actual G normals are separate.
                        dot(
                            chart.normal(cell.source)?,
                            cross(self.rows[0], self.rows[1]),
                        )
                        .lo > 0.0
                    } else {
                        let h = star_jet(
                            &domain,
                            recipe.outgoing,
                            recipe.incoming,
                            cell.piece,
                            Interval { lo: 0.0, hi: 1.0 },
                            t,
                            recipe.bulge,
                            work,
                            cancel,
                        )?;
                        let comparison_uv = if comparison_owned {
                            let comparison = recipe
                                .comparison
                                .as_ref()
                                .and_then(OriginalCornerComparison::native_uv)
                                .ok_or_else(|| {
                                    self.gate_failure(
                                        face,
                                        "non-spherical source graph lost its native-UV comparison",
                                    )
                                })?;
                            comparison
                                .star_jet(
                                    &domain,
                                    recipe,
                                    cell.piece,
                                    Interval { lo: 0.0, hi: 1.0 },
                                    t,
                                    work,
                                    cancel,
                                )?
                                .uv
                        } else {
                            h.uv
                        };
                        {
                            let surface = &domain.surface_operand();
                            (|| {
                                let jet = surface_enclosure(
                                    surface,
                                    uv_box(hull2(h.uv, comparison_uv)),
                                    work,
                                    cancel,
                                )?;
                                let n = normal(&jet, domain.orientation());
                                Ok(dot(n, cross(self.rows[0], self.rows[1])).lo > 0.0)
                            })()
                        }?
                    };
                    if !transverse {
                        return Err(self.gate_failure(
                        face,
                        "active source vertex carrier lacks physical source-P graph transversality",
                    ));
                    }
                }
            }
            let support = self
                .cells
                .iter()
                .fold(self.point.map(Interval::point), |a, c| hull3(a, c.realized))
                .map(|v| {
                    v.add(Interval {
                        lo: -allowance,
                        hi: allowance,
                    })
                });
            phase = "active cone whole-source nonincident separation";
            if !separate_nonincident(source, &self.recipes, support, scratch, work, cancel)? {
                return Err(self.gate_failure(
                    self.recipes[0].outgoing.use_.face,
                    "active source conical star lacks complete nonincident source separation",
                ));
            }
            Ok(())
        })();
        outcome.map_err(|error| work.annotate_error(error, before, format_args!(
            "source corner geometry vertex_mm={:?} recipe_face_coedges={active_recipe:?} cell={active_cell:?} phase={phase}",
            self.point
        )))
    }

    /// The caller reserves local plan storage before lending query scratch.
    /// Activity is still tested before any source geometry qualification.
    pub(super) fn prove_radial<'a>(
        &'a self,
        source: MaterialSourceView<'_, 'd>,
        prism: &SamplePrism,
        request: &SlabRequest,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<SourceVertexRadialProof<'a, 'd>>, GeometryError> {
        work.charge(1, cancel)?;
        if prism.range != request.height_interval_mm {
            return Err(refusal(
                "source conical radial query lost its closed height interval",
            ));
        }
        let functional = prism.source_height;
        let mut active = false;
        for cell in &self.cells {
            work.charge(1, cancel)?;
            let h = functional.height(cell.realized);
            active |=
                h.hi >= request.height_interval_mm[0] && h.lo <= request.height_interval_mm[1];
        }
        if !active {
            return Ok(Some(SourceVertexRadialProof {
                plan: self,
                request: *request,
                outside_closed_slab: true,
            }));
        }
        self.qualify_geometry(source, remaining_bytes, work, cancel)?;
        for cell in &self.cells {
            work.charge(1, cancel)?;
            let h = functional.height(cell.realized);
            if h.hi < request.height_interval_mm[0] || h.lo > request.height_interval_mm[1] {
                continue;
            }
            // n.(G-center)=n.(V-center) for every s in disk mode. A toroidal
            // field is not linear, so only its whole actual q enclosure is used.
            let radial = if prism.core_radius.is_none() {
                relative(self.point.map(Interval::point), prism.center)
            } else {
                let Some((q, _)) = prism.radial_projection_frame(cell.realized)? else {
                    return Err(self.gate_failure(
                        self.recipes[cell.recipe].outgoing.use_.face,
                        "active source cone has no whole toroidal radial field enclosure",
                    ));
                };
                q
            };
            if dot(cell.normal, radial).lo <= 0.0 {
                return Err(self.gate_failure(
                    self.recipes[cell.recipe].outgoing.use_.face,
                    "active source cone outer trace does not prove the whole radial sign",
                ));
            }
        }
        Ok(Some(SourceVertexRadialProof {
            plan: self,
            request: *request,
            outside_closed_slab: false,
        }))
    }

    fn gate_failure(&self, face: usize, message: &'static str) -> GeometryError {
        refusal(format!(
            "{message}: definition={} vertex_mm={:?} source_face_index={face} source_endpoint_fraction={}",
            self.definition.id().as_str(),
            self.point,
            self.fraction,
        ))
    }
}

fn corners(
    source: MaterialSourceView<'_, '_>,
    incidence: &SourceEdgeIncidence,
    vertex: VertexId,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<(Vec<Corner>, [f64; 3])>, GeometryError> {
    let before = work.consumed();
    let mut point = None;
    let mut face_coedge = None;
    let outcome = (|| {
        // The prepared source owner validates one captured STEP VertexPoint
        // identity and its exact mm coefficients across the complete star.
        // Native IDs select incidence only; healed native positions do not
        // choose the realization apex.
        point = Some(
            source
                .trim_source()
                .source_vertex_point(vertex, work, cancel)?,
        );
        let mut result = Vec::new();
        for edge_id in incidence.vertex_edges(vertex) {
            work.charge(1, cancel)?;
            let Some(pair) = incidence.get(edge_id) else {
                return Ok(None);
            };
            let Some(second) = pair.second else {
                return Ok(None);
            };
            for use_ in [pair.first, second] {
                face_coedge = Some((use_.face, use_.coedge));
                let domain_bytes = remaining_bytes
                    .checked_sub(
                        result
                            .capacity()
                            .saturating_mul(std::mem::size_of::<Corner>()),
                    )
                    .ok_or_else(|| {
                        GeometryError::new(
                            GeometryErrorCode::ResourceLimit,
                            "source conical inventory exhausts domain scratch",
                        )
                    })?;
                let domain = source.face_domain(use_.face, domain_bytes, work, cancel)?;
                let definition = source.definition();
                let wire = crate::analysis::source_incidence::boundary_loop(
                    definition, use_.face, use_.wire,
                )?;
                let (entry, edge) = crate::analysis::source_incidence::native_coedge(
                    definition, use_.face, use_.wire, use_.index,
                )?;
                let orientation = crate::analysis::source_incidence::face_orientation(
                    definition, use_.face, work, cancel,
                )?;
                let ends = edge.vertices;
                let origin = ends[usize::from(!use_.orientation)];
                if origin != vertex {
                    continue;
                }
                if entry.edge != edge_id
                    || (entry.forward == orientation) != use_.orientation
                    || domain.coedges()[use_.coedge].edgeuse != use_.edgeuse
                {
                    return Ok(None);
                }
                let adjacent = if orientation {
                    if use_.index == 0 {
                        wire.edges.len() - 1
                    } else {
                        use_.index - 1
                    }
                } else {
                    (use_.index + 1) % wire.edges.len()
                };
                let (neighbor_entry, neighbor) = crate::analysis::source_incidence::native_coedge(
                    definition, use_.face, use_.wire, adjacent,
                )?;
                let neighbor_orientation = neighbor_entry.forward == orientation;
                let neighbor_ends = neighbor.vertices;
                if neighbor_ends[usize::from(neighbor_orientation)] != vertex {
                    return Ok(None);
                }
                let binding = domain
                    .original_coedge_correspondence(use_.coedge)?
                    .binding()
                    .ok_or_else(|| {
                        refusal("source vertex corner lost its original endpoint binding")
                    })?;
                let target = binding.source_vertex_points_mm[usize::from(!use_.orientation)];
                if point.is_some_and(|prior| prior != target) {
                    return Err(refusal(
                        "incident original source vertex coefficients disagree",
                    ));
                }
                point = Some(target);
                if result
                    .len()
                    .saturating_add(1)
                    .saturating_mul(std::mem::size_of::<Corner>())
                    > remaining_bytes
                {
                    return Err(GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "source conical corner inventory exceeds scratch allowance",
                    ));
                }
                result.try_reserve_exact(1).map_err(|_| {
                    GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "source conical corner inventory allocation refused",
                    )
                })?;
                result.push(Corner {
                    outgoing: use_,
                    incoming: CoedgeUse {
                        face: use_.face,
                        wire: use_.wire,
                        index: adjacent,
                        coedge: use_.coedge - use_.index + adjacent,
                        orientation: neighbor_orientation,
                        edgeuse: domain.coedges()[use_.coedge - use_.index + adjacent].edgeuse,
                    },
                });
            }
        }
        if result.is_empty() {
            return Ok(None);
        }
        result.sort_unstable_by_key(|c| (c.outgoing.face, c.outgoing.wire, c.outgoing.index));
        // Reorder ONLY by the retained paired-use permutation. In particular a
        // closed original edge supplies its two different parameter germs.
        for index in 0..result.len() {
            face_coedge = Some((result[index].incoming.face, result[index].incoming.coedge));
            let Some(pair) = incidence.get(
                source
                    .face_domain(
                        result[index].incoming.face,
                        remaining_bytes.saturating_sub(
                            result
                                .capacity()
                                .saturating_mul(std::mem::size_of::<Corner>()),
                        ),
                        work,
                        cancel,
                    )?
                    .coedges()[result[index].incoming.coedge]
                    .identity()
                    .edge_id,
            ) else {
                return Ok(None);
            };
            let next = if pair.first == result[index].incoming {
                pair.second
            } else if pair.second == Some(result[index].incoming) {
                Some(pair.first)
            } else {
                None
            };
            let Some(next) = next else {
                return Ok(None);
            };
            if index + 1 == result.len() {
                if next != result[0].outgoing {
                    return Ok(None);
                }
            } else {
                let Some(offset) = result[index + 1..].iter().position(|c| c.outgoing == next)
                else {
                    return Ok(None);
                };
                result.swap(index + 1, index + 1 + offset);
            }
        }
        Ok(point.map(|point| (result, point)))
    })();
    outcome.map_err(|error| work.annotate_error(error, before, format_args!(
        "SourceVertexTransportPlan corner inventory vertex_mm={point:?} source_face_coedge={face_coedge:?}",
    )))
}

fn source_frame(
    source: MaterialSourceView<'_, '_>,
    corners: &[Corner],
    scratch: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[Box3; 2]>, GeometryError> {
    let mut candidate = [0.0; 3];
    for corner in corners {
        let mut spatial_window = None;
        let before = work.consumed();
        let evaluated = (|| {
            work.charge(1, cancel)?;
            let domain = source.face_domain(corner.outgoing.face, scratch, work, cancel)?;
            let n = {
                let surface = &domain.surface_operand();
                (|| {
                    let proof = domain.original_coedge_correspondence(corner.outgoing.coedge)?;
                    let binding = proof.binding().ok_or_else(|| {
                        refusal("source frame lost its selected spatial endpoint mapping")
                    })?;
                    let high = !corner.outgoing.orientation ^ !binding.spatial_forward;
                    let t = proof.spatial_range()[usize::from(high)];
                    spatial_window = Some(proof.spatial_range());
                    let uv = chart_jet(&domain, corner.outgoing.coedge, [t; 2], work, cancel)?;
                    let jet = surface_enclosure(surface, uv_box(uv.position), work, cancel)?;
                    if let Some(chart) =
                        source_chart::SphereChart::new(surface, &domain, work, cancel)?
                    {
                        chart.normal(jet.position)
                    } else {
                        Ok(normal(&jet, domain.orientation()))
                    }
                })()
            }?;
            Ok(n)
        })();
        let n = evaluated.map_err(|error| work.annotate_error(error, before, format_args!(
            "source_frame incident normal source_face={} coedge={} spatial_parameter_bounds={spatial_window:?}",
            corner.outgoing.face, corner.outgoing.coedge,
        )))?;
        let representative = n.map(Interval::mid);
        let norm = representative
            .into_iter()
            .map(|v| v * v)
            .sum::<f64>()
            .sqrt();
        if !norm.is_finite() || norm <= 0.0 {
            return Ok(None);
        }
        for axis in 0..3 {
            candidate[axis] += representative[axis] / norm;
        }
    }
    let norm = candidate.into_iter().map(|v| v * v).sum::<f64>().sqrt();
    if !norm.is_finite() || norm <= 0.0 {
        return Ok(None);
    }
    let n = candidate.map(|v| Interval::point(v / norm));
    for axis in 0..3 {
        let e = std::array::from_fn(|i| Interval::point(if i == axis { 1.0 } else { 0.0 }));
        let u = cross(e, n);
        if length(u)?.lo > 0.0 {
            let rows = [u, cross(n, u)].map(|row| row.map(|v| Interval::point(v.mid())));
            if length(cross(rows[0], rows[1]))?.lo > 0.0 {
                return Ok(Some(rows));
            }
        }
    }
    Ok(None)
}

fn prepare_end(
    domain: &PreparedFaceTrimDomain<'_>,
    use_: CoedgeUse,
    incoming: bool,
    fraction: f64,
    reuse: Option<End>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<End>, GeometryError> {
    let before = work.consumed();
    let mut spatial_window = None;
    let mut spatial_endpoint_exit = None;
    let mut stage = "coedge correspondence";
    let result = (|| {
        let proof = domain.original_coedge_correspondence(use_.coedge)?;
        let Some(binding) = proof.binding() else {
            return Ok(None);
        };
        if binding.owner != [use_.face, use_.wire, use_.index] {
            return Ok(None);
        }
        let range = proof.spatial_range();
        spatial_window = Some(range);
        let high = (use_.orientation == incoming) ^ !binding.spatial_forward;
        let endpoint = range[usize::from(high)];
        let exit = endpoint
            + if high {
                -(range[1] - range[0]) * fraction
            } else {
                (range[1] - range[0]) * fraction
            };
        if exit <= range[0] || exit >= range[1] {
            return Ok(None);
        }
        let delta_t = Interval::point(exit).sub(Interval::point(endpoint));
        spatial_endpoint_exit = Some((endpoint, exit));
        stage = "end chart evaluation";
        let joint_uv = chart_jet(domain, use_.coedge, [endpoint; 2], work, cancel)?.position;
        stage = "end collar evaluation";
        let Some(collar) = OriginalCollar::for_enclosure(domain, use_, work, cancel)? else {
            return Ok(None);
        };
        stage = "end native spatial evaluation";
        let spatial = match reuse.filter(|prior| {
            prior.edge == binding.edge && prior.spatial_high == high && prior.exit == exit
        }) {
            Some(prior) => prior.spatial,
            None => {
                domain
                    .original_coedge_jets(use_.coedge, [exit; 2], work, cancel)?
                    .0
            }
        };
        stage = "end finite B evaluation";
        let boundary = collar.boundary_jet(exit, [0.0, 1.0], &spatial, work, cancel)?;
        Ok(Some(End {
            use_,
            edge: binding.edge,
            spatial_high: high,
            endpoint,
            exit,
            delta_t,
            joint_uv,
            offset: boundary.uv_offset,
            collar: !boundary.constant,
            spatial,
        }))
    })();
    result.map_err(|error| work.annotate_error(error, before, format_args!(
        "SourceVertexTransportPlan {stage} source_face={} coedge={} incoming={incoming} fraction={fraction} spatial_parameter_bounds={spatial_window:?} endpoint_exit={spatial_endpoint_exit:?}",
        use_.face, use_.coedge,
    )))
}

fn end_curve(
    domain: &PreparedFaceTrimDomain<'_>,
    end: End,
    s: Interval,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<crate::parametric::ChartCurveEnclosure, GeometryError> {
    let s = source_unit_parameter(s)?;
    let t = Interval::point(end.endpoint).add(end.delta_t.mul(s));
    let proof = domain.original_coedge_correspondence(end.use_.coedge)?;
    let range = proof.spatial_range();
    // The actual affine image of a proved unit coordinate lies between these
    // SAME literal source endpoints, independently of arithmetic roundoff.
    let image = Interval {
        lo: end.endpoint.min(end.exit),
        hi: end.endpoint.max(end.exit),
    };
    if image.lo < range[0] || image.hi > range[1] {
        return Err(refusal(
            "source conical endpoint recipe leaves its original spatial provider range",
        ));
    }
    let t = intersection(t, image)
        .ok_or_else(|| refusal("source conical side leaves its proved affine endpoint image"))?;
    let mut jet = chart_jet(domain, end.use_.coedge, [t.lo, t.hi], work, cancel)?;
    jet.derivative = jet.derivative.map(|v| v.mul(end.delta_t));
    jet.second_derivative = jet.second_derivative.map(|v| v.mul(end.delta_t.square()));
    Ok(jet)
}

/// Bound S(uv) - V with V folded into the original carrier coefficients.
/// This is an actual source evaluation, not a Taylor surrogate at the joint.
fn centered_source_point(
    surface: &(impl crate::parametric::SurfaceEvaluator + ?Sized),
    uv: Box2,
    point: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Box3, GeometryError> {
    use crate::parametric::{SurfaceFunctional, surface_functional_enclosures};
    let origin = point.map(Interval::point);
    let functionals = std::array::from_fn(|axis| SurfaceFunctional {
        direction: std::array::from_fn(|i| Interval::point(if i == axis { 1.0 } else { 0.0 })),
        origin,
        offset: Interval::point(0.0),
    });
    Ok(
        surface_functional_enclosures::<3>(surface, uv_box(uv), &functionals, work, cancel)?
            .map(|jet| jet.position),
    )
}

/// This is an image proof, not an endpoint/center test or reference-domain
/// ownership. Every possible original closure remains within the physical
/// chart error and retains the already selected source projection direction.
#[allow(clippy::too_many_arguments)]
fn whole_original_joint_image_fits(
    surface: &(impl crate::parametric::SurfaceEvaluator + ?Sized),
    uv: Box2,
    point: [f64; 3],
    projection_normal: Box3,
    orientation: bool,
    sphere_chart: Option<&source_chart::SphereChart>,
    allowance: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let jet =
        crate::parametric::surface_piecewise_first_enclosure(surface, uv_box(uv), work, cancel)?;
    let normal = if let Some(chart) = sphere_chart {
        // Already includes face and processor orientation. This geometric
        // area/gradient field stays nonzero at a collapsed original UV pole;
        // it is not Euclidean-normalized and must not be flipped again.
        chart.normal(jet.position)?
    } else {
        let normal = cross(jet.du, jet.dv);
        if orientation {
            normal
        } else {
            normal.map(Interval::neg)
        }
    };
    if dot(normal, projection_normal).lo <= 0.0 {
        return Ok(false);
    }
    let residual = centered_source_point(surface, uv, point, work, cancel)?;
    Ok(length(residual)?.hi <= allowance)
}

/// Actual G(1/2) - S(H(1/2)) for a fixed source generator. Even a tight
/// enclosure can only refute an allowance: it proves nothing about the rest
/// of the cone, its source domain, determinant, or winding.
fn source_generator_displacement(
    surface: &(impl crate::parametric::SurfaceEvaluator + ?Sized),
    boundary_uv: Box2,
    halfway_uv: Box2,
    point: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Box3, GeometryError> {
    let boundary = centered_source_point(surface, boundary_uv, point, work, cancel)?;
    let halfway = centered_source_point(surface, halfway_uv, point, work, cancel)?;
    Ok(std::array::from_fn(|i| {
        Interval::point(0.5).mul(boundary[i]).sub(halfway[i])
    }))
}

/// The middle-piece theta endpoints are the outgoing/incoming full-width
/// ribbons, independent of bulge. Their original UV recipes are
/// B = S(exit_uv + offset), H(s) = end_uv(s) + s * offset.
/// A validated lower norm above the unchanged physical allowance disproves
/// this entire radius. A non-refutation NEVER accepts a candidate.
fn source_radius_refuted(
    domain: &PreparedFaceTrimDomain<'_>,
    end: End,
    point: [f64; 3],
    allowance: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let before = work.consumed();
    let result = (|| {
        work.charge(1, cancel)?;
        let boundary = end_curve(domain, end, Interval::point(1.0), work, cancel)?.position;
        let halfway = end_curve(domain, end, Interval::point(0.5), work, cancel)?.position;
        let boundary_uv = std::array::from_fn(|i| boundary[i].add(end.offset[i]));
        let halfway_uv =
            std::array::from_fn(|i| halfway[i].add(Interval::point(0.5).mul(end.offset[i])));
        {
            let surface = &domain.surface_operand();
            (|| {
                let displacement = source_generator_displacement(
                    surface,
                    boundary_uv,
                    halfway_uv,
                    point,
                    work,
                    cancel,
                )?;
                Ok(length(displacement)?.lo > allowance)
            })()
        }
    })();
    result.map_err(|error| work.annotate_error(error, before, format_args!(
        "SourceVertexTransportPlan actual source generator radius refutation vertex_mm={point:?} source_face={} coedge={} spatial_endpoint_exit={:?} admitted_source_error_mm={allowance}",
        end.use_.face, end.use_.coedge, (end.endpoint, end.exit),
    )))
}

/// Direct comparison with the ORIGINAL shared three-dimensional edge. The
/// actual cone germ is V+s(C(exit)-V), not an invented carrier intersection.
/// Linear interpolation's C2 remainder plus the literal NativeV residual
/// bounds the complete edge in BOTH directions under the same parameter.
fn original_endpoint_chord_error(
    definition: &Definition,
    domain: &PreparedFaceTrimDomain<'_>,
    end: End,
    point: [f64; 3],
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    work.charge(1, cancel)?;
    let proof = domain.original_coedge_correspondence(end.use_.coedge)?;
    let binding = proof
        .binding()
        .ok_or_else(|| refusal("original shared-edge chord lost its source binding"))?;
    let (entry, edge) = crate::analysis::source_incidence::native_coedge(
        definition,
        end.use_.face,
        end.use_.wire,
        end.use_.index,
    )?;
    if entry.edge != end.edge
        || entry.edge != binding.edge
        || edge.vertices != binding.endpoints
        || [edge.interval.min(), edge.interval.max()] != binding.native_interval
    {
        return Err(refusal("original shared-edge chord operands changed"));
    }
    let span = [end.endpoint.min(end.exit), end.endpoint.max(end.exit)];
    proof.uv_parameter_image(span)?;
    domain.with_original_spatial_curve(
        end.use_.coedge,
        remaining_bytes,
        work,
        cancel,
        |source, work, cancel| {
            // Both queries use the verified original spatial provider gauge,
            // never a native rational parameter at an original radian value.
            let endpoint = crate::parametric::curve_enclosure(
                source,
                (end.endpoint, end.endpoint),
                work,
                cancel,
            )?;
            let whole =
                crate::parametric::curve_enclosure(source, (span[0], span[1]), work, cancel)?;
            endpoint_chord_error_bound(
                point,
                endpoint.position,
                whole.second_derivative,
                end.delta_t,
            )
        },
    )
}

fn endpoint_chord_error_bound(
    point: [f64; 3],
    endpoint: Box3,
    second_derivative: Box3,
    delta: Interval,
) -> Result<f64, GeometryError> {
    let residual = length(relative(endpoint, point))?;
    let curvature = length(second_derivative)?;
    let error = residual.add(curvature.mul(delta.square()).mul(Interval::point(0.125)));
    if !error.hi.is_finite() {
        return Err(refusal(
            "original shared-edge chord has no finite source error bound",
        ));
    }
    Ok(error.hi)
}

/// Strengthen the SOURCE comparison only AFTER geometry/radius selection.
/// Failure here refuses that fixed M; it never selects a query-specific finer
/// radius. Every support and downstream error consumer sees the charged cost.
#[allow(clippy::too_many_arguments)]
fn qualify_fixed_source_costs(
    source: MaterialSourceView<'_, '_>,
    vertex: VertexId,
    point: [f64; 3],
    allowance: f64,
    remaining_bytes: usize,
    recipes: &mut [CornerRecipe],
    cells: &mut [ConeCell],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let available = remaining_bytes;
    let mut edge_error_mm = 0.0_f64;
    for recipe in recipes.iter() {
        let domain = source.face_domain(recipe.outgoing.use_.face, available, work, cancel)?;
        let scratch = available;
        // Each outgoing germ occurs once in the complete native corner cycle;
        // its opposed incoming use was checked against the SAME literal exit.
        edge_error_mm = edge_error_mm.max(original_endpoint_chord_error(
            source.definition(),
            &domain,
            recipe.outgoing,
            point,
            scratch,
            work,
            cancel,
        )?);
    }
    if edge_error_mm > allowance {
        return Err(refusal(format!(
            "fixed source realization shared-edge chord exceeds admitted source error: vertex_mm={point:?} bound_mm={edge_error_mm} admitted_mm={allowance}"
        )));
    }
    for (index, recipe) in recipes.iter_mut().enumerate() {
        let domain = source.face_domain(recipe.outgoing.use_.face, available, work, cancel)?;
        let owned_scratch = available;
        match OriginalCornerComparison::prepare(
            &domain,
            recipe,
            vertex,
            allowance,
            owned_scratch,
            work,
            cancel,
        )? {
            UvComparisonPreparation::Prepared(comparison) => {
                recipe.comparison = Some(comparison);
                recipe.comparison_refusal = None;
            }
            UvComparisonPreparation::Refused(reason) => {
                recipe.comparison = None;
                recipe.comparison_refusal = Some(reason.stage);
            }
        }
        for cell in cells.iter_mut().filter(|cell| cell.recipe == index) {
            work.charge(1, cancel)?;
            cell.error_mm = cell.error_mm.max(edge_error_mm);
            if let Some(comparison) = &recipe.comparison {
                if comparison.is_identity() {
                    // This is exact original-operand identity, not a small
                    // shift threshold: the already proved source cost is reused.
                    continue;
                }
                let cost = comparison.source_cost(
                    &domain,
                    recipe,
                    cell.piece,
                    Interval {
                        lo: cell.parameter[0],
                        hi: cell.parameter[1],
                    },
                    point,
                    work,
                    cancel,
                )?;
                if !cost.homotopy_mm.is_finite()
                    || cost.homotopy_mm < 0.0
                    || !cost.error_mm.is_finite()
                    || cost.error_mm > allowance
                {
                    return Err(refusal(format!(
                        "fixed source realization completed UV comparison exceeds admitted source error: vertex_mm={point:?} source_face={} bound_mm={} completion_mm={} admitted_mm={allowance}",
                        recipe.outgoing.use_.face, cost.error_mm, cost.homotopy_mm,
                    )));
                }
                cell.source = hull3(cell.source, cost.source);
                cell.displacement = hull3(cell.displacement, cost.displacement);
                cell.error_mm = cell.error_mm.max(cost.error_mm);
            }
        }
    }
    Ok(())
}

fn star_jet(
    domain: &PreparedFaceTrimDomain<'_>,
    a: End,
    b: End,
    piece: Piece,
    s: Interval,
    t: Interval,
    bulge: Box2,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<StarJet, GeometryError> {
    let s = source_unit_parameter(s)?;
    let t = source_unit_parameter(t)?;
    if !matches!(piece, Piece::Middle) {
        let end = if matches!(piece, Piece::Outgoing) {
            a
        } else {
            b
        };
        let w = if matches!(piece, Piece::Outgoing) {
            t
        } else {
            source_unit_complement(t)?
        };
        let edge = end_curve(domain, end, s, work, cancel)?;
        return Ok(StarJet {
            uv: std::array::from_fn(|i| edge.position[i].add(s.mul(w).mul(end.offset[i]))),
            ds: std::array::from_fn(|i| edge.derivative[i].add(w.mul(end.offset[i]))),
            dss: edge.second_derivative,
        });
    }
    let e0 = end_curve(domain, a, s, work, cancel)?;
    let e1 = end_curve(domain, b, s, work, cancel)?;
    let k = mix(a.offset, b.offset, t);
    let bubble = t.mul(source_unit_complement(t)?);
    let uv = mix(e0.position, e1.position, t);
    let ds = mix(e0.derivative, e1.derivative, t);
    Ok(StarJet {
        uv: std::array::from_fn(|i| uv[i].add(s.mul(k[i].add(bubble.mul(bulge[i]))))),
        ds: std::array::from_fn(|i| ds[i].add(k[i]).add(bubble.mul(bulge[i]))),
        dss: mix(e0.second_derivative, e1.second_derivative, t),
    })
}

fn boundary_jet(
    domain: &PreparedFaceTrimDomain<'_>,
    a: End,
    b: End,
    piece: Piece,
    t: Interval,
    bulge: Box2,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<BoundaryJet, GeometryError> {
    boundary_jet_with_byte_limit(domain, a, b, piece, t, bulge, usize::MAX, work, cancel)
}

#[allow(clippy::too_many_arguments)]
fn boundary_jet_with_byte_limit(
    domain: &PreparedFaceTrimDomain<'_>,
    a: End,
    b: End,
    piece: Piece,
    t: Interval,
    bulge: Box2,
    metadata_inclusive_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<BoundaryJet, GeometryError> {
    let t = source_unit_parameter(t)?;
    if matches!(piece, Piece::Middle) {
        let p0 = chart_jet(domain, a.use_.coedge, [a.exit; 2], work, cancel)?.position;
        let p1 = chart_jet(domain, b.use_.coedge, [b.exit; 2], work, cancel)?.position;
        let u0: Box2 = std::array::from_fn(|i| p0[i].add(a.offset[i]));
        let u1: Box2 = std::array::from_fn(|i| p1[i].add(b.offset[i]));
        let blend = mix(u0, u1, t);
        let bubble = t.mul(source_unit_complement(t)?);
        let uv = std::array::from_fn(|i| blend[i].add(bubble.mul(bulge[i])));
        let derivative_uv = std::array::from_fn(|i| {
            u1[i].sub(u0[i]).add(
                Interval::point(1.0)
                    .sub(Interval::point(2.0).mul(t))
                    .mul(bulge[i]),
            )
        });
        return {
            let surface = &domain.surface_operand();
            (|| {
                let jet = surface_enclosure(surface, uv_box(uv), work, cancel)?;
                Ok(BoundaryJet {
                    position: jet.position,
                    derivative: compose_first(&jet.du, &jet.dv, derivative_uv),
                    displacement: [Interval::point(0.0); 3],
                    uv,
                })
            })()
        };
    }
    let end = if matches!(piece, Piece::Outgoing) {
        a
    } else {
        b
    };
    let w = if matches!(piece, Piece::Outgoing) {
        t
    } else {
        source_unit_complement(t)?
    };
    let Some(collar) = OriginalCollar::for_enclosure_with_byte_limit(
        domain,
        end.use_,
        metadata_inclusive_bytes,
        work,
        cancel,
    )?
    else {
        return Err(refusal(
            "source conical boundary lost its retained collar recipe",
        ));
    };
    let jet = collar.boundary_jet(end.exit, [w.lo, w.hi], &end.spatial, work, cancel)?;
    let weight = source_unit_complement(w)?;
    Ok(BoundaryJet {
        position: jet.position,
        derivative: if matches!(piece, Piece::Outgoing) {
            jet.derivative
        } else {
            jet.derivative.map(Interval::neg)
        },
        displacement: jet.boundary_displacement.map(|v| weight.mul(v)),
        uv: jet.uv,
    })
}

fn endpoint_support_needs_refinement(
    support: &SourceTransportSupport,
    reference: [f64; 2],
    tolerance: f64,
) -> bool {
    let base = support.base_interval;
    let corrected = support.corrected_interval;
    let displacement = support.displacement_interval;
    // A genuinely transported endpoint need not have zero displacement. Once
    // both images are locally resolved, final aggregation decides the policy.
    if up(base[1] - base[0]) <= tolerance && up(corrected[1] - corrected[0]) <= tolerance {
        return false;
    }
    // A complete-G witness may discharge only inward loss of a raw feasible
    // endpoint. It does not shrink BASE/G or excuse actual outward movement.
    let minimum_retained = support
        .realized_witness_interval
        .is_some_and(|witness| up(witness[1] - reference[0]) <= tolerance);
    let maximum_retained = support
        .realized_witness_interval
        .is_some_and(|witness| up(reference[1] - witness[0]) <= tolerance);
    (base[0] <= reference[0] && displacement[1] > tolerance && !minimum_retained)
        || (base[1] >= reference[1] && displacement[0] < -tolerance && !maximum_retained)
        || (corrected[0] < reference[0] - tolerance * 2.0 && displacement[0] < -tolerance)
        || (corrected[1] > reference[1] + tolerance * 2.0 && displacement[1] > tolerance)
}

#[allow(clippy::too_many_arguments)]
fn cone_boundary_scalar(
    domain: &PreparedFaceTrimDomain<'_>,
    recipe: &CornerRecipe,
    piece: Piece,
    range: [f64; 2],
    direction: Box3,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(Interval, Interval), GeometryError> {
    let classifier_bytes = remaining_bytes;
    let boundary = boundary_jet_with_byte_limit(
        domain,
        recipe.outgoing,
        recipe.incoming,
        piece,
        Interval::new(range[0], range[1])?,
        recipe.bulge,
        classifier_bytes,
        work,
        cancel,
    )?;
    {
        let surface = &domain.surface_operand();
        (|| {
            // Fold the covector into native coefficients before evaluating the
            // scalar carrier. The displacement is the literal boundary recipe,
            // not a global physical OriginalF/G error or a padding allowance.
            let scalar = super::transport::scalar_surface_jet(
                surface,
                uv_box(boundary.uv),
                direction,
                work,
                cancel,
            )?
            .0
            .add(dot(direction, boundary.displacement));
            let height = intersection(dot(direction, boundary.position), scalar)
                .ok_or_else(|| refusal("source cone boundary scalar enclosures disagree"))?;
            Ok((height, dot(direction, boundary.derivative)))
        })()
    }
}

#[allow(clippy::too_many_arguments)]
fn cover_piece(
    domain: &PreparedFaceTrimDomain<'_>,
    a: End,
    b: End,
    recipe: usize,
    piece: Piece,
    bulge: Box2,
    point: [f64; 3],
    rows: [Box3; 2],
    allowance: f64,
    scratch: usize,
    cells: &mut Vec<ConeCell>,
    rays: &mut Vec<Box2>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let before = work.consumed();
    let mut parameter_window = [0.0, 1.0];
    let mut stage = "finite B subdivision";
    let result = (|| {
        let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
        stack[0] = ([0.0, 1.0], 0);
        let mut pending = 1;
        while pending != 0 {
            pending -= 1;
            let (range, depth) = stack[pending];
            parameter_window = range;
            stage = "finite B boundary evaluation";
            work.charge(1, cancel)?;
            let t = Interval {
                lo: range[0],
                hi: range[1],
            };
            let boundary = boundary_jet(domain, a, b, piece, t, bulge, work, cancel)?;
            let ray = relative(boundary.position, point);
            let n = cross(ray, boundary.derivative);
            let projected_box = projected(ray, rows);
            let orientation = determinant(projected_box, projected(boundary.derivative, rows));
            let mut candidate = None;
            if orientation.lo > 0.0
                && !(projected_box[0].contains_zero() && projected_box[1].contains_zero())
            {
                stage = "finite B star evaluation";
                let h = star_jet(
                    domain,
                    a,
                    b,
                    piece,
                    Interval { lo: 0.0, hi: 1.0 },
                    t,
                    bulge,
                    work,
                    cancel,
                )?;
                let j = star_jet(
                    domain,
                    a,
                    b,
                    piece,
                    Interval::point(0.0),
                    t,
                    bulge,
                    work,
                    cancel,
                )?;
                stage = "finite B carrier and radius evaluation";
                candidate = {
                    let surface = &domain.surface_operand();
                    (|| {
                        let jet = surface_enclosure(surface, uv_box(h.uv), work, cancel)?;
                        let at_joint =
                            surface_enclosure(surface, uv_box(j.uv), work, cancel)?.position;
                        let d0 = relative(at_joint, point).map(Interval::neg);
                        let acceleration: Box3 = std::array::from_fn(|i| {
                            jet.duu[i]
                                .mul(h.ds[0].square())
                                .add(
                                    Interval::point(2.0)
                                        .mul(jet.duv[i])
                                        .mul(h.ds[0])
                                        .mul(h.ds[1]),
                                )
                                .add(jet.dvv[i].mul(h.ds[1].square()))
                                .add(jet.du[i].mul(h.dss[0]))
                                .add(jet.dv[i].mul(h.dss[1]))
                        });
                        let displacement = std::array::from_fn(|i| {
                            let remainder = acceleration[i].abs_upper() * 0.125;
                            d0[i].hull(boundary.displacement[i]).add(Interval {
                                lo: -up(remainder),
                                hi: up(remainder),
                            })
                        });
                        let radius_before = work.consumed();
                        let error_mm = length(displacement).map_err(|error| work.annotate_error(
                    error, radius_before, format_args!("SourceVertexTransportPlan physical radius evaluation vertex_mm={point:?} source_face={} coedges={:?} parameter_window={range:?}", a.use_.face, [a.use_.coedge, b.use_.coedge]),
                ))?.hi;
                        if !error_mm.is_finite() || error_mm > allowance {
                            return Ok(None);
                        }
                        Ok(Some(ConeCell {
                            source: jet.position,
                            realized: hull3(point.map(Interval::point), boundary.position),
                            displacement,
                            normal: n,
                            error_mm,
                            recipe,
                            piece,
                            parameter: range,
                        }))
                    })()
                }?;
            }
            if let Some(cell) = candidate {
                stage = "finite B endpoint evaluation";
                let endpoint = boundary_jet(
                    domain,
                    a,
                    b,
                    piece,
                    Interval::point(range[0]),
                    bulge,
                    work,
                    cancel,
                )?;
                let needed = cells
                    .capacity()
                    .saturating_add(1)
                    .saturating_mul(std::mem::size_of::<ConeCell>())
                    .saturating_add(
                        rays.capacity()
                            .saturating_add(1)
                            .saturating_mul(std::mem::size_of::<Box2>()),
                    );
                if needed > scratch {
                    return Err(GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "source conical boundary subdivision exceeds scratch allowance",
                    ));
                }
                cells.try_reserve_exact(1).map_err(|_| {
                    GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "source conical cell allocation refused",
                    )
                })?;
                rays.try_reserve_exact(1).map_err(|_| {
                    GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "source conical winding allocation refused",
                    )
                })?;
                cells.push(cell);
                rays.push(projected(relative(endpoint.position, point), rows));
                continue;
            }
            let mid = t.mid();
            if depth >= MAX_DEPTH || mid <= t.lo || mid >= t.hi {
                return Ok(false);
            }
            stack[pending] = ([mid, t.hi], depth + 1);
            stack[pending + 1] = ([t.lo, mid], depth + 1);
            pending += 2;
        }
        Ok(true)
    })();
    result.map_err(|error| work.annotate_error(error, before, format_args!(
        "SourceVertexTransportPlan {stage} vertex_mm={point:?} source_face={} coedges={:?} spatial_endpoint_exit={:?} parameter_window={parameter_window:?} piece={}",
        a.use_.face, [a.use_.coedge, b.use_.coedge], [(a.endpoint, a.exit), (b.endpoint, b.exit)],
        match piece { Piece::Outgoing => "outgoing", Piece::Middle => "middle", Piece::Incoming => "incoming" },
    )))
}

fn separate_nonincident(
    source: MaterialSourceView<'_, '_>,
    corners: &[CornerRecipe],
    support: Box3,
    scratch: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for index in 0..source.definition().face_ids().len() {
        work.charge(1, cancel)?;
        if corners
            .iter()
            .any(|corner| corner.outgoing.use_.face == index)
        {
            continue;
        }
        let domain = source.face_domain(index, scratch, work, cancel)?;
        let separated = {
            let surface = &domain.surface_operand();
            (|| {
                for span in surface_parameter_boxes(surface, domain.uv_domain(), work, cancel)? {
                    let mut stack = [(span, 0_u8); MAX_DEPTH as usize + 1];
                    let mut pending = 1;
                    while pending != 0 {
                        pending -= 1;
                        let (uv, depth) = stack[pending];
                        work.charge(1, cancel)?;
                        // Exclude the complete carrier before classifying its
                        // trimmed subset. Only position is needed, including at
                        // continuous native knot interfaces.
                        let position = centered_source_point(
                            surface,
                            uv.map(|[lo, hi]| Interval { lo, hi }),
                            [0.0; 3],
                            work,
                            cancel,
                        )?;
                        if disjoint(position, support)
                            || domain.classify(uv, work, cancel)? == TrimClassification::Outside
                        {
                            continue;
                        }
                        if depth >= MAX_DEPTH {
                            return Ok(false);
                        }
                        let children = split(uv, depth, index)?;
                        for child in children {
                            if pending >= stack.len() {
                                return Err(GeometryError::new(
                                    GeometryErrorCode::ResourceLimit,
                                    "source vertex separation stack exhausted",
                                ));
                            }
                            stack[pending] = (child, depth + 1);
                            pending += 1;
                        }
                    }
                }
                Ok(true)
            })()
        }?;
        if !separated {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Clone, Copy)]
struct UvPoint {
    coedge: usize,
    high: bool,
}

#[derive(Clone, Copy)]
struct TrimLine {
    owner: Option<usize>,
    a: UvPoint,
    b: UvPoint,
    origin: Box2,
    direction: Box2,
    sign: f64,
}

fn uv_point(
    domain: &PreparedFaceTrimDomain<'_>,
    point: UvPoint,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Box2, GeometryError> {
    let t = domain
        .original_coedge_correspondence(point.coedge)?
        .spatial_range()[usize::from(point.high)];
    Ok(chart_jet(domain, point.coedge, [t; 2], work, cancel)?.position)
}

fn same_uv_point(
    domain: &PreparedFaceTrimDomain<'_>,
    a: UvPoint,
    b: UvPoint,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if a.coedge == b.coedge && a.high == b.high {
        return Ok(true);
    }
    let pa = domain.original_coedge_correspondence(a.coedge)?;
    let pb = domain.original_coedge_correspondence(b.coedge)?;
    super::vertex_fan::original_uv_endpoints_equal(
        &domain.coedge_operand(a.coedge)?,
        pa.uv_endpoint(a.high),
        &domain.coedge_operand(b.coedge)?,
        pb.uv_endpoint(b.high),
        work,
        cancel,
    )
}

fn line_value(
    domain: &PreparedFaceTrimDomain<'_>,
    line: TrimLine,
    point: UvPoint,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Interval, GeometryError> {
    if line.owner == Some(point.coedge)
        || same_uv_point(domain, point, line.a, work, cancel)?
        || same_uv_point(domain, point, line.b, work, cancel)?
    {
        return Ok(Interval::point(0.0));
    }
    let position = uv_point(domain, point, work, cancel)?;
    Ok(determinant(
        line.direction,
        std::array::from_fn(|i| position[i].sub(line.origin[i])),
    )
    .mul(Interval::point(line.sign)))
}

fn end_line_values(
    domain: &PreparedFaceTrimDomain<'_>,
    line: TrimLine,
    end: End,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(Interval, Interval, Interval), GeometryError> {
    let range = domain
        .original_coedge_correspondence(end.use_.coedge)?
        .spatial_range();
    let j = line_value(
        domain,
        line,
        UvPoint {
            coedge: end.use_.coedge,
            high: end.endpoint == range[1],
        },
        work,
        cancel,
    )?;
    let delta = if line.owner == Some(end.use_.coedge) {
        Interval::point(0.0)
    } else {
        let edge = end_curve(domain, end, Interval { lo: 0.0, hi: 1.0 }, work, cancel)?;
        determinant(line.direction, edge.derivative).mul(Interval::point(line.sign))
    };
    let offset = if line.owner == Some(end.use_.coedge) && end.collar {
        let Some(collar) = OriginalCollar::for_enclosure(domain, end.use_, work, cancel)? else {
            return Err(refusal("source affine corner lost its collar recipe"));
        };
        let CarrierTransport::Collar { width, inward } = collar.transport else {
            return Err(refusal("source affine collar changed its transport kind"));
        };
        let d = chart_jet(domain, end.use_.coedge, [end.exit; 2], work, cancel)?.derivative;
        let beta = collar.bubble([end.exit; 2])?.0;
        // det(d,J_inward d)=-inward |d|² for rotate(d)=(dy,-dx).
        d[0].square()
            .add(d[1].square())
            .mul(Interval::point(-inward * line.sign * width))
            .mul(beta)
    } else {
        determinant(line.direction, end.offset).mul(Interval::point(line.sign))
    };
    Ok((j, j.add(delta), j.add(delta).add(offset)))
}

/// Closed-set source ownership for an actual coefficient-affine convex wire.
/// Every original connector is checked; neither an interval endpoint overlap
/// nor a joint's bounding rectangle is treated as an interior-domain receipt.
fn convex_affine_star_inside(
    domain: &PreparedFaceTrimDomain<'_>,
    a: End,
    b: End,
    bulge: Box2,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let anchor = star_jet(
        domain,
        a,
        b,
        Piece::Middle,
        Interval::point(0.5),
        Interval::point(0.5),
        bulge,
        work,
        cancel,
    )?
    .uv;
    if domain.classify(uv_box(anchor), work, cancel)? != TrimClassification::Inside {
        return Ok(false);
    }
    let wire_index = a.use_.wire;
    if b.use_.wire != wire_index {
        return Ok(false);
    }
    let coedges = domain.coedges();
    for (index, coedge) in coedges.iter().enumerate() {
        if coedge.identity().wire_index != wire_index {
            continue;
        }
        work.charge(1, cancel)?;
        if !super::vertex_fan::original_uv_affine(&domain.coedge_operand(index)?, work, cancel)? {
            return Ok(false);
        }
        let range = domain
            .original_coedge_correspondence(index)?
            .spatial_range();
        let jet = chart_jet(domain, index, range, work, cancel)?;
        let origin = uv_point(
            domain,
            UvPoint {
                coedge: index,
                high: false,
            },
            work,
            cancel,
        )?;
        let anchor_side = determinant(
            jet.derivative,
            std::array::from_fn(|i| anchor[i].sub(origin[i])),
        );
        let sign = if anchor_side.lo > 0.0 {
            1.0
        } else if anchor_side.hi < 0.0 {
            -1.0
        } else {
            return Ok(false);
        };
        let line = TrimLine {
            owner: Some(index),
            a: UvPoint {
                coedge: index,
                high: false,
            },
            b: UvPoint {
                coedge: index,
                high: true,
            },
            origin,
            direction: jet.derivative,
            sign,
        };
        if !validate_trim_line(domain, line, wire_index, a, b, bulge, work, cancel)? {
            return Ok(false);
        }
    }
    // Complete the original UV boundary with the retained ORIGINAL connector
    // at every native corner, including the actual polar quotient connector.
    let wire = crate::analysis::source_incidence::boundary_loop(
        domain.definition(),
        domain.face_index(),
        wire_index,
    )?;
    for index in 0..wire.edges.len() {
        work.charge(1, cancel)?;
        let next = (index + 1) % wire.edges.len();
        let flat = a.use_.coedge - a.use_.index;
        let incoming = if domain.native_orientation() {
            flat + index
        } else {
            flat + next
        };
        let outgoing = if domain.native_orientation() {
            flat + next
        } else {
            flat + index
        };
        let incoming_binding = domain
            .original_coedge_correspondence(incoming)?
            .binding()
            .ok_or_else(|| refusal("original connector lost its incoming source endpoint map"))?;
        let outgoing_binding = domain
            .original_coedge_correspondence(outgoing)?
            .binding()
            .ok_or_else(|| refusal("original connector lost its outgoing source endpoint map"))?;
        let p0 = UvPoint {
            coedge: incoming,
            high: (coedges[incoming].forward == domain.native_orientation())
                ^ !incoming_binding.spatial_forward,
        };
        let p1 = UvPoint {
            coedge: outgoing,
            high: (coedges[outgoing].forward != domain.native_orientation())
                ^ !outgoing_binding.spatial_forward,
        };
        if same_uv_point(domain, p0, p1, work, cancel)? {
            continue;
        }
        let origin = uv_point(domain, p0, work, cancel)?;
        let end = uv_point(domain, p1, work, cancel)?;
        let mut retained = false;
        for joint in domain.original_joints()? {
            work.charge(1, cancel)?;
            if joint.incoming == coedges[incoming].identity()
                && joint.outgoing == coedges[outgoing].identity()
                && joint.vertex_id
                    == domain
                        .definition()
                        .brep()
                        .edge(wire.edges[index].edge)
                        .map_err(|error| refusal(format!("native connector edge: {error}")))?
                        .vertices[usize::from(wire.edges[index].forward)]
                && contains(joint.enclosure, hull2(origin, end))
            {
                if !matches!(joint.kind, TrimJointKind::SphericalPole { .. }) {
                    // Ordinary source corners require the coefficient endpoint
                    // identity above. A rectangle does not invent a connector.
                    continue;
                }
                if source_chart::SphereChart::new(&domain.surface_operand(), domain, work, cancel)?
                    .is_none()
                {
                    return Ok(false);
                }
                // The opaque joint binds these SAME original endpoint
                // expressions and directed source maps by construction.
                // Its interval endpoint tubes are not literal PCurve controls.
                let forward = joint
                    .connector_endpoint_is_bound(domain, 0, incoming, p0.high, work, cancel)?
                    && joint
                        .connector_endpoint_is_bound(domain, 1, outgoing, p1.high, work, cancel)?;
                let reverse = !forward
                    && joint
                        .connector_endpoint_is_bound(domain, 1, incoming, p0.high, work, cancel)?
                    && joint
                        .connector_endpoint_is_bound(domain, 0, outgoing, p1.high, work, cancel)?;
                if !forward && !reverse {
                    // Canonical pole endpoint legs need an actual source-domain
                    // coverage proof; enclosure overlap is not that proof.
                    return Ok(false);
                }
                if retained {
                    return Ok(false);
                }
                retained = true;
            }
        }
        if !retained {
            return Ok(false);
        }
        let direction = std::array::from_fn(|i| end[i].sub(origin[i]));
        let side = determinant(direction, std::array::from_fn(|i| anchor[i].sub(origin[i])));
        let sign = if side.lo > 0.0 {
            1.0
        } else if side.hi < 0.0 {
            -1.0
        } else {
            return Ok(false);
        };
        let line = TrimLine {
            owner: None,
            a: p0,
            b: p1,
            origin,
            direction,
            sign,
        };
        if !validate_trim_line(domain, line, wire_index, a, b, bulge, work, cancel)? {
            return Ok(false);
        }
    }
    // Other wires cannot pass through the owned star. The inside anchor fixes
    // material rather than hole ownership; every boundary is inventoried.
    let uv = hull2(
        hull2(a.joint_uv, b.joint_uv),
        star_jet(
            domain,
            a,
            b,
            Piece::Middle,
            Interval { lo: 0.0, hi: 1.0 },
            Interval { lo: 0.0, hi: 1.0 },
            bulge,
            work,
            cancel,
        )?
        .uv,
    );
    for (index, coedge) in coedges.iter().enumerate() {
        if coedge.identity().wire_index == wire_index {
            continue;
        }
        if !trim_span_misses(domain, index, uv, work, cancel)? {
            return Ok(false);
        }
    }
    for joint in domain.original_joints()? {
        work.charge(1, cancel)?;
        if joint.incoming.wire_index == wire_index {
            continue;
        }
        if !disjoint(uv, joint.enclosure) {
            return Ok(false);
        }
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn validate_trim_line(
    domain: &PreparedFaceTrimDomain<'_>,
    line: TrimLine,
    wire: usize,
    a: End,
    b: End,
    bulge: Box2,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for (index, coedge) in domain.coedges().iter().enumerate() {
        if coedge.identity().wire_index != wire {
            continue;
        }
        for high in [false, true] {
            work.charge(1, cancel)?;
            if line_value(
                domain,
                line,
                UvPoint {
                    coedge: index,
                    high,
                },
                work,
                cancel,
            )?
            .lo < 0.0
            {
                return Ok(false);
            }
        }
    }
    let (ja, ea, ia) = end_line_values(domain, line, a, work, cancel)?;
    let (jb, eb, ib) = end_line_values(domain, line, b, work, cancel)?;
    if [ja, ea, ia, jb, eb, ib].into_iter().any(|v| v.lo < 0.0) {
        return Ok(false);
    }
    let bend = determinant(line.direction, bulge).mul(Interval::point(line.sign));
    let minimum = ia.lo.min(ib.lo);
    if bend.lo < 0.0
        && Interval::point(minimum)
            .add(bend.mul(Interval::point(0.25)))
            .lo
            < 0.0
    {
        return Ok(false);
    }
    Ok(true)
}

fn trim_span_misses(
    domain: &PreparedFaceTrimDomain<'_>,
    index: usize,
    uv: Box2,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for span in domain.boundary_parameter_intervals(index)? {
        let mut stack = [(span, 0_u8); MAX_DEPTH as usize + 1];
        let mut pending = 1;
        while pending != 0 {
            pending -= 1;
            let (range, depth) = stack[pending];
            work.charge(1, cancel)?;
            let bounds = domain
                .boundary_position_enclosure(index, range, work, cancel)?
                .map(|v| Interval { lo: v[0], hi: v[1] });
            if disjoint(uv, bounds) {
                continue;
            }
            let mid = range[0] * 0.5 + range[1] * 0.5;
            if depth >= MAX_DEPTH || mid <= range[0] || mid >= range[1] {
                return Ok(false);
            }
            stack[pending] = ([mid, range[1]], depth + 1);
            stack[pending + 1] = ([range[0], mid], depth + 1);
            pending += 2;
        }
    }
    Ok(true)
}

fn affine_bridge_exits_collar(
    domain: &PreparedFaceTrimDomain<'_>,
    end: End,
    other: End,
    bulge: Box2,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if !end.collar {
        return Ok(true);
    }
    if !super::vertex_fan::original_uv_affine(
        &domain.coedge_operand(end.use_.coedge)?,
        work,
        cancel,
    )? {
        return Ok(false);
    }
    let Some(collar) = OriginalCollar::for_enclosure(domain, end.use_, work, cancel)? else {
        return Ok(false);
    };
    let CarrierTransport::Collar { width, inward } = collar.transport else {
        return Ok(false);
    };
    let at = chart_jet(domain, end.use_.coedge, [end.exit; 2], work, cancel)?;
    let there = chart_jet(domain, other.use_.coedge, [other.exit; 2], work, cancel)?;
    let delta: Box2 = std::array::from_fn(|i| {
        there.position[i]
            .add(other.offset[i])
            .sub(at.position[i].add(end.offset[i]))
    });
    let drift: Box2 =
        std::array::from_fn(|i| delta[i].add(Interval { lo: 0.0, hi: 1.0 }.mul(bulge[i])));
    let speed2 = at.derivative[0].square().add(at.derivative[1].square());
    if speed2.lo <= 0.0 {
        return Ok(false);
    }
    let native_rate = at.derivative[0]
        .mul(drift[0])
        .add(at.derivative[1].mul(drift[1]))
        .div(speed2)?;
    let inward = rotate(at.derivative, inward);
    let transverse_rate = inward[0]
        .mul(drift[0])
        .add(inward[1].mul(drift[1]))
        .div(speed2)?;
    let beta_t = collar.bubble([end.exit; 2])?.1;
    // The omitted quadratic term is width*theta*(native_rate/span)^2 >= 0.
    // The actual gap from the fixed inner collar boundary is theta times
    // this bracket, so equality at the intended join is an identity, not an
    // endpoint epsilon or a discarded unresolved parameter.
    Ok(transverse_rate
        .sub(Interval::point(width).mul(beta_t).mul(native_rate))
        .lo
        > 0.0)
}

fn collar_graph_transverse(
    domain: &PreparedFaceTrimDomain<'_>,
    end: End,
    rows: [Box3; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if !end.collar {
        return Ok(true);
    }
    let Some(collar) = OriginalCollar::for_enclosure(domain, end.use_, work, cancel)? else {
        return Err(failure(
            GeometryErrorCode::AnalysisIndeterminate,
            "outside-G collar geometry unavailable before UV embedding",
            end.use_.face,
            (end.use_.coedge, end.exit),
        ));
    };
    let step = (end.exit - end.endpoint).abs() * 0.5;
    let nominal_cut = (collar.range[1] - collar.range[0])
        * domain.material_realization().endpoint_support_fraction();
    let germ = if end.spatial_high {
        [end.exit - step, end.exit]
    } else {
        [end.exit, end.exit + step]
    };
    let remains_central = if end.spatial_high {
        germ[0] > collar.range[0] + nominal_cut
    } else {
        germ[1] < collar.range[1] - nominal_cut
    };
    if !remains_central || germ[0] >= germ[1] {
        return Err(failure(
            GeometryErrorCode::ConditioningFailure,
            "outside-G source-only germ has no proved central parameter scope",
            end.use_.face,
            (end.use_.coedge, germ),
        ));
    }
    let Some(embedding) = collar.embedding(germ, [0.0, 1.0], work, cancel)? else {
        return Err(failure(
            GeometryErrorCode::AnalysisIndeterminate,
            "outside-G collar constructor cannot prove scoped original UV embedding",
            end.use_.face,
            (end.use_.coedge, germ),
        ));
    };
    if !embedding.covers(&collar, [end.exit; 2], [0.0, 1.0]) {
        return Err(failure(
            GeometryErrorCode::InvalidGeometry,
            "outside-G scoped UV embedding lost its actual exit cross-section",
            end.use_.face,
            (end.use_.coedge, end.exit),
        ));
    }
    let CarrierTransport::Collar { width, inward } = collar.transport else {
        return Ok(false);
    };
    let uv = chart_jet(domain, end.use_.coedge, [end.exit; 2], work, cancel)?;
    let (beta, beta_t) = collar.bubble([end.exit; 2])?;
    let v = rotate(uv.derivative, inward);
    let vt = rotate(uv.second_derivative, inward);
    let a_t = std::array::from_fn(|i| {
        uv.derivative[i]
            .add(Interval { lo: 0.0, hi: width }.mul(beta_t.mul(v[i]).add(beta.mul(vt[i]))))
    });
    let across = std::array::from_fn(|i| {
        uv.position[i].add(Interval { lo: 0.0, hi: 1.0 }.mul(end.offset[i]))
    });
    // boundary_jet acquires this same carrier mutex.
    let cross_section = collar.boundary_jet(end.exit, [0.0, 1.0], &end.spatial, work, cancel)?;
    {
        let surface = &domain.surface_operand();
        (|| {
            let boundary = surface_enclosure(
                surface,
                uv_box(add_margin(uv.position, collar.margin)),
                work,
                cancel,
            )?;
            let patch = surface_enclosure(
                surface,
                uv_box(add_margin(across, collar.margin)),
                work,
                cancel,
            )?;
            let boundary_t = compose_first(&boundary.du, &boundary.dv, uv.derivative);
            let d_t: Box3 = std::array::from_fn(|i| end.spatial.derivative[i].sub(boundary_t[i]));
            let f_t = compose_first(&patch.du, &patch.dv, a_t);
            let f_w = compose_first(&patch.du, &patch.dv, end.offset);
            let g_t =
                std::array::from_fn(|i| f_t[i].add(Interval { lo: 0.0, hi: 1.0 }.mul(d_t[i])));
            let g_w = std::array::from_fn(|i| {
                f_w[i]
                    .sub(Interval { lo: 0.0, hi: 1.0 }.mul(cross_section.boundary_displacement[i]))
            });
            let mut n = cross(g_t, g_w);
            if !end.use_.orientation {
                n = n.map(Interval::neg);
            }
            let projection = dot(n, cross(rows[0], rows[1]));
            if projection.lo <= 0.0 {
                return Err(failure(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "outside-G local physical source-P graph dot is not strictly positive",
                    end.use_.face,
                    (end.use_.coedge, end.exit, projection),
                ));
            }
            Ok(true)
        })()
    }
}

#[allow(clippy::too_many_arguments)]
fn outer_cutover_owned(
    domain: &PreparedFaceTrimDomain<'_>,
    a: End,
    b: End,
    piece: Piece,
    bulge: Box2,
    boundary: &BoundaryJet,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let end = if matches!(piece, Piece::Incoming) {
        b
    } else {
        a
    };
    if matches!(piece, Piece::Middle)
        && (!affine_bridge_exits_collar(domain, a, b, bulge, work, cancel)?
            || !affine_bridge_exits_collar(domain, b, a, bulge, work, cancel)?)
    {
        return Ok(false);
    }
    let Some(collar) = OriginalCollar::for_enclosure(domain, end.use_, work, cancel)? else {
        return Ok(false);
    };
    for (index, _) in domain.coedges().iter().enumerate() {
        let joining = if matches!(piece, Piece::Middle) {
            index == a.use_.coedge || index == b.use_.coedge
        } else {
            index == end.use_.coedge
        };
        if joining {
            continue;
        }
        for span in domain.boundary_parameter_intervals(index)? {
            let mut stack = [(span, 0_u8); MAX_DEPTH as usize + 1];
            let mut pending = 1;
            while pending != 0 {
                pending -= 1;
                let (range, depth) = stack[pending];
                work.charge(1, cancel)?;
                if disjoint(
                    boundary.uv,
                    collar.other_collar_enclosure(index, range, work, cancel)?,
                ) {
                    continue;
                }
                let mid = range[0] * 0.5 + range[1] * 0.5;
                if depth >= MAX_DEPTH || mid <= range[0] || mid >= range[1] {
                    return Ok(false);
                }
                stack[pending] = ([mid, range[1]], depth + 1);
                stack[pending + 1] = ([range[0], mid], depth + 1);
                pending += 2;
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solidcore::curve::curve::Curve;
    use solidcore::nurbs::{
        b_spline_curve::BSplineCurve,
        b_spline_surface::BSplineSurface,
        crv_basic,
        math::{CPoint, NOW},
        srf_basic,
    };

    fn native_surface(
        points: Vec<Vec<CPoint>>,
        degree: [usize; 2],
        knots: [Vec<f64>; 2],
    ) -> Surface {
        let [u, v] = knots;
        BSplineSurface::from_nurb(
            srf_basic::Surface::new(points, degree[0], degree[1], u, v).unwrap(),
            true,
        )
        .into()
    }

    fn native_plane() -> Surface {
        native_surface(
            [-1.0, 1.0]
                .map(|u| [-1.0, 1.0].map(|v| CPoint::new(u, v, 0.0, NOW)).to_vec())
                .to_vec(),
            [1, 1],
            [vec![-1.0, -1.0, 1.0, 1.0], vec![-1.0, -1.0, 1.0, 1.0]],
        )
    }

    #[test]
    fn retained_cone_apex_avoids_only_inward_endpoint_refinement() {
        let tolerance = 1.0e-6;
        for (range, apex, outward) in [
            ([0.0, 1.0], 0.0, [-0.01, 1.0]),
            ([9.0, 10.0], 10.0, [9.0, 10.01]),
        ] {
            let mut support = SourceTransportSupport {
                owner: SourceTransportOwner::OriginalVertex {
                    point_mm: [0.0, 0.0, apex],
                },
                base_interval: range,
                corrected_interval: range,
                displacement_interval: [-0.1, 0.1],
                realized_witness_interval: None,
                realized_endpoint_witnesses: None,
                source_error_mm: 0.1,
            };
            assert!(endpoint_support_needs_refinement(
                &support,
                [0.0, 10.0],
                tolerance
            ));
            support.realized_witness_interval = Some([apex, apex]);
            assert!(!endpoint_support_needs_refinement(
                &support,
                [0.0, 10.0],
                tolerance
            ));
            support.corrected_interval = outward;
            assert!(endpoint_support_needs_refinement(
                &support,
                [0.0, 10.0],
                tolerance
            ));

            // An actual but nonextremal G point cannot retain the raw endpoint.
            support.corrected_interval = range;
            support.realized_witness_interval = Some([5.0, 5.0]);
            assert!(endpoint_support_needs_refinement(
                &support,
                [0.0, 10.0],
                tolerance
            ));
        }
    }

    #[test]
    fn closed_cone_atlas_covers_apex_fan_and_both_closed_radial_ends() {
        // B(t)=(1,t,t^2), t in [0,1]. All limiting normals at the
        // collapsed apex must survive, not merely the t-midpoint normal.
        let boundary = BoundaryJet {
            position: [
                Interval::point(1.0),
                Interval { lo: 0.0, hi: 1.0 },
                Interval { lo: 0.0, hi: 1.0 },
            ],
            derivative: [
                Interval::point(0.0),
                Interval::point(1.0),
                Interval { lo: 0.0, hi: 2.0 },
            ],
            displacement: [Interval::point(0.0); 3],
            uv: [Interval::point(0.0); 2],
        };
        let apex = closed_cone_jet([0.0; 3], boundary, [0.0, 0.0]).unwrap();
        let outer = closed_cone_jet([0.0; 3], boundary, [1.0, 1.0]).unwrap();
        let closed = closed_cone_jet([0.0; 3], boundary, [0.0, 1.0]).unwrap();
        let normal = apex.normal;
        assert!(
            normal[2].lo > 0.0,
            "apex has a strict oriented limiting fan"
        );
        assert!(apex.position.iter().all(|value| value.contains_zero()));
        assert!(apex.dt.iter().all(|value| value.contains_zero()));
        for t in [0.0_f64, 0.25, 0.5, 0.75, 1.0] {
            let raw = [t * t, -2.0 * t, 1.0];
            for i in 0..3 {
                assert!(normal[i].lo <= raw[i] && raw[i] <= normal[i].hi);
            }
            for s in [0.0, 0.5, 1.0] {
                let point = [s, s * t, s * t * t].map(Interval::point);
                assert!(contains(closed.position, point));
                if s == 1.0 {
                    assert!(contains(outer.position, point));
                }
            }
        }
        let reversed = BoundaryJet {
            derivative: boundary.derivative.map(Interval::neg),
            ..boundary
        };
        let reversed = closed_cone_jet([0.0; 3], reversed, [0.0, 1.0]).unwrap();
        assert!(reversed.normal[2].hi < 0.0);
    }

    #[test]
    fn closed_cone_atlas_refuses_unproved_radial_domains() {
        let boundary = BoundaryJet {
            position: [1.0, 0.0, 0.0].map(Interval::point),
            derivative: [1.0, 0.0, 0.0].map(Interval::point),
            displacement: [Interval::point(0.0); 3],
            uv: [Interval::point(0.0); 2],
        };
        assert!(closed_cone_jet([0.0; 3], boundary, [down(0.0), 1.0]).is_err());
        assert!(closed_cone_jet([0.0; 3], boundary, [0.0, up(1.0)]).is_err());
    }

    #[test]
    fn nonincident_carrier_exclusion_preserves_trimmed_holes_and_actual_caps() {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!("../../../../../../fixtures/geometry/through-hole.step"),
            &cancel,
        )
        .unwrap();
        // The bottom carrier covers both points, but the original cap has a
        // radius-three hole centered at (10,10). Empty trim is not material;
        // the same plane's filled annulus must still block a vertex support.
        for (center, separated) in [([10.0, 10.0, 0.0], true), ([15.0, 10.0, 0.0], false)] {
            let support = center.map(|coordinate| Interval {
                lo: coordinate - 0.05,
                hi: coordinate + 0.05,
            });
            let mut work = GeometryWorkBudget::new(20_000_000);
            let prepared = crate::analysis::prepare_solid(
                &definition,
                crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert_eq!(
                separate_nonincident(
                    prepared.source_view().unwrap(),
                    &[],
                    support,
                    MAX_SCRATCH_BYTES,
                    &mut work,
                    &cancel,
                )
                .unwrap(),
                separated,
                "source support centered at {center:?}"
            );
        }
    }

    #[test]
    fn normalized_recipe_reflection_uses_proved_domain_not_roundoff_as_geometry() {
        let unit = source_unit_complement(Interval { lo: 0.0, hi: 1.0 }).unwrap();
        assert_eq!([unit.lo, unit.hi], [0.0, 1.0]);
        let upper_half = source_unit_complement(Interval { lo: 0.0, hi: 0.5 }).unwrap();
        assert!(upper_half.lo <= 0.5 && upper_half.hi == 1.0);
        assert!(
            source_unit_complement(Interval {
                lo: down(0.0),
                hi: up(1.0)
            })
            .is_err()
        );
        assert!(
            source_unit_parameter(Interval {
                lo: f64::NAN,
                hi: 1.0
            })
            .is_err()
        );
    }

    #[test]
    fn direct_original_curve_chord_bound_retains_native_vertex_residual() {
        // The source is the actual borrowed native spatial coefficient curve.
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let linear = [1.0, 2.0, 0.5];
        for shift in [-16.0, 0.0, 1024.0] {
            let origin = [shift, -2.0 * shift, 0.5 * shift];
            for quadratic in [[0.0; 3], [0.25, -0.5, 1.0], [-1.0, 2.0, 0.5]] {
                let points = [(0.0, 0.0), (0.5, 0.0), (1.0, 1.0)].map(
                    |(linear_weight, quadratic_weight)| {
                        let p: [f64; 3] = std::array::from_fn(|i| {
                            origin[i] + linear_weight * linear[i] + quadratic_weight * quadratic[i]
                        });
                        CPoint::new(p[0], p[1], p[2], NOW)
                    },
                );
                let curve: Curve = BSplineCurve::from_nurb(
                    crv_basic::Curve::new(points.to_vec(), 2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0])
                        .unwrap(),
                    3,
                    true,
                )
                .into();
                let endpoint =
                    crate::parametric::curve_enclosure(&curve, (0.0, 0.0), &mut work, &cancel)
                        .unwrap();
                for span in [0.125, 0.5] {
                    let jet =
                        crate::parametric::curve_enclosure(&curve, (0.0, span), &mut work, &cancel)
                            .unwrap();
                    for residual in [0.0, 1.0 / 262144.0] {
                        let point = [origin[0] + residual, origin[1], origin[2]];
                        let bound = endpoint_chord_error_bound(
                            point,
                            endpoint.position,
                            jet.second_derivative,
                            Interval::point(span),
                        )
                        .unwrap();
                        let midpoint_error: [f64; 3] = std::array::from_fn(|i| {
                            quadratic[i] * span * span * 0.25
                                + if i == 0 { residual * 0.5 } else { 0.0 }
                        });
                        let expected = midpoint_error
                            .into_iter()
                            .map(|x| x * x)
                            .sum::<f64>()
                            .sqrt();
                        assert!(
                            bound >= expected && bound >= residual,
                            "original coefficient curvature and NativeV datum both belong to source cost"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn original_circle_chord_cost_retains_its_radian_provider_gauge() {
        let original = solidcraft_step::geom::Curve::Circle {
            position: solidcraft_step::geom::Placement {
                origin: [10.0, -2.0, 3.0],
                axis: [0.0, 0.0, 1.0],
                ref_direction: [1.0, 0.0, 0.0],
            },
            radius: 4.0,
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let provider = crate::kernel::source_operand::PreparedOriginalCurve::new(
            &original,
            [0.0, 0.5],
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let endpoint =
            crate::parametric::curve_enclosure(&provider, (0.0, 0.0), &mut work, &cancel).unwrap();
        assert!(
            endpoint.derivative[1].contains(4.0),
            "an original radian derivative is not a native degree or rational derivative"
        );
        for span in [0.125_f64, 0.5] {
            let whole =
                crate::parametric::curve_enclosure(&provider, (0.0, span), &mut work, &cancel)
                    .unwrap();
            let bound = endpoint_chord_error_bound(
                [14.0, -2.0, 3.0],
                endpoint.position,
                whole.second_derivative,
                Interval::point(span),
            )
            .unwrap();
            let actual_midpoint_error = 4.0 * (1.0 - (span * 0.5).cos());
            assert!(
                bound >= actual_midpoint_error && bound < 1.0,
                "the complete source-provider C2 remainder must cover its actual circular chord"
            );
        }
    }

    #[test]
    fn actual_source_generator_refutation_covers_quadratic_coefficient_families() {
        // Both polynomial and homogeneous native source coefficient families.
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let linear = [1.0, 2.0, 3.0];
        let across = [-2.0, 1.0, 0.5];
        for shift in [-16.0, 0.0, 1024.0] {
            let origin = [shift, -2.0 * shift, 0.5 * shift];
            for quadratic in [
                [0.0; 3],
                [0.25, -0.5, 1.0],
                [2.0, 1.0, -0.25],
                [-1.0, 0.0, 0.0],
            ] {
                // S(u,v) = V + A*u + D*v + C*u^2, represented by its
                // complete original Bernstein coefficients, not sampled jets.
                let points: Vec<Vec<[f64; 3]>> = [(0.0, 0.0), (0.5, 0.0), (1.0, 1.0)]
                    .into_iter()
                    .map(|(u, q)| {
                        [0.0, 1.0]
                            .into_iter()
                            .map(|v| {
                                std::array::from_fn::<_, 3, _>(|i| {
                                    origin[i] + linear[i] * u + across[i] * v + quadratic[i] * q
                                })
                            })
                            .collect()
                    })
                    .collect();
                let knots = [vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0], vec![0.0, 0.0, 1.0, 1.0]];
                for weight in [1.0, 2.0] {
                    let controls = points
                        .iter()
                        .map(|row| {
                            row.iter()
                                .map(|p| {
                                    if weight == 1.0 {
                                        CPoint::new(p[0], p[1], p[2], NOW)
                                    } else {
                                        CPoint::new(
                                            weight * p[0],
                                            weight * p[1],
                                            weight * p[2],
                                            weight,
                                        )
                                    }
                                })
                                .collect()
                        })
                        .collect();
                    let surface = native_surface(controls, [2, 1], knots.clone());
                    for h in [0.125, 0.5, 1.0] {
                        for offset in [0.0, 0.125] {
                            let boundary_uv = [h, 0.5 * h + offset].map(Interval::point);
                            let halfway_uv =
                                [0.5 * h, 0.25 * h + 0.5 * offset].map(Interval::point);
                            let displacement = source_generator_displacement(
                                &surface,
                                boundary_uv,
                                halfway_uv,
                                origin,
                                &mut work,
                                &cancel,
                            )
                            .unwrap();
                            let norm = length(displacement).unwrap();
                            let exact = quadratic.map(|value| {
                                Interval::point(value)
                                    .mul(Interval::point(h).square())
                                    .mul(Interval::point(0.25))
                            });
                            assert!(!disjoint(displacement, exact));
                            if quadratic == [0.0; 3] {
                                // Affine source generators cannot refute this
                                // positive allowance; that is NOT acceptance.
                                assert!(norm.hi < 1e-10);
                                assert!(!(norm.lo > 1e-10));
                            } else {
                                let exact_norm = length(exact).unwrap();
                                let overbudget_allowance = exact_norm.lo * 0.5;
                                let within_budget_allowance = exact_norm.hi * 2.0;
                                assert!(norm.lo > overbudget_allowance);
                                assert!(norm.hi < within_budget_allowance);
                                assert!(!(norm.lo > within_budget_allowance));
                            }
                        }
                    }
                }
            }
        }
        assert!(work.consumed() > 0);
    }

    #[test]
    fn actual_source_generator_refutation_preserves_work_cancel_and_conditioning_errors() {
        let surface = native_plane();
        let boundary_uv = [1.0, 0.5].map(Interval::point);
        let halfway_uv = [0.5, 0.25].map(Interval::point);
        for (limit, cancelled, code) in [
            (0, false, GeometryErrorCode::ResourceLimit),
            (1_000_000, true, GeometryErrorCode::Cancelled),
        ] {
            let mut work = GeometryWorkBudget::new(limit);
            assert_eq!(
                source_generator_displacement(
                    &surface,
                    boundary_uv,
                    halfway_uv,
                    [0.0; 3],
                    &mut work,
                    &AtomicBool::new(cancelled),
                )
                .unwrap_err()
                .code,
                code
            );
        }
        let pole = native_surface(
            vec![
                vec![
                    CPoint::new(0.0, 0.0, 0.0, 1.0),
                    CPoint::new(0.0, 1.0, 0.0, 1.0),
                ],
                vec![
                    CPoint::new(-1.0, 0.0, 0.0, -1.0),
                    CPoint::new(-1.0, -1.0, 0.0, -1.0),
                ],
            ],
            [1, 1],
            [vec![0.0, 0.0, 1.0, 1.0], vec![0.0, 0.0, 1.0, 1.0]],
        );
        let mut work = GeometryWorkBudget::new(1_000_000);
        assert_eq!(
            source_generator_displacement(
                &pole,
                boundary_uv,
                halfway_uv,
                [0.0; 3],
                &mut work,
                &AtomicBool::new(false),
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ConditioningFailure
        );
        assert!(work.consumed() > 0);
    }

    #[test]
    fn original_joint_image_needs_whole_envelope_and_same_oriented_projection() {
        let surface = native_plane();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let projection = [0.0, 0.0, 1.0].map(Interval::point);
        let small = [Interval {
            lo: -1.0e-8,
            hi: 1.0e-8,
        }; 2];
        assert!(
            whole_original_joint_image_fits(
                &surface, small, [0.0; 3], projection, true, None, 1.0e-6, &mut work, &cancel,
            )
            .unwrap()
        );
        // The midpoint is still exactly NativeV and the carrier still regular,
        // but the complete original closure image is not a bounded endpoint.
        let wide = [small[0], Interval { lo: -0.5, hi: 0.5 }];
        assert!(
            !whole_original_joint_image_fits(
                &surface, wide, [0.0; 3], projection, true, None, 1.0e-6, &mut work, &cancel,
            )
            .unwrap()
        );
        assert!(
            !whole_original_joint_image_fits(
                &surface,
                small,
                [0.0; 3],
                projection.map(Interval::neg),
                true,
                None,
                1.0e-6,
                &mut work,
                &cancel,
            )
            .unwrap()
        );
        // A foreign original vertex is not interchangeable with the matching
        // source image even when the same UV box/projection is retained.
        assert!(
            !whole_original_joint_image_fits(
                &surface,
                small,
                [1.0, 0.0, 0.0],
                projection,
                true,
                None,
                1.0e-6,
                &mut work,
                &cancel,
            )
            .unwrap()
        );
    }

    fn housing() -> Definition {
        crate::from_step_bytes_with_cancel(include_bytes!(
            "../../../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
        ), &AtomicBool::new(false)).unwrap()
    }

    fn spherical_corner(definition: &Definition) -> VertexId {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(20_000_000);
        for (ordinal, &id) in definition.face_ids().iter().enumerate() {
            let surface = definition.face_surface(id).unwrap();
            for axis in 0..2 {
                for upper in [false, true] {
                    let Some(pole) = source_chart::native_collapsed_boundary(
                        surface, axis, upper, &mut work, &cancel,
                    )
                    .unwrap() else {
                        continue;
                    };
                    let (_, face) =
                        crate::analysis::source_incidence::native_face(definition, ordinal)
                            .unwrap();
                    for &loop_id in &face.loops {
                        for entry in &definition.brep().loop_(loop_id).unwrap().edges {
                            for vertex in definition.brep().edge(entry.edge).unwrap().vertices {
                                let p = definition.brep().vertex(vertex).unwrap().point;
                                if pole
                                    .world
                                    .into_iter()
                                    .zip([p.x, p.y, p.z])
                                    .all(|(bound, value)| bound.contains(value))
                                {
                                    return vertex;
                                }
                            }
                        }
                    }
                }
            }
        }
        panic!("original housing must retain its coefficient-qualified collapsed corner");
    }

    fn incidence<'d>(
        definition: &'d Definition,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> SourceEdgeIncidence<'d> {
        let mut incidence =
            SourceEdgeIncidence::new(definition, MAX_SCRATCH_BYTES, work, cancel).unwrap();
        incidence
            .prepare_vertices(MAX_SCRATCH_BYTES, work, cancel)
            .unwrap();
        incidence
    }

    #[test]
    fn original_housing_overbudget_generator_refutes_radius_before_finite_boundary_cover() {
        let definition = housing();
        let before = format!("{:?}", definition.brep());
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(20_000_000);
        let incidence = incidence(&definition, &mut work, &cancel);
        let prepared = crate::analysis::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let source = prepared.source_view().unwrap().with_vertex_plans(None);
        let (_, edge) =
            crate::analysis::source_incidence::native_coedge(&definition, 2, 0, 0).unwrap();
        let domain = source
            .face_domain(2, MAX_SCRATCH_BYTES, &mut work, &cancel)
            .unwrap();
        let vertex = edge.vertices
            [usize::from(!(domain.coedges()[0].forward == domain.native_orientation()))];
        let (corners, point) = corners(
            source,
            &incidence,
            vertex,
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap()
        .unwrap();
        let corner = corners
            .iter()
            .find(|corner| {
                corner.outgoing.face == 2
                    && corner.outgoing.coedge == 0
                    && corner.incoming.coedge == 3
            })
            .expect("retain the actual failing original face/coedge corner");
        let domain = source
            .face_domain(2, MAX_SCRATCH_BYTES, &mut work, &cancel)
            .unwrap();
        let allowance = source_allowance(&definition).unwrap();
        assert_eq!(allowance, 1e-7);
        for (fraction, expected_refutation) in [(1.0 / 2048.0, true), (1.0 / 4096.0, false)] {
            let outgoing = prepare_end(
                &domain,
                corner.outgoing,
                false,
                fraction,
                None,
                &mut work,
                &cancel,
            )
            .unwrap()
            .unwrap();
            let incoming = prepare_end(
                &domain,
                corner.incoming,
                true,
                fraction,
                None,
                &mut work,
                &cancel,
            )
            .unwrap()
            .unwrap();
            let outgoing_refutes =
                source_radius_refuted(&domain, outgoing, point, allowance, &mut work, &cancel)
                    .unwrap();
            let incoming_refutes =
                source_radius_refuted(&domain, incoming, point, allowance, &mut work, &cancel)
                    .unwrap();
            assert_eq!(outgoing_refutes || incoming_refutes, expected_refutation);
        }
        drop(domain);
        let plan = SourceVertexTransportPlan::prepare(
            source,
            &incidence,
            vertex,
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap()
        .expect("the actual housing consumer must advance to a qualified finer source radius");
        assert!(plan.fraction < 1.0 / 2048.0);
        assert!(!plan.cells.is_empty());
        assert!(plan.source_error_mm() <= allowance);
        assert_eq!(before, format!("{:?}", definition.brep()));
    }

    #[test]
    fn original_housing_cone_has_actual_source_correspondence_and_no_source_mutation() {
        let definition = housing();
        let before = format!("{:?}", definition.brep());
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000_000);
        let incidence = incidence(&definition, &mut work, &cancel);
        let vertex = spherical_corner(&definition);
        let prepared = crate::analysis::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let source = prepared.source_view().unwrap().with_vertex_plans(None);
        let plan = SourceVertexTransportPlan::prepare(source, &incidence, vertex,
            MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap()
            .expect("actual original rounded corner must produce a finite source-only conical geometry plan");
        assert_eq!(
            plan.point,
            source
                .trim_source()
                .source_vertex_point(vertex, &mut work, &cancel)
                .unwrap(),
            "only the captured original source vertex chooses the canonical apex"
        );
        let allowance = definition.provenance.uncertainty_mm.unwrap();
        assert!(plan.source_error_mm() <= allowance);
        assert!(!plan.cells.is_empty());
        for cell in &plan.cells {
            assert!(cell.error_mm <= allowance);
            assert!(length(cell.displacement).unwrap().hi <= allowance);
            let recipe = plan.recipes[cell.recipe];
            let domain = source
                .face_domain(
                    recipe.outgoing.use_.face,
                    MAX_SCRATCH_BYTES,
                    &mut work,
                    &cancel,
                )
                .unwrap();
            let t = Interval::point(cell.parameter[0] * 0.5 + cell.parameter[1] * 0.5);
            let b = boundary_jet(
                &domain,
                recipe.outgoing,
                recipe.incoming,
                cell.piece,
                t,
                recipe.bulge,
                &mut work,
                &cancel,
            )
            .unwrap();
            for s in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let h = star_jet(
                    &domain,
                    recipe.outgoing,
                    recipe.incoming,
                    cell.piece,
                    Interval::point(s),
                    t,
                    recipe.bulge,
                    &mut work,
                    &cancel,
                )
                .unwrap();
                let original = {
                    let surface = &domain.surface_operand();
                    (|| surface_enclosure(surface, uv_box(h.uv), &mut work, &cancel))()
                }
                .unwrap()
                .position;
                let realized: Box3 = std::array::from_fn(|i| {
                    Interval::point(plan.point[i]).add(
                        Interval::point(s).mul(b.position[i].sub(Interval::point(plan.point[i]))),
                    )
                });
                let actual_delta: Box3 = std::array::from_fn(|i| realized[i].sub(original[i]));
                assert!(length(actual_delta).unwrap().hi <= allowance);
            }
        }
        // Covectors need not be unit vectors. Their scalar deltas scale, while
        // the admitted SOURCE physical error remains in millimetres.
        let direction = [2.0, -3.0, 0.5].map(Interval::point);
        let mut supports = Vec::new();
        plan.visit_supports(direction, &mut work, &cancel, &mut |support| {
            supports.push(support);
            Ok(())
        })
        .unwrap();
        assert_eq!(supports.len(), plan.cells.len());
        assert!(
            supports
                .iter()
                .all(|support| support.source_error_mm <= allowance)
        );
        assert!(supports.iter().all(|support| matches!(support.owner,
            SourceTransportOwner::OriginalVertex { point_mm } if point_mm == plan.point)));
        let invalid = [
            Interval { lo: 2.0, hi: 1.0 },
            Interval::point(0.0),
            Interval::point(0.0),
        ];
        assert_eq!(
            plan.visit_supports(invalid, &mut work, &cancel, &mut |_| Ok(()))
                .unwrap_err()
                .code,
            GeometryErrorCode::ConditioningFailure
        );
        assert_eq!(before, format!("{:?}", definition.brep()));
    }

    #[test]
    fn retained_source_plans_preserve_supports_and_reject_foreign_cancelled_or_underfunded_access()
    {
        let definition = housing();
        let foreign = housing();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000_000);
        let prepared = crate::analysis::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let foreign_prepared = crate::analysis::prepare_solid(
            &foreign,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let source = prepared.source_view().unwrap().with_vertex_plans(None);
        let foreign_view = foreign_prepared
            .source_view()
            .unwrap()
            .with_vertex_plans(None);
        let plans =
            PreparedSourceVertexPlans::prepare(source, MAX_SCRATCH_BYTES, &mut work, &cancel)
                .unwrap();
        let retained_source = source.with_vertex_plans(Some(&plans));
        let vertex = spherical_corner(&definition);
        let source_incidence = incidence(&definition, &mut work, &cancel);
        let mut expected_positions = Vec::new();
        for vertex in source_incidence.vertices().unwrap() {
            plans
                .plan(&definition, vertex, &mut work, &cancel)
                .unwrap()
                .visit_position_supports(&mut work, &cancel, &mut |realized, _comparison| {
                    expected_positions.push(realized);
                    Ok(())
                })
                .unwrap();
        }
        let mut actual_positions = Vec::new();
        plans
            .visit_position_supports(&definition, &mut work, &cancel, &mut |realized| {
                actual_positions.push(realized);
                Ok(())
            })
            .unwrap();
        let order = |left: &Box3, right: &Box3| {
            for axis in 0..3 {
                let order = left[axis]
                    .lo
                    .total_cmp(&right[axis].lo)
                    .then_with(|| left[axis].hi.total_cmp(&right[axis].hi));
                if order != std::cmp::Ordering::Equal {
                    return order;
                }
            }
            std::cmp::Ordering::Equal
        };
        actual_positions.sort_unstable_by(order);
        expected_positions.sort_unstable_by(order);
        assert!(!actual_positions.is_empty());
        assert_eq!(
            actual_positions, expected_positions,
            "retained bounds must stream actual G positions, not comparison hulls"
        );
        let cancelled_positions = AtomicBool::new(true);
        for (owner, cancellation, expected) in [
            (&foreign, &cancel, GeometryErrorCode::AnalysisIndeterminate),
            (
                &definition,
                &cancelled_positions,
                GeometryErrorCode::Cancelled,
            ),
        ] {
            let mut visited = false;
            let error = plans
                .visit_position_supports(owner, &mut work, cancellation, &mut |_| {
                    visited = true;
                    Ok(())
                })
                .unwrap_err();
            assert_eq!(error.code, expected);
            assert!(
                !visited,
                "unqualified retained bounds must not invoke the visitor"
            );
        }
        let selected = SourceVertexPlan::prepare(
            retained_source,
            &source_incidence,
            vertex,
            0,
            &mut work,
            &cancel,
        )
        .unwrap()
        .expect("retained source selection needs no local plan allocation");
        assert!(selected.plan().source_error_mm() <= source_allowance(&definition).unwrap());
        assert_eq!(
            SourceVertexPlan::prepare(source, &source_incidence, vertex, 0, &mut work, &cancel,)
                .err()
                .expect("query-local selection must reserve its local plan storage")
                .code,
            GeometryErrorCode::ResourceLimit,
        );
        assert_eq!(
            plans
                .plan(&foreign, spherical_corner(&foreign), &mut work, &cancel)
                .err()
                .expect("a foreign NativeV must not borrow a retained plan")
                .code,
            GeometryErrorCode::AnalysisIndeterminate,
        );
        let frame = PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
        };
        let rows = crate::analysis::placed::ActualMaterialCoordinates::source_rows(&frame).unwrap();
        let z = selected.plan().point[2];
        let coordinates = crate::analysis::placed::ActualMaterialCoordinates::source(
            &definition,
            &frame,
            &rows,
            [z - 1.0, z + 1.0],
            z,
            &mut work,
            &cancel,
        )
        .unwrap();
        let cancelled_atlas = AtomicBool::new(true);
        let mut atlas_visited = false;
        for (owner, bytes, cancellation, expected) in [
            (
                foreign_view,
                MAX_SCRATCH_BYTES,
                &cancel,
                GeometryErrorCode::AnalysisIndeterminate,
            ),
            (source, 0, &cancel, GeometryErrorCode::ResourceLimit),
            (
                source,
                MAX_SCRATCH_BYTES,
                &cancelled_atlas,
                GeometryErrorCode::Cancelled,
            ),
        ] {
            let error = selected
                .plan()
                .visit_closed_atlas_geometry(
                    owner,
                    &coordinates,
                    bytes,
                    &mut work,
                    cancellation,
                    &mut |_, _, _, _, _, _| {
                        atlas_visited = true;
                        Ok(super::super::radial_product::BoundaryVisit::Accepted)
                    },
                )
                .unwrap_err();
            assert_eq!(error.code, expected);
        }
        assert!(
            !atlas_visited,
            "unqualified atlas access must not invoke a query visitor"
        );

        let direction = [0.0, 0.0, 1.0].map(Interval::point);
        let local = source_transport_supports(
            source,
            direction,
            None,
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let retained = source_transport_supports(
            retained_source,
            direction,
            None,
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let summary = |supports: &[SourceTransportSupport]| {
            let mut bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 3];
            let mut error = 0.0f64;
            for support in supports {
                for (bound, interval) in bounds.iter_mut().zip([
                    support.base_interval,
                    support.corrected_interval,
                    support.displacement_interval,
                ]) {
                    bound[0] = bound[0].min(interval[0]);
                    bound[1] = bound[1].max(interval[1]);
                }
                error = error.max(support.source_error_mm);
            }
            (bounds, error)
        };
        assert_eq!(summary(&local), summary(&retained));
        let foreign_source = foreign_view.with_vertex_plans(Some(&plans));
        assert_eq!(
            source_transport_supports(
                foreign_source,
                direction,
                None,
                MAX_SCRATCH_BYTES,
                &mut work,
                &cancel,
            )
            .err()
            .expect("scalar consumers must reject a foreign source owner")
            .code,
            GeometryErrorCode::AnalysisIndeterminate,
        );
        let foreign_incidence = incidence(&foreign, &mut work, &cancel);
        assert_eq!(
            ClosedRadialLinks::geometry(
                foreign_source,
                &foreign_incidence,
                MAX_SCRATCH_BYTES,
                &mut work,
                &cancel,
                &mut |_, _, _| Ok(()),
            )
            .err()
            .expect("critical consumers must reject a foreign source owner")
            .code,
            GeometryErrorCode::AnalysisIndeterminate,
        );

        let cancelled = AtomicBool::new(true);
        assert_eq!(
            source_transport_supports(
                retained_source,
                direction,
                None,
                MAX_SCRATCH_BYTES,
                &mut work,
                &cancelled,
            )
            .err()
            .expect("retained scalar access must still check cancellation")
            .code,
            GeometryErrorCode::Cancelled,
        );
        assert_eq!(
            ClosedRadialLinks::geometry(
                retained_source,
                &source_incidence,
                MAX_SCRATCH_BYTES,
                &mut work,
                &cancelled,
                &mut |_, _, _| Ok(()),
            )
            .err()
            .expect("retained critical access must still check cancellation")
            .code,
            GeometryErrorCode::Cancelled,
        );
        assert_eq!(
            PreparedSourceVertexPlans::prepare(
                source,
                plans.retained_bytes() - 1,
                &mut work,
                &cancel,
            )
            .err()
            .expect("retained-byte admission must reject insufficient capacity")
            .code,
            GeometryErrorCode::ResourceLimit,
        );
    }

    #[test]
    fn foreign_original_incidence_cannot_supply_a_vertex_transport_plan() {
        let definition = housing();
        let foreign = housing();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000_000);
        let incidence = incidence(&definition, &mut work, &cancel);
        let prepared = crate::analysis::prepare_solid(
            &foreign,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let result = SourceVertexTransportPlan::prepare(
            prepared.source_view().unwrap().with_vertex_plans(None),
            &incidence,
            spherical_corner(&definition),
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn source_conical_geometry_cannot_borrow_an_increased_physical_error_allowance() {
        let mut definition = housing();
        definition.provenance.uncertainty_mm = Some(f64::MIN_POSITIVE);
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000_000);
        let incidence = incidence(&definition, &mut work, &cancel);
        let prepared = crate::analysis::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let source = match prepared.source_view() {
            Ok(source) => source.with_vertex_plans(None),
            Err(error) => {
                assert!(matches!(
                    error.code,
                    GeometryErrorCode::AnalysisIndeterminate
                        | GeometryErrorCode::ShapeErrorExceeded
                ));
                return;
            }
        };
        match SourceVertexTransportPlan::prepare(
            source,
            &incidence,
            spherical_corner(&definition),
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        ) {
            Ok(None) => {}
            Err(error)
                if matches!(
                    error.code,
                    GeometryErrorCode::AnalysisIndeterminate
                        | GeometryErrorCode::ShapeErrorExceeded
                ) => {}
            Ok(Some(_)) => {
                panic!("an actually curved original corner cannot spend a larger source allowance")
            }
            Err(error) => panic!(
                "error-budget refusal must not be a resource or cancellation failure: {error:?}"
            ),
        }
    }
}

fn validate_spatial_exits(
    recipes: &[CornerRecipe],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    if recipes.is_empty() {
        return Err(refusal(
            "source vertex has no actual spatial-provider endpoint exits",
        ));
    }
    for (index, recipe) in recipes.iter().enumerate() {
        work.charge(1, cancel)?;
        let incoming = recipe.incoming;
        let outgoing = recipes[(index + 1) % recipes.len()].outgoing;
        if incoming.edge != outgoing.edge
            || incoming.spatial_high != outgoing.spatial_high
            || incoming.exit != outgoing.exit
            || incoming.use_.orientation == outgoing.use_.orientation
        {
            return Err(GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                format!(
                    "opposed original vertex uses do not share one literal spatial endpoint exit: incoming_face_coedge={:?} outgoing_face_coedge={:?} incoming_spatial_exit={} outgoing_spatial_exit={} incoming_spatial_high={} outgoing_spatial_high={}",
                    [incoming.use_.face, incoming.use_.coedge],
                    [outgoing.use_.face, outgoing.use_.coedge],
                    incoming.exit,
                    outgoing.exit,
                    incoming.spatial_high,
                    outgoing.spatial_high,
                ),
            ));
        }
    }
    Ok(())
}
