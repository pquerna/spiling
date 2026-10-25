// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Original-use, bounded material transport. These receipts never assert C1 or
//! an exact common carrier normal. The immutable recipe is source-relative;
//! changing a query's radial witness cannot change its boundary realization.
//!
//! Coedge slots are flattened across every original wire, while owner indices
//! remain wire-local. Scoped UV receipts bind both identities and the retained
//! parameter map; neither a matching curve nor an enclosure-only recipe grants
//! another occurrence's embedding authority.

use super::super::source_incidence::CoedgeUse;
use super::*;
use crate::parametric::{
    ChartCurveEnclosure, CurveEnclosure, SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES,
    SurfaceFirstEnclosure, surface_piecewise_first_enclosure,
};

mod atlas;
mod coverage;
mod fiber;
mod owned_uv_ribbon;
mod radial_product;
mod transport;
mod vertex_fan;
mod vertex_transport;
mod wall;
pub(super) use coverage::{ClosedRadialLinks, MAX_CORRECTION_BYTES, source_f_point_is_retained};
use owned_uv_ribbon::OwnedUvRibbon;
pub(in crate::analysis) use radial_product::{
    BoundaryCell, BoundaryVisit, ClosedRadialMaterialProduct, RadialRegionKind,
    certify_closed_radial_material_product,
    certify_closed_radial_material_product_with_boundary_visitor,
};
pub(crate) use transport::{
    SourceTransportOwner, SourceTransportSupport, source_transport_projection_cover,
    source_transport_supports,
};
pub(in crate::analysis) use vertex_transport::OriginalFaceCornerAttachments;
pub(crate) use vertex_transport::PreparedSourceVertexPlans;
pub(crate) use wall::prove_source_wall_component;

pub(super) struct MaterialLinkCell {
    pub position: [Interval; 3],
    pub source_error_mm: f64,
    pub convex_link: bool,
    outward: [Interval; 3],
    second_outward: [Interval; 3],
}

/// Refinable proof refusals, kept distinct from hard geometry/work failures.
/// Diagnostics name source occurrences and stages, never Native allocation IDs.
pub(super) enum MaterialLinkRefusal {
    ConstructorSourceEmbedding,
    LocalGeometry {
        owner: [usize; 3],
        proof: LocalGeometryRefusal,
    },
    RadialProjection,
    OutwardRadialDot,
}

impl std::fmt::Debug for MaterialLinkRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConstructorSourceEmbedding => f.write_str("ConstructorSourceEmbedding"),
            Self::LocalGeometry { owner, proof } => f
                .debug_struct("LocalGeometry")
                .field("owner", owner)
                .field("proof", proof)
                .finish(),
            Self::RadialProjection => f.write_str("RadialProjection"),
            Self::OutwardRadialDot => f.write_str("OutwardRadialDot"),
        }
    }
}

/// Failed sufficient premises, not assertions that the fixed G ribbon folds.
/// The interval operands distinguish source/ownership uncertainty from failure
/// of the actual transported first jet without changing any recipe operand.
pub(super) enum LocalGeometryRefusal {
    ExactCarrierNormal {
        normal: [Interval; 3],
    },
    EndpointBubble {
        beta: Interval,
    },
    ForeignCollar {
        coedge: usize,
        uv_parameter_range: [f64; 2],
        owned: [Interval; 2],
        other: [Interval; 2],
    },
    ForwardResidual {
        spatial: [Interval; 3],
        carrier: [Interval; 3],
        admitted_error_mm: f64,
    },
    SourceError {
        error_mm: f64,
        allowance_mm: f64,
    },
    TransportJacobian {
        uv: [Interval; 2],
        margin_uv: [f64; 2],
        width: f64,
        beta: Interval,
        displacement: [Interval; 3],
        displacement_t: [Interval; 3],
        tangent: [Interval; 3],
        across: [Interval; 3],
        normal: [Interval; 3],
    },
}

impl std::fmt::Debug for LocalGeometryRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExactCarrierNormal { normal } => f
                .debug_struct("ExactCarrierNormal")
                .field("normal", normal)
                .finish(),
            Self::EndpointBubble { beta } => f
                .debug_struct("EndpointBubble")
                .field("beta", beta)
                .finish(),
            Self::ForeignCollar {
                coedge,
                uv_parameter_range,
                owned,
                other,
            } => f
                .debug_struct("ForeignCollar")
                .field("coedge", coedge)
                .field("uv_parameter_range", uv_parameter_range)
                .field("owned", owned)
                .field("other", other)
                .finish(),
            Self::ForwardResidual {
                spatial,
                carrier,
                admitted_error_mm,
            } => f
                .debug_struct("ForwardResidual")
                .field("spatial", spatial)
                .field("carrier", carrier)
                .field("admitted_error_mm", admitted_error_mm)
                .finish(),
            Self::SourceError {
                error_mm,
                allowance_mm,
            } => f
                .debug_struct("SourceError")
                .field("error_mm", error_mm)
                .field("allowance_mm", allowance_mm)
                .finish(),
            Self::TransportJacobian {
                uv,
                margin_uv,
                width,
                beta,
                displacement,
                displacement_t,
                tangent,
                across,
                normal,
            } => f
                .debug_struct("TransportJacobian")
                .field("uv", uv)
                .field("margin_uv", margin_uv)
                .field("width", width)
                .field("beta", beta)
                .field("displacement", displacement)
                .field("displacement_t", displacement_t)
                .field("tangent", tangent)
                .field("across", across)
                .field("normal", normal)
                .finish(),
        }
    }
}

impl MaterialLinkCell {
    pub(super) fn regular_flow_rate(
        &self,
        tangent: [Interval; 3],
        height: [Interval; 3],
        vertical: [Interval; 3],
    ) -> Result<Option<f64>, GeometryError> {
        let mut rate = material_edge_flow_rate(tangent, height, vertical)?;
        include_flow_rate(
            &mut rate,
            material_face_flow_rate(self.outward, height, vertical)?,
        );
        include_flow_rate(
            &mut rate,
            material_face_flow_rate(self.second_outward, height, vertical)?,
        );
        Ok(rate)
    }
}

/// Actual cross-ribbon position and first jet at ONE selected original spatial parameter.
/// All fields use the unchanged original coefficient/map operands.
#[derive(Debug, Clone, Copy)]
struct CollarBoundaryJet {
    uv_offset: [Interval; 2],
    uv: [Interval; 2],
    position: [Interval; 3],
    derivative: [Interval; 3],
    boundary_displacement: [Interval; 3],
    constant: bool,
}

