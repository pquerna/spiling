// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod cylinder;
mod trim;
mod wall;
pub(crate) use trim::{
    PreparedFaceTrimDomain, TrimClassification, certified_curve2d_is_straight_regular,
    prepare_face_trim_domain_with_byte_limit,
};

use monstertruck_io::step::load::step_geometry::CurveParameterMap;
use monstertruck_modeling::Solid;
use serde::{Deserialize, Serialize};
use spiling_contracts::geometry::{AabbMm, DefinitionId, FaceInfo, SourceProvenance, StepEntityId};

pub type BBox3Mm = AabbMm;

/// Surface parameter mapping between STEP UV and solidcore analytic surface parameters.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SurfaceParameterMap {
    Identity,
    ScaleOffset {
        u_scale: f64,
        u_offset: f64,
        v_scale: f64,
        v_offset: f64,
    },
}

impl SurfaceParameterMap {
    pub fn step_to_core(&self, uv: [f64; 2]) -> [f64; 2] {
        match self {
            Self::Identity => uv,
            Self::ScaleOffset {
                u_scale,
                u_offset,
                v_scale,
                v_offset,
            } => [(uv[0] - u_offset) * u_scale, (uv[1] - v_offset) * v_scale],
        }
    }

    pub fn core_to_step(&self, uv: [f64; 2]) -> [f64; 2] {
        match self {
            Self::Identity => uv,
            Self::ScaleOffset {
                u_scale,
                u_offset,
                v_scale,
                v_offset,
            } => [uv[0] / u_scale + u_offset, uv[1] / v_scale + v_offset],
        }
    }
}

/// Supporting extrema for narrow finish certificates use the same native
/// all-family proof as other consumers, with the caller's cumulative work.
fn finish_support_extent(
    definition: &Definition,
    direction: [f64; 3],
    work: &mut crate::GeometryWorkBudget,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<[f64; 2], crate::GeometryError> {
    use crate::parametric::Interval;
    let magnitude = direction
        .into_iter()
        .fold(Interval::point(0.0), |sum, value| {
            sum.add(Interval::point(value).square())
        })
        .sqrt()?;
    let proof = crate::projected_bounds(
        definition,
        direction,
        crate::BoundsPolicy {
            maximum_endpoint_uncertainty_mm: 1.0e-8 / magnitude.hi,
        },
        work,
        cancel,
    )?;
    let minimum = Interval {
        lo: proof.minimum[0],
        hi: proof.minimum[1],
    }
    .mul(magnitude);
    let maximum = Interval {
        lo: proof.maximum[0],
        hi: proof.maximum[1],
    }
    .mul(magnitude);
    if minimum.width() > 2.5e-8 || maximum.width() > 2.5e-8 {
        return Err(crate::GeometryError::new(
            spiling_contracts::geometry::GeometryErrorCode::ConditioningFailure,
            "native finish support cannot meet its fixed residual under coordinate conditioning",
        ));
    }
    Ok([minimum.lo, maximum.hi])
}

/// Immutable native definition. Kernel representation never leaves this crate.
///
/// Thread safety note: `Definition` implements `Send` and can be moved across threads.
/// However, upstream `solidcore::Brep` uses interior mutability (`CrvOnSurf::break_cache: RefCell<BreakCache>`),
/// which prevents `solidcore::Brep` (and thus `Definition`) from implementing `Sync`.
/// Sharing an `Arc<Definition>` across threads requires external synchronization (`Arc<Mutex<Definition>>`)
/// or thread confinement.
pub struct Definition {
    pub(crate) id: DefinitionId,
    pub(crate) provenance: SourceProvenance,
    pub(crate) faces: Vec<FaceInfo>,
    pub(crate) bounds: AabbMm,
    pub(crate) source_bytes: u32,

    // solidcore native B-Rep substrate (M1)
    pub(crate) brep: solidcore::Brep,
    pub(crate) shell_id: solidcore::topology::ShellId,
    pub(crate) face_step_ids: Vec<StepEntityId>,
    pub(crate) face_surface_maps: Vec<SurfaceParameterMap>,
    pub(crate) source_units_scale_to_mm: f64,

    // Legacy fields retained for downstream modules until M2-M5
    pub(crate) solid: Solid,
    pub(crate) boundary_curves: Vec<BoundaryCurveMm>,
    pub(crate) face_boundaries: Vec<FaceBoundaryRefs>,
    /// Exact decoded-source/native maps in native edge order. Original STEP
    /// selectors and genealogy live in source_topology; provenance owns mm scale.
    pub(crate) boundary_parameters: Vec<Option<CurveParameterMap>>,
    /// Original implicit source branches, shared by all oriented uses and
    /// occurrences. These are not explicit Identity parameter maps.
    pub(crate) boundary_continuations: Vec<Option<OriginalBoundaryContinuation>>,
    pub(crate) original_continuations: Vec<monstertruck_modeling::PreparedValidatedCurve<'static>>,
    pub(crate) continuation_bytes: usize,
    pub(crate) boundary_curve_indices: std::collections::HashMap<
        monstertruck_topology::EdgeId<monstertruck_modeling::Curve>,
        usize,
    >,
    pub(crate) source_topology: monstertruck_io::step::load::SourceShellProvenance,
    pub(crate) trimmed_solid: monstertruck_topology::trimmed::TrimmedSolid<
        monstertruck_modeling::Point3,
        monstertruck_modeling::Curve,
        monstertruck_modeling::Surface,
        monstertruck_modeling::Curve2D,
    >,
}

pub(crate) struct OriginalBoundaryContinuation {
    pub(crate) prepared: usize,
    pub(crate) source_range: (f64, f64),
    pub(crate) original_range: (f64, f64),
    pub(crate) source_edge: Option<u64>,
    pub(crate) raw_step_gauge: monstertruck_io::step::load::step_geometry::SourceParameterGauge,
    pub(crate) source_domain: monstertruck_io::step::load::step_geometry::CurveParameterDomain,
    pub(crate) scale_to_mm: f64,
    pub(crate) edge: Option<monstertruck_topology::EdgeId<monstertruck_modeling::Curve>>,
}

/// Positional trim uses borrow native geometry. Their witness indices also
/// select the shared source/native curve maps; no second coedge index is needed.
pub(crate) struct FaceBoundaryRefs {
    pub(crate) face: monstertruck_topology::FaceId<monstertruck_modeling::Surface>,
    pub(crate) wires: Vec<Vec<BoundaryCoedgeRef>>,
    pub(crate) parameter: FaceParameterWitness,
}

/// Whole-source covering-chart authority. A pole graph does not prove a
/// regular longitude inverse; its physical and UV bounds remain on the coedge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SourceChartCertificate {
    Unproved,
    InverseOrExact,
    PoleMeridianGraph {
        latitude_axis: usize,
        pole_endpoints: [bool; 2],
    },
}

