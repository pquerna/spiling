// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Query-local evidence over the immutable material realization. Raw F boxes
//! over-cover the EXACT complement of the reference ribbons/corners; they are
//! not asserted to partition the actual remainder. No polygon participates.

use super::radial_product::RadialRegionKind;
use super::*;
use crate::analysis::material_source::MaterialSourceView;
use crate::analysis::placed::ActualMaterialCoordinates;
use crate::analysis::source_incidence::native_coedge;
use crate::parametric::{
    SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES, SurfaceFirstEnclosure, SurfaceFunctional,
    SurfaceFunctionalEnclosure, surface_functional_enclosures, surface_functional_scratch_bytes,
    surface_piecewise_first_enclosure,
};

type Box3 = [Interval; 3];
/// Full retained UV connector/expression overcover, not joint ownership or
/// physical collapse. It remains valid without trusting a nominal UV box to
/// include a connector or a separate endpoint-expression enclosure.
pub(super) const ORIGINAL_JOINT_UV_SCRATCH_BYTES: usize =
    std::mem::size_of::<[[Interval; 3]; 4]>() + 4 * std::mem::size_of::<[Interval; 2]>();

pub(super) fn original_joint_uv_enclosure(
    joint: &crate::kernel::trim::PreparedTrimJoint,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 2], GeometryError> {
    use crate::parametric::CurveEvaluator as _;
    work.charge(1, cancel)?;
    let mut image = joint.enclosure;
    for value in image {
        Interval::new(value.lo, value.hi)?;
    }
    for endpoint in joint.endpoint_uv {
        for axis in 0..2 {
            image[axis] = image[axis].hull(Interval::new(endpoint[axis][0], endpoint[axis][1])?);
        }
    }
    let connector = joint.connector.tensor([0.0, 1.0], 0, work, cancel)?[0];
    for axis in 0..2 {
        image[axis] = image[axis].hull(connector[axis]);
    }
    if let crate::kernel::trim::TrimJointKind::SphericalPole {
        connector,
        polar_axis,
        polar_coordinate,
        ..
    } = &joint.kind
    {
        if *polar_axis >= 2 {
            return Err(GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original pole joint has no valid selected polar axis",
            ));
        }
        image[*polar_axis] =
            image[*polar_axis].hull(Interval::new(polar_coordinate.lo, polar_coordinate.hi)?);
        let connector = connector.tensor([0.0, 1.0], 0, work, cancel)?[0];
        for axis in 0..2 {
            image[axis] = image[axis].hull(connector[axis]);
        }
    }
    Ok(image)
}

/// Fixed live storage for the actual original connector/world-image overcover.
/// Callers must admit this before evaluating any joint, independently of source
/// domain, vertex-link and cone ownership.
pub(super) const ORIGINAL_JOINT_POSITION_SCRATCH_BYTES: usize =
    (if SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES
        > crate::kernel::source_operand::SOURCE_OPERAND_SCRATCH_BYTES
    {
        SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES
    } else {
        crate::kernel::source_operand::SOURCE_OPERAND_SCRATCH_BYTES
    }) + ORIGINAL_JOINT_UV_SCRATCH_BYTES
        + std::mem::size_of::<SurfaceFirstEnclosure>()
        + std::mem::size_of::<[[Interval; 3]; 4]>()
        + 4 * std::mem::size_of::<Box3>()
        + 2 * std::mem::size_of::<[[f64; 2]; 2]>()
        + std::mem::size_of::<crate::kernel::trim::PreparedTrimSurface<'_, '_>>();

/// Positional overcover only: neither a joint's UV rectangle nor its pole label
/// proves ownership. Keep the selected connector expressions, retained physical
/// pole image and original declared endpoint points, even when those physical
/// points are absent from S(joint.enclosure).
pub(super) fn original_joint_position_enclosure(
    domain: &PreparedFaceTrimDomain<'_>,
    joint: &crate::kernel::trim::PreparedTrimJoint,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Box3, GeometryError> {
    work.charge(1, cancel)?;
    let joint_uv = original_joint_uv_enclosure(joint, work, cancel)?;
    // The connector rectangle is an auxiliary overcover, not a selected
    // material-domain query. Literal coefficient support still applies.
    let mut image = domain.original_carrier_position_bounds(
        joint_uv.map(|value| [value.lo, value.hi]),
        work,
        cancel,
    )?;
    let mut include = |position: Box3| -> Result<(), GeometryError> {
        for value in position {
            Interval::new(value.lo, value.hi)?;
        }
        image = std::array::from_fn(|axis| image[axis].hull(position[axis]));
        Ok(())
    };
    if let crate::kernel::trim::TrimJointKind::SphericalPole {
        world_enclosure, ..
    } = &joint.kind
    {
        include(*world_enclosure)?;
    }
    include(joint.vertex_point.map(Interval::point))?;
    for (identity, incoming) in [(joint.incoming, true), (joint.outgoing, false)] {
        let mut coedge = None;
        for (index, candidate) in domain.coedges().iter().enumerate() {
            work.charge(1, cancel)?;
            if candidate.identity() == identity {
                coedge = Some(index);
                break;
            }
        }
        let coedge = coedge.ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original joint image lost its actual material coedge",
            )
        })?;
        let correspondence = domain.original_coedge_correspondence(coedge)?;
        let binding = correspondence.binding().ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original joint image lost its selected source endpoint binding",
            )
        })?;
        let spatial_high = binding.material_forward == incoming;
        include(
            binding.source_vertex_points_mm[binding.native_endpoint_index(spatial_high)]
                .map(Interval::point),
        )?;
    }
    Ok(image)
}

fn position_boxes_disjoint(first: Box3, second: Box3) -> bool {
    (0..3).any(|axis| first[axis].hi < second[axis].lo || second[axis].hi < first[axis].lo)
}

pub(super) type BoundaryVisitor<'a> = dyn for<'s> FnMut(
        BoundaryCell<'s>,
        bool,
        bool,
        &mut GeometryWorkBudget,
        &AtomicBool,
    ) -> Result<BoundaryVisit, GeometryError>
    + 'a;
const STACK_BYTES: usize = UV_EMBEDDING_STACK_BYTES
    + std::mem::size_of::<[([[f64; 2]; 2], u8, bool, bool, bool); MAX_DEPTH as usize + 1]>()
    + std::mem::size_of::<[([f64; 2], u8); MAX_DEPTH as usize + 1]>()
    + std::mem::size_of::<RadialInventory<'_, '_>>()
    + std::mem::size_of::<ClosedRadialLinks<'_>>()
    + std::mem::size_of::<vertex_transport::SourceVertexPlan<'_, '_>>()
    + std::mem::size_of::<Option<vertex_transport::CompletePositionCoverFailure>>()
    + std::mem::size_of::<Option<([f64; 2], usize, LocalGeometryRefusal)>>()
    + std::mem::size_of::<[ChartCurveEnclosure; 2]>()
    + std::mem::size_of::<[Box3; 2]>()
    + std::mem::size_of::<BoundaryCell<'_>>()
    + std::mem::size_of::<[[Interval; 2]; 2]>()
    + std::mem::size_of::<SurfaceFunctional>()
    + std::mem::size_of::<SurfaceFunctionalEnclosure>()
    + surface_functional_scratch_bytes::<1>()
    + SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES
    + std::mem::size_of::<SurfaceFirstEnclosure>()
    + std::mem::size_of::<Option<source_chart::SphereChart>>()
    + crate::analysis::slab::HEIGHT_CONTRACTION_SCRATCH_BYTES
    + ORIGINAL_JOINT_POSITION_SCRATCH_BYTES;

