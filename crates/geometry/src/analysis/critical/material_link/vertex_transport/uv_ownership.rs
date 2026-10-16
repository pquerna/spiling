// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

mod apex_factor;
mod completion;
use completion::{ComparisonCompletion, CorrectionJet};

mod comparison_transport;
use comparison_transport::GermTransport;
mod pole_comparison;
use pole_comparison::PoleComparison;

/// Distinct coordinate charts are never reinterpreted as original native UV.
#[derive(Clone, Copy)]
pub(super) enum OriginalCornerComparison {
    NativeUv(UvJointComparison),
    SphericalPole(PoleComparison),
}

type NativeUse = monstertruck_topology::trimmed::ChartCoedgeIdentity<
    monstertruck_modeling::Curve,
    monstertruck_modeling::Surface,
>;

#[derive(Clone, Copy)]
enum EndpointAuthority {
    ExactNativeAdjacency,
    EndpointUncertaintyJoint { index: usize, enclosure: Box2 },
    SphericalPoleJoint { index: usize },
}

/// A source-comparison expression, never an input to the fixed B/G recipe.
/// Retain typed NativeUse/whole-forward identities, not duplicate spatial jets.
#[derive(Clone, Copy)]
pub(super) struct UvJointComparison {
    vertex: VertexId<Point3>,
    ends: [EndKey; 2],
    joint: Box2,
    correction: [Box2; 2],
    completion: ComparisonCompletion,
    exact_joint: bool,
    authority: EndpointAuthority,
    transport: Option<[GermTransport; 2]>,
    allowance: f64,
}

/// Preparation never turns missing original authority into a generic UV gate.
pub(super) enum UvComparisonPreparation {
    Prepared(OriginalCornerComparison),
    Refused(UvPrepareRefusal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UvPrepareStage {
    CornerOwner,
    CoedgeInventory,
    WholeForwardReceipt,
    NativeBinding,
    Provenance,
    BindingOwner,
    NativeVertex,
    NativeWire,
    NativeEdge,
    NativeOrientation,
    NativeEndpoints,
    NativeFace,
    WholeForwardOwnership,
    Gauge,
    NativeEndpoint,
    EndpointOrientation,
    EndpointDelta,
    InteriorExit,
    EndpointExpression,
    FibreMargin,
    JointOutsideBothFibres,
    NativeOperandAssociation,
    NativeAdjacency,
    UnsupportedNativeJoint,
    FreshGermTransport,
    SphericalPoleComparison,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct UvPrepareEndContext {
    pub(super) owner: [usize; 3],
    pub(super) coedge: usize,
    pub(super) absolute_back: bool,
    pub(super) orientation: bool,
    pub(super) endpoint: f64,
    pub(super) exit: f64,
    pub(super) joint_uv: Box2,
    pub(super) native_range: Option<[f64; 2]>,
    pub(super) uv_range: Option<[f64; 2]>,
    pub(super) margin_uv: Option<[f64; 2]>,
    pub(super) position_error_mm: Option<f64>,
    pub(super) relative_derivative_error: Option<f64>,
    pub(super) span_inventory: Option<[usize; 3]>,
    pub(super) provenance: Option<crate::kernel::OriginalCorrespondenceProvenance>,
    pub(super) source_face: Option<u64>,
    pub(super) source_face_use: Option<u64>,
    pub(super) source_edge: Option<u64>,
    pub(super) source_coedge: Option<u64>,
    pub(super) binding_owner: Option<[usize; 3]>,
    pub(super) native_orientation: Option<bool>,
    pub(super) source_chart_margin_uv: Option<[f64; 2]>,
    pub(super) source_chart_error_mm: Option<f64>,
    pub(super) source_chart_parameter_range: Option<(f64, f64)>,
    pub(super) source_chart_certificate: Option<crate::kernel::SourceChartCertificate>,
    pub(super) source_chart_declared: bool,
    pub(super) binding_present: bool,
    pub(super) native_vertex_matches: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct UvPrepareRefusal {
    pub(super) stage: UvPrepareStage,
    pub(super) end: Option<usize>,
    pub(super) vertex_point: Option<[f64; 3]>,
    pub(super) ends: [UvPrepareEndContext; 2],
    /// Exact midpoint-minus-endpoint expressions, not a rounded distance.
    pub(super) correction: [Box2; 2],
}

impl std::fmt::Display for UvPrepareEndContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "owner={:?} coedge={} absolute_back={} orientation={} endpoint_exit={:?} joint_uv={:?} native_range={:?} uv_range={:?} margin_uv={:?} position_error_mm={:?} relative_derivative_error={:?} span_inventory={:?} provenance={:?} binding_present={} native_vertex_matches={} source_face={:?} source_face_use={:?} source_edge={:?} source_coedge={:?} binding_owner={:?} native_orientation={:?} source_chart_margin_uv={:?} source_chart_error_mm={:?} source_chart_parameter_range={:?} source_chart_certificate={:?} source_chart_declared={}",
            self.owner,
            self.coedge,
            self.absolute_back,
            self.orientation,
            [self.endpoint, self.exit],
            self.joint_uv,
            self.native_range,
            self.uv_range,
            self.margin_uv,
            self.position_error_mm,
            self.relative_derivative_error,
            self.span_inventory,
            self.provenance,
            self.binding_present,
            self.native_vertex_matches,
            self.source_face,
            self.source_face_use,
            self.source_edge,
            self.source_coedge,
            self.binding_owner,
            self.native_orientation,
            self.source_chart_margin_uv,
            self.source_chart_error_mm,
            self.source_chart_parameter_range,
            self.source_chart_certificate,
            self.source_chart_declared,
        )
    }
}

impl std::fmt::Display for UvPrepareRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "UV comparison prepare stage={:?} end={:?} vertex_mm={:?} exact_midpoint_corrections={:?} outgoing=({}) incoming=({})",
            self.stage, self.end, self.vertex_point, self.correction, self.ends[0], self.ends[1],
        )
    }
}

#[derive(Clone, Copy)]
struct EndKey {
    binding: crate::kernel::OriginalCoedgeBinding,
    coedge: usize,
    orientation: bool,
    absolute_back: bool,
    endpoint: f64,
    exit: f64,
    joint_uv: Box2,
    offset: Box2,
    collar: bool,
    native_range: [f64; 2],
    uv_range: [f64; 2],
    margin: [f64; 2],
    position_error: f64,
    provenance: crate::kernel::OriginalCorrespondenceProvenance,
}

pub(super) struct ComparisonStarJet {
    pub(super) q: Box2,
    pub(super) qt: Box2,
    /// det(H_s,H_theta)/s, including its continuous value at s=0.
    pub(super) factored_jacobian: Interval,
}

pub(super) struct ComparisonSourceCost {
    pub(super) source: Box3,
    pub(super) displacement: Box3,
    pub(super) error_mm: f64,
    pub(super) homotopy_mm: f64,
}

const UNIT: Interval = Interval { lo: 0.0, hi: 1.0 };
const ZERO2: Box2 = [Interval { lo: 0.0, hi: 0.0 }; 2];

fn equal_box(a: Box2, b: Box2) -> bool {
    (0..2).all(|i| a[i].lo == b[i].lo && a[i].hi == b[i].hi)
}

fn same_end(a: EndKey, b: End, proof: &crate::kernel::OriginalCoedgeCorrespondence) -> bool {
    let Some(binding) = proof.binding() else {
        return false;
    };
    let delta = Interval::point(a.exit).sub(Interval::point(a.endpoint));
    a.binding.face == binding.face
        && a.binding.edge == binding.edge
        && a.binding.endpoints == binding.endpoints
        && a.binding.owner == binding.owner
        && a.binding.source_face == binding.source_face
        && a.binding.source_face_use == binding.source_face_use
        && a.binding.source.native_orientation == binding.source.native_orientation
        && binding.owner == [b.use_.face, b.use_.wire, b.use_.index]
        && a.coedge == b.use_.coedge
        && a.orientation == b.use_.orientation
        && a.binding.edge == b.edge
        && a.absolute_back == b.absolute_back
        && a.endpoint == b.endpoint
        && a.exit == b.exit
        && b.delta_t.lo == delta.lo
        && b.delta_t.hi == delta.hi
        && equal_box(a.joint_uv, b.joint_uv)
        && equal_box(a.offset, b.offset)
        && a.collar == b.collar
        && a.native_range == proof.native_range()
        && a.uv_range == proof.uv_range()
        && a.margin == proof.margin_uv()
        && a.position_error == proof.position_error_mm()
        && a.provenance == proof.provenance()
}

fn same_native_use(a: NativeUse, b: NativeUse) -> bool {
    a.face_id == b.face_id
        && a.wire_index == b.wire_index
        && a.coedge_index == b.coedge_index
        && a.edge_id == b.edge_id
        && a.orientation == b.orientation
}