impl SourceChartCertificate {
    pub(crate) fn is_certified(self) -> bool {
        !matches!(self, Self::Unproved)
    }

    pub(crate) fn has_regular_inverse(self) -> bool {
        matches!(self, Self::InverseOrExact)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OriginalCorrespondenceProvenance {
    ExactCoefficients,
    Forward,
    CompletedPoleGraph,
}

#[derive(Clone, Debug)]
pub(crate) struct OriginalCoedgeCorrespondence {
    pub(crate) provenance: OriginalCorrespondenceProvenance,
    pub(crate) native_range: [f64; 2],
    pub(crate) uv_range: [f64; 2],
}

impl OriginalCoedgeCorrespondence {
    pub(crate) fn new(
        provenance: OriginalCorrespondenceProvenance,
        native_range: [f64; 2],
        uv_range: [f64; 2],
    ) -> Self {
        Self {
            provenance,
            native_range,
            uv_range,
        }
    }

    pub(crate) fn exact_coefficients(native_range: [f64; 2], uv_range: [f64; 2]) -> Self {
        Self {
            provenance: OriginalCorrespondenceProvenance::ExactCoefficients,
            native_range,
            uv_range,
        }
    }

    pub(crate) fn provenance(&self) -> OriginalCorrespondenceProvenance {
        self.provenance
    }

    pub(crate) fn native_range(&self) -> [f64; 2] {
        self.native_range
    }

    pub(crate) fn uv_range(&self) -> [f64; 2] {
        self.uv_range
    }

    pub(crate) fn position_error_mm(&self) -> f64 {
        0.0
    }

    pub(crate) fn relative_derivative_error_bound(&self) -> f64 {
        0.0
    }

    pub(crate) fn margin_uv(&self) -> [f64; 2] {
        [0.0, 0.0]
    }

    pub(crate) fn binding(&self) -> Option<&OriginalCoedgeBinding> {
        None
    }

    pub(crate) fn same_affine_native_uv_map(&self, other: &Self) -> bool {
        self.provenance != OriginalCorrespondenceProvenance::CompletedPoleGraph
            && other.provenance != OriginalCorrespondenceProvenance::CompletedPoleGraph
            && self.native_range == other.native_range
            && self.uv_range == other.uv_range
    }

    pub(crate) fn uv_parameter_image(
        &self,
        range: [f64; 2],
    ) -> Result<crate::parametric::Interval, crate::GeometryError> {
        let t0 = self.native_range[0];
        let t1 = self.native_range[1];
        let u0 = self.uv_range[0];
        let u1 = self.uv_range[1];
        let len = t1 - t0;
        let frac0 = if len.abs() < 1e-15 { 0.0 } else { (range[0] - t0) / len };
        let frac1 = if len.abs() < 1e-15 { 1.0 } else { (range[1] - t0) / len };
        let img0 = u0 + frac0 * (u1 - u0);
        let img1 = u0 + frac1 * (u1 - u0);
        crate::parametric::Interval::new(img0.min(img1), img0.max(img1))
    }

    pub(crate) fn affine_native_parameter_preimage(
        &self,
        range: [f64; 2],
    ) -> Result<Option<crate::parametric::Interval>, crate::GeometryError> {
        if self.provenance == OriginalCorrespondenceProvenance::CompletedPoleGraph {
            return Ok(None);
        }
        let t0 = self.native_range[0];
        let t1 = self.native_range[1];
        let u0 = self.uv_range[0];
        let u1 = self.uv_range[1];
        let len = u1 - u0;
        let frac0 = if len.abs() < 1e-15 { 0.0 } else { (range[0] - u0) / len };
        let frac1 = if len.abs() < 1e-15 { 1.0 } else { (range[1] - u0) / len };
        let pre0 = t0 + frac0 * (t1 - t0);
        let pre1 = t0 + frac1 * (t1 - t0);
        let iv = crate::parametric::Interval::new(pre0.min(pre1), pre0.max(pre1))?;
        Ok(Some(iv))
    }

    pub(crate) fn span_inventory(&self) -> OriginalSpanInventory {
        OriginalSpanInventory::default()
    }

    pub(crate) fn uv_parameter_derivative(&self) -> crate::parametric::Interval {
        let t0 = self.native_range[0];
        let t1 = self.native_range[1];
        let u0 = self.uv_range[0];
        let u1 = self.uv_range[1];
        let dt = t1 - t0;
        let du = u1 - u0;
        let slope = if dt.abs() < 1e-15 { 1.0 } else { du / dt };
        crate::parametric::Interval::point(slope)
    }

    pub(crate) fn visit_span_boundaries(
        &self,
        _curve: &monstertruck_modeling::Curve,
        _chart: &monstertruck_modeling::Curve2D,
        _continuation: Option<&monstertruck_modeling::PreparedValidatedCurve<'static>>,
        _work: &mut crate::GeometryWorkBudget,
        _cancel: &std::sync::atomic::AtomicBool,
        _visit: &mut impl FnMut(crate::parametric::Interval) -> Result<(), crate::GeometryError>,
    ) -> Result<(), crate::GeometryError> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OriginalSpanInventory {
    pub(crate) source_knot_boundaries: usize,
    pub(crate) uv_knot_boundaries: usize,
    pub(crate) continuation_spans: usize,
}

/// Original/native ownership after normalized topology identity checks.
#[derive(Clone, Copy)]
pub(crate) struct OriginalCoedgeBinding {
    pub(crate) face: monstertruck_topology::FaceId<monstertruck_modeling::Surface>,
    pub(crate) edge: monstertruck_topology::EdgeId<monstertruck_modeling::Curve>,
    pub(crate) endpoints: [monstertruck_topology::VertexId<monstertruck_modeling::Point3>; 2],
    pub(crate) owner: [usize; 3],
    pub(crate) source_face: Option<u64>,
    pub(crate) source_face_use: Option<u64>,
    pub(crate) source: monstertruck_io::step::load::SourceCoedgeProvenance,
}

/// Source-relative realization recipe identity, independent of slabs, poses,
/// adaptive proof cells and available work. This descriptor grants no collar
/// or endpoint completion authority.
///
/// Version 1 uses native-span/1024 collar width and first/last 1/1024 endpoint
/// supports, with the normalized bubble (t-a)(b-t)/(b-a)^2 on the complete
/// retained native range [a,b]. Every consumer must prove this same recipe.
#[derive(Clone, Copy)]
pub(crate) struct OriginalMaterialRealization<'a> {
    definition: &'a Definition,
}

/// The actual implemented realization variant. A future carrier-law recipe
/// requires its complete source-only selector and shared vertex plan before it
/// can acquire a distinct identity; an unimplemented plan is not a variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaterialRealizationVariant {
    GenericGTransport,
}

/// Owned recipe descriptor carried alongside a capture's existing immutable
/// definition ID. Exact bit equality preserves absent allowance and every
/// source-relative recipe parameter without floating-point tolerance matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MaterialRealizationIdentity {
    version: u32,
    variant: MaterialRealizationVariant,
    source_allowance_bits: Option<u64>,
    collar_parameter_fraction_bits: u64,
    endpoint_support_fraction_bits: u64,
}
impl MaterialRealizationIdentity {
    pub(crate) fn new(definition: &Definition) -> Self {
        Self::with_allowance(definition.provenance.uncertainty_mm)
    }
    pub(crate) fn with_allowance(source_allowance_mm: Option<f64>) -> Self {
        Self {
            version: 1,
            variant: MaterialRealizationVariant::GenericGTransport,
            source_allowance_bits: source_allowance_mm.map(f64::to_bits),
            collar_parameter_fraction_bits: (1.0_f64 / 1024.0).to_bits(),
            endpoint_support_fraction_bits: (1.0_f64 / 1024.0).to_bits(),
        }
    }
    /// Fixed canonical hash input, not a persisted wire representation.
    pub(crate) fn identity_bytes(self) -> [u8; 30] {
        let mut bytes = [0; 30];
        bytes[..4].copy_from_slice(&self.version.to_le_bytes());
        bytes[4] = match self.variant {
            MaterialRealizationVariant::GenericGTransport => 0,
        };
        bytes[5] = u8::from(self.source_allowance_bits.is_some());
        bytes[6..14].copy_from_slice(&self.source_allowance_bits.unwrap_or(0).to_le_bytes());
        bytes[14..22].copy_from_slice(&self.collar_parameter_fraction_bits.to_le_bytes());
        bytes[22..30].copy_from_slice(&self.endpoint_support_fraction_bits.to_le_bytes());
        bytes
    }
}