fn exact_difference(a: Interval, b: Interval) -> Interval {
    if a.lo == a.hi && b.lo == b.hi && a.lo == b.lo {
        Interval::point(0.0)
    } else {
        a.sub(b)
    }
}

fn rotate(vector: [Interval; 2], inward: f64) -> [Interval; 2] {
    [
        mul(vector[1].neg(), Interval::point(inward)),
        mul(vector[0], Interval::point(inward)),
    ]
}
fn determinant(a: [Interval; 2], b: [Interval; 2]) -> Interval {
    add(mul(a[0], b[1]), mul(a[1], b[0]).neg())
}
fn chart_jet(
    domain: &PreparedFaceTrimDomain<'_>,
    coedge: usize,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ChartCurveEnclosure, GeometryError> {
    domain.original_coedge_uv_jet(coedge, range, work, cancel)
}

/// The recipe's SAME spatial-parameter beta=x(1-x), restricted to a proved
/// subinterval. In particular an endpoint subdivision must not keep the
/// unrelated whole-edge upper bound 1/4.
fn spatial_endpoint_bubble(
    whole: [f64; 2],
    parameter: Interval,
) -> Result<Interval, GeometryError> {
    if !whole[0].is_finite()
        || !whole[1].is_finite()
        || whole[0] >= whole[1]
        || parameter.lo < whole[0]
        || parameter.hi > whole[1]
    {
        return Err(GeometryError::new(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "foreign collar bubble leaves its original spatial parameter range",
        ));
    }
    let span = Interval::point(whole[1]).sub(Interval::point(whole[0]));
    let normalized = parameter.sub(Interval::point(whole[0])).div(span)?;
    // Scope supplies this independent exact polynomial range, not a tolerance.
    intersection(
        mul(normalized, Interval::point(1.0).sub(normalized)),
        Interval { lo: 0.0, hi: 0.25 },
    )
    .ok_or_else(|| {
        GeometryError::new(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "foreign collar bubble contradicts its original affine parameter scope",
        )
    })
}

fn add_margin(position: [Interval; 2], margin: [f64; 2]) -> [Interval; 2] {
    std::array::from_fn(|axis| {
        add(
            position[axis],
            Interval {
                lo: -margin[axis],
                hi: margin[axis],
            },
        )
    })
}
fn uv_box(position: [Interval; 2]) -> [[f64; 2]; 2] {
    position.map(|value| [value.lo, value.hi])
}
fn compose_first(du: &[Interval; 3], dv: &[Interval; 3], tangent: [Interval; 2]) -> [Interval; 3] {
    std::array::from_fn(|axis| add(mul(du[axis], tangent[0]), mul(dv[axis], tangent[1])))
}
fn source_allowance(definition: &Definition) -> Result<f64, GeometryError> {
    definition
        .provenance
        .uncertainty_mm
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "source has no finite admitted material-realization allowance",
                0,
                (),
            )
        })
}