/// Both APIs resolve the stored curve through this same immutable
/// (face, absolute wire, absolute occurrence). The retained OriginalMap supplies
/// the endpoint parameter law; matching evaluated UV boxes is not association.
fn native_endpoint_operands(
    domain: &PreparedFaceTrimDomain<'_>,
    recipe: &CornerRecipe,
    vertex: VertexId<Point3>,
) -> Result<[NativeUse; 2], UvPrepareStage> {
    let face = domain.native().face();
    let ends = [recipe.outgoing, recipe.incoming];
    let mut uses = [None; 2];
    for (i, end) in ends.into_iter().enumerate() {
        let proof = domain
            .original_coedge_correspondence(end.use_.coedge)
            .map_err(|_| UvPrepareStage::NativeOperandAssociation)?;
        let binding = proof
            .binding()
            .ok_or(UvPrepareStage::NativeOperandAssociation)?;
        let prepared = domain
            .native()
            .coedges()
            .get(end.use_.coedge)
            .ok_or(UvPrepareStage::NativeOperandAssociation)?;
        let expected = NativeUse {
            face_id: binding.face,
            wire_index: end.use_.wire,
            coedge_index: end.use_.index,
            edge_id: end.edge,
            orientation: end.use_.orientation,
        };
        let endpoint_index = usize::from(i == 1);
        if !same_native_use(prepared.identity, expected)
            || binding.owner != [end.use_.face, end.use_.wire, end.use_.index]
            || binding.face != face.id()
            || end.endpoint != proof.native_range()[usize::from(end.absolute_back)]
            || proof.uv_range()[usize::from(end.absolute_back)]
                != prepared.oriented_range[endpoint_index]
            || binding.endpoints
                [usize::from(end.absolute_back == binding.source.native_orientation)]
                != vertex
        {
            return Err(UvPrepareStage::NativeOperandAssociation);
        }
        uses[i] = Some(prepared.identity);
    }
    let [outgoing, incoming] = uses.map(|use_| use_.expect("both operands associated"));
    let wire = face
        .absolute_boundaries()
        .get(outgoing.wire_index)
        .ok_or(UvPrepareStage::NativeAdjacency)?;
    if incoming.wire_index != outgoing.wire_index || wire.is_empty() {
        return Err(UvPrepareStage::NativeAdjacency);
    }
    // Native admission closes consecutive ABSOLUTE uses; inversion reverses
    // material incoming/outgoing, not the absolute wire indexing.
    let (absolute_in, absolute_out) = if face.orientation() {
        (incoming, outgoing)
    } else {
        (outgoing, incoming)
    };
    if (absolute_in.coedge_index + 1) % wire.len() != absolute_out.coedge_index
        || wire[absolute_in.coedge_index].back().id() != vertex
        || wire[absolute_out.coedge_index].front().id() != vertex
    {
        return Err(UvPrepareStage::NativeAdjacency);
    }
    Ok([outgoing, incoming])
}