impl<'a> OriginalMaterialRealization<'a> {
    pub(crate) fn new(definition: &'a Definition) -> Self {
        Self { definition }
    }
    pub(crate) fn identity(&self) -> MaterialRealizationIdentity {
        MaterialRealizationIdentity::new(self.definition)
    }
    pub(crate) fn collar_parameter_fraction(&self) -> f64 {
        f64::from_bits(self.identity().collar_parameter_fraction_bits)
    }
    pub(crate) fn endpoint_support_fraction(&self) -> f64 {
        f64::from_bits(self.identity().endpoint_support_fraction_bits)
    }
}

pub(crate) struct BoundaryCoedgeRef {
    pub(crate) witness: usize,
    pub(crate) orientation: bool,
    pub(crate) edge: monstertruck_topology::EdgeId<monstertruck_modeling::Curve>,
    /// Inverse-only/unproved paths cannot manufacture material-link authority.
    pub(crate) correspondence: Option<OriginalCoedgeCorrespondence>,
    /// Whole-coedge source-chart uncertainty, not a guessed glue tolerance.
    pub(crate) source_chart_margin_uv: [f64; 2],
    pub(crate) source_chart_error_mm: f64,
    /// UV trim parameters are independent of rational native edge parameters.
    pub(crate) source_chart_parameter_range: Option<(f64, f64)>,
    /// An unproved original declared trim remains retained without authorizing
    /// operations that need a proved source/native covering chart.
    pub(crate) source_chart_certificate: SourceChartCertificate,
    /// Retained original source PCURVE/literal parameter expression. A derived
    /// inverse or small physical residual alone never supplies this authority.
    pub(crate) source_chart_declared: bool,
}