/// Pointwise scalar law for an entire realized face, not an embedding receipt.
///
/// The source carrier, every original boundary curve and every NativeV must
/// have the SAME exact scalar. Then the collar's affine blend and the selected
/// conical completion preserve that scalar identically. An interval merely
/// containing the candidate height is insufficient, even at roundoff scale.
pub(crate) fn constant_material_face_height(
    source: MaterialSourceView<'_, '_>,
    face_index: usize,
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<f64>, GeometryError> {
    let domain = source.face_domain(face_index, MAX_SCRATCH_BYTES, work, cancel)?;
    let Some(height) = super::super::slab::constant_spline_height(
        &domain.surface_operand(),
        functional,
        work,
        cancel,
    )?
    else {
        return Ok(None);
    };
    if !height.lo.is_finite() || height.lo != height.hi {
        return Ok(None);
    }
    for (index, coedge) in domain.coedges().iter().enumerate() {
        work.charge(1, cancel)?;
        let curve_height = domain.with_original_spatial_curve(
            index,
            MAX_SCRATCH_BYTES,
            work,
            cancel,
            |curve, work, cancel| {
                super::super::slab::constant_native_curve_height(curve, functional, work, cancel)
            },
        )?;
        if curve_height.is_none_or(|value| value.lo != height.lo || value.hi != height.hi) {
            return Ok(None);
        }
        // The admitted native use circuits are closed, so their start vertices
        // cover every incident apex without checking each twice. The coordinate
        // is the exact original VertexPoint from the complete admitted star.
        let vertex = coedge.vertices[usize::from(!coedge.forward)];
        let point = source
            .trim_source()
            .source_vertex_point(vertex, work, cancel)?;
        let value = functional.vertex(point);
        if value.lo != height.lo || value.hi != height.hi {
            return Ok(None);
        }
    }
    Ok(Some(height.lo))
}

/// One source-only choice of a complete coedge collar. The range, transverse
/// field and width depend exclusively on retained original coefficients. Query
/// refinement cannot select a different realization at a shared height.
#[derive(Clone, Copy, PartialEq)]
enum CarrierTransport {
    Exact,
    Collar { width: f64, inward: f64 },
}
struct OriginalCollar<'a, 'd> {
    domain: &'a PreparedFaceTrimDomain<'d>,
    use_: CoedgeUse,
    /// Increasing selected spatial t follows the material boundary traversal.
    spatial_orientation: bool,
    range: [f64; 2],
    transport: CarrierTransport,
    margin: [f64; 2],
    position_error_mm: f64,
}

/// Source-owned proof only: these operands never select or modify G. A
/// positive-width collar excludes the collapsed original endpoints; NativeV
/// owns that quotient separately. The borrowed domain fixes the exact retained
/// owner and parameter map, not just a geometrically similar curve.
struct UvRibbonEmbedding<'a, 'd> {
    domain: &'a PreparedFaceTrimDomain<'d>,
    use_: CoedgeUse,
    source_range: [f64; 2],
    uv_range: [f64; 2],
    map_derivative: [f64; 2],
    transport: CarrierTransport,
    range: [f64; 2],
    across: [f64; 2],
}
impl UvRibbonEmbedding<'_, '_> {
    fn covers(&self, collar: &OriginalCollar<'_, '_>, range: [f64; 2], across: [f64; 2]) -> bool {
        let Ok(proof) = collar
            .domain
            .original_coedge_correspondence(collar.use_.coedge)
        else {
            return false;
        };
        let slope = proof.uv_parameter_derivative();
        std::ptr::eq(self.domain, collar.domain)
            && self.use_ == collar.use_
            && self.source_range == collar.range
            && self.uv_range == proof.uv_range()
            && self.map_derivative == [slope.lo, slope.hi]
            && self.transport == collar.transport
            && contained_parameter_range(range, self.range)
            && contained_parameter_range(across, self.across)
    }
}

fn contained_parameter_range(inner: [f64; 2], outer: [f64; 2]) -> bool {
    inner[0].is_finite()
        && inner[1].is_finite()
        && outer[0] <= inner[0]
        && inner[0] <= inner[1]
        && inner[1] <= outer[1]
}

type UvPairCell = ([f64; 2], [f64; 2], u8);
pub(super) const UV_EMBEDDING_STACK_BYTES: usize = std::mem::size_of::<
    [UvPairCell; 2 * MAX_DEPTH as usize + 1],
>() + if SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES
    > crate::kernel::source_operand::SOURCE_OPERAND_SCRATCH_BYTES
{
    SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES
} else {
    crate::kernel::source_operand::SOURCE_OPERAND_SCRATCH_BYTES
} + 2
    * std::mem::size_of::<SurfaceFirstEnclosure>();

fn normal_uv_jet(
    jet: &ChartCurveEnclosure,
    width: f64,
    inward: f64,
    normal: Interval,
) -> ([Interval; 2], [Interval; 2]) {
    let transverse = rotate(jet.derivative, inward).map(|value| mul(Interval::point(width), value));
    let bend = rotate(jet.second_derivative, inward);
    let longitudinal = std::array::from_fn(|axis| {
        add(
            jet.derivative[axis],
            mul(mul(Interval::point(width), normal), bend[axis]),
        )
    });
    (longitudinal, transverse)
}

/// Fixed output rows T and inward*JT. All principal minors of the transformed
/// normal-coordinate Jacobian are positive on a rectangular proof superset.
/// Native's strict UV jet evaluator refuses genuine internal C0 knots.
fn normal_uv_pmatrix(
    jet: &ChartCurveEnclosure,
    midpoint_tangent: [Interval; 2],
    width: f64,
    inward: f64,
    normal: Interval,
) -> bool {
    let tangent = midpoint_tangent.map(|value| Interval::point(value.lo * 0.5 + value.hi * 0.5));
    let row_normal = rotate(tangent, inward);
    let (ut, ur) = normal_uv_jet(jet, width, inward, normal);
    let along = add(mul(tangent[0], ut[0]), mul(tangent[1], ut[1]));
    let across = add(mul(row_normal[0], ur[0]), mul(row_normal[1], ur[1]));
    along.lo > 0.0 && across.lo > 0.0 && mul(determinant(ut, ur), Interval::point(inward)).lo > 0.0
}
impl<'a, 'd> OriginalCollar<'a, 'd> {
    /// Evaluate the immutable recipe without granting material-chart authority.
    /// In particular the endpoint bubble always uses the WHOLE spatial query range.
    fn for_enclosure(
        domain: &'a PreparedFaceTrimDomain<'d>,
        use_: CoedgeUse,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        Self::for_enclosure_with_byte_limit(domain, use_, usize::MAX, work, cancel)
    }

    /// The native classifier consumes scratch only. The PreparedSolid owner
    /// already accounts for every retained domain; callers provide the current
    /// coexistent scratch allowance without adding that metadata again.
    fn for_enclosure_with_byte_limit(
        domain: &'a PreparedFaceTrimDomain<'d>,
        use_: CoedgeUse,
        remaining_scratch_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        work.charge(1, cancel)?;
        let proof = domain.original_coedge_correspondence(use_.coedge)?;
        let Some(binding) = proof.binding() else {
            return Ok(None);
        };
        let Some(coedge) = domain.coedges().get(use_.coedge) else {
            return Ok(None);
        };
        if binding.owner != [use_.face, use_.wire, use_.index]
            || binding.edge != coedge.edge
            || binding.face != domain.face()
            || binding.edgeuse != use_.edgeuse
            || binding.endpoints != coedge.vertices
            || binding.native_interval != coedge.native_range
            || use_.edgeuse != coedge.edgeuse
            || use_.orientation != (coedge.forward == domain.native_orientation())
            || binding.material_forward != (use_.orientation == binding.spatial_forward)
        {
            return Ok(None);
        }
        let spatial_orientation = binding.material_forward;
        let range = proof.spatial_range();
        let margin = proof.margin_uv();
        if proof.provenance()
            == crate::kernel::trim::OriginalCorrespondenceProvenance::ExactCoefficients
            && proof.position_error_mm() == 0.0
        {
            return Ok(Some(Self {
                domain,
                use_,
                spatial_orientation,
                range,
                transport: CarrierTransport::Exact,
                margin,
                position_error_mm: 0.0,
            }));
        }
        let width =
            (range[1] - range[0]) * domain.material_realization().collar_parameter_fraction();
        if !width.is_finite() || width <= 0.0 {
            return Ok(None);
        }
        let midpoint = range[0] * 0.5 + range[1] * 0.5;
        let middle = chart_jet(domain, use_.coedge, [midpoint; 2], work, cancel)?;
        let offset =
            rotate(middle.derivative, 1.0).map(|value| mul(value, Interval::point(width * 0.125)));
        let mut inside = None;
        for sign in [1.0, -1.0] {
            let probe = std::array::from_fn(|axis| {
                add(
                    middle.position[axis],
                    mul(offset[axis], Interval::point(sign)),
                )
            });
            if domain.classify_with_byte_limit(
                uv_box(add_margin(probe, margin)),
                remaining_scratch_bytes,
                work,
                cancel,
            )? == TrimClassification::Inside
            {
                if inside.is_some() {
                    return Ok(None);
                }
                inside = Some(sign);
            }
        }
        let Some(inward) = inside else {
            return Ok(None);
        };
        Ok(Some(Self {
            domain,
            use_,
            spatial_orientation,
            range,
            transport: CarrierTransport::Collar { width, inward },
            margin,
            position_error_mm: proof.position_error_mm(),
        }))
    }

    fn normal_chart(
        &self,
        range: [f64; 2],
        normal: Interval,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let CarrierTransport::Collar { width, inward } = self.transport else {
            return Ok(true);
        };
        let midpoint = range[0] * 0.5 + range[1] * 0.5;
        let middle = chart_jet(self.domain, self.use_.coedge, [midpoint; 2], work, cancel)?;
        let whole = chart_jet(self.domain, self.use_.coedge, range, work, cancel)?;
        Ok(normal_uv_pmatrix(
            &whole,
            middle.derivative,
            width,
            inward,
            normal,
        ))
    }

    /// `range` uses the selected ORIGINAL C3D query gauge, not the
    /// original PCurve parameter inventory returned by the trim domain.
    fn normal_image(
        &self,
        range: [f64; 2],
        normal: Interval,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 2], GeometryError> {
        let jet = chart_jet(self.domain, self.use_.coedge, range, work, cancel)?;
        let CarrierTransport::Collar { width, inward } = self.transport else {
            return Ok(jet.position);
        };
        let offset = rotate(jet.derivative, inward);
        Ok(std::array::from_fn(|axis| {
            add(
                jet.position[axis],
                mul(mul(Interval::point(width), normal), offset[axis]),
            )
        }))
    }

    /// Prove the fixed ribbon only on its source-owned used scope. Put
    /// r=beta(t)*w: U=gamma+k*r*Jgamma', Ut=gamma'+k*r*Jgamma'',
    /// Ur=k*Jgamma'. The original (t,w)->(t,r) Jacobian is
    /// [[1,0],[beta'(t)*w,beta(t)]], INCLUDING its shear, with det beta>0.
    /// Consequently this is an exact change of coordinates, not removal of
    /// beta' from the actual G jets below.
    fn embedding(
        &self,
        range: [f64; 2],
        across: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<UvRibbonEmbedding<'a, 'd>>, GeometryError> {
        work.charge(1, cancel)?;
        if !contained_parameter_range(range, self.range)
            || !contained_parameter_range(across, [0.0, 1.0])
        {
            return Ok(None);
        }
        let proof = self
            .domain
            .original_coedge_correspondence(self.use_.coedge)?;
        if proof.provenance()
            == crate::kernel::trim::OriginalCorrespondenceProvenance::CompletedPoleGraph
        {
            return Ok(None);
        }
        if matches!(self.transport, CarrierTransport::Collar { .. }) {
            // Collapsed NativeV endpoints are not a strict two-dimensional
            // chart. Their original quotient ownership is a separate receipt.
            let beta = self.bubble(range)?.0;
            if beta.lo <= 0.0 {
                return Ok(None);
            }
            let normal = Interval {
                lo: 0.0,
                hi: mul(beta, Interval::new(across[0], across[1])?).hi,
            };
            // Try one generic tangent(mid), perpendicular P chart FIRST.
            // If that cannot cover a closed curved ribbon, exhaust the
            // triangular parameter-pair domain: LL, LR, RR. Each pair either
            // shares a single P chart on its HULL or its complete images miss.
            // Adjacent charts alone would not exclude a nonlocal crossing.
            let mut stack = [([0.0; 2], [0.0; 2], 0_u8); 2 * MAX_DEPTH as usize + 1];
            stack[0] = (range, range, 0);
            let mut pending = 1;
            while pending != 0 {
                pending -= 1;
                let (a, b, depth) = stack[pending];
                work.charge(1, cancel)?;
                if a != b {
                    let image_a = self.normal_image(a, normal, work, cancel)?;
                    let image_b = self.normal_image(b, normal, work, cancel)?;
                    if (0..2).any(|axis| {
                        image_a[axis].hi < image_b[axis].lo || image_b[axis].hi < image_a[axis].lo
                    }) {
                        continue;
                    }
                }
                if self.normal_chart([a[0].min(b[0]), a[1].max(b[1])], normal, work, cancel)? {
                    continue;
                }
                if depth >= MAX_DEPTH {
                    return Ok(None);
                }
                if a == b {
                    let midpoint = a[0] * 0.5 + a[1] * 0.5;
                    if midpoint <= a[0] || midpoint >= a[1] {
                        return Ok(None);
                    }
                    let left = [a[0], midpoint];
                    let right = [midpoint, a[1]];
                    stack[pending] = (right, right, depth + 1);
                    stack[pending + 1] = (left, right, depth + 1);
                    stack[pending + 2] = (left, left, depth + 1);
                    pending += 3;
                } else {
                    let split_a = a[1] - a[0] >= b[1] - b[0];
                    let split = if split_a { a } else { b };
                    let midpoint = split[0] * 0.5 + split[1] * 0.5;
                    if midpoint <= split[0] || midpoint >= split[1] {
                        return Ok(None);
                    }
                    let left = [split[0], midpoint];
                    let right = [midpoint, split[1]];
                    stack[pending] = if split_a {
                        (left, b, depth + 1)
                    } else {
                        (a, left, depth + 1)
                    };
                    stack[pending + 1] = if split_a {
                        (right, b, depth + 1)
                    } else {
                        (a, right, depth + 1)
                    };
                    pending += 2;
                }
            }
        }
        let slope = proof.uv_parameter_derivative();
        Ok(Some(UvRibbonEmbedding {
            domain: self.domain,
            use_: self.use_,
            source_range: self.range,
            uv_range: proof.uv_range(),
            map_derivative: [slope.lo, slope.hi],
            transport: self.transport,
            range,
            across,
        }))
    }

    fn bubble(&self, range: [f64; 2]) -> Result<(Interval, Interval), GeometryError> {
        let span = Interval::point(self.range[1]).sub(Interval::point(self.range[0]));
        let normalized = Interval::new(range[0], range[1])?
            .sub(Interval::point(self.range[0]))
            .div(span)?;
        // The interval expression is intersected with the independent exact
        // range bound of x(1-x), not a rounding or geometric tolerance clamp.
        let beta = intersection(
            mul(normalized, Interval::point(1.0).sub(normalized)),
            Interval { lo: 0.0, hi: 0.25 },
        )
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "material collar parameter leaves its original range",
                self.use_.face,
                range,
            )
        })?;
        let beta_t = Interval::point(1.0)
            .sub(mul(Interval::point(2.0), normalized))
            .div(span)?;
        Ok((beta, beta_t))
    }

    /// Position-only covering for activity and regular-fiber exclusion. This
    /// grants no embedding or material authority.
    fn swept_enclosure(
        &self,
        range: [f64; 2],
        curve: &CurveEnclosure,
        allowance: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 3], GeometryError> {
        let CarrierTransport::Collar { width, inward } = self.transport else {
            return Ok(curve.position.map(|value| {
                add(
                    value,
                    Interval {
                        lo: -allowance,
                        hi: allowance,
                    },
                )
            }));
        };
        let uv = chart_jet(self.domain, self.use_.coedge, range, work, cancel)?;
        let (beta, _) = self.bubble(range)?;
        let v = rotate(uv.derivative, inward);
        let tube = std::array::from_fn(|axis| {
            add(
                uv.position[axis],
                mul(Interval { lo: 0.0, hi: width }, mul(beta, v[axis])),
            )
        });
        {
            let surface = &self.domain.surface_operand();
            Ok(surface_piecewise_first_enclosure(
                surface,
                uv_box(add_margin(tube, self.margin)),
                work,
                cancel,
            )?
            .position
            .map(|value| {
                add(
                    value,
                    Interval {
                        lo: -allowance,
                        hi: allowance,
                    },
                )
            }))
        }
    }

    fn other_collar_enclosure(
        &self,
        coedge: usize,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 2], GeometryError> {
        Self::other_collar_overcover(self.domain, coedge, range, work, cancel)
    }

    /// UV-only exclusion overcover of the immutable recipe, for both inward
    /// signs. This does not select a material side or construct a collar.
    fn other_collar_overcover(
        domain: &PreparedFaceTrimDomain<'_>,
        coedge: usize,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 2], GeometryError> {
        let position = domain
            .boundary_position_enclosure(coedge, range, work, cancel)?
            .map(|value| Interval {
                lo: value[0],
                hi: value[1],
            });
        let proof = domain.original_coedge_correspondence(coedge)?;
        if proof.provenance()
            == crate::kernel::trim::OriginalCorrespondenceProvenance::ExactCoefficients
            && proof.position_error_mm() == 0.0
        {
            return Ok(position);
        }
        let jet = crate::parametric::curve2d_piecewise_jet(
            &domain.coedge_operand(coedge)?,
            range,
            work,
            cancel,
        )?;
        let derivative = jet
            .derivative
            .map(|value| mul(value, proof.uv_parameter_derivative()));
        let v = rotate(derivative, 1.0);
        let spatial_range = proof.spatial_range();
        let width = (spatial_range[1] - spatial_range[0])
            * domain.material_realization().collar_parameter_fraction();
        let beta = match proof.affine_spatial_parameter_preimage(range)? {
            Some(parameter) => spatial_endpoint_bubble(spatial_range, parameter)?,
            // This correspondence kind does not grant affine preimage
            // authority. Keep the complete old enclosure, never infer a map.
            None => Interval { lo: 0.0, hi: 0.25 },
        };
        let offset = mul(
            Interval::point(width),
            Interval {
                lo: -beta.hi,
                hi: beta.hi,
            },
        );
        // Both inward signs cover its already fixed source-only recipe.
        // Refining this ORIGINAL parameter span also refines its SAME beta;
        // source UV margin and every nonowning use remain in the enclosure.
        Ok(std::array::from_fn(|axis| {
            add(position[axis], mul(offset, v[axis]))
        }))
    }

    /// The same two-sign exclusion overcover in the sealed spatial query gauge.
    /// Exact correspondences retain their literal zero-offset realization.
    fn spatial_collar_overcover(
        domain: &PreparedFaceTrimDomain<'_>,
        coedge: usize,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 2], GeometryError> {
        let jet = chart_jet(domain, coedge, range, work, cancel)?;
        let proof = domain.original_coedge_correspondence(coedge)?;
        if proof.provenance()
            == crate::kernel::trim::OriginalCorrespondenceProvenance::ExactCoefficients
            && proof.position_error_mm() == 0.0
        {
            return Ok(jet.position);
        }
        let spatial_range = proof.spatial_range();
        let width = (spatial_range[1] - spatial_range[0])
            * domain.material_realization().collar_parameter_fraction();
        let beta = spatial_endpoint_bubble(spatial_range, Interval::new(range[0], range[1])?)?;
        let offset = mul(
            Interval::point(width),
            Interval {
                lo: -beta.hi,
                hi: beta.hi,
            },
        );
        let transverse = rotate(jet.derivative, 1.0);
        Ok(std::array::from_fn(|axis| {
            add(jet.position[axis], mul(offset, transverse[axis]))
        }))
    }

    /// Literal central G(t,w), including both CLOSED across limits. First
    /// derivatives schedule C0 range refinement only; this proves neither
    /// original-F ownership nor a physical distance to an original face.
    fn position_first_jet(
        &self,
        parameter: [[f64; 2]; 2],
        curve: &CurveEnclosure,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<([Interval; 3], [Interval; 3], [Interval; 3]), GeometryError> {
        work.charge(1, cancel)?;
        let [range, across] = parameter;
        if !contained_parameter_range(range, self.range)
            || !contained_parameter_range(across, [0.0, 1.0])
        {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "actual-G central cell leaves its fixed native/across recipe",
                self.use_.face,
                parameter,
            ));
        }
        let CarrierTransport::Collar { width, inward } = self.transport else {
            return Ok((curve.position, curve.derivative, [Interval::point(0.0); 3]));
        };
        let uv = chart_jet(self.domain, self.use_.coedge, range, work, cancel)?;
        let (beta, beta_t) = self.bubble(range)?;
        let across = Interval::new(across[0], across[1])?;
        let transverse = rotate(uv.derivative, inward);
        let transverse_t = rotate(uv.second_derivative, inward);
        let offset = transverse.map(|value| mul(mul(Interval::point(width), beta), value));
        let offset_t: [Interval; 2] = std::array::from_fn(|axis| {
            mul(
                Interval::point(width),
                add(mul(beta_t, transverse[axis]), mul(beta, transverse_t[axis])),
            )
        });
        let at = std::array::from_fn(|axis| add(uv.position[axis], mul(across, offset[axis])));
        let along =
            std::array::from_fn(|axis| add(uv.derivative[axis], mul(across, offset_t[axis])));
        let weight = intersection(
            Interval::point(1.0).sub(across),
            Interval { lo: 0.0, hi: 1.0 },
        )
        .ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "actual-G central across weight has no unit image",
            )
        })?;
        {
            let surface = &self.domain.surface_operand();
            let boundary = surface_piecewise_first_enclosure(
                surface,
                uv_box(add_margin(uv.position, self.margin)),
                work,
                cancel,
            )?;
            let base = surface_piecewise_first_enclosure(
                surface,
                uv_box(add_margin(at, self.margin)),
                work,
                cancel,
            )?;
            // This is the retained coupled C3D(t)-S(gamma(t)) defining
            // residual, not a claimed correspondence to trimmed Original F.
            let displacement = self
                .forward_displacement(curve.position, boundary.position)
                .map_err(|_| {
                    GeometryError::new(
                        GeometryErrorCode::ParameterCorrespondenceUnavailable,
                        "actual-G central formula contradicts its retained coupled residual",
                    )
                })?;
            let boundary_t = compose_first(&boundary.du, &boundary.dv, uv.derivative);
            let base_t = compose_first(&base.du, &base.dv, along);
            let base_w = compose_first(&base.du, &base.dv, offset);
            Ok((
                std::array::from_fn(|axis| {
                    add(base.position[axis], mul(weight, displacement[axis]))
                }),
                std::array::from_fn(|axis| {
                    add(
                        base_t[axis],
                        mul(
                            weight,
                            exact_difference(curve.derivative[axis], boundary_t[axis]),
                        ),
                    )
                }),
                std::array::from_fn(|axis| exact_difference(base_w[axis], displacement[axis])),
            ))
        }
    }

    fn boundary_jet(
        &self,
        parameter: f64,
        across: [f64; 2],
        curve: &CurveEnclosure,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<CollarBoundaryJet, GeometryError> {
        work.charge(1, cancel)?;
        if !across[0].is_finite()
            || !across[1].is_finite()
            || across[0] < 0.0
            || across[1] > 1.0
            || across[0] > across[1]
        {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "source collar cross-section leaves its fixed recipe",
                self.use_.face,
                across,
            ));
        }
        let across = Interval::new(across[0], across[1])?;
        let uv = chart_jet(self.domain, self.use_.coedge, [parameter; 2], work, cancel)?;
        let CarrierTransport::Collar { width, inward } = self.transport else {
            return Ok(CollarBoundaryJet {
                uv_offset: [Interval::point(0.0); 2],
                uv: uv.position,
                position: curve.position,
                derivative: [Interval::point(0.0); 3],
                boundary_displacement: [Interval::point(0.0); 3],
                constant: true,
            });
        };
        let (beta, _) = self.bubble([parameter; 2])?;
        let offset = rotate(uv.derivative, inward)
            .map(|value| mul(mul(Interval::point(width), beta), value));
        let at = std::array::from_fn(|axis| add(uv.position[axis], mul(across, offset[axis])));
        {
            let surface = &self.domain.surface_operand();
            let boundary = surface_enclosure(
                surface,
                uv_box(add_margin(uv.position, self.margin)),
                work,
                cancel,
            )?;
            let base =
                surface_enclosure(surface, uv_box(add_margin(at, self.margin)), work, cancel)?;
            let displacement = std::array::from_fn(|axis| {
                intersection(
                    exact_difference(curve.position[axis], boundary.position[axis]),
                    Interval {
                        lo: -self.position_error_mm,
                        hi: self.position_error_mm,
                    },
                )
            });
            let [Some(dx), Some(dy), Some(dz)] = displacement else {
                return Err(failure(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original collar endpoint contradicts retained forward bound",
                    self.use_.face,
                    parameter,
                ));
            };
            let displacement = [dx, dy, dz];
            // Across was validated in [0,1] before this independently known
            // affine range is used; this never clips unproved coordinates.
            let weight = intersection(
                Interval::point(1.0).sub(across),
                Interval { lo: 0.0, hi: 1.0 },
            )
            .ok_or_else(|| {
                failure(
                    GeometryErrorCode::InvalidGeometry,
                    "source collar weight has no unit image",
                    self.use_.face,
                    across,
                )
            })?;
            let position = std::array::from_fn(|axis| {
                add(base.position[axis], mul(weight, displacement[axis]))
            });
            let first = compose_first(&base.du, &base.dv, offset);
            let derivative =
                std::array::from_fn(|axis| exact_difference(first[axis], displacement[axis]));
            Ok(CollarBoundaryJet {
                uv_offset: offset,
                uv: at,
                position,
                derivative,
                boundary_displacement: displacement,
                constant: false,
            })
        }
    }

    fn cell(
        &self,
        embedding: &OwnedUvRibbon<'_, '_>,
        range: [f64; 2],
        curve: &CurveEnclosure,
        prism: &SamplePrism,
        allowance: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Result<MaterialLinkCell, MaterialLinkRefusal>, GeometryError> {
        let cell =
            match self.geometry_cell_proof(embedding, range, curve, allowance, work, cancel)? {
                Ok(cell) => cell,
                Err(proof) => {
                    return Ok(Err(MaterialLinkRefusal::LocalGeometry {
                        owner: [self.use_.face, self.use_.wire, self.use_.index],
                        proof,
                    }));
                }
            };
        let Some((radial, _)) = prism.radial_projection_frame(cell.position)? else {
            return Ok(Err(MaterialLinkRefusal::RadialProjection));
        };
        if dot(cell.outward, radial).lo <= 0.0 {
            return Ok(Err(MaterialLinkRefusal::OutwardRadialDot));
        }
        Ok(Ok(cell))
    }

    /// The actual source-owned ribbon, independent of a certification field.
    /// A caller must still prove its physical projection and global degree.
    fn geometry_cell(
        &self,
        embedding: &OwnedUvRibbon<'_, '_>,
        range: [f64; 2],
        curve: &CurveEnclosure,
        allowance: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<MaterialLinkCell>, GeometryError> {
        Ok(self
            .geometry_cell_proof(embedding, range, curve, allowance, work, cancel)?
            .ok())
    }

    /// Both bounds enclose the SAME coupled C3D(t)-S(gamma(t)) residual.
    /// Independent range subtraction alone loses this whole-source coefficient
    /// authority and can swamp a valid transverse collar as t varies.
    fn forward_displacement(
        &self,
        spatial: [Interval; 3],
        carrier: [Interval; 3],
    ) -> Result<[Interval; 3], LocalGeometryRefusal> {
        let displacement = std::array::from_fn(|axis| {
            intersection(
                exact_difference(spatial[axis], carrier[axis]),
                Interval {
                    lo: -self.position_error_mm,
                    hi: self.position_error_mm,
                },
            )
        });
        let [Some(dx), Some(dy), Some(dz)] = displacement else {
            return Err(LocalGeometryRefusal::ForwardResidual {
                spatial,
                carrier,
                admitted_error_mm: self.position_error_mm,
            });
        };
        Ok([dx, dy, dz])
    }

    fn geometry_cell_proof(
        &self,
        embedding: &OwnedUvRibbon<'_, '_>,
        range: [f64; 2],
        curve: &CurveEnclosure,
        allowance: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Result<MaterialLinkCell, LocalGeometryRefusal>, GeometryError> {
        self.geometry_cell_proof_kind(embedding, range, curve, allowance, false, work, cancel)
    }

    /// Same immutable G evaluator, with complete one-sided FIRST matrix boxes.
    /// The atlas caller must first prove carrier continuity and gamma C1.
    fn geometry_cell_proof_kind(
        &self,
        embedding: &OwnedUvRibbon<'_, '_>,
        range: [f64; 2],
        curve: &CurveEnclosure,
        allowance: f64,
        piecewise: bool,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Result<MaterialLinkCell, LocalGeometryRefusal>, GeometryError> {
        if !embedding.covers(self, range, [0.0, 1.0]) {
            return Err(failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "source collar cell leaves its owner/map/native-range/across boundary-ownership receipt",
                self.use_.face,
                (self.use_.wire, self.use_.index, range),
            ));
        }
        let (collar_width, inward) = match self.transport {
            CarrierTransport::Exact => {
                let normal = coedge_normal_enclosure(
                    self.domain,
                    self.use_.coedge,
                    curve.position,
                    work,
                    cancel,
                )?;
                if length(normal)?.lo <= 0.0 {
                    return Ok(Err(LocalGeometryRefusal::ExactCarrierNormal { normal }));
                }
                return Ok(Ok(MaterialLinkCell {
                    position: curve.position,
                    source_error_mm: 0.0,
                    convex_link: true,
                    outward: normal,
                    second_outward: normal,
                }));
            }
            CarrierTransport::Collar { width, inward } => (width, inward),
        };
        work.charge(1, cancel)?;
        let uv = chart_jet(self.domain, self.use_.coedge, range, work, cancel)?;
        let (beta, beta_t) = self.bubble(range)?;
        if beta.lo <= 0.0 {
            return Ok(Err(LocalGeometryRefusal::EndpointBubble { beta }));
        }
        let v = rotate(uv.derivative, inward);
        let vt = rotate(uv.second_derivative, inward);
        let width = Interval::point(collar_width);
        let offset = v.map(|value| mul(mul(width, beta), value));
        let tube = std::array::from_fn(|axis| {
            add(
                uv.position[axis],
                mul(Interval { lo: 0.0, hi: 1.0 }, offset[axis]),
            )
        });
        let owned_tube = add_margin(tube, self.margin);
        // The owned receipt calibrates the complete inner image against ALL
        // original boundaries and joints, including this edge's unused pieces.
        // Separately retain full nonowning COLLAR separation for actual G.
        for (index, _) in self.domain.coedges().iter().enumerate() {
            if index == self.use_.coedge {
                continue;
            }
            for span in self.domain.boundary_parameter_intervals(index)? {
                let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                stack[0] = (span, 0);
                let mut pending = 1;
                let mut last_other = None;
                while pending != 0 {
                    pending -= 1;
                    let (span, depth) = stack[pending];
                    // Retain operands already evaluated by this proof. Only a
                    // terminal resource error formats them; no extra jet/query.
                    let resource_context = |mut error: GeometryError| {
                        if error.code == GeometryErrorCode::ResourceLimit {
                            error.message = format!(
                                "{}; source collar ForeignCollar owner={:?} spatial={range:?} foreign_coedge={index} foreign_uv_parameter={span:?} owned_uv={owned_tube:?} last_evaluated_foreign={last_other:?}",
                                error.message,
                                [self.use_.face, self.use_.wire, self.use_.index],
                            );
                        }
                        error
                    };
                    work.charge(1, cancel).map_err(&resource_context)?;
                    let other = self
                        .other_collar_enclosure(index, span, work, cancel)
                        .map_err(resource_context)?;
                    last_other = Some((span, other));
                    if (0..2).any(|axis| {
                        other[axis].hi < owned_tube[axis].lo || other[axis].lo > owned_tube[axis].hi
                    }) {
                        continue;
                    }
                    let mid = span[0] * 0.5 + span[1] * 0.5;
                    if depth >= MAX_DEPTH || mid <= span[0] || mid >= span[1] {
                        return Ok(Err(LocalGeometryRefusal::ForeignCollar {
                            coedge: index,
                            uv_parameter_range: span,
                            owned: owned_tube,
                            other,
                        }));
                    }
                    stack[pending] = ([span[0], mid], depth + 1);
                    stack[pending + 1] = ([mid, span[1]], depth + 1);
                    pending += 2;
                }
            }
        }
        {
            let surface = &self.domain.surface_operand();
            let enclosure = |surface: &PreparedTrimSurface<'_, '_>,
                             uv,
                             work: &mut GeometryWorkBudget,
                             cancel: &AtomicBool| {
                if piecewise {
                    surface_piecewise_first_enclosure(surface, uv, work, cancel)
                } else {
                    // Keep the existing strict source/C1 qualification before
                    // extracting only the fields this first-matrix proof uses.
                    let jet = surface_enclosure(surface, uv, work, cancel)?;
                    Ok(SurfaceFirstEnclosure {
                        position: jet.position,
                        du: jet.du,
                        dv: jet.dv,
                    })
                }
            };
            let boundary = enclosure(
                surface,
                uv_box(add_margin(uv.position, self.margin)),
                work,
                cancel,
            )?;
            let patch = enclosure(surface, uv_box(owned_tube), work, cancel)?;
            let boundary_t = compose_first(&boundary.du, &boundary.dv, uv.derivative);
            // Retained whole-forward error is an independent coefficient proof.
            // Intersect it with reevaluated component differences so subdivision
            // contracts derivatives without losing the original global bound.
            let displacement = match self.forward_displacement(curve.position, boundary.position) {
                Ok(displacement) => displacement,
                Err(refusal) => return Ok(Err(refusal)),
            };
            let displacement_t: [Interval; 3] = std::array::from_fn(|axis| {
                exact_difference(curve.derivative[axis], boundary_t[axis])
            });
            let cost = length(displacement)?.hi.min(self.position_error_mm);
            if !cost.is_finite() || cost > allowance {
                return Ok(Err(LocalGeometryRefusal::SourceError {
                    error_mm: cost,
                    allowance_mm: allowance,
                }));
            }
            let at = std::array::from_fn(|axis| {
                add(
                    uv.derivative[axis],
                    mul(
                        Interval {
                            lo: 0.0,
                            hi: collar_width,
                        },
                        add(mul(beta_t, v[axis]), mul(beta, vt[axis])),
                    ),
                )
            });
            let aw = v.map(|value| mul(mul(width, beta), value));
            let ft = compose_first(&patch.du, &patch.dv, at);
            let fw = compose_first(&patch.du, &patch.dv, aw);
            // lambda ranges over the ENTIRE transport homotopy; no computed
            // near-zero dihedral is replaced by equality or convexity.
            let gt = std::array::from_fn(|axis| {
                add(
                    ft[axis],
                    mul(Interval { lo: 0.0, hi: 1.0 }, displacement_t[axis]),
                )
            });
            let gw = std::array::from_fn(|axis| {
                fw[axis].sub(mul(Interval { lo: 0.0, hi: 1.0 }, displacement[axis]))
            });
            let mut normal = cross(gt, gw);
            if !self.spatial_orientation {
                normal = normal.map(Interval::neg);
            }
            let position = std::array::from_fn(|axis| {
                add(
                    patch.position[axis],
                    mul(Interval { lo: 0.0, hi: 1.0 }, displacement[axis]),
                )
            });
            if length(normal)?.lo <= 0.0 {
                return Ok(Err(LocalGeometryRefusal::TransportJacobian {
                    uv: uv.position,
                    margin_uv: self.margin,
                    width: collar_width,
                    beta,
                    displacement,
                    displacement_t,
                    tangent: gt,
                    across: gw,
                    normal,
                }));
            }
            Ok(Ok(MaterialLinkCell {
                position,
                source_error_mm: cost,
                convex_link: false,
                outward: normal,
                second_outward: normal,
            }))
        }
    }
}

/// Paired original ownership and two complete source-only collar recipes.
/// A successful cell proves a two-sided local radial link, never a dihedral or
/// global convexity fact. The complete closed-boundary consumer separately
/// supplies endpoint fans, caps and degree-one global coverage.
pub(super) struct PairedMaterialLink<'a, 'd> {
    first: OriginalCollar<'a, 'd>,
    second: OriginalCollar<'a, 'd>,
    first_embedding: OwnedUvRibbon<'a, 'd>,
    second_embedding: OwnedUvRibbon<'a, 'd>,
    allowance: f64,
}
impl<'a, 'd> PairedMaterialLink<'a, 'd> {
    pub fn new(
        definition: &Definition,
        first_domain: &'a PreparedFaceTrimDomain<'d>,
        first: CoedgeUse,
        second_domain: &'a PreparedFaceTrimDomain<'d>,
        second: CoedgeUse,
        central_range: [f64; 2],
        remaining_scratch_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        work.charge(1, cancel)?;
        if !std::ptr::eq(first_domain.definition(), definition)
            || !std::ptr::eq(second_domain.definition(), definition)
            || first.orientation == second.orientation
        {
            return Ok(None);
        }
        let a = first_domain.original_coedge_correspondence(first.coedge)?;
        let b = second_domain.original_coedge_correspondence(second.coedge)?;
        let (Some(a_binding), Some(b_binding)) = (a.binding(), b.binding()) else {
            return Ok(None);
        };
        if a_binding.edge != b_binding.edge
            || a_binding.native_interval != b_binding.native_interval
            || a_binding.spatial_forward != b_binding.spatial_forward
            || a.spatial_range() != b.spatial_range()
            || a_binding.owner != [first.face, first.wire, first.index]
            || b_binding.owner != [second.face, second.wire, second.index]
            || !first_domain.same_original_spatial_trace(
                first.coedge,
                second_domain,
                second.coedge,
                work,
                cancel,
            )?
        {
            return Ok(None);
        }
        let allowance = source_allowance(definition)?;
        let Some(first) = OriginalCollar::for_enclosure(first_domain, first, work, cancel)? else {
            return Ok(None);
        };
        let Some(second) = OriginalCollar::for_enclosure(second_domain, second, work, cancel)?
        else {
            return Ok(None);
        };
        let Some(first_embedding) =
            first.owned_embedding(central_range, remaining_scratch_bytes, work, cancel)?
        else {
            return Ok(None);
        };
        let Some(second_embedding) =
            second.owned_embedding(central_range, remaining_scratch_bytes, work, cancel)?
        else {
            return Ok(None);
        };
        Ok(Some(Self {
            first,
            second,
            first_embedding,
            second_embedding,
            allowance,
        }))
    }
    pub fn swept_enclosure(
        &self,
        range: [f64; 2],
        curve: &CurveEnclosure,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 3], GeometryError> {
        let a = self
            .first
            .swept_enclosure(range, curve, self.allowance, work, cancel)?;
        let b = self
            .second
            .swept_enclosure(range, curve, self.allowance, work, cancel)?;
        Ok(std::array::from_fn(|axis| a[axis].hull(b[axis])))
    }
    pub fn cell(
        &self,
        range: [f64; 2],
        curve: &CurveEnclosure,
        prism: &SamplePrism,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Result<MaterialLinkCell, MaterialLinkRefusal>, GeometryError> {
        let first = match self.first.cell(
            &self.first_embedding,
            range,
            curve,
            prism,
            self.allowance,
            work,
            cancel,
        )? {
            Ok(cell) => cell,
            Err(reason) => return Ok(Err(reason)),
        };
        let second = match self.second.cell(
            &self.second_embedding,
            range,
            curve,
            prism,
            self.allowance,
            work,
            cancel,
        )? {
            Ok(cell) => cell,
            Err(reason) => return Ok(Err(reason)),
        };
        let tangent = if self.first.spatial_orientation {
            curve.derivative
        } else {
            curve.derivative.map(Interval::neg)
        };
        let convex_link = first.convex_link
            && second.convex_link
            && dot(second.outward, cross(first.outward, tangent)).hi <= 0.0;
        Ok(Ok(MaterialLinkCell {
            position: std::array::from_fn(|axis| first.position[axis].hull(second.position[axis])),
            source_error_mm: first.source_error_mm.max(second.source_error_mm),
            convex_link,
            outward: first.outward,
            second_outward: second.outward,
        }))
    }
}

#[cfg(test)]
mod scalar_law_tests;