fn native_endpoint_authority(
    domain: &PreparedFaceTrimDomain<'_>,
    recipe: &CornerRecipe,
    vertex: VertexId<Point3>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Result<EndpointAuthority, UvPrepareStage>, GeometryError> {
    let [outgoing, incoming] = match native_endpoint_operands(domain, recipe, vertex) {
        Ok(uses) => uses,
        Err(stage) => return Ok(Err(stage)),
    };
    let mut authority = None;
    for (index, joint) in domain.native().joints().iter().enumerate() {
        work.charge(1, cancel)?;
        if !same_native_use(joint.incoming, incoming) || !same_native_use(joint.outgoing, outgoing)
        {
            continue;
        }
        if authority.is_some() || joint.vertex_id != vertex {
            return Ok(Err(UvPrepareStage::UnsupportedNativeJoint));
        }
        authority = Some(match joint.kind {
            monstertruck_modeling::TrimJointKind::EndpointUncertainty => {
                EndpointAuthority::EndpointUncertaintyJoint {
                    index,
                    enclosure: joint.enclosure.map(|v| Interval {
                        lo: v.lower,
                        hi: v.upper,
                    }),
                }
            }
            monstertruck_modeling::TrimJointKind::SphericalPole { .. } => {
                EndpointAuthority::SphericalPoleJoint { index }
            }
            _ => return Ok(Err(UvPrepareStage::UnsupportedNativeJoint)),
        });
    }
    // This is the COMPLETE prepared chart's closed-adjacency theorem, only
    // after actual adjacency, NativeV, both stored operands and endpoint laws
    // above. Native stores every nonexact adjacent closure as a joint.
    Ok(Ok(
        authority.unwrap_or(EndpointAuthority::ExactNativeAdjacency)
    ))
}

/// Caller has already established that Native's convex envelope belongs to
/// these exact operands. Interval intersection only sharpens their known image.
fn admitted_joint_midpoint(a: Box2, b: Box2, enclosure: Box2) -> Option<Box2> {
    let mut joint = ZERO2;
    for i in 0..2 {
        joint[i] = intersection(a[i].add(b[i]).mul(Interval::point(0.5)), enclosure[i])?;
    }
    Some(joint)
}

impl OriginalCornerComparison {
    pub(super) fn prepare(
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        vertex: VertexId<Point3>,
        allowance: f64,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<UvComparisonPreparation, GeometryError> {
        work.charge(1, cancel)?;
        let ends = [recipe.outgoing, recipe.incoming];
        let originals = ends.map(|end| domain.original_coedge(end.use_.coedge).ok());
        let contexts = std::array::from_fn(|i| {
            let end = ends[i];
            let proof = originals[i].and_then(|original| original.correspondence.as_ref());
            let inventory = proof.map(|proof| proof.span_inventory());
            UvPrepareEndContext {
                owner: [end.use_.face, end.use_.wire, end.use_.index],
                coedge: end.use_.coedge,
                absolute_back: end.absolute_back,
                orientation: end.use_.orientation,
                endpoint: end.endpoint,
                exit: end.exit,
                joint_uv: end.joint_uv,
                native_range: proof.map(|proof| proof.native_range()),
                uv_range: proof.map(|proof| proof.uv_range()),
                margin_uv: proof.map(|proof| proof.margin_uv()),
                position_error_mm: proof.map(|proof| proof.position_error_mm()),
                relative_derivative_error: proof
                    .map(|proof| proof.relative_derivative_error_bound()),
                span_inventory: inventory.map(|inventory| {
                    [
                        inventory.source_knot_boundaries,
                        inventory.uv_knot_boundaries,
                        inventory.continuation_spans,
                    ]
                }),
                provenance: proof.map(|proof| proof.provenance()),
                source_face: proof
                    .and_then(|proof| proof.binding())
                    .and_then(|binding| binding.source_face),
                source_face_use: proof
                    .and_then(|proof| proof.binding())
                    .and_then(|binding| binding.source_face_use),
                source_edge: proof
                    .and_then(|proof| proof.binding())
                    .and_then(|binding| binding.source.source_edge),
                source_coedge: proof
                    .and_then(|proof| proof.binding())
                    .and_then(|binding| binding.source.source_coedge),
                binding_owner: proof
                    .and_then(|proof| proof.binding())
                    .map(|binding| binding.owner),
                native_orientation: proof
                    .and_then(|proof| proof.binding())
                    .map(|binding| binding.source.native_orientation),
                source_chart_margin_uv: originals[i]
                    .map(|original| original.source_chart_margin_uv),
                source_chart_error_mm: originals[i].map(|original| original.source_chart_error_mm),
                source_chart_parameter_range: originals[i]
                    .and_then(|original| original.source_chart_parameter_range),
                source_chart_certificate: originals[i]
                    .map(|original| original.source_chart_certificate),
                source_chart_declared: originals[i]
                    .is_some_and(|original| original.source_chart_declared),
                binding_present: proof.is_some_and(|proof| proof.binding().is_some()),
                native_vertex_matches: proof.and_then(|proof| proof.binding()).is_some_and(
                    |binding| {
                        binding.endpoints
                            [usize::from(end.absolute_back == binding.source.native_orientation)]
                            == vertex
                    },
                ),
            }
        });
        let shift: Box2 = std::array::from_fn(|i| {
            exact_difference(ends[1].joint_uv[i], ends[0].joint_uv[i]).mul(Interval::point(0.5))
        });
        let vertex_point = domain
            .native()
            .face()
            .absolute_boundaries()
            .get(ends[0].use_.wire)
            .and_then(|wire| wire.get(ends[0].use_.index))
            .and_then(|edge| {
                if edge.front().id() == vertex {
                    Some(edge.front().point().into())
                } else if edge.back().id() == vertex {
                    Some(edge.back().point().into())
                } else {
                    None
                }
            });
        let reject = |stage, end| {
            UvComparisonPreparation::Refused(UvPrepareRefusal {
                stage,
                end,
                vertex_point,
                ends: contexts,
                correction: [shift, shift.map(Interval::neg)],
            })
        };
        if ends[0].use_.face != ends[1].use_.face || ends[0].use_.wire != ends[1].use_.wire {
            return Ok(reject(UvPrepareStage::CornerOwner, None));
        }
        let mut keys = [None; 2];
        for (i, end) in ends.iter().enumerate() {
            let Some(original) = originals[i] else {
                return Ok(reject(UvPrepareStage::CoedgeInventory, Some(i)));
            };
            let Some(proof) = original.correspondence.as_ref() else {
                return Ok(reject(UvPrepareStage::WholeForwardReceipt, Some(i)));
            };
            let Some(binding) = proof.binding() else {
                return Ok(reject(UvPrepareStage::NativeBinding, Some(i)));
            };
            if proof.provenance()
                == crate::kernel::OriginalCorrespondenceProvenance::CompletedPoleGraph
            {
                return Ok(reject(UvPrepareStage::Provenance, Some(i)));
            }
            if binding.owner != [end.use_.face, end.use_.wire, end.use_.index] {
                return Ok(reject(UvPrepareStage::BindingOwner, Some(i)));
            }
            if !contexts[i].native_vertex_matches {
                return Ok(reject(UvPrepareStage::NativeVertex, Some(i)));
            }
            let face = domain.native().face();
            let Some(wire) = face.absolute_boundaries().get(end.use_.wire) else {
                return Ok(reject(UvPrepareStage::NativeWire, Some(i)));
            };
            let Some(edge) = wire.get(end.use_.index) else {
                return Ok(reject(UvPrepareStage::NativeEdge, Some(i)));
            };
            if edge.id() != binding.edge || binding.edge != end.edge {
                return Ok(reject(UvPrepareStage::NativeEdge, Some(i)));
            }
            if binding.source.native_orientation != edge.orientation() {
                return Ok(reject(UvPrepareStage::NativeOrientation, Some(i)));
            }
            if [edge.front().id(), edge.back().id()] != binding.endpoints {
                return Ok(reject(UvPrepareStage::NativeEndpoints, Some(i)));
            }
            if binding.face != face.id() {
                return Ok(reject(UvPrepareStage::NativeFace, Some(i)));
            }
            if original.source_chart_parameter_range
                != Some((proof.uv_range()[0], proof.uv_range()[1]))
            {
                return Ok(reject(UvPrepareStage::Gauge, Some(i)));
            }
            // Reuse the complete retained-receipt guard, including source-chart
            // certification and metadata orientation; no alternate authority.
            if domain
                .original_coedge_correspondence(end.use_.coedge)
                .is_err()
            {
                return Ok(reject(UvPrepareStage::WholeForwardOwnership, Some(i)));
            }
            let range = proof.native_range();
            let delta = Interval::point(end.exit).sub(Interval::point(end.endpoint));
            if end.endpoint != range[usize::from(end.absolute_back)] {
                return Ok(reject(UvPrepareStage::NativeEndpoint, Some(i)));
            }
            if end.absolute_back != (end.use_.orientation == (i == 1)) {
                return Ok(reject(UvPrepareStage::EndpointOrientation, Some(i)));
            }
            if end.delta_t.lo != delta.lo || end.delta_t.hi != delta.hi {
                return Ok(reject(UvPrepareStage::EndpointDelta, Some(i)));
            }
            if !end.exit.is_finite() || end.exit <= range[0] || end.exit >= range[1] {
                return Ok(reject(UvPrepareStage::InteriorExit, Some(i)));
            }
            if !equal_box(
                chart_jet(domain, end.use_.coedge, [end.endpoint; 2], work, cancel)?.position,
                end.joint_uv,
            ) {
                return Ok(reject(UvPrepareStage::EndpointExpression, Some(i)));
            }
            keys[i] = Some(EndKey {
                binding: *binding,
                coedge: end.use_.coedge,
                orientation: end.use_.orientation,
                absolute_back: end.absolute_back,
                endpoint: end.endpoint,
                exit: end.exit,
                joint_uv: end.joint_uv,
                offset: end.offset,
                collar: end.collar,
                native_range: range,
                uv_range: proof.uv_range(),
                margin: proof.margin_uv(),
                position_error: proof.position_error_mm(),
                provenance: proof.provenance(),
            });
            if proof
                .margin_uv()
                .into_iter()
                .any(|margin| !margin.is_finite() || margin < 0.0)
            {
                return Ok(reject(UvPrepareStage::FibreMargin, Some(i)));
            }
        }
        if !allowance.is_finite() || allowance < 0.0 {
            return Ok(reject(UvPrepareStage::FreshGermTransport, None));
        }
        let authority = match native_endpoint_authority(domain, recipe, vertex, work, cancel)? {
            Ok(authority) => authority,
            Err(stage) => return Ok(reject(stage, None)),
        };
        let (joint, correction, exact) = match authority {
            EndpointAuthority::ExactNativeAdjacency => (ends[0].joint_uv, [ZERO2; 2], true),
            EndpointAuthority::EndpointUncertaintyJoint { enclosure, .. } => {
                // Native's convex envelope contains these SAME exact endpoint
                // operands. Their exact average and both straight connectors
                // therefore lie inside it. Intersect only with that known image,
                // never with an OriginalMap numerical tube.
                let Some(joint) =
                    admitted_joint_midpoint(ends[0].joint_uv, ends[1].joint_uv, enclosure)
                else {
                    return Ok(reject(UvPrepareStage::JointOutsideBothFibres, None));
                };
                (joint, [shift, shift.map(Interval::neg)], false)
            }
            EndpointAuthority::SphericalPoleJoint { index } => {
                let Some(comparison) = PoleComparison::prepare(
                    domain,
                    recipe,
                    vertex,
                    index,
                    allowance,
                    remaining_bytes,
                    work,
                    cancel,
                )?
                else {
                    return Ok(reject(UvPrepareStage::SphericalPoleComparison, None));
                };
                return Ok(UvComparisonPreparation::Prepared(Self::SphericalPole(
                    comparison,
                )));
            }
        };
        let mut candidate = UvJointComparison {
            vertex,
            ends: keys.map(|key| key.expect("both original germs checked")),
            joint,
            correction,
            completion: ComparisonCompletion::Linear,
            exact_joint: exact,
            authority,
            transport: None,
            allowance,
        };
        if !candidate.is_identity() {
            let Some(transport) = comparison_transport::certify(
                domain,
                recipe,
                &candidate,
                allowance,
                remaining_bytes,
                work,
                cancel,
            )?
            else {
                return Ok(reject(UvPrepareStage::FreshGermTransport, None));
            };
            candidate.transport = Some(transport);
        }
        // This is a source-only reference-chart proposal. The physical G,
        // original map and literal cutovers above are never changed.
        if let Some(completion) =
            completion::propose(domain, recipe, correction, allowance, work, cancel)?
        {
            let mut smooth = candidate;
            smooth.completion = completion;
            smooth.transport = None;
            if let Some(transport) = comparison_transport::certify(
                domain,
                recipe,
                &smooth,
                allowance,
                remaining_bytes,
                work,
                cancel,
            )? {
                smooth.transport = Some(transport);
                candidate = smooth;
            }
        }
        Ok(UvComparisonPreparation::Prepared(Self::NativeUv(candidate)))
    }

    pub(super) fn is_identity(&self) -> bool {
        matches!(self, Self::NativeUv(comparison) if comparison.is_identity())
    }

    pub(super) fn native_uv(&self) -> Option<&UvJointComparison> {
        match self {
            Self::NativeUv(comparison) => Some(comparison),
            Self::SphericalPole(_) => None,
        }
    }

    pub(super) fn validates(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        vertex: VertexId<Point3>,
    ) -> Result<(), GeometryError> {
        match self {
            Self::NativeUv(comparison) => comparison.validates(domain, recipe, vertex),
            Self::SphericalPole(comparison) => comparison.validate(domain, recipe, vertex),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn source_cost(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        piece: Piece,
        t: Interval,
        point: [f64; 3],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<ComparisonSourceCost, GeometryError> {
        match self {
            Self::NativeUv(comparison) => {
                comparison.source_cost(domain, recipe, piece, t, point, work, cancel)
            }
            Self::SphericalPole(comparison) => {
                comparison.source_cost(domain, recipe, piece, t, point, work, cancel)
            }
        }
    }

    pub(super) fn disk_owned(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        match self {
            Self::NativeUv(comparison) => {
                comparison.disk_owned(domain, recipe, remaining_bytes, work, cancel)
            }
            Self::SphericalPole(comparison) => {
                comparison.disk_owned(domain, recipe, remaining_bytes, work, cancel)
            }
        }
    }
}

impl UvJointComparison {
    pub(super) fn is_identity(&self) -> bool {
        self.exact_joint
            && matches!(self.completion, ComparisonCompletion::Linear)
            && self
                .correction
                .into_iter()
                .all(|shift| equal_box(shift, ZERO2))
    }

    fn validate(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
    ) -> Result<(), GeometryError> {
        for (i, end) in [recipe.outgoing, recipe.incoming].into_iter().enumerate() {
            let proof = domain.original_coedge_correspondence(end.use_.coedge)?;
            let binding = proof
                .binding()
                .ok_or_else(|| refusal("comparison lost NativeUse binding"))?;
            if !same_end(self.ends[i], end, proof)
                || binding.endpoints
                    [usize::from(end.absolute_back == binding.source.native_orientation)]
                    != self.vertex
            {
                return Err(refusal(
                    "UV comparison belongs to different original endpoint germs",
                ));
            }
        }
        let [outgoing, incoming] =
            native_endpoint_operands(domain, recipe, self.vertex).map_err(|stage| {
                refusal(format!(
                    "comparison lost native endpoint operands: {stage:?}"
                ))
            })?;
        // Complete closed-adjacency admission was consumed once at preparation.
        // The immutable operand guards above retain that theorem; do not scan
        // the entire chart's joint inventory again for every star cell.
        let same_authority = match self.authority {
            EndpointAuthority::ExactNativeAdjacency => {
                self.exact_joint
                    && equal_box(self.joint, self.ends[0].joint_uv)
                    && self
                        .correction
                        .into_iter()
                        .all(|shift| equal_box(shift, ZERO2))
            }
            EndpointAuthority::EndpointUncertaintyJoint { index, enclosure } => {
                domain.native().joints().get(index).is_some_and(|joint| {
                    joint.vertex_id == self.vertex
                        && same_native_use(joint.incoming, incoming)
                        && same_native_use(joint.outgoing, outgoing)
                        && matches!(
                            joint.kind,
                            monstertruck_modeling::TrimJointKind::EndpointUncertainty
                        )
                        && equal_box(
                            enclosure,
                            joint.enclosure.map(|v| Interval {
                                lo: v.lower,
                                hi: v.upper,
                            }),
                        )
                })
            }
            EndpointAuthority::SphericalPoleJoint { .. } => false,
        };
        if !same_authority || !self.allowance.is_finite() || self.allowance < 0.0 {
            return Err(refusal(
                "comparison endpoint authority or allowance changed",
            ));
        }
        if !self.is_identity() {
            let Some(transport) = self.transport else {
                return Err(refusal("comparison has no fresh whole-germ transport"));
            };
            if transport.into_iter().any(|bound| {
                !bound.position_error_mm.is_finite()
                    || bound.position_error_mm < 0.0
                    || bound.position_error_mm > self.allowance
                    || !bound.relative_derivative_error.is_finite()
                    || bound.relative_derivative_error < 0.0
                    || bound.relative_derivative_error >= 0.25
            }) {
                return Err(refusal(
                    "comparison whole-germ transport exceeds its original allowance",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn validates(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        vertex: VertexId<Point3>,
    ) -> Result<(), GeometryError> {
        if vertex != self.vertex {
            return Err(refusal("comparison cannot borrow a foreign actual NativeV"));
        }
        self.validate(domain, recipe)
    }

    fn germ_correction(&self, index: usize, s: Interval) -> Result<CorrectionJet, GeometryError> {
        self.completion.correction_jet(self.correction, index, s)
    }

    pub(super) fn apex_line(&self) -> Option<(Box2, Interval)> {
        self.completion.apex_line()
    }

    fn star_correction(
        &self,
        piece: Piece,
        s: Interval,
        t: Interval,
    ) -> Result<CorrectionJet, GeometryError> {
        match piece {
            Piece::Outgoing => self.germ_correction(0, s),
            Piece::Incoming => self.germ_correction(1, s),
            Piece::Middle => {
                let a = self.germ_correction(0, s)?;
                let b = self.germ_correction(1, s)?;
                Ok(CorrectionJet {
                    position: mix(a.position, b.position, t),
                    derivative: mix(a.derivative, b.derivative, t),
                    second_derivative: mix(a.second_derivative, b.second_derivative, t),
                })
            }
        }
    }

    /// The completion coefficients name the original endpoint derivative
    /// operators, not independently chosen vectors from their enclosures.
    pub(super) fn completed_end_jet(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        index: usize,
        s: Interval,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<crate::parametric::ChartCurveEnclosure, GeometryError> {
        let end = if index == 0 {
            recipe.outgoing
        } else {
            recipe.incoming
        };
        let mut jet = end_curve(domain, end, s, work, cancel)?;
        let correction = self.germ_correction(index, s)?;
        jet.position = std::array::from_fn(|i| jet.position[i].add(correction.position[i]));
        jet.derivative = std::array::from_fn(|i| jet.derivative[i].add(correction.derivative[i]));
        jet.second_derivative =
            std::array::from_fn(|i| jet.second_derivative[i].add(correction.second_derivative[i]));
        if s.lo == 0.0 && s.hi == 0.0 {
            jet.position = self.joint;
            if let Some((tangent, scale)) = self.apex_line() {
                jet.derivative = if index == 0 {
                    tangent
                } else {
                    tangent.map(|v| scale.neg().mul(v))
                };
            }
        }
        Ok(jet)
    }

    /// A coordinate hull contains every lambda in the straight comparison
    /// homotopy, in either direction, including its endpoint connectors.
    fn homotopy_star_image(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        piece: Piece,
        s: Interval,
        t: Interval,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Box2, GeometryError> {
        let completed = self.star_jet(domain, recipe, piece, s, t, work, cancel)?.uv;
        if self.is_identity() {
            return Ok(completed);
        }
        let actual = super::star_jet(
            domain,
            recipe.outgoing,
            recipe.incoming,
            piece,
            s,
            t,
            recipe.bulge,
            work,
            cancel,
        )?
        .uv;
        Ok(hull2(actual, completed))
    }

    /// Apply the sealed reference completion and ALL its radial derivatives.
    /// The actual carrier star, outer B and literal cutovers stay untouched.
    pub(super) fn star_jet(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        piece: Piece,
        s: Interval,
        t: Interval,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<StarJet, GeometryError> {
        self.validate(domain, recipe)?;
        let mut jet = super::star_jet(
            domain,
            recipe.outgoing,
            recipe.incoming,
            piece,
            s,
            t,
            recipe.bulge,
            work,
            cancel,
        )?;
        let correction = self.star_correction(piece, s, source_unit_parameter(t)?)?;
        jet.uv = std::array::from_fn(|i| jet.uv[i].add(correction.position[i]));
        jet.ds = std::array::from_fn(|i| jet.ds[i].add(correction.derivative[i]));
        jet.dss = std::array::from_fn(|i| jet.dss[i].add(correction.second_derivative[i]));
        // At the apex use the proven common expression, not an interval sum
        // whose dependency loss would invent a spurious open UV connector.
        if s.lo == 0.0 && s.hi == 0.0 {
            jet.uv = self.joint;
        }
        Ok(jet)
    }

    /// Divided differences use the integral identities
    /// (gamma(s)-gamma(0))/s = integral_0^1 gamma'(s v) dv,
    /// d_s[(gamma(s)-gamma(0))/s] = integral_0^1 v gamma''(s v) dv.
    /// They remain valid at the closed apex, without division by zero or a
    /// third-derivative assumption. The native jet evaluator must accept the
    /// complete restriction; an unqualified C0 crossing propagates refusal.
    fn divided_end(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        index: usize,
        s: Interval,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(Box2, Box2), GeometryError> {
        let whole = self.completed_end_jet(
            domain,
            recipe,
            index,
            Interval { lo: 0.0, hi: s.hi },
            work,
            cancel,
        )?;
        let mut q = whole.derivative;
        let qs = whole.second_derivative.map(|v| v.mul(Interval::point(0.5)));
        if s.lo > 0.0 {
            let at = self.completed_end_jet(domain, recipe, index, s, work, cancel)?;
            for i in 0..2 {
                q[i] = intersection(q[i], at.position[i].sub(self.joint[i]).div(s)?).ok_or_else(
                    || refusal("completed UV divided difference enclosures disagree"),
                )?;
            }
        }
        Ok((q, qs))
    }

    pub(super) fn factored_jet(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        piece: Piece,
        s: Interval,
        t: Interval,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<ComparisonStarJet, GeometryError> {
        self.validate(domain, recipe)?;
        let s = source_unit_parameter(s)?;
        let t = source_unit_parameter(t)?;
        let (q, qs, qt) = match piece {
            Piece::Outgoing | Piece::Incoming => {
                let incoming = matches!(piece, Piece::Incoming);
                let index = usize::from(incoming);
                let (q, qs) = self.divided_end(domain, recipe, index, s, work, cancel)?;
                let weight = if incoming {
                    source_unit_complement(t)?
                } else {
                    t
                };
                let offset = self.ends[index].offset;
                (
                    std::array::from_fn(|i| q[i].add(weight.mul(offset[i]))),
                    qs,
                    if incoming {
                        offset.map(Interval::neg)
                    } else {
                        offset
                    },
                )
            }
            Piece::Middle => {
                let (qa, qsa) = self.divided_end(domain, recipe, 0, s, work, cancel)?;
                let (qb, qsb) = self.divided_end(domain, recipe, 1, s, work, cancel)?;
                let qa: Box2 = std::array::from_fn(|i| qa[i].add(recipe.outgoing.offset[i]));
                let qb: Box2 = std::array::from_fn(|i| qb[i].add(recipe.incoming.offset[i]));
                let blend = mix(qa, qb, t);
                let bubble = t.mul(source_unit_complement(t)?);
                let slope = Interval::point(1.0).sub(Interval::point(2.0).mul(t));
                (
                    std::array::from_fn(|i| blend[i].add(bubble.mul(recipe.bulge[i]))),
                    mix(qsa, qsb, t),
                    std::array::from_fn(|i| qb[i].sub(qa[i]).add(slope.mul(recipe.bulge[i]))),
                )
            }
        };
        let ds: Box2 = std::array::from_fn(|i| q[i].add(s.mul(qs[i])));
        let mut star = self.star_jet(domain, recipe, piece, s, t, work, cancel)?;
        for i in 0..2 {
            star.ds[i] = intersection(star.ds[i], ds[i]).ok_or_else(|| {
                refusal("completed UV first jet disagrees with divided difference")
            })?;
        }
        let factored_jacobian = determinant(star.ds, qt);
        Ok(ComparisonStarJet {
            q,
            qt,
            factored_jacobian,
        })
    }

    /// Recompute at the fixed, already chosen M/B/exits. The returned cost
    /// includes BOTH directions of the straight UV comparison homotopy, whose
    /// common image and derivative bound are identical under reversal.
    pub(super) fn source_cost(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        piece: Piece,
        t: Interval,
        point: [f64; 3],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<ComparisonSourceCost, GeometryError> {
        work.charge(1, cancel)?;
        let completed = self.star_jet(domain, recipe, piece, UNIT, t, work, cancel)?;
        let actual = super::star_jet(
            domain,
            recipe.outgoing,
            recipe.incoming,
            piece,
            UNIT,
            t,
            recipe.bulge,
            work,
            cancel,
        )?;
        let boundary = boundary_jet(
            domain,
            recipe.outgoing,
            recipe.incoming,
            piece,
            t,
            recipe.bulge,
            work,
            cancel,
        )?;
        let correction = self.star_correction(piece, UNIT, source_unit_parameter(t)?)?;
        domain.native().face().with_surface(|surface| {
            let comparison = surface_enclosure(surface, uv_box(completed.uv), work, cancel)?;
            let at_joint = centered_source_point(surface, self.joint, point, work, cancel)?;
            let acceleration: Box3 = std::array::from_fn(|i| {
                comparison.duu[i]
                    .mul(completed.ds[0].square())
                    .add(
                        Interval::point(2.0)
                            .mul(comparison.duv[i])
                            .mul(completed.ds[0])
                            .mul(completed.ds[1]),
                    )
                    .add(comparison.dvv[i].mul(completed.ds[1].square()))
                    .add(comparison.du[i].mul(completed.dss[0]))
                    .add(comparison.dv[i].mul(completed.dss[1]))
            });
            let face_displacement: Box3 = std::array::from_fn(|i| {
                let remainder = up(acceleration[i].abs_upper() * 0.125);
                at_joint[i]
                    .neg()
                    .hull(boundary.displacement[i])
                    .add(Interval {
                        lo: -remainder,
                        hi: remainder,
                    })
            });
            let homotopy = surface_enclosure(
                surface,
                uv_box(hull2(actual.uv, completed.uv)),
                work,
                cancel,
            )?;
            let physical_shift = compose_first(&homotopy, correction.position);
            let homotopy_mm = length(physical_shift)?.hi;
            let transport_mm = self.transport.map_or(0.0, |bounds| {
                bounds[0].position_error_mm.max(bounds[1].position_error_mm)
            });
            let displacement: Box3 = std::array::from_fn(|i| {
                // The correspondence can be traversed in either direction.
                let cost = physical_shift[i].abs_upper();
                face_displacement[i].add(Interval {
                    lo: -cost,
                    hi: cost,
                })
            });
            // These are distinct reference paths, not concatenated transports:
            // M -> F(original H) uses the face and UV-homotopy bounds;
            // F(completed germ) -> original C uses the fresh receipt. The
            // original-C -> M chord bound is consumed separately by the plan.
            let error_mm = up(length(face_displacement)?.hi + homotopy_mm).max(transport_mm);
            if !error_mm.is_finite() || !homotopy_mm.is_finite() {
                return Err(refusal(
                    "fixed source comparison has no finite physical error bound",
                ));
            }
            Ok(ComparisonSourceCost {
                source: homotopy.position,
                displacement,
                error_mm,
                homotopy_mm,
            })
        })
    }
}

// All traversals are binary, depth-first and use fixed scratch. Nested image
// exclusion can retain one native-range stack while traversing a star stack.
pub(super) const OWNERSHIP_STACK_BYTES: usize =
    3 * std::mem::size_of::<[([[f64; 2]; 2], u8); MAX_DEPTH as usize + 1]>()
        + std::mem::size_of::<[([f64; 2], u8); MAX_DEPTH as usize + 1]>()
        + comparison_transport::STACK_BYTES
        + completion::STACK_BYTES
        + apex_factor::STACK_BYTES
        + pole_comparison::STACK_BYTES;

#[derive(Clone, Copy, PartialEq, Debug)]
enum ComparisonBoundary {
    OutgoingGerm,
    OutgoingCross,
    Bridge,
    IncomingCross,
    IncomingGerm,
}

struct UvBoundaryJet {
    position: Box2,
    derivative: Box2,
}

fn ownership_failure(recipe: &CornerRecipe, gate: impl std::fmt::Display) -> GeometryError {
    refusal(format!(
        "original UV comparison ownership gate={gate} source_face={} coedges={:?} native_endpoint_exits={:?}",
        recipe.outgoing.use_.face,
        [recipe.outgoing.use_.coedge, recipe.incoming.use_.coedge],
        [
            (recipe.outgoing.endpoint, recipe.outgoing.exit),
            (recipe.incoming.endpoint, recipe.incoming.exit)
        ],
    ))
}

/// Uniform contraction proves injectivity on the CLOSED pair rectangle.
/// The separately established exact endpoint root supplies existence; no
/// strict interval-Newton inclusion at a boundary root is required.
fn pair_jacobian_injective(a: Box2, b: Box2) -> bool {
    let jac = [[a[0], b[0].neg()], [a[1], b[1].neg()]];
    let mid = jac.map(|row| row.map(|v| v.mid()));
    let det = mid[0][0] * mid[1][1] - mid[0][1] * mid[1][0];
    if !det.is_finite() || det == 0.0 {
        return false;
    }
    let inverse = [
        [mid[1][1] / det, -mid[0][1] / det],
        [-mid[1][0] / det, mid[0][0] / det],
    ];
    if inverse.into_iter().flatten().any(|v| !v.is_finite()) {
        return false;
    }
    (0..2).all(|i| {
        let remainder = std::array::from_fn::<_, 2, _>(|j| {
            Interval::point(if i == j { 1.0 } else { 0.0 }).sub(
                Interval::point(inverse[i][0])
                    .mul(jac[0][j])
                    .add(Interval::point(inverse[i][1]).mul(jac[1][j])),
            )
        });
        Interval::point(remainder[0].abs_upper())
            .add(Interval::point(remainder[1].abs_upper()))
            .hi
            < 1.0
    })
}

impl UvJointComparison {
    fn boundary_jet(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        boundary: ComparisonBoundary,
        t: Interval,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<UvBoundaryJet, GeometryError> {
        let t = source_unit_parameter(t)?;
        let (piece, reverse) = match boundary {
            ComparisonBoundary::OutgoingGerm => (Piece::Outgoing, false),
            ComparisonBoundary::IncomingGerm => (Piece::Incoming, true),
            ComparisonBoundary::OutgoingCross => (Piece::Outgoing, false),
            ComparisonBoundary::IncomingCross => (Piece::Incoming, false),
            ComparisonBoundary::Bridge => (Piece::Middle, false),
        };
        if matches!(
            boundary,
            ComparisonBoundary::OutgoingGerm | ComparisonBoundary::IncomingGerm
        ) {
            let s = if reverse {
                source_unit_complement(t)?
            } else {
                t
            };
            let theta = Interval::point(if reverse { 1.0 } else { 0.0 });
            let jet = self.star_jet(domain, recipe, piece, s, theta, work, cancel)?;
            return Ok(UvBoundaryJet {
                position: jet.uv,
                derivative: if reverse {
                    jet.ds.map(Interval::neg)
                } else {
                    jet.ds
                },
            });
        }
        let position = super::star_jet(
            domain,
            recipe.outgoing,
            recipe.incoming,
            piece,
            Interval::point(1.0),
            t,
            recipe.bulge,
            work,
            cancel,
        )?
        .uv;
        let derivative = match boundary {
            ComparisonBoundary::OutgoingCross => recipe.outgoing.offset,
            ComparisonBoundary::IncomingCross => recipe.incoming.offset.map(Interval::neg),
            ComparisonBoundary::Bridge => {
                let a = end_curve(domain, recipe.outgoing, Interval::point(1.0), work, cancel)?;
                let b = end_curve(domain, recipe.incoming, Interval::point(1.0), work, cancel)?;
                let slope = Interval::point(1.0).sub(Interval::point(2.0).mul(t));
                std::array::from_fn(|i| {
                    b.position[i]
                        .add(recipe.incoming.offset[i])
                        .sub(a.position[i].add(recipe.outgoing.offset[i]))
                        .add(slope.mul(recipe.bulge[i]))
                })
            }
            _ => unreachable!("germ boundary returned above"),
        };
        Ok(UvBoundaryJet {
            position,
            derivative,
        })
    }

    fn side_germs_separated(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let (a, _) = self.divided_end(domain, recipe, 0, UNIT, work, cancel)?;
        let (b, _) = self.divided_end(domain, recipe, 1, UNIT, work, cancel)?;
        let tangent =
            end_curve(domain, recipe.outgoing, Interval::point(0.0), work, cancel)?.derivative;
        // q_a and q_b are divided differences of the COMPLETED side curves.
        // Strict opposite projections exclude every open pair, including a
        // smooth NativeV join with collinear, opposite endpoint tangents.
        Ok([tangent, a, b.map(Interval::neg)]
            .into_iter()
            .chain(divided_ray_separator(a, b))
            .any(|row| dot2(row, a).lo > 0.0 && dot2(row, b).hi < 0.0))
    }

    fn boundary_pair_misses(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        a: ComparisonBoundary,
        b: ComparisonBoundary,
        root: Option<[f64; 2]>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let mut stack = [([[0.0; 2]; 2], 0_u8); MAX_DEPTH as usize + 1];
        stack[0] = ([[0.0, 1.0]; 2], 0);
        let mut pending = 1;
        while pending != 0 {
            pending -= 1;
            let (cell, depth) = stack[pending];
            work.charge(1, cancel)?;
            let ja = self.boundary_jet(
                domain,
                recipe,
                a,
                Interval::new(cell[0][0], cell[0][1])?,
                work,
                cancel,
            )?;
            let jb = self.boundary_jet(
                domain,
                recipe,
                b,
                Interval::new(cell[1][0], cell[1][1])?,
                work,
                cancel,
            )?;
            if disjoint(ja.position, jb.position) {
                continue;
            }
            if let Some(root) = root {
                let ar = [cell[0][0].min(root[0]), cell[0][1].max(root[0])];
                let br = [cell[1][0].min(root[1]), cell[1][1].max(root[1])];
                let da = self
                    .boundary_jet(
                        domain,
                        recipe,
                        a,
                        Interval::new(ar[0], ar[1])?,
                        work,
                        cancel,
                    )?
                    .derivative;
                let db = self
                    .boundary_jet(
                        domain,
                        recipe,
                        b,
                        Interval::new(br[0], br[1])?,
                        work,
                        cancel,
                    )?
                    .derivative;
                if pair_jacobian_injective(da, db) {
                    continue;
                }
            }
            if depth >= MAX_DEPTH {
                return Err(ownership_failure(
                    recipe,
                    format_args!(
                        "simpleBoundary pair={a:?}/{b:?} parameter_pair={cell:?} known_endpoint={root:?}"
                    ),
                ));
            }
            let axis = usize::from(cell[1][1] - cell[1][0] > cell[0][1] - cell[0][0]);
            let mid = cell[axis][0] * 0.5 + cell[axis][1] * 0.5;
            if mid <= cell[axis][0] || mid >= cell[axis][1] {
                return Err(ownership_failure(
                    recipe,
                    format_args!("simpleBoundary indivisible_pair={cell:?}"),
                ));
            }
            let mut left = cell;
            let mut right = cell;
            left[axis][1] = mid;
            right[axis][0] = mid;
            stack[pending] = (right, depth + 1);
            stack[pending + 1] = (left, depth + 1);
            pending += 2;
        }
        Ok(true)
    }

    fn simple_boundary(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        use ComparisonBoundary::*;
        let mut boundary = [OutgoingGerm; 5];
        let mut count = 0;
        for item in [
            OutgoingGerm,
            OutgoingCross,
            Bridge,
            IncomingCross,
            IncomingGerm,
        ] {
            if item == OutgoingCross && !recipe.outgoing.collar
                || item == IncomingCross && !recipe.incoming.collar
            {
                continue;
            }
            boundary[count] = item;
            count += 1;
            let whole = self.boundary_jet(domain, recipe, item, UNIT, work, cancel)?;
            let midpoint =
                self.boundary_jet(domain, recipe, item, Interval::point(0.5), work, cancel)?;
            // A whole-expression monotone row proves each component simple.
            if dot2(midpoint.derivative, whole.derivative).lo <= 0.0 {
                return Err(ownership_failure(
                    recipe,
                    format_args!("simpleBoundary nonmonotone_component={item:?} parameter=[0,1]"),
                ));
            }
        }
        if !self.side_germs_separated(domain, recipe, work, cancel)? {
            return Err(ownership_failure(
                recipe,
                "simpleBoundary apexSideGerms parameter=[0,1]",
            ));
        }
        for i in 0..count {
            for j in i + 1..count {
                if i == 0 && j + 1 == count {
                    continue;
                } // exact common NativeV, opposite divided rays
                let root = if j == i + 1 { Some([1.0, 0.0]) } else { None };
                if !self.boundary_pair_misses(
                    domain,
                    recipe,
                    boundary[i],
                    boundary[j],
                    root,
                    work,
                    cancel,
                )? {
                    return Err(ownership_failure(
                        recipe,
                        "simpleBoundary foreign_component_pair",
                    ));
                }
            }
        }
        Ok(true)
    }

    fn image_misses_box(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        other: Box2,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        for piece in [Piece::Outgoing, Piece::Middle, Piece::Incoming] {
            if matches!(piece, Piece::Outgoing) && !recipe.outgoing.collar
                || matches!(piece, Piece::Incoming) && !recipe.incoming.collar
            {
                continue;
            }
            let mut stack = [([[0.0; 2]; 2], 0_u8); MAX_DEPTH as usize + 1];
            stack[0] = ([[0.0, 1.0]; 2], 0);
            let mut pending = 1;
            while pending != 0 {
                pending -= 1;
                let (cell, depth) = stack[pending];
                work.charge(1, cancel)?;
                let h = self.homotopy_star_image(
                    domain,
                    recipe,
                    piece,
                    Interval::new(cell[0][0], cell[0][1])?,
                    Interval::new(cell[1][0], cell[1][1])?,
                    work,
                    cancel,
                )?;
                if disjoint(h, other) {
                    continue;
                }
                if depth >= MAX_DEPTH {
                    return Ok(false);
                }
                let axis = usize::from(cell[1][1] - cell[1][0] > cell[0][1] - cell[0][0]);
                let mid = cell[axis][0] * 0.5 + cell[axis][1] * 0.5;
                if mid <= cell[axis][0] || mid >= cell[axis][1] {
                    return Ok(false);
                }
                let mut left = cell;
                let mut right = cell;
                left[axis][1] = mid;
                right[axis][0] = mid;
                stack[pending] = (right, depth + 1);
                stack[pending + 1] = (left, depth + 1);
                pending += 2;
            }
        }
        Ok(true)
    }

    /// A closed-half-plane support for the ENTIRE star at one owned exit.
    /// The zero dot product of gamma'(exit) and its canonical rotated offset
    /// is an operand identity, not an interval test for numerical zero.
    fn before_exit(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        index: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let end = if index == 0 {
            recipe.outgoing
        } else {
            recipe.incoming
        };
        let at = chart_jet(domain, end.use_.coedge, [end.exit; 2], work, cancel)?;
        let direction = if end.absolute_back { -1.0 } else { 1.0 };
        let row = at.derivative.map(|v| v.mul(Interval::point(direction)));
        let own = end_curve(domain, end, UNIT, work, cancel)?;
        let correction = self.germ_correction(index, UNIT)?;
        let completed_derivative =
            std::array::from_fn(|i| own.derivative[i].add(correction.derivative[i]));
        if dot2(row, hull2(own.derivative, completed_derivative)).lo <= 0.0 {
            return Ok(false);
        }
        let other_piece = if index == 0 {
            Piece::Incoming
        } else {
            Piece::Outgoing
        };
        let other_star =
            self.homotopy_star_image(domain, recipe, other_piece, UNIT, UNIT, work, cancel)?;
        let relative_other = std::array::from_fn(|i| other_star[i].sub(at.position[i]));
        let other_support = dot2(row, relative_other);
        let bulge = dot2(row, recipe.bulge);
        // Linear interpolation plus theta(1-theta)*bulge: a positive bulge
        // needs to fit inside the strict support margin on the opposite side.
        if other_support.hi >= 0.0
            || other_support.add(Interval::point(bulge.hi.max(0.0))).hi >= 0.0
        {
            return Ok(false);
        }
        Ok(true)
    }

    fn regular_closed_star(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let proposal = self.factored_jet(
            domain,
            recipe,
            Piece::Middle,
            Interval::point(0.5),
            Interval::point(0.5),
            work,
            cancel,
        )?;
        let sign = if proposal.factored_jacobian.lo > 0.0 {
            1.0
        } else if proposal.factored_jacobian.hi < 0.0 {
            -1.0
        } else {
            return Err(ownership_failure(
                recipe,
                "factoredJac orientation_proposal s=theta=1/2",
            ));
        };
        for piece in [Piece::Outgoing, Piece::Middle, Piece::Incoming] {
            if matches!(piece, Piece::Outgoing) && !recipe.outgoing.collar
                || matches!(piece, Piece::Incoming) && !recipe.incoming.collar
            {
                continue;
            }
            let mut stack = [([[0.0; 2]; 2], 0_u8); MAX_DEPTH as usize + 1];
            stack[0] = ([[0.0, 1.0]; 2], 0);
            let mut pending = 1;
            while pending != 0 {
                pending -= 1;
                let (cell, depth) = stack[pending];
                work.charge(1, cancel)?;
                let s = Interval::new(cell[0][0], cell[0][1])?;
                let theta = Interval::new(cell[1][0], cell[1][1])?;
                if matches!(piece, Piece::Middle)
                    && s.lo == 0.0
                    && matches!(self.completion, ComparisonCompletion::Hermite(_))
                    && apex_factor::proves(domain, recipe, self, s, theta, sign, work, cancel)?
                {
                    continue;
                }
                let h = self.factored_jet(domain, recipe, piece, s, theta, work, cancel)?;
                if h.factored_jacobian.mul(Interval::point(sign)).lo > 0.0
                    && determinant(h.q, h.qt).mul(Interval::point(sign)).lo > 0.0
                    && !(h.q[0].contains_zero() && h.q[1].contains_zero())
                {
                    continue;
                }
                if depth >= MAX_DEPTH {
                    return Err(ownership_failure(
                        recipe,
                        format_args!(
                            "factoredJac piece={} parameter_cell={cell:?} determinant={:?} angular={:?} q={:?} offsets={:?} bulge={:?} completed_apex_line={:?}",
                            match piece {
                                Piece::Outgoing => "outgoing",
                                Piece::Middle => "middle",
                                Piece::Incoming => "incoming",
                            },
                            h.factored_jacobian,
                            determinant(h.q, h.qt),
                            h.q,
                            [recipe.outgoing.offset, recipe.incoming.offset],
                            recipe.bulge,
                            self.apex_line(),
                        ),
                    ));
                }
                let axis = usize::from(cell[1][1] - cell[1][0] > cell[0][1] - cell[0][0]);
                let mid = cell[axis][0] * 0.5 + cell[axis][1] * 0.5;
                if mid <= cell[axis][0] || mid >= cell[axis][1] {
                    return Err(ownership_failure(
                        recipe,
                        format_args!("factoredJac indivisible_cell={cell:?}"),
                    ));
                }
                let mut left = cell;
                let mut right = cell;
                left[axis][1] = mid;
                right[axis][0] = mid;
                stack[pending] = (right, depth + 1);
                stack[pending + 1] = (left, depth + 1);
                pending += 2;
            }
        }
        Ok(true)
    }

    fn owned_central_misses(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        end: End,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let Some(collar) = OriginalCollar::for_enclosure(domain, end.use_, work, cancel)? else {
            return Err(ownership_failure(recipe, "constructorOwned central_collar"));
        };
        let Some(embedding) = collar.embedding([end.exit; 2], [0.0, 1.0], work, cancel)? else {
            return Err(ownership_failure(recipe, "ownedCentral scopedExitAtlas"));
        };
        if !embedding.covers(&collar, [end.exit; 2], [0.0, 1.0]) {
            return Err(ownership_failure(
                recipe,
                "ownedCentral scopedExitAtlas identity",
            ));
        }
        let mut central = collar.range;
        for owned in [recipe.outgoing, recipe.incoming] {
            if owned.edge != end.edge {
                continue;
            }
            if owned.absolute_back {
                central[1] = owned.exit
            } else {
                central[0] = owned.exit
            }
        }
        if central[0] > central[1] {
            return Err(ownership_failure(
                recipe,
                format_args!("ownedCentral inverted_scope={central:?}"),
            ));
        }
        let mut support = [false; 2];
        for (i, owned) in [recipe.outgoing, recipe.incoming].into_iter().enumerate() {
            if owned.edge == end.edge {
                support[i] = self.before_exit(domain, recipe, i, work, cancel)?;
            }
        }
        let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
        stack[0] = (central, 0);
        let mut pending = 1;
        while pending != 0 {
            pending -= 1;
            let (range, depth) = stack[pending];
            work.charge(1, cancel)?;
            let beta = collar.bubble(range)?.0;
            let image = collar.normal_image(range, UNIT.mul(beta), work, cancel)?;
            // Compare whole images, not just their boundaries. The endpoint
            // support alternative below handles the exact shared cross-section.
            let whole_star =
                self.homotopy_star_image(domain, recipe, Piece::Middle, UNIT, UNIT, work, cancel)?;
            if disjoint(
                image,
                hull2(
                    whole_star,
                    hull2(recipe.outgoing.joint_uv, recipe.incoming.joint_uv),
                ),
            ) && self.image_misses_box(domain, recipe, image, work, cancel)?
            {
                continue;
            }
            let mut supported = false;
            for (i, owned) in [recipe.outgoing, recipe.incoming].into_iter().enumerate() {
                if owned.edge != end.edge || !support[i] {
                    continue;
                }
                let prefix = [range[0].min(owned.exit), range[1].max(owned.exit)];
                let jet = chart_jet(domain, owned.use_.coedge, prefix, work, cancel)?;
                let (beta, beta_t) = collar.bubble(prefix)?;
                let at = chart_jet(domain, owned.use_.coedge, [owned.exit; 2], work, cancel)?;
                let direction = if owned.absolute_back { -1.0 } else { 1.0 };
                let row = at.derivative.map(|v| v.mul(Interval::point(direction)));
                let derivative = match collar.transport {
                    CarrierTransport::Exact => jet.derivative,
                    CarrierTransport::Collar { width, inward } => {
                        let n = rotate(jet.derivative, inward);
                        let nt = rotate(jet.second_derivative, inward);
                        std::array::from_fn(|j| {
                            jet.derivative[j].add(
                                Interval { lo: 0.0, hi: width }
                                    .mul(beta_t.mul(n[j]).add(beta.mul(nt[j]))),
                            )
                        })
                    }
                };
                if dot2(row, derivative.map(|v| v.mul(Interval::point(direction)))).lo > 0.0 {
                    // Exact equality is confined to t=exit and its fixed
                    // cross-section; native t strictly beyond it lies outside.
                    supported = true;
                    break;
                }
            }
            if supported {
                continue;
            }
            let mid = range[0] * 0.5 + range[1] * 0.5;
            if depth >= MAX_DEPTH || mid <= range[0] || mid >= range[1] {
                return Err(ownership_failure(
                    recipe,
                    format_args!(
                        "ownedCentral parameter_window={range:?} complete_scope={central:?} endpoint_support={support:?}"
                    ),
                ));
            }
            stack[pending] = ([mid, range[1]], depth + 1);
            stack[pending + 1] = ([range[0], mid], depth + 1);
            pending += 2;
        }
        Ok(true)
    }

    /// Closed comparison disk ownership. Jordan boundary simplicity together
    /// with the common-sign interior Jacobian gives boundary degree exactly
    /// one (with the chosen orientation), rather than a sampled winding claim.
    /// Every nonowning trim/collar IMAGE is excluded before one Inside point
    /// calibrates the material component. The apex is the bound NativeV
    /// quotient, never a box asserted strictly Inside the nominal trim.
    pub(super) fn disk_owned(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        scratch: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        work.charge(1, cancel)?;
        self.validate(domain, recipe)?;
        if scratch < OWNERSHIP_STACK_BYTES {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "closed comparison-star ownership scratch exceeds remaining allowance",
            ));
        }
        if !self.simple_boundary(domain, recipe, work, cancel)? {
            return Err(ownership_failure(recipe, "simpleBoundary"));
        }
        if !self.regular_closed_star(domain, recipe, work, cancel)? {
            return Err(ownership_failure(recipe, "factoredJac"));
        }
        let Some(collar) =
            OriginalCollar::for_enclosure(domain, recipe.outgoing.use_, work, cancel)?
        else {
            return Err(ownership_failure(
                recipe,
                "constructorOwned nonowning_inventory",
            ));
        };
        for (index, _) in domain.native().coedges().iter().enumerate() {
            work.charge(1, cancel)?;
            if index == recipe.outgoing.use_.coedge || index == recipe.incoming.use_.coedge {
                continue;
            }
            for span in domain.boundary_parameter_intervals(index)? {
                let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                stack[0] = (span, 0);
                let mut pending = 1;
                while pending != 0 {
                    pending -= 1;
                    let (range, depth) = stack[pending];
                    work.charge(1, cancel)?;
                    let image = collar.other_collar_enclosure(index, range, work, cancel)?;
                    if self.image_misses_box(domain, recipe, image, work, cancel)? {
                        continue;
                    }
                    let mid = range[0] * 0.5 + range[1] * 0.5;
                    if depth >= MAX_DEPTH || mid <= range[0] || mid >= range[1] {
                        return Err(ownership_failure(
                            recipe,
                            format_args!(
                                "foreignTrim+collar coedge={index} parameter_window={range:?} image={image:?}"
                            ),
                        ));
                    }
                    stack[pending] = ([mid, range[1]], depth + 1);
                    stack[pending + 1] = ([range[0], mid], depth + 1);
                    pending += 2;
                }
            }
        }
        for joint in domain.native().joints() {
            work.charge(1, cancel)?;
            let owned = joint.vertex_id == self.vertex
                && joint.incoming.face_id == domain.native().face().id()
                && joint.outgoing.face_id == domain.native().face().id()
                && joint.incoming.wire_index == recipe.incoming.use_.wire
                && joint.outgoing.wire_index == recipe.outgoing.use_.wire
                && joint.incoming.coedge_index == recipe.incoming.use_.index
                && joint.outgoing.coedge_index == recipe.outgoing.use_.index
                && joint.incoming.edge_id == recipe.incoming.edge
                && joint.outgoing.edge_id == recipe.outgoing.edge
                && joint.incoming.orientation == recipe.incoming.use_.orientation
                && joint.outgoing.orientation == recipe.outgoing.use_.orientation;
            if owned {
                continue;
            }
            let image = joint.enclosure.map(|v| Interval {
                lo: v.lower,
                hi: v.upper,
            });
            if !self.image_misses_box(domain, recipe, image, work, cancel)? {
                return Err(ownership_failure(
                    recipe,
                    format_args!(
                        "foreignJoint incoming={} outgoing={} image={image:?}",
                        joint.incoming.coedge_index, joint.outgoing.coedge_index
                    ),
                ));
            }
        }
        if !self.owned_central_misses(domain, recipe, recipe.outgoing, work, cancel)?
            || recipe.incoming.edge != recipe.outgoing.edge
                && !self.owned_central_misses(domain, recipe, recipe.incoming, work, cancel)?
        {
            return Err(ownership_failure(recipe, "ownedCentral"));
        }
        let anchor = self
            .star_jet(
                domain,
                recipe,
                Piece::Middle,
                Interval::point(0.5),
                Interval::point(0.5),
                work,
                cancel,
            )?
            .uv;
        if domain.classify(uv_box(anchor), work, cancel)? != TrimClassification::Inside {
            return Err(ownership_failure(
                recipe,
                format_args!("calibration uv={anchor:?}"),
            ));
        }
        Ok(true)
    }
}

fn dot2(a: Box2, b: Box2) -> Interval {
    a[0].mul(b[0]).add(a[1].mul(b[1]))
}

/// Midpoints propose a row only. Strict whole-expression inequalities at the
/// caller certify separation; orthogonal rays need not support each other's
/// tangent, while a normalized difference supports both distinct directions.
fn divided_ray_separator(a: Box2, b: Box2) -> Option<Box2> {
    let a = a.map(|v| v.mid());
    let b = b.map(|v| v.mid());
    let lengths = [a[0].hypot(a[1]), b[0].hypot(b[1])];
    if lengths
        .into_iter()
        .any(|length| !length.is_finite() || length <= 0.0)
    {
        return None;
    }
    Some(std::array::from_fn(|i| {
        Interval::point(a[i] / lengths[0] - b[i] / lengths[1])
    }))
}

#[cfg(test)]
mod properties {
    use super::*;
    fn actual_corner_property(bytes: &[u8]) {
        let cancel = AtomicBool::new(false);
        let definition = crate::import_step(bytes, &cancel).unwrap();
        let original = serde_json::to_vec(&definition.solid.boundaries()[0].compress()).unwrap();
        let mut work = GeometryWorkBudget::new(20_000_000);
        let mut vertices = Vec::new();
        let mut expected_corners = Vec::new();
        // NativeUse occurrences, not approximate coordinates, supply every
        // actual endpoint identity. Closed edges keep both parameter germs in
        // the corner recipes even when their two endpoint IDs are identical.
        for (face_index, face) in definition.solid.face_iter().enumerate() {
            let mut flat = 0;
            for (wire_index, wire) in face.absolute_boundaries().iter().enumerate() {
                for (index, edge) in wire.iter().enumerate() {
                    expected_corners.push((face_index, wire_index, index, flat));
                    flat += 1;
                    for endpoint in [edge.front(), edge.back()] {
                        if !vertices.contains(&endpoint.id()) {
                            vertices.push(endpoint.id());
                        }
                    }
                }
            }
        }
        assert!(!vertices.is_empty() && !expected_corners.is_empty());
        let mut covered_corners = Vec::new();
        let mut failures = Vec::new();
        let mut incidence =
            SourceEdgeIncidence::new(&definition, MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
        incidence
            .prepare_vertices(MAX_SCRATCH_BYTES, &mut work, &cancel)
            .unwrap();
        let source = OriginalTrimSource::standalone(&definition);
        for &vertex in &vertices {
            let foreign = *vertices
                .iter()
                .find(|&&candidate| candidate != vertex)
                .unwrap();
            let plan = match SourceVertexTransportPlan::prepare(
                source,
                &incidence,
                vertex,
                MAX_SCRATCH_BYTES,
                &mut work,
                &cancel,
            ) {
                Ok(Some(plan)) => plan,
                Ok(None) => {
                    failures
                        .push("actual NativeV has no complete fixed source-only G plan".to_owned());
                    continue;
                }
                Err(error) => {
                    failures.push(format!("actual NativeV plan: {error:?}"));
                    continue;
                }
            };
            let point = plan.point;
            for &recipe in &plan.recipes {
                let outgoing = recipe.outgoing;
                let incoming = recipe.incoming;
                let face_index = outgoing.use_.face;
                covered_corners.push((
                    face_index,
                    outgoing.use_.wire,
                    outgoing.use_.index,
                    outgoing.use_.coedge,
                ));
                let domain = source
                    .face_domain(face_index, MAX_SCRATCH_BYTES, &mut work, &cancel)
                    .unwrap();
                let allowance = source_allowance(&definition).unwrap();
                let comparison_scratch = MAX_SCRATCH_BYTES
                    .checked_sub(plan.retained_bytes() + CONICAL_STACK_BYTES + domain.owned_bytes())
                    .unwrap();
                let original_margins = [outgoing, incoming].map(|end| {
                    domain
                        .original_coedge_correspondence(end.use_.coedge)
                        .unwrap()
                        .margin_uv()
                });
                let comparison = match OriginalCornerComparison::prepare(
                    &domain,
                    &recipe,
                    vertex,
                    allowance,
                    comparison_scratch,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                {
                    UvComparisonPreparation::Prepared(comparison) => *comparison
                        .native_uv()
                        .expect("these original carriers have native UV corners"),
                    UvComparisonPreparation::Refused(reason) => {
                        failures.push(reason.to_string());
                        continue;
                    }
                };
                for (i, end) in [outgoing, incoming].into_iter().enumerate() {
                    assert_eq!(
                        domain
                            .original_coedge_correspondence(end.use_.coedge)
                            .unwrap()
                            .margin_uv(),
                        original_margins[i],
                        "completion must not change OriginalMap margins",
                    );
                }
                if !comparison.is_identity() {
                    let mut missing_transport = comparison;
                    missing_transport.transport = None;
                    assert!(
                        missing_transport
                            .validates(&domain, &recipe, vertex)
                            .is_err()
                    );
                    let mut overbudget = comparison;
                    overbudget.transport.as_mut().unwrap()[0].position_error_mm =
                        up(allowance + 1.0);
                    assert!(overbudget.validates(&domain, &recipe, vertex).is_err());
                    let mut nonregular = comparison;
                    nonregular.transport.as_mut().unwrap()[0].relative_derivative_error = 0.25;
                    assert!(nonregular.validates(&domain, &recipe, vertex).is_err());
                }
                for end in [outgoing, incoming] {
                    let range = domain
                        .original_coedge_correspondence(end.use_.coedge)
                        .unwrap()
                        .native_range();
                    let expected_exit = end.endpoint
                        + if end.absolute_back {
                            -(range[1] - range[0]) * plan.fraction
                        } else {
                            (range[1] - range[0]) * plan.fraction
                        };
                    assert_eq!(
                        end.exit, expected_exit,
                        "comparison must retain the fixed source-only exit"
                    );
                }
                assert!(
                    recipe.comparison.is_some(),
                    "the actual fixed recipe consumes its original comparison"
                );
                match comparison.disk_owned(
                    &domain,
                    &recipe,
                    comparison_scratch,
                    &mut work,
                    &cancel,
                ) {
                    Ok(true) => {}
                    Ok(false) => {
                        failures.push(format!(
                            "complete disk ownership refused face={face_index} vertex_mm={point:?}"
                        ));
                        continue;
                    }
                    Err(error) => {
                        failures.push(format!("complete disk ownership face={face_index} vertex_mm={point:?}: {error:?}"));
                        continue;
                    }
                }
                assert!(matches!(
                    OriginalCornerComparison::prepare(
                        &domain,
                        &recipe,
                        foreign,
                        allowance,
                        comparison_scratch,
                        &mut work,
                        &cancel,
                    )
                    .unwrap(),
                    UvComparisonPreparation::Refused(UvPrepareRefusal {
                        stage: UvPrepareStage::NativeVertex,
                        ..
                    })
                ));
                let mut wrong_owner = recipe;
                wrong_owner.outgoing.use_.index += 1;
                assert!(matches!(
                    OriginalCornerComparison::prepare(
                        &domain,
                        &wrong_owner,
                        vertex,
                        allowance,
                        comparison_scratch,
                        &mut work,
                        &cancel,
                    )
                    .unwrap(),
                    UvComparisonPreparation::Refused(UvPrepareRefusal {
                        stage: UvPrepareStage::BindingOwner,
                        end: Some(0),
                        ..
                    })
                ));
                let mut wrong_endpoint = recipe;
                wrong_endpoint.outgoing.endpoint = outgoing.exit;
                assert!(matches!(
                    OriginalCornerComparison::prepare(
                        &domain,
                        &wrong_endpoint,
                        vertex,
                        allowance,
                        comparison_scratch,
                        &mut work,
                        &cancel
                    )
                    .unwrap(),
                    UvComparisonPreparation::Refused(UvPrepareRefusal {
                        stage: UvPrepareStage::NativeEndpoint,
                        end: Some(0),
                        ..
                    })
                ));
                comparison.validates(&domain, &recipe, vertex).unwrap();
                assert!(comparison.validates(&domain, &recipe, foreign).is_err());
                let anchor = comparison
                    .star_jet(
                        &domain,
                        &recipe,
                        Piece::Middle,
                        Interval::point(0.5),
                        Interval::point(0.5),
                        &mut work,
                        &cancel,
                    )
                    .unwrap()
                    .uv;
                assert!(
                    !comparison
                        .image_misses_box(&domain, &recipe, anchor, &mut work, &cancel)
                        .unwrap(),
                    "a foreign trim image hidden inside the star cannot pass image exclusion"
                );
                let mut crossing = recipe;
                let exit_tangent = chart_jet(
                    &domain,
                    outgoing.use_.coedge,
                    [outgoing.exit; 2],
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .derivative;
                let direction = if outgoing.absolute_back { -1.0 } else { 1.0 };
                crossing.bulge = exit_tangent.map(|v| v.mul(Interval::point(1024.0 * direction)));
                assert!(
                    !comparison
                        .before_exit(&domain, &crossing, 0, &mut work, &cancel)
                        .unwrap(),
                    "an original-coefficient bridge crossing the retained exit support must refuse cutover"
                );
                for piece in [Piece::Outgoing, Piece::Middle, Piece::Incoming] {
                    for theta in [0.0, 0.25, 0.5, 0.75, 1.0] {
                        let t = Interval::point(theta);
                        let apex = comparison
                            .star_jet(
                                &domain,
                                &recipe,
                                piece,
                                Interval::point(0.0),
                                t,
                                &mut work,
                                &cancel,
                            )
                            .unwrap();
                        assert!(equal_box(apex.uv, comparison.joint));
                        let exit = comparison
                            .star_jet(
                                &domain,
                                &recipe,
                                piece,
                                Interval::point(1.0),
                                t,
                                &mut work,
                                &cancel,
                            )
                            .unwrap();
                        let original_exit = super::super::star_jet(
                            &domain,
                            outgoing,
                            incoming,
                            piece,
                            Interval::point(1.0),
                            t,
                            recipe.bulge,
                            &mut work,
                            &cancel,
                        )
                        .unwrap();
                        assert!(
                            contains(exit.uv, original_exit.uv)
                                || contains(original_exit.uv, exit.uv)
                        );
                        let factored = comparison
                            .factored_jet(&domain, &recipe, piece, UNIT, t, &mut work, &cancel)
                            .unwrap();
                        assert!(factored.factored_jacobian.lo.is_finite());
                        assert!(factored.factored_jacobian.hi.is_finite());
                        let cost = comparison
                            .source_cost(&domain, &recipe, piece, t, point, &mut work, &cancel)
                            .unwrap();
                        assert!(cost.error_mm.is_finite() && cost.error_mm >= cost.homotopy_mm);
                    }
                }
                let mut altered = recipe;
                altered.outgoing.exit = (outgoing.endpoint + outgoing.exit) * 0.5;
                assert!(
                    comparison
                        .star_jet(
                            &domain,
                            &altered,
                            Piece::Middle,
                            UNIT,
                            UNIT,
                            &mut work,
                            &cancel
                        )
                        .is_err()
                );
            }
        }
        covered_corners.sort_unstable();
        expected_corners.sort_unstable();
        if covered_corners != expected_corners {
            failures.push(format!(
                "every original NativeUse face corner must be exercised once: covered={covered_corners:?} expected={expected_corners:?}",
            ));
        }
        assert_eq!(
            original,
            serde_json::to_vec(&definition.solid.boundaries()[0].compress()).unwrap()
        );
        assert!(
            failures.is_empty(),
            "actual all-NativeUse UV qualification:\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn actual_blindpocket_full_native_corner_comparison_preserves_fixed_exits() {
        let name = "native-parity-blind-pocket.step";
        let (_, solid, _) = crate::parity_fixtures::recipes()
            .into_iter()
            .find(|(candidate, _, _)| *candidate == name)
            .expect("actual native blind-pocket recipe");
        let step = crate::parity_fixtures::original(name, solid);
        actual_corner_property(step.as_bytes());
    }

    #[test]
    fn actual_narrow_pocket_all_native_corners_preserve_fixed_exits() {
        actual_corner_property(include_bytes!(
            "../../../../../../../fixtures/geometry/independent-exporters/occt-narrow-pocket-mm.step"
        ));
    }

    #[test]
    fn actual_fitting_seam_uv_fibres_bind_both_native_endpoint_germs() {
        actual_corner_property(include_bytes!(
            "../../../../../../../fixtures/geometry/independent-exporters/occt-tapered-fitting-mm.step"
        ));
    }

    #[test]
    fn admitted_convex_joint_contains_exact_average_and_endpoint_connectors() {
        let a = [0.0, 1.0].map(Interval::point);
        let b = [0.25, 1.5].map(Interval::point);
        let envelope = hull2(a, b);
        let joint = admitted_joint_midpoint(a, b, envelope).unwrap();
        for endpoint in [a, b] {
            let connector = hull2(endpoint, joint);
            assert!((0..2).all(|i| {
                connector[i].lo >= envelope[i].lo && connector[i].hi <= envelope[i].hi
            }));
        }
        let outside = [4.0, 5.0].map(Interval::point);
        assert!(admitted_joint_midpoint(a, b, outside).is_none());
    }

    #[test]
    fn homotopy_exclusion_must_include_foreign_crossings_between_end_images() {
        let actual = [-1.0, 0.0].map(Interval::point);
        let completed = [1.0, 0.0].map(Interval::point);
        let foreign = ZERO2;
        assert!(disjoint(actual, foreign) && disjoint(completed, foreign));
        assert!(!disjoint(hull2(actual, completed), foreign));
    }

    #[test]
    fn divided_ray_separator_certifies_orthogonal_and_oblique_whole_germs() {
        let a = [8.0, 0.0].map(Interval::point);
        for b in [[0.0, 0.125], [0.125, 0.125], [-0.125, 0.0]] {
            let b = b.map(Interval::point);
            let row = divided_ray_separator(a, b).unwrap();
            assert!(dot2(row, a).lo > 0.0 && dot2(row, b).hi < 0.0);
        }
        let same = [0.125, 0.0].map(Interval::point);
        let row = divided_ray_separator(a, same).unwrap();
        assert!(!(dot2(row, a).lo > 0.0 && dot2(row, same).hi < 0.0));
        let uncertain = [Interval { lo: -1.0, hi: 1.0 }, Interval::point(0.125)];
        let row = divided_ray_separator(a, uncertain).unwrap();
        assert!(!(dot2(row, a).lo > 0.0 && dot2(row, uncertain).hi < 0.0));
        assert!(divided_ray_separator(a, ZERO2).is_none());
    }

    #[test]
    fn endpoint_pair_injectivity_cannot_discard_foreign_or_tangent_roots() {
        let x = [1.0, 0.0].map(Interval::point);
        let y = [0.0, 1.0].map(Interval::point);
        assert!(pair_jacobian_injective(x, y));
        assert!(!pair_jacobian_injective(x, x));
        assert!(!pair_jacobian_injective(
            [Interval { lo: -1.0, hi: 1.0 }, Interval::point(0.0)],
            y,
        ));
    }
}
