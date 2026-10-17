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
use crate::parametric::{ChartCurveEnclosure, CurveEnclosure};

mod atlas;
mod coverage;
mod fiber;
mod owned_uv_ribbon;
mod radial_product;
mod transport;
mod vertex_fan;
mod vertex_transport;
mod wall;
pub(super) use coverage::{ClosedRadialLinks, MAX_CORRECTION_BYTES};
pub(in crate::analysis) use radial_product::{
    ClosedRadialMaterialProduct, RadialRegionKind, certify_closed_radial_material_product,
};
use owned_uv_ribbon::OwnedUvRibbon;
pub(crate) use transport::{
    SourceTransportOwner, SourceTransportSupport, source_transport_supports,
};
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

/// Actual cross-ribbon position and first jet at ONE original native parameter.
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

/// The recipe's SAME native-parameter beta=x(1-x), restricted to a proved
/// subinterval. In particular an endpoint subdivision must not keep the
/// unrelated whole-edge upper bound 1/4.
fn native_endpoint_bubble(whole: [f64; 2], parameter: Interval) -> Result<Interval, GeometryError> {
    if !whole[0].is_finite()
        || !whole[1].is_finite()
        || whole[0] >= whole[1]
        || parameter.lo < whole[0]
        || parameter.hi > whole[1]
    {
        return Err(GeometryError::new(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "foreign collar bubble leaves its original native parameter range",
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
fn compose_first(jet: &SurfaceEnclosure, tangent: [Interval; 2]) -> [Interval; 3] {
    std::array::from_fn(|axis| add(mul(jet.du[axis], tangent[0]), mul(jet.dv[axis], tangent[1])))
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
    definition: &Definition,
    face_index: usize,
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<f64>, GeometryError> {
    let Some(height) = super::super::slab::constant_source_face_height(
        definition, face_index, functional, work, cancel,
    )?
    else {
        return Ok(None);
    };
    if !height.lo.is_finite() || height.lo != height.hi {
        return Ok(None);
    }
    let face = definition
        .solid
        .face_iter()
        .nth(face_index)
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::InvalidGeometry,
                "material scalar law lost its original face",
                face_index,
                (),
            )
        })?;
    for wire in face.absolute_boundaries() {
        for edge in wire {
            work.charge(1, cancel)?;
            let curve_height = edge.with_curve(|curve| {
                super::super::slab::constant_native_curve_height(curve, functional, work, cancel)
            })?;
            if curve_height.is_none_or(|value| value.lo != height.lo || value.hi != height.hi) {
                return Ok(None);
            }
        }
    }
    for vertex in face.vertex_iter() {
        work.charge(1, cancel)?;
        let point: [f64; 3] = vertex.point().into();
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
pub(super) const UV_EMBEDDING_STACK_BYTES: usize =
    std::mem::size_of::<[UvPairCell; 2 * MAX_DEPTH as usize + 1]>();

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
    /// In particular the endpoint bubble always uses the WHOLE native range.
    fn for_enclosure(
        domain: &'a PreparedFaceTrimDomain<'d>,
        use_: CoedgeUse,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        work.charge(1, cancel)?;
        let proof = domain.original_coedge_correspondence(use_.coedge)?;
        let Some(binding) = proof.binding() else {
            return Ok(None);
        };
        let face = domain.native().face();
        let boundaries = face.absolute_boundaries();
        let Some(edge) = boundaries
            .get(use_.wire)
            .and_then(|wire| wire.get(use_.index))
        else {
            return Ok(None);
        };
        if binding.owner != [use_.face, use_.wire, use_.index]
            || binding.edge != edge.id()
            || use_.orientation != (edge.orientation() == face.orientation())
        {
            return Ok(None);
        }
        let range = proof.native_range();
        let margin = proof.margin_uv();
        if proof.provenance() == crate::kernel::OriginalCorrespondenceProvenance::ExactCoefficients
            && proof.position_error_mm() == 0.0
        {
            return Ok(Some(Self {
                domain,
                use_,
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
            if domain.classify(uv_box(add_margin(probe, margin)), work, cancel)?
                == TrimClassification::Inside
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
        if proof.provenance() == crate::kernel::OriginalCorrespondenceProvenance::CompletedPoleGraph
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
        self.domain.native().face().with_surface(|surface| {
            Ok(crate::parametric::surface_piecewise_enclosure(
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
        })
    }

    fn other_collar_enclosure(
        &self,
        coedge: usize,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 2], GeometryError> {
        let position = self
            .domain
            .boundary_position_enclosure(coedge, range, work, cancel)?
            .map(|value| Interval {
                lo: value[0],
                hi: value[1],
            });
        let proof = self.domain.original_coedge_correspondence(coedge)?;
        if proof.provenance() == crate::kernel::OriginalCorrespondenceProvenance::ExactCoefficients
            && proof.position_error_mm() == 0.0
        {
            return Ok(position);
        }
        let original = &self.domain.native().coedges()[coedge];
        let mut native = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let result = monstertruck_modeling::validated_curve2d_jet(
            original.curve,
            range,
            &mut native,
            cancel,
        );
        crate::parametric::debit_native_work(work, native.consumed(), cancel)?;
        let jet = result.map_err(crate::parametric::native_error)?;
        let derivative = jet.first.map(|value| {
            mul(
                Interval {
                    lo: value.lower,
                    hi: value.upper,
                },
                proof.uv_parameter_derivative(),
            )
        });
        let v = rotate(derivative, 1.0);
        let native_range = proof.native_range();
        let width = (native_range[1] - native_range[0])
            * self
                .domain
                .material_realization()
                .collar_parameter_fraction();
        let beta = match proof.affine_native_parameter_preimage(range)? {
            Some(parameter) => native_endpoint_bubble(native_range, parameter)?,
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
        self.domain.native().face().with_surface(|surface| {
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
            let first = compose_first(&base, offset);
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
        })
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
        for (index, _) in self.domain.native().coedges().iter().enumerate() {
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
                                "{}; source collar ForeignCollar owner={:?} native={range:?} foreign_coedge={index} foreign_uv_parameter={span:?} owned_uv={owned_tube:?} last_evaluated_foreign={last_other:?}",
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
        self.domain.native().face().with_surface(|surface| {
            let enclosure = if piecewise {
                crate::parametric::surface_piecewise_enclosure
            } else {
                surface_enclosure
            };
            let boundary = enclosure(
                surface,
                uv_box(add_margin(uv.position, self.margin)),
                work,
                cancel,
            )?;
            let patch = enclosure(surface, uv_box(owned_tube), work, cancel)?;
            let boundary_t = compose_first(&boundary, uv.derivative);
            // Retained whole-forward error is an independent coefficient proof.
            // Intersect it with reevaluated component differences so subdivision
            // contracts derivatives without losing the original global bound.
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
                return Ok(Err(LocalGeometryRefusal::ForwardResidual {
                    spatial: curve.position,
                    carrier: boundary.position,
                    admitted_error_mm: self.position_error_mm,
                }));
            };
            let displacement = [dx, dy, dz];
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
            let ft = compose_first(&patch, at);
            let fw = compose_first(&patch, aw);
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
            if !self.use_.orientation {
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
        })
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
        if first.orientation == second.orientation {
            return Ok(None);
        }
        let a = first_domain.original_coedge_correspondence(first.coedge)?;
        let b = second_domain.original_coedge_correspondence(second.coedge)?;
        let (Some(a_binding), Some(b_binding)) = (a.binding(), b.binding()) else {
            return Ok(None);
        };
        if a_binding.edge != b_binding.edge
            || a.native_range() != b.native_range()
            || a_binding.owner != [first.face, first.wire, first.index]
            || b_binding.owner != [second.face, second.wire, second.index]
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
        let tangent = if self.first.use_.orientation {
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
mod scalar_law_tests {
    use super::*;

    #[test]
    fn nonowner_collar_bubble_contracts_at_original_native_endpoints() {
        let whole = [10.0, 12.0];
        let first =
            native_endpoint_bubble(whole, Interval::new(10.0, 10.0 + 1.0 / 1024.0).unwrap())
                .unwrap();
        let last = native_endpoint_bubble(whole, Interval::new(12.0 - 1.0 / 1024.0, 12.0).unwrap())
            .unwrap();
        assert!(first.lo >= 0.0 && first.hi < 1.0 / 1024.0);
        assert!(last.lo >= 0.0 && last.hi < 1.0 / 1024.0);
        let center = native_endpoint_bubble(whole, Interval::point(11.0)).unwrap();
        assert!(center.lo <= 0.25 && center.hi == 0.25);
        let complete = native_endpoint_bubble(whole, Interval::new(10.0, 12.0).unwrap()).unwrap();
        assert_eq!((complete.lo, complete.hi), (0.0, 0.25));
        assert_eq!(
            native_endpoint_bubble(whole, Interval::point(9.0))
                .unwrap_err()
                .code,
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
        );
    }

    #[test]
    fn actual_fitting_complete_paired_source_collars_have_local_geometry() {
        let cancel = AtomicBool::new(false);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry/independent-exporters/occt-tapered-fitting-mm.step");
        let definition = crate::import_step(&std::fs::read(path).unwrap(), &cancel).unwrap();
        let mut work = GeometryWorkBudget::new(20_000_000);
        let standalone = OriginalTrimSource::standalone(&definition);
        let originals = standalone
            .prepare_domains_if_missing(MAX_SCRATCH_BYTES, &mut work, &cancel)
            .unwrap()
            .expect("one immutable original-domain owner");
        let source = standalone.borrowing_domains(Some(&originals));
        let available = MAX_SCRATCH_BYTES - originals.retained_bytes();
        let mut incidence =
            SourceEdgeIncidence::new(&definition, available, &mut work, &cancel).unwrap();
        incidence
            .prepare_vertices(available, &mut work, &cancel)
            .unwrap();
        let available = available - incidence.owned_bytes();

        // Visit ALL paired incidences in source-occurrence order, not hash or
        // allocation-ID order. No observed failing parameter/face selects scope.
        let mut pairs = incidence.values().collect::<Vec<_>>();
        pairs.sort_unstable_by_key(|pair| (pair.first.face, pair.first.wire, pair.first.index));
        let mut vertices = Vec::new();
        for pair in &pairs {
            assert!(
                pair.second.is_some(),
                "complete original opposed-use inventory"
            );
            for vertex in pair.vertices {
                if !vertices.contains(&vertex) {
                    vertices.push(vertex);
                }
            }
        }
        let mut exits = Vec::new();
        for vertex in vertices {
            let plan = vertex_transport::SourceVertexTransportPlan::prepare(
                source, &incidence, vertex, available, &mut work, &cancel,
            )
            .unwrap()
            .expect("the unchanged canonical source-only NativeV plan");
            plan.visit_native_exits(&mut work, &cancel, &mut |edge, back, parameter| {
                if let Some(&(_, _, previous)) = exits
                    .iter()
                    .find(|&&(candidate, end, _)| candidate == edge && end == back)
                {
                    assert_eq!(previous, parameter, "same actual NativeEdge endpoint germ");
                } else {
                    exits.push((edge, back, parameter));
                }
                Ok(())
            })
            .unwrap();
        }

        let mut complete_pairs = 0;
        for pair in pairs {
            let first = pair.first;
            let second = pair.second.unwrap();
            let first_owner = [first.face, first.wire, first.index];
            let second_owner = [second.face, second.wire, second.index];
            let a = source
                .face_domain(first.face, available, &mut work, &cancel)
                .unwrap();
            let b = source
                .face_domain(second.face, available, &mut work, &cancel)
                .unwrap();
            let edge = &a.native().face().absolute_boundaries()[first.wire][first.index];
            let proof = a.original_coedge_correspondence(first.coedge).unwrap();
            let endpoint = |back| {
                exits
                    .iter()
                    .find(|&&(candidate, end, _)| candidate == edge.id() && end == back)
                    .map(|&(_, _, parameter)| parameter)
                    .expect("actual original NativeV exit")
            };
            let central = [endpoint(false), endpoint(true)];
            let whole = proof.native_range();
            assert!(
                whole[0] < central[0] && central[0] < central[1] && central[1] < whole[1],
                "fixed source cutouts: {first_owner:?} {second_owner:?} {whole:?} {central:?}",
            );
            let link = PairedMaterialLink::new(
                &definition,
                &a,
                first,
                &b,
                second,
                central,
                available
                    .checked_sub(a.owned_bytes())
                    .and_then(|bytes| bytes.checked_sub(b.owned_bytes()))
                    .expect("live paired-domain byte reservation"),
                &mut work,
                &cancel,
            )
            .unwrap()
            .unwrap_or_else(|| {
                panic!(
                    "whole used paired UV embedding: {first_owner:?} {second_owner:?} {central:?}"
                )
            });
            // Prepare original knots before borrowing the same physical edge.
            let cuts = original_coedge_parameter_cuts(
                [(&a, first.coedge), (&b, second.coedge)],
                available,
                &mut work,
                &cancel,
            )
            .unwrap();
            let mut covered = 0;
            let mut current_range = central;
            let mut current_owner = first_owner;
            let mut current_phase = "original spatial preparation";
            let mut last_refusal = None;
            a.with_original_spatial_curve(first.coedge, available, &mut work, &cancel, |curve, work, cancel| {
                for span in curve.parameter_intervals() {
                    let start = span.0.max(central[0]);
                    let finish = span.1.min(central[1]);
                    if start > finish {
                        continue;
                    }
                    let first_cut = cuts.partition_point(|cut| *cut <= start);
                    let last_cut = cuts.partition_point(|cut| *cut < finish).max(first_cut);
                    let mut start = start;
                    for finish in cuts[first_cut..last_cut].iter().copied()
                        .chain(std::iter::once(finish))
                    {
                        let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                        stack[0] = ([start, finish], 0);
                        let mut pending = 1;
                        while pending != 0 {
                            pending -= 1;
                            let (range, depth) = stack[pending];
                            current_range = range;
                            current_phase = "original spatial enclosure";
                            work.charge(1, cancel)?;
                            let jet = curve.jet((range[0], range[1]), work, cancel)?;
                            let mut refused = None;
                            for (collar, embedding) in [
                                (&link.first, &link.first_embedding),
                                (&link.second, &link.second_embedding),
                            ] {
                                current_owner = [
                                    collar.use_.face, collar.use_.wire, collar.use_.index,
                                ];
                                current_phase = "source collar local geometry";
                                if let Err(proof) = collar.geometry_cell_proof(
                                    embedding, range, &jet, link.allowance, work, cancel,
                                )? {
                                    refused = Some((collar.use_, proof));
                                    break;
                                }
                            }
                            if let Some((owner, proof)) = refused {
                                let owner = [owner.face, owner.wire, owner.index];
                                let mid = range[0] * 0.5 + range[1] * 0.5;
                                assert!(
                                    depth < MAX_DEPTH && mid > range[0] && mid < range[1],
                                    "complete paired original-source local geometry: owner={owner:?} pair={first_owner:?}/{second_owner:?} central={central:?} native={range:?} proof={proof:?} work={}",
                                    work.consumed(),
                                );
                                last_refusal = Some((owner, range, proof));
                                stack[pending] = ([mid, range[1]], depth + 1);
                                stack[pending + 1] = ([range[0], mid], depth + 1);
                                pending += 2;
                            } else {
                                covered += 1;
                            }
                        }
                        start = finish;
                    }
                }
                Ok(())
            })
            .map_err(|mut error| {
                if error.code == GeometryErrorCode::ResourceLimit {
                    error.message = format!(
                        "{}; direct paired source cover phase={current_phase} owner={current_owner:?} current_native={current_range:?} last_local_refusal={last_refusal:?}",
                        error.message,
                    );
                }
                error
            })
            .unwrap_or_else(|error| panic!(
                "whole original paired collar: {first_owner:?}/{second_owner:?} central={central:?} work={}: {error:?}",
                work.consumed(),
            ));
            assert!(
                covered > 0,
                "every whole original paired central range is covered"
            );
            complete_pairs += 1;
        }
        assert!(complete_pairs > 0);
    }

    #[test]
    fn actual_fitting_circular_rim_normal_embedding_covers_unchanged_used_ribbon() {
        let cancel = AtomicBool::new(false);
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry/independent-exporters/occt-tapered-fitting-mm.step");
        let definition = crate::import_step(&std::fs::read(path).unwrap(), &cancel).unwrap();
        let mut work = GeometryWorkBudget::new(20_000_000);
        let standalone = OriginalTrimSource::standalone(&definition);
        let originals = standalone
            .prepare_domains_if_missing(MAX_SCRATCH_BYTES, &mut work, &cancel)
            .unwrap()
            .expect("one owner for the unchanged original trim domains");
        let source = standalone.borrowing_domains(Some(&originals));
        let available = MAX_SCRATCH_BYTES - originals.retained_bytes();
        let mut incidence =
            SourceEdgeIncidence::new(&definition, available, &mut work, &cancel).unwrap();
        incidence
            .prepare_vertices(available, &mut work, &cancel)
            .unwrap();
        let available = available - incidence.owned_bytes();

        // Both annular wires, including every occurrence produced by periodic
        // splitting. The flat slot counts ALL preceding wires, just as the
        // original incidence inventory does; wire-local index is not that slot.
        let face_index = 2;
        let face = definition.solid.face_iter().nth(face_index).unwrap();
        let source_face = &definition.source_topology.faces[face_index];
        assert_eq!(source_face.wires.len(), 2);
        assert_eq!(source_face.wires.len(), face.absolute_boundaries().len());
        let mut uses = Vec::new();
        let mut vertices = Vec::new();
        let mut flat = 0;
        for (wire_index, source_wire) in source_face.wires.iter().enumerate() {
            let wire = &face.absolute_boundaries()[wire_index];
            assert_eq!(source_wire.coedges.len(), wire.len());
            assert!(
                !wire.is_empty(),
                "each actual annular rim must be exercised"
            );
            for (index, source_use) in source_wire.coedges.iter().enumerate() {
                let edge = &wire[index];
                assert_eq!(source_use.native_orientation, edge.orientation());
                let expected = CoedgeUse {
                    face: face_index,
                    coedge: flat,
                    wire: wire_index,
                    index,
                    orientation: edge.orientation() == face.orientation(),
                };
                flat += 1;
                let pair = incidence.get(edge.id()).expect("actual original rim edge");
                let second = pair.second.expect("opposed actual original rim use");
                assert!(pair.first == expected || second == expected);
                // Exercise the paired original occurrence as well: its chart,
                // gauge and orientation need not match the annular plane's.
                uses.extend([pair.first, second]);
                for vertex in pair.vertices {
                    if !vertices.contains(&vertex) {
                        vertices.push(vertex);
                    }
                }
            }
        }
        assert_eq!(uses.len(), 2 * flat);

        // Literal exits from the SAME complete source-only NativeV geometry
        // plans as production: no coordinate lookup, uniform fraction, UV
        // qualification or query-dependent shortening.
        let mut exits = Vec::new();
        for vertex in vertices {
            let plan = vertex_transport::SourceVertexTransportPlan::prepare(
                source, &incidence, vertex, available, &mut work, &cancel,
            )
            .unwrap()
            .expect("actual rim NativeV has its unchanged source-only geometry plan");
            plan.visit_native_exits(&mut work, &cancel, &mut |edge, back, parameter| {
                if let Some(&(_, _, previous)) = exits
                    .iter()
                    .find(|&&(candidate, end, _)| candidate == edge && end == back)
                {
                    assert_eq!(
                        previous, parameter,
                        "one literal exit per original endpoint germ"
                    );
                } else {
                    exits.push((edge, back, parameter));
                }
                Ok(())
            })
            .unwrap();
        }
        let mut nonzero_collars = 0;
        for use_ in uses {
            let domain = source
                .face_domain(use_.face, available, &mut work, &cancel)
                .unwrap();
            let face = domain.native().face();
            let edge = &face.absolute_boundaries()[use_.wire][use_.index];
            let source_use =
                &definition.source_topology.faces[use_.face].wires[use_.wire].coedges[use_.index];
            let proof = domain.original_coedge_correspondence(use_.coedge).unwrap();
            let binding = proof.binding().expect("final actual original-use binding");
            assert_eq!(binding.face, face.id());
            assert_eq!(binding.edge, edge.id());
            assert_eq!(binding.owner, [use_.face, use_.wire, use_.index]);
            assert_eq!(binding.source, *source_use);
            assert_eq!(binding.endpoints, [edge.front().id(), edge.back().id()]);
            let native = edge.with_curve(|curve| curve.range_tuple());
            assert_eq!(proof.native_range(), [native.0, native.1]);
            let slope = proof.uv_parameter_derivative();
            let context = format!(
                "face={} coedge={} wire={} index={} source={source_use:?} native={:?} uv={:?} map_derivative={:?} margin={:?} physical_error_mm={}",
                use_.face,
                use_.coedge,
                use_.wire,
                use_.index,
                proof.native_range(),
                proof.uv_range(),
                [slope.lo, slope.hi],
                proof.margin_uv(),
                proof.position_error_mm(),
            );
            assert!(
                proof.position_error_mm() <= source_allowance(&definition).unwrap(),
                "{context}"
            );
            let uv_range = proof.uv_range();
            assert!(
                (uv_range[0] < uv_range[1] && slope.lo > 0.0)
                    || (uv_range[1] < uv_range[0] && slope.hi < 0.0),
                "retained original parameter map must be strictly oriented: {context}"
            );
            let range = proof.native_range();
            let width =
                (range[1] - range[0]) * domain.material_realization().collar_parameter_fraction();
            assert!(
                width.is_finite() && width > 0.0,
                "source-only collar width: {context}"
            );
            let collar = OriginalCollar::for_enclosure(&domain, use_, &mut work, &cancel)
                .unwrap_or_else(|error| panic!("original collar evaluation: {context}: {error:?}"))
                .unwrap_or_else(|| panic!("source-only inward classification refused after actual owner/map/width audit: {context}"));
            let has_collar = matches!(collar.transport, CarrierTransport::Collar { .. });
            nonzero_collars += usize::from(has_collar);
            let find_exit = |back| {
                exits
                    .iter()
                    .find(|&&(candidate, end, _)| candidate == edge.id() && end == back)
                    .map(|&(_, _, parameter)| parameter)
                    .unwrap_or_else(|| {
                        panic!("missing original endpoint germ back={back}: {context}")
                    })
            };
            let central = [find_exit(false), find_exit(true)];
            assert!(
                range[0] < central[0] && central[0] < central[1] && central[1] < range[1],
                "actual NativeV exits partition the whole original rim: {context} central={central:?}"
            );
            let midpoint = range[0] * 0.5 + range[1] * 0.5;
            let (curve, _) = domain
                .original_coedge_jets(use_.coedge, [midpoint; 2], available, &mut work, &cancel)
                .unwrap();
            let before = collar
                .boundary_jet(midpoint, [0.0, 1.0], &curve, &mut work, &cancel)
                .unwrap();
            let embedding = collar
                .embedding(central, [0.0, 1.0], &mut work, &cancel)
                .unwrap_or_else(|error| panic!("scoped original UV proof: {context} central={central:?}: {error:?}"))
                .unwrap_or_else(|| panic!("whole USED original normal ribbon is not proved injective: {context} central={central:?}"));
            assert!(embedding.covers(&collar, central, [0.0, 1.0]), "{context}");
            assert!(
                !embedding.covers(&collar, range, [0.0, 1.0]),
                "a central receipt must never claim the collapsed NativeV quotient: {context}"
            );
            assert!(
                !embedding.covers(&collar, central, [0.0, 1.0001]),
                "{context}"
            );
            let after = collar
                .boundary_jet(midpoint, [0.0, 1.0], &curve, &mut work, &cancel)
                .unwrap();
            assert_eq!(
                before.position.map(|v| [v.lo, v.hi]),
                after.position.map(|v| [v.lo, v.hi]),
                "{context}"
            );
            assert_eq!(
                before.derivative.map(|v| [v.lo, v.hi]),
                after.derivative.map(|v| [v.lo, v.hi]),
                "{context}"
            );
            if has_collar {
                assert!(
                    collar
                        .embedding(range, [0.0, 1.0], &mut work, &cancel)
                        .unwrap()
                        .is_none(),
                    "collapsed original endpoints are not a strict UV chart: {context}"
                );
            }

            // Right geometry, wrong original occurrence: neither constructor
            // ownership nor a borrowed receipt may accept it.
            let pair = incidence.get(edge.id()).unwrap();
            let other = if pair.first == use_ {
                pair.second.unwrap()
            } else {
                pair.first
            };
            let wrong_use = CoedgeUse {
                coedge: use_.coedge,
                ..other
            };
            assert!(
                OriginalCollar::for_enclosure(&domain, wrong_use, &mut work, &cancel)
                    .unwrap()
                    .is_none(),
                "actual geometry cannot grant foreign owner authority: {context}"
            );
            let owned = collar
                .owned_embedding(central, available, &mut work, &cancel)
                .unwrap()
                .expect("whole boundary-owned original ribbon");
            let wrong_owner = OriginalCollar {
                use_: wrong_use,
                ..collar
            };
            assert!(
                !embedding.covers(&wrong_owner, central, [0.0, 1.0]),
                "{context}"
            );
            assert!(
                wrong_owner
                    .geometry_cell(
                        &owned,
                        central,
                        &curve,
                        source_allowance(&definition).unwrap(),
                        &mut work,
                        &cancel,
                    )
                    .is_err(),
                "geometry consumer must reject another owner's receipt: {context}"
            );
            let narrow = collar
                .owned_embedding([midpoint; 2], available, &mut work, &cancel)
                .unwrap()
                .unwrap();
            assert!(!narrow.covers(&collar, central, [0.0, 1.0]), "{context}");
            assert!(
                collar
                    .geometry_cell(
                        &narrow,
                        central,
                        &curve,
                        source_allowance(&definition).unwrap(),
                        &mut work,
                        &cancel,
                    )
                    .is_err(),
                "every cell must consume its full native scope: {context}"
            );
            let narrow_across = collar
                .embedding([midpoint; 2], [0.0, 0.5], &mut work, &cancel)
                .unwrap()
                .unwrap();
            assert!(
                !narrow_across.covers(&collar, [midpoint; 2], [0.0, 1.0]),
                "a partial PURE across proof cannot grant whole owned-G authority: {context}"
            );
        }
        assert!(
            nonzero_collars > 0,
            "actual rims must exercise nonzero original transport"
        );
    }

    #[test]
    fn normal_coordinate_pmatrix_refuses_a_genuine_curvature_fold() {
        // At t=0 of a unit circle, U=(1-k*r)*gamma. k=2 and
        // r in [0,1] includes an actual collapsed circle and orientation fold.
        let jet = ChartCurveEnclosure {
            position: [1.0, 0.0].map(Interval::point),
            derivative: [0.0, 1.0].map(Interval::point),
            second_derivative: [-1.0, 0.0].map(Interval::point),
        };
        assert!(normal_uv_pmatrix(
            &jet,
            jet.derivative,
            0.25,
            1.0,
            Interval { lo: 0.0, hi: 1.0 }
        ));
        assert!(!normal_uv_pmatrix(
            &jet,
            jet.derivative,
            2.0,
            1.0,
            Interval { lo: 0.0, hi: 1.0 }
        ));
    }

    fn pocket_floor() -> (Definition, usize) {
        let (_, solid, _) = crate::parity_fixtures::recipes()
            .into_iter()
            .find(|(name, _, _)| *name == "native-parity-blind-pocket.step")
            .expect("original native pocket recipe");
        let source = crate::parity_fixtures::original("material-floor-scalar-law.step", solid);
        let definition = crate::import_step(source.as_bytes(), &AtomicBool::new(false))
            .expect("unchanged original coefficients");
        let mut work = GeometryWorkBudget::new(20_000_000);
        let functional = HeightFunctional {
            direction: [0.0, 0.0, 1.0].map(Interval::point),
            offset: Interval::point(0.0),
        };
        let floor = (0..definition.solid.face_iter().count())
            .find(|&face| {
                super::super::super::slab::constant_source_face_height(
                    &definition,
                    face,
                    functional,
                    &mut work,
                    &AtomicBool::new(false),
                )
                .unwrap()
                .is_some_and(|height| height.lo == 4.0 && height.hi == 4.0)
            })
            .expect("coefficient-backed interior floor");
        (definition, floor)
    }

    #[test]
    fn actual_generic_transport_floor_preserves_complete_exact_scalar_law() {
        let (definition, floor) = pocket_floor();
        let functional = HeightFunctional {
            direction: [0.0, 0.0, 1.0].map(Interval::point),
            offset: Interval::point(0.0),
        };
        let mut work = GeometryWorkBudget::new(20_000_000);
        assert!(
            !definition
                .material_transport_is_proved_zero(&mut work, &AtomicBool::new(false),)
                .unwrap()
        );
        assert_eq!(
            constant_material_face_height(
                &definition,
                floor,
                functional,
                &mut work,
                &AtomicBool::new(false),
            )
            .unwrap(),
            Some(4.0)
        );
    }

    #[test]
    fn complete_material_scalar_law_rejects_sub_roundoff_functional_tilt() {
        let (definition, floor) = pocket_floor();
        let functional = HeightFunctional {
            direction: [1.0e-18, 0.0, 1.0].map(Interval::point),
            offset: Interval::point(0.0),
        };
        let mut work = GeometryWorkBudget::new(20_000_000);
        assert_eq!(
            constant_material_face_height(
                &definition,
                floor,
                functional,
                &mut work,
                &AtomicBool::new(false),
            )
            .unwrap(),
            None,
            "rounded point heights cannot establish a scalar identity"
        );
    }
}