pub(crate) struct FaceParameterWitness {
    /// Source UV and native UV remain distinct even after physical mm scaling.
    pub(crate) conversion: monstertruck_io::step::load::step_geometry::SurfaceParameterMap,
    /// Original bounded spline carrier domain, transported by exact conversion.
    /// A lowered sweep's incidental zero-to-one net is not this witness.
    pub(crate) native_uv_domain: Option<[[f64; 2]; 2]>,
}
/// Exact analytic fast paths plus native endpoint witnesses for general curves.
/// `Native` is not a line chord and never authorizes analytic extrema/finishes.
#[derive(Debug, Clone)]
pub(crate) enum BoundaryCurveMm {
    Line {
        start_mm: [f64; 3],
        end_mm: [f64; 3],
    },
    CircleArc {
        center_mm: [f64; 3],
        u_mm: [f64; 3],
        v_mm: [f64; 3],
        start_angle: f64,
        end_angle: f64,
    },
    Native {
        start_mm: [f64; 3],
        end_mm: [f64; 3],
    },
}

impl BoundaryCurveMm {
    pub(crate) fn endpoints(&self) -> ([f64; 3], [f64; 3]) {
        match self {
            Self::Line { start_mm, end_mm } | Self::Native { start_mm, end_mm } => {
                (*start_mm, *end_mm)
            }
            Self::CircleArc {
                center_mm,
                u_mm,
                v_mm,
                start_angle,
                end_angle,
            } => {
                let point = |angle: f64| {
                    std::array::from_fn(|i| {
                        center_mm[i] + angle.cos() * u_mm[i] + angle.sin() * v_mm[i]
                    })
                };
                (point(*start_angle), point(*end_angle))
            }
        }
    }
}