fn resource() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "closed radial material atlas exhausts remaining scratch allowance",
    )
}

fn outside(z: Interval, interval: [f64; 2]) -> bool {
    z.hi < interval[0] || z.lo > interval[1]
}

/// Reference ownership is distinct from inner-half actual-G ownership. This
/// receipt binds the pure embedding to the original raw domain, and therefore
/// supplies the complementary F half-neighborhood at w=1.
struct ReferenceRibbon<'a, 'd> {
    embedding: UvRibbonEmbedding<'a, 'd>,
}

impl ReferenceRibbon<'_, '_> {
    fn covers(&self, collar: &OriginalCollar<'_, '_>, range: [f64; 2]) -> bool {
        self.embedding.covers(collar, range, [0.0, 1.0])
    }
}

/// A common literal trace and a directed transverse coordinate. Both sides use
/// the same original C3D parameter, not merely equal-looking source IDs.
struct PairedTrace;

impl PairedTrace {
    fn qualify(
        first: &OriginalCollar<'_, '_>,
        second: &OriginalCollar<'_, '_>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        work.charge(1, cancel)?;
        let a = first
            .domain
            .original_coedge_correspondence(first.use_.coedge)?;
        let b = second
            .domain
            .original_coedge_correspondence(second.use_.coedge)?;
        let (Some(a), Some(b)) = (a.binding(), b.binding()) else {
            return Ok(None);
        };
        if !std::ptr::eq(first.domain.definition(), second.domain.definition()) {
            return Ok(None);
        }
        let (first_use, first_edge) = native_coedge(
            first.domain.definition(),
            first.use_.face,
            first.use_.wire,
            first.use_.index,
        )?;
        let (second_use, second_edge) = native_coedge(
            second.domain.definition(),
            second.use_.face,
            second.use_.wire,
            second.use_.index,
        )?;
        if a.edge != b.edge
            || first.range != second.range
            || first.use_.orientation == second.use_.orientation
            || first_use.edge != second_use.edge
            || first_edge.vertices != second_edge.vertices
            || a.native_interval != b.native_interval
            || a.spatial_forward != b.spatial_forward
            || !first.domain.same_original_spatial_trace(
                first.use_.coedge,
                second.domain,
                second.use_.coedge,
                work,
                cancel,
            )?
        {
            return Ok(None);
        }
        for collar in [first, second] {
            if let CarrierTransport::Collar { inward, .. } = collar.transport {
                // The inward UV half must agree with the literal boundary
                // traversal. Otherwise the normal convention alone would
                // silently reverse the physical material half-neighbourhood.
                if (inward > 0.0) != (collar.spatial_orientation == collar.domain.orientation()) {
                    return Ok(None);
                }
            }
        }
        Ok(Some(Self))
    }
}