impl Definition {
    /// Zero transport is a coefficient theorem, not a small residual or a
    /// source-family assertion. This narrow lane also proves every original
    /// curve endpoint equals its actual normalized topology vertex.
    pub(crate) fn material_transport_is_proved_zero(
        &self,
        work: &mut crate::GeometryWorkBudget,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<bool, crate::GeometryError> {
        crate::check_cancel(cancel)?;
        if self.solid.boundaries().len() != 1
            || self.solid.boundaries()[0].len() != self.face_boundaries.len()
        {
            return Ok(false);
        }
        for (face, retained) in self.solid.boundaries()[0].iter().zip(&self.face_boundaries) {
            work.charge(1, cancel)?;
            let wires = face.absolute_boundaries();
            if face.id() != retained.face || wires.len() != retained.wires.len() {
                return Ok(false);
            }
            for (wire, references) in wires.iter().zip(&retained.wires) {
                if wire.len() != references.len() {
                    return Ok(false);
                }
                for (edge, reference) in wire.iter().zip(references) {
                    work.charge(1, cancel)?;
                    let Some(proof) = reference.correspondence.as_ref() else {
                        return Ok(false);
                    };
                    if proof.provenance() != OriginalCorrespondenceProvenance::ExactCoefficients
                        || proof.position_error_mm() != 0.0
                        || proof.relative_derivative_error_bound() != 0.0
                        || proof.margin_uv() != [0.0; 2]
                        || reference.source_chart_margin_uv != [0.0; 2]
                        || reference.source_chart_error_mm != 0.0
                        || !reference.source_chart_certificate.has_regular_inverse()
                        || proof.binding().is_none_or(|binding| {
                            binding.face != face.id()
                                || binding.edge != edge.id()
                                || binding.endpoints != [edge.front().id(), edge.back().id()]
                        })
                        || edge.id() != reference.edge
                        || edge.orientation() != reference.orientation
                    {
                        return Ok(false);
                    }
                    let front = edge.front().point();
                    let back = edge.back().point();
                    let exact_endpoints =
                        edge.with_curve(|curve| -> Result<bool, crate::GeometryError> {
                            if let monstertruck_modeling::Curve::Line(line) = curve {
                                let (a, b) = if reference.orientation {
                                    (line.0, line.1)
                                } else {
                                    (line.1, line.0)
                                };
                                return Ok(a == front
                                    && b == back
                                    && proof.native_range() == [0.0, 1.0]);
                            }
                            let [a, b] = proof.native_range();
                            let parameters = if reference.orientation {
                                [a, b]
                            } else {
                                [b, a]
                            };
                            for (parameter, vertex) in parameters.into_iter().zip([front, back]) {
                                // A singleton outward enclosure is an exact
                                // coefficient/evaluation identity, not equality of
                                // two sampled f64 points or periodic nearness.
                                let jet = match crate::parametric::curve_enclosure(
                                    curve,
                                    (parameter, parameter),
                                    work,
                                    cancel,
                                ) {
                                    Ok(jet) => jet,
                                    Err(error)
                                        if matches!(error.code,
                                    spiling_contracts::geometry::GeometryErrorCode::Cancelled
                                    | spiling_contracts::geometry::GeometryErrorCode::ResourceLimit
                                ) =>
                                    {
                                        return Err(error);
                                    }
                                    Err(_) => return Ok(false),
                                };
                                if (0..3).any(|axis| {
                                    jet.position[axis].lo != vertex[axis]
                                        || jet.position[axis].hi != vertex[axis]
                                }) {
                                    return Ok(false);
                                }
                            }
                            Ok(true)
                        })?;
                    if !exact_endpoints {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }
    pub(crate) fn prepare_boundary_curve<'a>(
        &'a self,
        edge: monstertruck_topology::EdgeId<monstertruck_modeling::Curve>,
        curve: &'a monstertruck_modeling::Curve,
        range: (f64, f64),
        remaining_bytes: usize,
        work: &mut crate::GeometryWorkBudget,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<crate::parametric::PreparedCurveEnclosure<'a>, crate::GeometryError> {
        if let Some(prepared) = self.original_boundary_curve(edge, range, work, cancel)? {
            return Ok(prepared);
        }
        crate::parametric::prepare_curve_enclosure(curve, range, remaining_bytes, work, cancel)
    }
    pub fn id(&self) -> &DefinitionId {
        &self.id
    }
    pub fn provenance(&self) -> &SourceProvenance {
        &self.provenance
    }
    pub fn faces(&self) -> &[FaceInfo] {
        &self.faces
    }
    pub fn bounds_mm(&self) -> AabbMm {
        self.bounds
    }
    pub fn source_bytes(&self) -> u32 {
        self.source_bytes
    }
    /// Zero-mutex borrow of a face's underlying surface.
    #[inline]
    pub fn face_surface(&self, id: solidcore::FaceId) -> Option<&solidcore::surface::surface::Surface> {
        self.brep.face(id).ok().and_then(|f| f.surface.as_ref())
    }

    /// Zero-mutex borrow of an edge's 3D curve.
    #[inline]
    pub fn edge_curve(&self, id: solidcore::EdgeId) -> Option<&solidcore::curve::curve::Curve> {
        self.brep.edge(id).ok().and_then(|e| e.curve.as_ref())
    }

    /// Zero-mutex borrow of an edgeuse's 2D UV trim curve.
    #[inline]
    pub fn edgeuse_uv_trim_curve(
        &self,
        id: solidcore::topology::EdgeuseId,
    ) -> Option<&solidcore::curve::curve::Curve> {
        self.brep.edgeuse(id).ok().and_then(|eu| eu.uv_trim_curve.as_ref())
    }

    /// Direct borrow of the underlying solidcore B-Rep.
    #[inline]
    pub fn brep(&self) -> &solidcore::Brep {
        &self.brep
    }

    /// Mutable borrow of the underlying solidcore B-Rep.
    #[inline]
    pub(crate) fn brep_mut(&mut self) -> &mut solidcore::Brep {
        &mut self.brep
    }

    /// Primary shell ID of this solid.
    #[inline]
    pub fn shell_id(&self) -> solidcore::topology::ShellId {
        self.shell_id
    }

    /// Canonical STEP entity IDs corresponding to each face ordinal.
    #[inline]
    pub fn face_step_ids(&self) -> &[StepEntityId] {
        &self.face_step_ids
    }

    /// Surface parameter maps corresponding to each face ordinal.
    #[inline]
    pub fn face_surface_maps(&self) -> &[SurfaceParameterMap] {
        &self.face_surface_maps
    }

    /// Source representation scale to mm.
    #[inline]
    pub fn source_units_scale_to_mm(&self) -> f64 {
        self.source_units_scale_to_mm
    }

    /// Pinned native query-proof capacities, counted once per definition.
    /// Sums arena vector sizes, knot/control point vectors of all surfaces and curves,
    /// and topological provenance tables.
    pub fn retained_query_proof_bytes(&self) -> usize {
        let mut total = 0usize;

        // 1. Brep Arenas
        total += self.brep.regions().len() * std::mem::size_of::<solidcore::topology::Region>();
        total += self.brep.shells().len() * std::mem::size_of::<solidcore::topology::Shell>();
        total += self.brep.faces().len() * std::mem::size_of::<solidcore::topology::Face>();
        total += self.brep.faceuses().len() * std::mem::size_of::<solidcore::topology::Faceuse>();
        total += self.brep.loops().len() * std::mem::size_of::<solidcore::topology::Loop>();
        total += self.brep.loopuses().len() * std::mem::size_of::<solidcore::topology::Loopuse>();
        total += self.brep.edges().len() * std::mem::size_of::<solidcore::topology::Edge>();
        total += self.brep.edgeuses().len() * std::mem::size_of::<solidcore::topology::Edgeuse>();
        total += self.brep.vertices().len() * std::mem::size_of::<solidcore::topology::Vertex>();
        total += self.brep.vertexuses().len() * std::mem::size_of::<solidcore::topology::Vertexuse>();

        // 2. Face Surfaces
        for (_, face) in self.brep.faces().iter() {
            if let Some(surface) = &face.surface {
                total += surface_owned_bytes(surface);
            }
        }

        // 3. Edge 3D Curves
        for (_, edge) in self.brep.edges().iter() {
            if let Some(curve) = &edge.curve {
                total += curve_owned_bytes(curve);
            }
        }

        // 4. Edgeuse 2D UV Trim Curves
        for (_, edgeuse) in self.brep.edgeuses().iter() {
            if let Some(uv_curve) = &edgeuse.uv_trim_curve {
                total += curve_owned_bytes(uv_curve);
            }
        }

        // 5. Metadata and Provenance
        total += self.faces.len() * std::mem::size_of::<FaceInfo>();
        total += self.face_step_ids.len() * std::mem::size_of::<StepEntityId>();
        total += self.face_surface_maps.len() * std::mem::size_of::<SurfaceParameterMap>();

        // Legacy continuation bytes
        total += self.continuation_bytes;

        total
    }
}

/// Constructs a deterministic thread-isolated `solidcore::Kernel` context with Spiling mm tolerances.
pub fn make_kernel_context() -> solidcore::Kernel {
    let mut tolerances = solidcore::Tolerances::default();
    tolerances.zone_tol_3d = 1.0e-6; // 1 um coincidence threshold
    tolerances.xsect_tol_3d = 2.0e-6; // 2 * zone_tol
    tolerances.approx_tol_3d = 5.0e-7; // zone_tol / 2
    tolerances.ang_tol_deg = 1.0e-6; // tight angular tolerance in degrees
    tolerances.model_size_estimate = 100.0; // 100 mm CAM part scale

    let options = solidcore::KernelOptions {
        tolerances,
        seed: solidcore::Rng::DEFAULT_SEED,
    };
    solidcore::Kernel::new(&options)
}

pub(crate) fn surface_owned_bytes(surface: &solidcore::surface::surface::Surface) -> usize {
    use solidcore::surface::surface::p2::SurfaceKind;
    let base = std::mem::size_of::<solidcore::surface::surface::Surface>();
    let kind_bytes = match surface.kind() {
        SurfaceKind::BSpline(bspline) => bspline_surface_bytes(bspline),
        SurfaceKind::Cone(_) => std::mem::size_of::<solidcore::surface::cone::Cone>(),
        SurfaceKind::Plane(_) => std::mem::size_of::<solidcore::surface::plane::Plane>(),
        SurfaceKind::Sphere(_) => std::mem::size_of::<solidcore::surface::sphere::Sphere>(),
        SurfaceKind::Torus(_) => std::mem::size_of::<solidcore::surface::torus::Torus>(),
        SurfaceKind::SurfOfRevolution(_) => {
            std::mem::size_of::<solidcore::surface::surf_of_revolution::SurfOfRevolution>()
        }
        SurfaceKind::SurfOfExtrusion(_) => {
            std::mem::size_of::<solidcore::surface::surf_of_extrusion::SurfOfExtrusion>()
        }
        SurfaceKind::CurveBounded(_) => {
            std::mem::size_of::<solidcore::surface::curve_bounded_surface::CurveBoundedSurface>()
        }
        SurfaceKind::OffsetSurface(_) => {
            std::mem::size_of::<solidcore::surface::offset_surface::OffsetSurface>()
        }
        SurfaceKind::Step(_) => {
            std::mem::size_of::<solidcore::surface::surface::p1::StepSurface>()
        }
        SurfaceKind::SrfInVolume(_) => {
            std::mem::size_of::<solidcore::surface::srf_in_volume::SrfInVolume>()
        }
    };
    base + kind_bytes
}

fn bspline_surface_bytes(bspline: &solidcore::nurbs::b_spline_surface::BSplineSurface) -> usize {
    use solidcore::nurbs::b_spline_surface::SurfParamType;
    let mut bytes = std::mem::size_of::<solidcore::nurbs::b_spline_surface::BSplineSurface>();
    let nu = bspline.number_control_points(SurfParamType::U);
    let nv = bspline.number_control_points(SurfParamType::V);
    bytes += nu * nv * std::mem::size_of::<solidcore::math::Point3>();
    bytes
}

pub(crate) fn curve_owned_bytes(curve: &solidcore::curve::curve::Curve) -> usize {
    use solidcore::curve::curve::p2::CurveKind;
    let base = std::mem::size_of::<solidcore::curve::curve::Curve>();
    let kind_bytes = match curve.kind() {
        CurveKind::BSpline(bspline) => bspline_curve_bytes(bspline),
        CurveKind::Line(_) => std::mem::size_of::<solidcore::curve::line::Line>(),
        CurveKind::Ellipse(_) => std::mem::size_of::<solidcore::curve::ellipse::Ellipse>(),
        CurveKind::Conic(_) => std::mem::size_of::<solidcore::curve::conic::Conic>(),
        CurveKind::Hermite(_) => {
            std::mem::size_of::<solidcore::curve::hermite_curve::HermiteCurve>()
        }
        CurveKind::Composite(_) => {
            std::mem::size_of::<solidcore::curve::composite_curve::CompositeCurve>()
        }
        CurveKind::CrvOnSurf(_) => {
            std::mem::size_of::<solidcore::curve::crv_on_surf::CrvOnSurf>()
        }
        CurveKind::Hyperbola(_) => {
            std::mem::size_of::<solidcore::curve::hyperbola::Hyperbola>()
        }
        CurveKind::Parabola(_) => {
            std::mem::size_of::<solidcore::curve::parabola::Parabola>()
        }
        CurveKind::Projected(_) => {
            std::mem::size_of::<solidcore::curve::projected_curve::ProjectedCurve>()
        }
        CurveKind::Iso(_) => std::mem::size_of::<solidcore::curve::iso_curve::IsoCurve>(),
        CurveKind::Offset(_) => {
            std::mem::size_of::<solidcore::curve::offset_curve::OffsetCurve>()
        }
        CurveKind::CrvInVolume(_) => {
            std::mem::size_of::<solidcore::curve::crv_in_volume::CrvInVolume>()
        }
    };
    base + kind_bytes
}

fn bspline_curve_bytes(bspline: &solidcore::nurbs::b_spline_curve::BSplineCurve) -> usize {
    let mut bytes = std::mem::size_of::<solidcore::nurbs::b_spline_curve::BSplineCurve>();
    let n_cpts = bspline.number_control_points().unwrap_or(0);
    bytes += n_cpts * std::mem::size_of::<solidcore::math::Point3>();
    bytes
}

impl std::fmt::Debug for Definition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Definition")
            .field("id", &self.id)
            .field("face_count", &self.faces.len())
            .field("bounds_mm", &self.bounds)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod material_realization_identity_tests {
    use super::MaterialRealizationIdentity;

    #[test]
    fn recipe_identity_preserves_absence_and_exact_allowance_bits() {
        let absent = MaterialRealizationIdentity::with_allowance(None);
        let zero = MaterialRealizationIdentity::with_allowance(Some(0.0));
        let negative_zero = MaterialRealizationIdentity::with_allowance(Some(-0.0));
        assert_ne!(absent, zero);
        assert_ne!(zero, negative_zero);
        assert_ne!(absent.identity_bytes(), zero.identity_bytes());
        assert_ne!(zero.identity_bytes(), negative_zero.identity_bytes());
        let a = MaterialRealizationIdentity::with_allowance(Some(1.0e-7));
        let b = MaterialRealizationIdentity::with_allowance(Some(f64::from_bits(
            1.0e-7_f64.to_bits() + 1,
        )));
        assert_ne!(a, b);
        assert_ne!(a.identity_bytes(), b.identity_bytes());
        let mut changed = a;
        changed.version += 1;
        assert_ne!(a.identity_bytes(), changed.identity_bytes());
        changed = a;
        changed.collar_parameter_fraction_bits += 1;
        assert_ne!(a.identity_bytes(), changed.identity_bytes());
        changed = a;
        changed.endpoint_support_fraction_bits += 1;
        assert_ne!(a.identity_bytes(), changed.identity_bytes());
        assert_eq!(
            a.identity_bytes()[4],
            0,
            "only the actual generic G recipe is implemented"
        );
    }
}

#[cfg(test)]
mod zero_material_transport_tests {
    use std::sync::atomic::AtomicBool;

    #[test]
    fn exact_normalized_box_endpoints_prove_zero_but_missing_receipt_cannot() {
        let cancel = AtomicBool::new(false);
        let mut definition = crate::import_step(
            include_bytes!("../../../../fixtures/geometry/box-mm.step"),
            &cancel,
        )
        .unwrap();
        let mut work = crate::GeometryWorkBudget::new(1_000_000);
        assert!(
            definition
                .material_transport_is_proved_zero(&mut work, &cancel)
                .unwrap()
        );
        assert_eq!(
            definition
                .material_transport_is_proved_zero(&mut work, &AtomicBool::new(true),)
                .unwrap_err()
                .code,
            spiling_contracts::geometry::GeometryErrorCode::Cancelled,
        );
        assert_eq!(
            definition
                .material_transport_is_proved_zero(&mut crate::GeometryWorkBudget::new(0), &cancel,)
                .unwrap_err()
                .code,
            spiling_contracts::geometry::GeometryErrorCode::ResourceLimit,
        );
        definition.face_boundaries[0].wires[0][0].correspondence = None;
        assert!(
            !definition
                .material_transport_is_proved_zero(&mut work, &cancel)
                .unwrap()
        );
    }
}