fn seam_transverse(
    collar: &OriginalCollar<'_, '_>,
    range: [f64; 2],
    curve: &CurveEnclosure,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Box3, GeometryError> {
    let uv = chart_jet(collar.domain, collar.use_.coedge, range, work, cancel)?;
    let surface = &collar.domain.surface_operand();
    (|| {
        let jet = surface_piecewise_first_enclosure(
            surface,
            uv_box(add_margin(uv.position, collar.margin)),
            work,
            cancel,
        )?;
        let (offset, correction) = match collar.transport {
            CarrierTransport::Exact => {
                let sign = if collar.domain.orientation() {
                    1.0
                } else {
                    -1.0
                };
                (rotate(uv.derivative, sign), [Interval::point(0.0); 3])
            }
            CarrierTransport::Collar { width, inward } => {
                let beta = collar.bubble(range)?.0;
                let offset = rotate(uv.derivative, inward)
                    .map(|v| mul(mul(Interval::point(width), beta), v));
                let correction = collar.forward_displacement(curve.position, jet.position)
                    .map_err(|_| GeometryError::new(
                        GeometryErrorCode::ParameterCorrespondenceUnavailable,
                        "original seam residual contradicts its retained whole-source forward bound",
                    ))?;
                (offset, correction)
            }
        };
        let across = compose_first(&jet.du, &jet.dv, offset);
        let across: Box3 = std::array::from_fn(|i| across[i].sub(correction[i]));
        Ok(
            if matches!(collar.transport, CarrierTransport::Collar { .. })
                && !collar.spatial_orientation
            {
                across.map(Interval::neg)
            } else {
                across
            },
        )
    })()
}

/// Both matrices have the SAME literal along-trace column, enclosing all its
/// one-sided derivatives and convex combinations. Keep their transverse
/// columns as a convex pair, not a Cartesian hull: at an ordinary box corner
/// the latter invents a zero column absent from the actual convex family.
fn paired_seam_normals(tangent: Box3, first: Box3, second: Box3) -> [Box3; 2] {
    [cross(tangent, first), cross(tangent, second)]
}

/// For each common T in the full interval column, cross(T,A_lambda) is LINEAR
/// in A_lambda=(1-lambda)A+lambda B. Applying the identical cofactor/radial
/// linear form then hulling its two scalar values encloses the whole family.
/// This is not authority from separately evaluated side normals: callers must
/// first seal the identical trace, common FIRST column and directed halves.
fn paired_radial_form(normals: [Box3; 2], radial: Box3) -> Interval {
    dot(normals[0], radial).hull(dot(normals[1], radial))
}
impl<'a, 'd> OriginalCollar<'a, 'd> {
    /// Complete source-only position cover, before preparing the shared C3D.
    /// The retained original forward receipt bounds delta=C3D-F(gamma).
    /// F(U) covers the full removed reference ribbon; padding by that residual
    /// covers every lambda*(1-w)*delta and hence all actual G homotopy points.
    /// This grants no embedding, source-error acceptance or chart authority.
    pub(super) fn source_relative_support(
        &self,
        range: [f64; 2],
        gamma: &ChartCurveEnclosure,
        allowance: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Box3>, GeometryError> {
        work.charge(1, cancel)?;
        if self
            .domain
            .original_coedge_correspondence(self.use_.coedge)?
            .provenance()
            == crate::kernel::trim::OriginalCorrespondenceProvenance::CompletedPoleGraph
        {
            // A completed physical pole graph is not a whole-forward receipt
            // for the literal UV expression used by this positional cover.
            return Ok(None);
        }
        if !self.position_error_mm.is_finite() || self.position_error_mm < 0.0 {
            return Ok(None);
        }
        let padding = add(
            Interval::point(self.position_error_mm),
            Interval::point(allowance),
        )
        .hi;
        if !padding.is_finite() {
            return Ok(None);
        }
        let tube = match self.transport {
            CarrierTransport::Exact => gamma.position,
            CarrierTransport::Collar { width, inward } => {
                let beta = self.bubble(range)?.0;
                let across = rotate(gamma.derivative, inward)
                    .map(|v| mul(mul(Interval::point(width), beta), v));
                std::array::from_fn(|i| {
                    add(
                        gamma.position[i],
                        mul(Interval { lo: 0.0, hi: 1.0 }, across[i]),
                    )
                })
            }
        };
        // This auxiliary UV rectangle need not lie inside the selected trim
        // envelope. Only coefficient-sheet positions are required for exclusion;
        // continuity, FIRST matrices and collar ownership are proved separately
        // if its complete support meets the closed certification scope.
        let position = self.domain.original_carrier_position_bounds(
            uv_box(add_margin(tube, self.margin)),
            work,
            cancel,
        )?;
        Ok(Some(position.map(|v| {
            add(
                v,
                Interval {
                    lo: -padding,
                    hi: padding,
                },
            )
        })))
    }

    fn reference_owned(
        &self,
        range: [f64; 2],
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<ReferenceRibbon<'a, 'd>>, GeometryError> {
        let before = work.consumed();
        let mut phase = "pure normal-coordinate embedding";
        let mut raw_coedge = None;
        let mut active_pair = None;
        let outcome = (|| {
            let Some(embedding) = self.embedding(range, [0.0, 1.0], work, cancel)? else {
                return Ok(None);
            };
            if matches!(self.transport, CarrierTransport::Exact) {
                return Ok(Some(ReferenceRibbon { embedding }));
            }
            let normal = Interval {
                lo: 0.0,
                hi: self.bubble(range)?.0.hi,
            };
            for coedge in 0..self.domain.coedges().len() {
                raw_coedge = Some(coedge);
                phase = "complete raw boundary cuts";
                let correspondence = self.domain.original_coedge_correspondence(coedge)?;
                let whole = correspondence.spatial_range();
                let cuts = original_coedge_parameter_cuts(
                    [(self.domain, coedge); 2],
                    remaining_bytes,
                    work,
                    cancel,
                )?;
                let mut start = whole[0];
                for end in cuts
                    .iter()
                    .copied()
                    .filter(|t| whole[0] < *t && *t < whole[1])
                    .chain(std::iter::once(whole[1]))
                {
                    let mut stack = [([0.0; 2], [0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                    let original_span_length = end - start;
                    stack[0] = (range, [start, end], 0);
                    start = end;
                    let mut pending = 1;
                    while pending != 0 {
                        pending -= 1;
                        let (a, b, depth) = stack[pending];
                        active_pair = Some((a, b, depth));
                        phase = "reference/raw original position pair";
                        work.charge(1, cancel)?;
                        let image = self.normal_image(a, normal, work, cancel)?;
                        // Literal original coefficients and their ORIGINAL native
                        // parameter map, never a source/prepared-UV chart alias.
                        let raw = self
                            .domain
                            .original_coedge_uv_piecewise_jet(coedge, b, work, cancel)?;
                        if (0..2).any(|i| {
                            image[i].hi < raw.position[i].lo || raw.position[i].hi < image[i].lo
                        }) {
                            continue;
                        }
                        if coedge == self.use_.coedge {
                            let hull = [a[0].min(b[0]), a[1].max(b[1])];
                            phase = "owning common normal-coordinate chart";
                            if self
                                .domain
                                .original_coedge_uv_c1_jet(coedge, hull, work, cancel)?
                                .is_some()
                                && self.normal_chart(hull, normal, work, cancel)?
                            {
                                // On this common injective (t,r) chart, equality
                                // with gamma(s)=U(s,0) forces r=0,t=s. Thus the
                                // ENTIRE open ribbon avoids its owning boundary,
                                // including unused pieces, without demanding false
                                // positive clearance at its exact shared trace.
                                continue;
                            }
                        }
                        if depth >= MAX_DEPTH {
                            return Ok(None);
                        }
                        let a_fraction = (a[1] - a[0]) / (range[1] - range[0]);
                        let b_fraction = (b[1] - b[0]) / original_span_length;
                        let split_a = a_fraction >= b_fraction;
                        let selected = if split_a { a } else { b };
                        let mid = selected[0] * 0.5 + selected[1] * 0.5;
                        if mid <= selected[0] || mid >= selected[1] {
                            return Ok(None);
                        }
                        if pending + 2 > stack.len() {
                            return Err(resource());
                        }
                        if split_a {
                            stack[pending] = ([mid, a[1]], b, depth + 1);
                            stack[pending + 1] = ([a[0], mid], b, depth + 1);
                        } else {
                            stack[pending] = (a, [mid, b[1]], depth + 1);
                            stack[pending + 1] = (a, [b[0], mid], depth + 1);
                        }
                        pending += 2;
                    }
                }
            }
            // Original completion images are not ordinary original coedges.
            // UV separation alone cannot exclude retained physical pole and
            // declared endpoint points outside S(joint.enclosure).
            for joint in self.domain.original_joints()? {
                phase = "complete original connector/world image exclusion";
                let joint_position =
                    original_joint_position_enclosure(self.domain, joint, work, cancel)?;
                let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                stack[0] = (range, 0);
                let mut pending = 1;
                while pending != 0 {
                    pending -= 1;
                    let (a, depth) = stack[pending];
                    work.charge(1, cancel)?;
                    let image = self.normal_image(a, normal, work, cancel)?;
                    let reference_position = surface_piecewise_first_enclosure(
                        &self.domain.surface_operand(),
                        uv_box(image),
                        work,
                        cancel,
                    )?
                    .position;
                    if position_boxes_disjoint(reference_position, joint_position) {
                        continue;
                    }
                    let mid = a[0] * 0.5 + a[1] * 0.5;
                    if depth >= MAX_DEPTH || mid <= a[0] || mid >= a[1] {
                        return Ok(None);
                    }
                    if pending + 2 > stack.len() {
                        return Err(resource());
                    }
                    stack[pending] = ([mid, a[1]], depth + 1);
                    stack[pending + 1] = ([a[0], mid], depth + 1);
                    pending += 2;
                }
            }
            // Every open reference point is boundary-free. A single strict Inside
            // calibration, including retained source/native uncertainty, fixes its
            // winding on the COMPLETE connected open ribbon.
            let t = range[0] * 0.5 + range[1] * 0.5;
            phase = "strict original Inside winding calibration";
            let beta = self.bubble([t; 2])?.0;
            let point =
                self.normal_image([t; 2], mul(beta, Interval::point(0.75)), work, cancel)?;
            if self.domain.classify_with_byte_limit(
                uv_box(add_margin(point, self.margin)),
                remaining_bytes.min(MAX_SCRATCH_BYTES),
                work,
                cancel,
            )? != TrimClassification::Inside
            {
                return Ok(None);
            }
            Ok(Some(ReferenceRibbon { embedding }))
        })();
        outcome.map_err(|error| work.annotate_error(error, before, format_args!(
            "closed radial reference ribbon owner={:?} phase={phase} raw_coedge={raw_coedge:?} pair={active_pair:?}",
            [self.use_.face, self.use_.wire, self.use_.index]
        )))
    }
}

struct RadialInventory<'v, 'f> {
    visitor: &'v mut BoundaryVisitor<'f>,
    outer_minimum: Option<f64>,
    inner_maximum: Option<f64>,
}

impl<'v, 'f> RadialInventory<'v, 'f> {
    fn new(visitor: &'v mut BoundaryVisitor<'f>) -> Self {
        Self {
            visitor,
            outer_minimum: None,
            inner_maximum: None,
        }
    }
    /// The cross product is formed from an interval FIRST-derivative matrix,
    /// not a hull of endpoint normals. It therefore includes every convex
    /// combination of one-sided matrices at native knots and intersections.
    fn include(
        &mut self,
        cell: BoundaryCell<'_>,
        normal: Box3,
        coordinates: &ActualMaterialCoordinates<'_>,
        can_refine: bool,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<BoundaryVisit, GeometryError> {
        let q = coordinates.position_relative_to_center_family(cell.position)?;
        let n = coordinates.normal(normal)?;
        let determinant = add(mul(n[0], q[0]), mul(n[1], q[1]));
        self.include_determinant(cell, q, determinant, can_refine, work, cancel)
    }

    #[allow(clippy::too_many_arguments)]
    fn include_seam(
        &mut self,
        _trace: &PairedTrace,
        position: Box3,
        tangent: Box3,
        first: Box3,
        second: Box3,
        coordinates: &ActualMaterialCoordinates<'_>,
        can_refine: bool,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let q = coordinates.position_relative_to_center_family(position)?;
        let [a, b] = paired_seam_normals(tangent, first, second);
        let determinant = paired_radial_form(
            [coordinates.normal(a)?, coordinates.normal(b)?],
            [q[0], q[1], Interval::point(0.0)],
        );
        Ok(self.include_determinant(
            BoundaryCell {
                position,
                original_face: None,
            },
            q,
            determinant,
            can_refine,
            work,
            cancel,
        )? == BoundaryVisit::Accepted)
    }

    fn include_determinant(
        &mut self,
        cell: BoundaryCell<'_>,
        q: Box3,
        determinant: Interval,
        can_refine: bool,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<BoundaryVisit, GeometryError> {
        let radius = add(square(q[0]), square(q[1]));
        if radius.lo <= 0.0 {
            return Ok(BoundaryVisit::RefineSpatial);
        }
        let inward = if determinant.lo > 0.0 {
            false
        } else if determinant.hi < 0.0 {
            true
        } else {
            return Ok(BoundaryVisit::RefineSpatial);
        };
        let visited = (self.visitor)(cell, inward, can_refine, work, cancel)?;
        if visited != BoundaryVisit::Accepted {
            return Ok(visited);
        }
        if inward {
            self.inner_maximum = Some(self.inner_maximum.map_or(radius.hi, |r| r.max(radius.hi)));
        } else {
            self.outer_minimum = Some(self.outer_minimum.map_or(radius.lo, |r| r.min(radius.lo)));
        }
        Ok(BoundaryVisit::Accepted)
    }

    fn ordered(&self) -> bool {
        match (self.inner_maximum, self.outer_minimum) {
            (Some(inner), Some(outer)) => inner < outer,
            (None, Some(_)) => true,
            _ => false,
        }
    }
}

fn visit_original_faces(
    source: MaterialSourceView<'_, '_>,
    incidence: &SourceEdgeIncidence,
    links: &ClosedRadialLinks<'_>,
    coordinates: &ActualMaterialCoordinates<'_>,
    remaining_bytes: usize,
    inventory: &mut RadialInventory<'_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let interval = coordinates.world_interval();
    let direction = coordinates.affine_rows()[2];
    let height = HeightFunctional {
        direction,
        offset: coordinates.height_at_anchor().sub(dot(
            direction,
            coordinates.source_anchor().map(Interval::point),
        )),
    };
    let allowance = source_allowance(source.definition())?;
    let uncertainty = height.dot(
        [Interval {
            lo: -allowance,
            hi: allowance,
        }; 3],
    );
    // A source point that can enter the closed scope under the admitted source
    // displacement must lie in this expanded functional preimage. This is a
    // necessary parameter cover, never a change to the requested cuts or G.
    let target = Interval {
        lo: interval[0],
        hi: interval[1],
    }
    .sub(uncertainty);
    let target = [target.lo, target.hi];
    let height_functional = SurfaceFunctional {
        direction: height.direction,
        origin: [Interval::point(0.0); 3],
        offset: height.offset,
    };
    for (fi, _) in source.definition().face_ids().iter().enumerate() {
        let domain = source.face_domain(fi, remaining_bytes, work, cancel)?;
        let surface = &domain.surface_operand();
        let orientation = domain.orientation();
        let before = work.consumed();
        let mut phase = "carrier continuity";
        let mut active_uv = domain.uv_domain();
        let mut active_depth = 0;
        let completed = (|| -> Result<bool, GeometryError> {
            for (joint_index, joint) in domain.original_joints()?.iter().enumerate() {
                phase = "original connector/world image closed-scope exclusion";
                active_uv = joint.enclosure.map(|value| [value.lo, value.hi]);
                let position = original_joint_position_enclosure(domain, joint, work, cancel)?;
                // As for raw F, retain any original point that can reach the
                // CLOSED scope under the admitted source displacement.
                if outside(height.height(position), target) {
                    continue;
                }
                phase = "bound closed source/reference/G and whole original joint image";
                if !links.qualified_original_joint(
                    source,
                    incidence,
                    domain,
                    joint_index,
                    remaining_bytes,
                    work,
                    cancel,
                )? {
                    return Ok(false);
                }
            }
            phase = "carrier continuity";
            // Admit continuity/Lipschitzness on the WHOLE closed native domain
            // before excluding any carrier by height. Discontinuous knots and
            // nested poles remain real Native errors. Only this immutable
            // coefficient proof is reusable; every partial jet below is fresh.
            let whole =
                surface_piecewise_first_enclosure(surface, domain.uv_domain(), work, cancel)?;
            // Original connector/world images need not be subsets of S(domain),
            // so their independent closed-image gates above must run first.
            if outside(height.height(whole.position), target) {
                return Ok(true);
            }
            (|| -> Result<bool, GeometryError> {
                let sphere_chart = source_chart::SphereChart::new(surface, domain, work, cancel)?;
                for span in surface_parameter_boxes(surface, domain.uv_domain(), work, cancel)? {
                    let mut stack =
                        [([[0.0; 2]; 2], 0_u8, false, true, false); MAX_DEPTH as usize + 1];
                    stack[0] = (span, 0, false, true, false);
                    let mut pending = 1;
                    while pending != 0 {
                        pending -= 1;
                        let (uv, depth, height_contained, recontract_height, mut trim_inside) =
                            stack[pending];
                        active_uv = uv;
                        active_depth = depth;
                        work.charge(1, cancel)?;
                        let (uv, jet, height_contained, strong_contraction) = if !height_contained
                            && recontract_height
                        {
                            phase = "raw F closed-height preimage contraction";
                            let clipped = crate::analysis::slab::clip_height_parameter_box(
                                surface, uv, height, target, work, cancel,
                            );
                            let Some(clipped) = clipped? else {
                                continue;
                            };
                            phase = "raw F contracted carrier first jet";
                            let jet =
                                surface_piecewise_first_enclosure(surface, clipped, work, cancel);
                            let jet = jet?;
                            let h = height.height(jet.position);
                            let contained = target[0] <= h.lo && h.hi <= target[1];
                            // A strong contraction can profitably repeat before
                            // tiling a height-invariant direction. This half-width
                            // trigger schedules work only; it proves no occupancy.
                            let strong = (0..2).any(|axis| {
                                let before = uv[axis][1] - uv[axis][0];
                                let after = clipped[axis][1] - clipped[axis][0];
                                before.is_finite() && before > 0.0 && after <= before * 0.5
                            });
                            (clipped, jet, contained, strong)
                        } else {
                            // Subsets retain the necessary preimage even when
                            // contraction is not scheduled. Only full image
                            // containment is proof and inherited as such. Every
                            // child still receives its fresh complete carrier jet.
                            phase = "raw F preconditioned carrier enclosure";
                            let jet = surface_piecewise_first_enclosure(surface, uv, work, cancel);
                            let jet = jet?;
                            let h = height.height(jet.position);
                            (
                                uv,
                                jet,
                                height_contained || target[0] <= h.lo && h.hi <= target[1],
                                false,
                            )
                        };
                        active_uv = uv;
                        if outside(coordinates.position(jet.position)?[2], interval) {
                            continue;
                        }
                        let can_refine = depth < MAX_DEPTH
                            && uv.iter().any(|range| {
                                let mid = range[0] * 0.5 + range[1] * 0.5;
                                range[0] < mid && mid < range[1]
                            });
                        phase = "raw F cylinder determinant/boundary visitor";
                        let n = if let Some(chart) = &sphere_chart {
                            // This independently qualified source chart retains
                            // its geometric normal at collapsed UV poles and
                            // already includes processor and face orientation.
                            chart.normal(jet.position)?
                        } else {
                            let n = cross(jet.du, jet.dv);
                            if orientation { n } else { n.map(Interval::neg) }
                        };
                        let included = inventory.include(
                            BoundaryCell {
                                position: jet.position,
                                original_face: Some((*surface, uv)),
                            },
                            n,
                            coordinates,
                            can_refine,
                            work,
                            cancel,
                        );
                        let included = included?;
                        if included == BoundaryVisit::Accepted {
                            continue;
                        }
                        // Successful full-carrier determinant and boundary
                        // witnesses cover every original trimmed point in this
                        // box. Keep winding as an exclusion for failed cells:
                        // a box intersecting a curved trim can otherwise spend
                        // the entire winding subdivision budget to return only
                        // Boundary, even though all F obligations already hold.
                        // The original joint/link inventory and signed native
                        // fibre census remain independent and mandatory.
                        phase = "raw F unresolved-cell trim exclusion";
                        if !trim_inside {
                            let classification =
                                domain.classify_with_byte_limit(uv, remaining_bytes, work, cancel);
                            match classification? {
                                TrimClassification::Outside => continue,
                                TrimClassification::Inside => trim_inside = true,
                                TrimClassification::Boundary => {}
                            }
                        }
                        if !can_refine {
                            return Ok(false);
                        }
                        let children = if included == BoundaryVisit::RefineActualXy {
                            // Only the actual-coordinate metric asks for this goal.
                            // Its XY error has no vertical box-diameter obligation.
                            // Unresolved radius/determinant cells always retain
                            // spatial refinement, and every child is requalified.
                            let rows = coordinates.affine_rows();
                            let du = [dot(rows[0], jet.du), dot(rows[1], jet.du)];
                            let dv = [dot(rows[0], jet.dv), dot(rows[1], jet.dv)];
                            crate::analysis::critical::split_metric_derivatives(
                                uv, depth, fi, &du, &dv,
                            )?
                        } else {
                            crate::analysis::critical::split_metric_derivatives(
                                uv, depth, fi, &jet.du, &jet.dv,
                            )?
                        };
                        if pending + children.len() > stack.len() {
                            return Err(resource());
                        }
                        let recontract_height = if height_contained {
                            false
                        } else if strong_contraction {
                            true
                        } else {
                            let axis = usize::from(children[0][0] == uv[0]);
                            let derivative = height.dot(if axis == 0 { jet.du } else { jet.dv });
                            if derivative.lo == 0.0 && derivative.hi == 0.0 {
                                false
                            } else {
                                // Fold the height covector into original native
                                // coefficients before choosing work. This retains
                                // exact axial cancellations lost by spatial boxes.
                                phase = "raw F height-sensitive refinement scheduling";
                                let gradient = surface_functional_enclosures(
                                    surface,
                                    uv,
                                    &[height_functional],
                                    work,
                                    cancel,
                                );
                                let [gradient] = gradient?;
                                let derivative = if axis == 0 { gradient.du } else { gradient.dv };
                                derivative.lo != 0.0 || derivative.hi != 0.0
                            }
                        };
                        for child in children {
                            // Inside is a whole-box proof inherited only by subsets;
                            // neither Boundary nor failed metric evidence is reused.
                            stack[pending] = (
                                child,
                                depth + 1,
                                height_contained,
                                recontract_height,
                                trim_inside,
                            );
                            pending += 1;
                        }
                    }
                }
                Ok(true)
            })()
        })();
        let completed = completed.map_err(|error| work.annotate_error(error, before, format_args!(
            "closed radial atlas original face={fi} phase={phase} uv={active_uv:?} depth={active_depth}"
        )))?;
        if !completed {
            return Ok(false);
        }
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn visit_original_links(
    source: MaterialSourceView<'_, '_>,
    incidence: &SourceEdgeIncidence,
    coordinates: &ActualMaterialCoordinates<'_>,
    links: &mut ClosedRadialLinks<'_>,
    remaining_bytes: usize,
    inventory: &mut RadialInventory<'_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let before = work.consumed();
    let mut phase = "paired source domains";
    let mut owners = None;
    let mut active_spatial = None;
    let mut last_geometry_refusal = None;
    let outcome = (|| {
        let allowance = source_allowance(source.definition())?;
        let interval = coordinates.world_interval();
        for uses in incidence.values() {
            active_spatial = None;
            last_geometry_refusal = None;
            owners = Some([
                [uses.first.face, uses.first.wire, uses.first.index],
                uses.second
                    .map(|u| [u.face, u.wire, u.index])
                    .unwrap_or([usize::MAX; 3]),
            ]);
            phase = "paired source domains";
            work.charge(1, cancel)?;
            let Some(second) = uses.second else {
                return Ok(false);
            };
            let available = remaining_bytes
                .checked_sub(links.allocated_bytes())
                .ok_or_else(resource)?;
            let first_domain = source.face_domain(uses.first.face, available, work, cancel)?;
            // Both domains are borrowed from the single prepared source owner;
            // their retained buffers are not query-local allocations.
            let second_domain = source.face_domain(second.face, available, work, cancel)?;
            phase = "immutable collar operands";
            let Some(first) =
                OriginalCollar::for_enclosure(first_domain, uses.first, work, cancel)?
            else {
                return Ok(false);
            };
            let Some(second) = OriginalCollar::for_enclosure(second_domain, second, work, cancel)?
            else {
                return Ok(false);
            };
            phase = "literal common trace/directed halves";
            let Some(_trace) = PairedTrace::qualify(&first, &second, work, cancel)? else {
                return Ok(false);
            };
            let binding = first_domain
                .original_coedge_correspondence(first.use_.coedge)?
                .binding()
                .expect("qualified paired original trace");
            let central = links.central_spatial_range(
                binding.edge,
                first.range,
                [first.use_.face, first.use_.wire, first.use_.index],
            )?;
            active_spatial = Some((central, 0));
            let inactive_pair = {
                phase = "literal full gamma C1 jets";
                let Some(first_gamma) = first_domain.original_coedge_uv_c1_jet(
                    first.use_.coedge,
                    central,
                    work,
                    cancel,
                )?
                else {
                    return Ok(false);
                };
                let Some(second_gamma) = second_domain.original_coedge_uv_c1_jet(
                    second.use_.coedge,
                    central,
                    work,
                    cancel,
                )?
                else {
                    return Ok(false);
                };
                phase = "complete source-relative paired reference/G support exclusion";
                let first_support = first.source_relative_support(
                    central,
                    &first_gamma,
                    allowance,
                    work,
                    cancel,
                )?;
                if let Some(a) = first_support {
                    if outside(coordinates.position(a)?[2], interval) {
                        match second.source_relative_support(
                            central,
                            &second_gamma,
                            allowance,
                            work,
                            cancel,
                        )? {
                            Some(b) => outside(coordinates.position(b)?[2], interval),
                            None => false,
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            };
            if inactive_pair {
                // Both COMPLETE changed supports miss the CLOSED scope for every
                // captured coordinate member, hence also its interior census
                // fibre. No inactive support becomes a root or whole-source proof.
                continue;
            }
            for collar in [&first, &second] {
                phase = "complete reference ribbon ownership";
                let reference = collar.reference_owned(central, available, work, cancel);
                let Some(reference) = reference? else {
                    return Ok(false);
                };
                if !reference.covers(collar, central) {
                    return Err(GeometryError::new(
                        GeometryErrorCode::KernelFailure,
                        "reference ribbon lost its exact source/map/across binding",
                    ));
                }
            }
            phase = "actual G inner ownership";
            let first_embedding = first.owned_embedding(central, available, work, cancel);
            let Some(first_embedding) = first_embedding? else {
                return Ok(false);
            };
            let second_embedding = second.owned_embedding(central, available, work, cancel);
            let Some(second_embedding) = second_embedding? else {
                return Ok(false);
            };
            phase = "complete original coedge cuts";
            let cuts = original_coedge_parameter_cuts(
                [
                    (first_domain, first.use_.coedge),
                    (second_domain, second.use_.coedge),
                ],
                available,
                work,
                cancel,
            )?;
            let cut_bytes = cuts
                .capacity()
                .checked_mul(std::mem::size_of::<f64>())
                .ok_or_else(resource)?;
            let available = available.checked_sub(cut_bytes).ok_or_else(resource)?;
            phase = "shared original spatial curve preparation";
            let completed = first_domain.with_original_spatial_curve(
                first.use_.coedge,
                available,
                work,
                cancel,
                |prepared, work, cancel| -> Result<bool, GeometryError> {
                    // The identical original spatial operator supplies both
                    // trace columns in SOURCE gauge, including every one-sided
                    // closed derivative. A discontinuous trace cannot be glued.
                    if !crate::parametric::curve_parameter_span_continuous(
                        prepared, central, work, cancel,
                    )? {
                        return Ok(false);
                    }
                    crate::parametric::curve_enclosure(
                        prepared,
                        (central[0], central[1]),
                        work,
                        cancel,
                    )?;
                    // A crossing terminal needs the EXACT qualified original
                    // NativeV/edge/absolute-germ/exit, not vertex activity. Its
                    // cone outer boundary is literally boundary_jet(exit,w)
                    // of this same collar; source qualification owns the full
                    // removed disk and its unchanged/paired-G cutover, with
                    // opposed uses sharing the exit. Closed cone G has already
                    // passed rank/radius/metric; the central closed cells and
                    // common-FIRST seam below must still all pass.
                    for (germ, endpoint) in central.into_iter().enumerate() {
                        phase = "closed terminal exclusion or qualified literal cutover";
                        active_spatial = Some(([endpoint; 2], 0));
                        let curve = crate::parametric::curve_enclosure(
                            prepared,
                            (endpoint, endpoint),
                            work,
                            cancel,
                        )?;
                        let qualified = links.qualified_closed_cutover(
                            uses.vertices[binding.native_endpoint_index(germ == 1)],
                            binding.edge,
                            germ == 1,
                            endpoint,
                        );
                        for collar in [&first, &second] {
                            let support = collar.swept_enclosure(
                                [endpoint; 2],
                                &curve,
                                allowance,
                                work,
                                cancel,
                            )?;
                            if !outside(coordinates.position(support)?[2], interval) && !qualified {
                                return Ok(false);
                            }
                        }
                    }
                    let mut start = central[0];
                    for end in cuts
                        .iter()
                        .copied()
                        .filter(|t| central[0] < *t && *t < central[1])
                        .chain(std::iter::once(central[1]))
                    {
                        let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                        stack[0] = ([start, end], 0);
                        start = end;
                        let mut pending = 1;
                        while pending != 0 {
                            pending -= 1;
                            let (range, depth) = stack[pending];
                            last_geometry_refusal = None;
                            active_spatial = Some((range, depth));
                            phase = "shared C3D and full reference/G support";
                            work.charge(1, cancel)?;
                            let mid = range[0] * 0.5 + range[1] * 0.5;
                            let can_refine = depth < MAX_DEPTH && range[0] < mid && mid < range[1];
                            let curve = crate::parametric::curve_enclosure(
                                prepared,
                                (range[0], range[1]),
                                work,
                                cancel,
                            )?;
                            let supports = [
                                first.swept_enclosure(range, &curve, allowance, work, cancel)?,
                                second.swept_enclosure(range, &curve, allowance, work, cancel)?,
                            ];
                            let mut accepted = true;
                            for ((collar, embedding), support) in
                                [(&first, &first_embedding), (&second, &second_embedding)]
                                    .into_iter()
                                    .zip(supports)
                            {
                                if outside(coordinates.position(support)?[2], interval) {
                                    continue;
                                }
                                phase = "all-lambda actual G first matrix/foreign collars";
                                let cell = if matches!(collar.transport, CarrierTransport::Exact) {
                                    let uv = chart_jet(
                                        collar.domain,
                                        collar.use_.coedge,
                                        range,
                                        work,
                                        cancel,
                                    )?;
                                    let jet = surface_piecewise_first_enclosure(
                                        &collar.domain.surface_operand(),
                                        uv_box(add_margin(uv.position, collar.margin)),
                                        work,
                                        cancel,
                                    )?;
                                    let outward = cross(jet.du, jet.dv);
                                    let outward = if collar.domain.orientation() {
                                        outward
                                    } else {
                                        outward.map(Interval::neg)
                                    };
                                    Some(MaterialLinkCell {
                                        position: curve.position,
                                        source_error_mm: 0.0,
                                        convex_link: false,
                                        outward,
                                        second_outward: outward,
                                    })
                                } else {
                                    match collar.geometry_cell_proof_kind(
                                        embedding, range, &curve, allowance, true, work, cancel,
                                    )? {
                                        Ok(cell) => Some(cell),
                                        Err(refusal) => {
                                            last_geometry_refusal =
                                                Some((range, collar.use_.face, refusal));
                                            None
                                        }
                                    }
                                };
                                let Some(cell) = cell else {
                                    accepted = false;
                                    break;
                                };
                                phase = "actual G cylinder determinant/boundary visitor";
                                let included = inventory.include(
                                    BoundaryCell {
                                        position: cell.position,
                                        original_face: None,
                                    },
                                    cell.outward,
                                    coordinates,
                                    can_refine,
                                    work,
                                    cancel,
                                );
                                if included? != BoundaryVisit::Accepted {
                                    accepted = false;
                                    break;
                                }
                            }
                            if accepted
                                && !outside(coordinates.position(curve.position)?[2], interval)
                            {
                                phase = "paired common FIRST matrix seam";
                                let a = seam_transverse(&first, range, &curve, work, cancel)?;
                                let b = seam_transverse(&second, range, &curve, work, cancel)?;
                                let included = inventory.include_seam(
                                    &_trace,
                                    curve.position,
                                    curve.derivative,
                                    a,
                                    b,
                                    coordinates,
                                    can_refine,
                                    work,
                                    cancel,
                                );
                                accepted = included?;
                            }
                            if accepted {
                                phase = "complete removed reference/G support admission";
                                // Pre-admit growth against ALL coexistent storage,
                                // including exits already retained by links. Record
                                // removed references even when displacement cost is zero.
                                let limit = remaining_bytes
                                    .checked_sub(cut_bytes)
                                    .ok_or_else(resource)?;
                                for (collar, support) in [&first, &second].into_iter().zip(supports)
                                {
                                    if matches!(collar.transport, CarrierTransport::Collar { .. }) {
                                        work.charge(1, cancel)?;
                                        links.record_support_with_byte_limit(support, limit)?;
                                    }
                                }
                                continue;
                            }
                            if !can_refine {
                                return Ok(false);
                            }
                            if pending + 2 > stack.len() {
                                return Err(resource());
                            }
                            stack[pending] = ([mid, range[1]], depth + 1);
                            stack[pending + 1] = ([range[0], mid], depth + 1);
                            pending += 2;
                        }
                    }
                    Ok(true)
                },
            )?;
            if !completed {
                return Ok(false);
            }
        }
        Ok(true)
    })();
    outcome.map_err(|error| {
        // The propagated cause must precede phase context and a sufficient-
        // premise refusal, so bounded operator errors cannot hide the cause.
        let consumed = work.consumed();
        GeometryError::new(error.code, format!(
            "{}; closed radial atlas paired links owners={owners:?} phase={phase} spatial={active_spatial:?} work_before={before} work_consumed={consumed} work_delta={} work_remaining={} last_geometry_refusal={last_geometry_refusal:?}",
            error.message, consumed.saturating_sub(before), work.remaining(),
        ))
    })
}

fn cylinder_fiber(coordinates: &ActualMaterialCoordinates<'_>, alpha: f64) -> fiber::OriginalFiber {
    let rows = coordinates.affine_rows();
    let eta = coordinates.center_offset_family();
    let a = Interval::point(alpha);
    fiber::OriginalFiber {
        center: coordinates.source_anchor(),
        rows: [
            std::array::from_fn(|i| rows[1][i].sub(mul(a, rows[0][i]))),
            rows[2],
        ],
        offsets: [
            eta[1].sub(mul(a, eta[0])),
            coordinates
                .height_at_anchor()
                .sub(Interval::point(coordinates.sample_world_z())),
        ],
        forward: std::array::from_fn(|i| add(rows[0][i], mul(a, rows[1][i]))),
        forward_offset: add(eta[0], mul(a, eta[1])),
        min_forward: 0.0,
        core: None,
    }
}

pub(super) fn certify(
    source: MaterialSourceView<'_, '_>,
    request: &SlabRequest,
    coordinates: &ActualMaterialCoordinates<'_>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    visitor: &mut BoundaryVisitor<'_>,
) -> Result<Option<RadialRegionKind>, GeometryError> {
    let before = work.consumed();
    let mut phase = "borrowed prepared native source";
    let outcome = (|| {
        work.charge(1, cancel)?;
        let available = remaining_bytes
            .checked_sub(STACK_BYTES)
            .ok_or_else(resource)?;
        phase = "original edge/vertex incidence";
        let mut incidence = SourceEdgeIncidence::new(source.definition(), available, work, cancel)?;
        incidence.prepare_vertices(available, work, cancel)?;
        let available = available
            .checked_sub(incidence.owned_bytes())
            .ok_or_else(resource)?;
        let mut inventory = RadialInventory::new(visitor);
        phase = "complete closed source-vertex atlas and changed support inventory";
        let Some(mut links) = ClosedRadialLinks::for_closed_atlas(
            source,
            &incidence,
            coordinates,
            available,
            work,
            cancel,
            &mut |_face, position, normal, can_refine, work, cancel| {
                inventory.include(
                    BoundaryCell {
                        position,
                        original_face: None,
                    },
                    normal,
                    coordinates,
                    can_refine,
                    work,
                    cancel,
                )
            },
        )?
        else {
            return Ok(None);
        };
        phase = "complete original F overcover";
        let faces = visit_original_faces(
            source,
            &incidence,
            &links,
            coordinates,
            available
                .checked_sub(links.allocated_bytes())
                .ok_or_else(resource)?,
            &mut inventory,
            work,
            cancel,
        );
        if !faces? {
            return Ok(None);
        }
        phase = "paired reference/actual G atlas";
        let links_complete = visit_original_links(
            source,
            &incidence,
            coordinates,
            &mut links,
            available,
            &mut inventory,
            work,
            cancel,
        );
        if !links_complete? || !inventory.ordered() {
            return Ok(None);
        }
        let anchor = coordinates.source_anchor();
        let rows = coordinates.affine_rows();
        let interval = coordinates.world_interval();
        let sample = coordinates.sample_world_z();
        let height = HeightFunctional {
            direction: rows[2],
            offset: coordinates
                .height_at_anchor()
                .sub(dot(rows[2], anchor.map(Interval::point))),
        };
        let census_request = SlabRequest {
            frame: request.frame,
            height_interval_mm: interval,
            sample_height_mm: sample,
            native_shape_error_mm: request.native_shape_error_mm,
        };
        for alpha in [0.0, 1.0, -1.0, 0.5, -0.5, 2.0, -2.0, 0.2718281828459045] {
            phase = "all-face signed original cylinder census";
            work.charge(1, cancel)?;
            let fiber = cylinder_fiber(coordinates, alpha);
            if !links.misses_all_supports(&fiber, work, cancel)? {
                continue;
            }
            let census = fiber::original_cylinder_fiber_census(
                source,
                &fiber,
                &census_request,
                height,
                available
                    .checked_sub(links.allocated_bytes())
                    .ok_or_else(resource)?,
                work,
                cancel,
            );
            if let Some(census) = census? {
                let kind = if census.inner_forward().is_some() {
                    RadialRegionKind::Annulus
                } else {
                    RadialRegionKind::Disk
                };
                if (kind == RadialRegionKind::Annulus) != inventory.inner_maximum.is_some() {
                    return Ok(None);
                }
                // Complete compact glued charts, strict closed-cut regularity and
                // the exhaustive SAME physical cylinder fibre give proper local
                // homeomorphisms of total degree one for every role. The full
                // boundary inventory and outward material crossing then fill the
                // disk, or the strictly ordered interval between the two graphs.
                return Ok(Some(kind));
            }
        }
        Ok(None)
    })();
    outcome.map_err(|error| {
        work.annotate_error(
            error,
            before,
            format_args!(
                "closed radial material product phase={phase} scope={:?} sample={}",
                coordinates.world_interval(),
                coordinates.sample_world_z()
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_first_column_preserves_rectangular_corner_convex_family() {
        let tangent = [0.0, 0.0, 1.0].map(Interval::point);
        let a = [0.0, -1.0, 0.0].map(Interval::point);
        let b = [1.0, 0.0, 0.0].map(Interval::point);
        let radial = [5.0, 10.0, 0.0].map(Interval::point);
        let cartesian = std::array::from_fn(|i| a[i].hull(b[i]));
        assert!(dot(cross(tangent, cartesian), radial).contains_zero());
        let paired = paired_radial_form(paired_seam_normals(tangent, a, b), radial);
        assert!(paired.lo > 0.0);
        assert!(paired.lo <= 5.0 && paired.hi >= 10.0);
        // A real reversed half-chart still includes zero. The convex pair
        // removes artificial Cartesian vertices, not the all-lambda gate.
        assert!(
            paired_radial_form(
                paired_seam_normals(tangent, a, a.map(Interval::neg)),
                radial
            )
            .contains_zero()
        );
    }

    #[test]
    fn first_matrix_convexification_refuses_same_normal_cusp() {
        let y = [0.0, 1.0, 0.0].map(Interval::point);
        let z = [0.0, 0.0, 1.0].map(Interval::point);
        // Both one-sided normals equal +X, yet both parameter derivatives
        // reverse. Their convex FIRST matrix contains the zero matrix.
        assert!(cross(y, z)[0].lo > 0.0);
        assert!(cross(y.map(Interval::neg), z.map(Interval::neg))[0].lo > 0.0);
        let tangent = std::array::from_fn(|i| y[i].hull(y[i].neg()));
        assert!(
            paired_radial_form(
                paired_seam_normals(tangent, z, z.map(Interval::neg)),
                [1.0, 0.0, 0.0].map(Interval::point)
            )
            .contains_zero()
        );
    }

    #[test]
    fn common_trace_requires_opposed_directed_halves_not_two_side_signs() {
        let tangent = [0.0, 1.0, 0.0].map(Interval::point);
        let transverse = [0.0, 0.0, 1.0].map(Interval::point);
        let radial = [1.0, 0.0, 0.0].map(Interval::point);
        assert!(
            paired_radial_form(
                paired_seam_normals(
                    tangent,
                    transverse,
                    transverse.map(|v| mul(Interval::point(2.0), v))
                ),
                radial
            )
            .lo > 0.0
        );
        assert!(
            paired_radial_form(
                paired_seam_normals(tangent, transverse, transverse.map(Interval::neg)),
                radial
            )
            .contains_zero()
        );
        assert!(
            paired_radial_form(
                paired_seam_normals([Interval::point(0.0); 3], transverse, transverse),
                radial
            )
            .contains_zero()
        );
    }

    #[test]
    fn closed_cut_exclusion_is_strict_on_both_sides() {
        let scope = [2.0, 5.0];
        for support in [
            Interval { lo: 1.0, hi: 2.0 },
            Interval { lo: 5.0, hi: 6.0 },
            Interval { lo: 2.0, hi: 5.0 },
        ] {
            assert!(!outside(support, scope));
        }
        assert!(outside(
            Interval {
                lo: 1.0,
                hi: down(2.0)
            },
            scope
        ));
        assert!(outside(
            Interval {
                lo: up(5.0),
                hi: 6.0
            },
            scope
        ));
        // The UV surface image can miss the scope while a retained physical
        // connector/world image touches its CLOSED cut.
        let uv_image = Interval { lo: 0.0, hi: 1.0 };
        let actual_world = uv_image.hull(Interval::point(scope[0]));
        assert!(outside(uv_image, scope));
        assert!(!outside(actual_world, scope));
    }

    #[test]
    fn physical_joint_exclusion_keeps_closed_world_contacts() {
        let reference = [Interval { lo: 0.0, hi: 1.0 }; 3];
        let uv_surface_image = [Interval { lo: 2.0, hi: 3.0 }; 3];
        assert!(position_boxes_disjoint(reference, uv_surface_image));
        let retained_world = [Interval::point(1.0); 3];
        let complete =
            std::array::from_fn(|axis| uv_surface_image[axis].hull(retained_world[axis]));
        assert!(!position_boxes_disjoint(reference, complete));
    }
}

#[cfg(test)]
mod center_family_tests {
    use super::*;

    #[test]
    fn independent_center_family_changes_both_angular_and_forward_ownership() {
        let cancel = AtomicBool::new(false);
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/geometry/box-mm.step"),
        )
        .unwrap();
        let definition = crate::from_step_bytes_with_cancel(&bytes, &cancel).unwrap();
        let bounds = definition.bounds_mm();
        let z = bounds.min[2] * 0.5 + bounds.max[2] * 0.5;
        let frame = PlaneFrameMm {
            origin_mm: [1.0e20, 0.0, 0.0],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
        };
        let rows = ActualMaterialCoordinates::source_rows(&frame).unwrap();
        let mut work = GeometryWorkBudget::new(100);
        let coordinates = ActualMaterialCoordinates::source(
            &definition,
            &frame,
            &rows,
            [bounds.min[2], bounds.max[2]],
            z,
            &mut work,
            &cancel,
        )
        .unwrap();
        let eta = coordinates.center_offset_family();
        assert!(eta[0].lo < -1.0 && eta[0].hi > 1.0);
        let fiber = cylinder_fiber(&coordinates, 1.0);
        // This support misses the intended-center angular line, but NOT the
        // independent center-family line. Omitting eta would falsely exclude it.
        let angular_support = [0.0, 1.0, 0.0].map(Interval::point);
        assert!(!dot(fiber.rows[0], angular_support).contains_zero());
        assert!(add(dot(fiber.rows[0], angular_support), fiber.offsets[0]).contains_zero());
        // Likewise a midpoint-forward point cannot be owned uniformly over all
        // centers: the forward covector needs its own matching eta offset.
        let anchor = coordinates.source_anchor();
        let forward_support = [anchor[0] + 0.5, anchor[1], anchor[2]].map(Interval::point);
        assert!(dot(fiber.forward, [0.5, 0.0, 0.0].map(Interval::point)).lo > 0.0);
        assert!(fiber::fiber_forward(&fiber, forward_support).contains_zero());
    }
}

#[cfg(test)]
mod boundary_terminal_tests {
    use super::*;

    #[test]
    fn metric_terminal_signal_preserves_error_without_relabeling_topology_refusal() {
        let position = [2.0, 0.0, 0.5].map(Interval::point);
        let mut refinable_calls = 0;
        let mut terminal_calls = 0;
        let mut work = GeometryWorkBudget::new(100);
        let cancel = AtomicBool::new(false);
        {
            let mut visitor = |_: BoundaryCell<'_>,
                               _: bool,
                               can_refine: bool,
                               _: &mut GeometryWorkBudget,
                               _: &AtomicBool| {
                if can_refine {
                    refinable_calls += 1;
                    Ok(BoundaryVisit::RefineActualXy)
                } else {
                    terminal_calls += 1;
                    Err(GeometryError::new(
                        GeometryErrorCode::ShapeErrorExceeded,
                        "qualified terminal boundary metric exceeds requested bound",
                    ))
                }
            };
            let mut inventory = RadialInventory::new(&mut visitor);
            // An unresolved cylinder determinant never reaches the metric:
            // even at a terminal cell it remains a topology refusal.
            assert_eq!(
                inventory
                    .include_determinant(
                        BoundaryCell {
                            position,
                            original_face: None
                        },
                        position,
                        Interval::point(0.0),
                        false,
                        &mut work,
                        &cancel
                    )
                    .unwrap(),
                BoundaryVisit::RefineSpatial
            );
            assert_eq!(
                inventory
                    .include_determinant(
                        BoundaryCell {
                            position,
                            original_face: None
                        },
                        position,
                        Interval::point(1.0),
                        true,
                        &mut work,
                        &cancel
                    )
                    .unwrap(),
                BoundaryVisit::RefineActualXy
            );
            let error = inventory
                .include_determinant(
                    BoundaryCell {
                        position,
                        original_face: None,
                    },
                    position,
                    Interval::point(1.0),
                    false,
                    &mut work,
                    &cancel,
                )
                .unwrap_err();
            assert_eq!(error.code, GeometryErrorCode::ShapeErrorExceeded);
        }
        assert_eq!(refinable_calls, 1);
        assert_eq!(terminal_calls, 1);
    }
}
