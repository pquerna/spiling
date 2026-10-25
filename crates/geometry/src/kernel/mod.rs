// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

pub(crate) mod source;
pub(crate) mod source_operand;

mod cylinder;
pub(crate) mod trim;
mod wall;
pub(crate) use trim::{
    OriginalCoedgeBinding, OriginalCoedgeCorrespondence, OriginalCorrespondenceProvenance,
    OriginalSpanInventory, PreparedFaceTrimDomain, PreparedNativeTrimSource, PreparedTrimCurve,
    PreparedTrimJoint, PreparedTrimSurface, SourceEndpointConnector, TrimClassification,
    TrimJointKind, certified_curve2d_is_straight_regular,
};

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
    pub(crate) face_ids: Vec<solidcore::FaceId>,
    pub(crate) face_step_ids: Vec<StepEntityId>,
    pub(crate) face_surface_maps: Vec<SurfaceParameterMap>,
    pub(crate) source_units_scale_to_mm: f64,
    pub(crate) original_source: source::OriginalSourceTopology,
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

/// Source-relative realization recipe identity, independent of slabs, poses,
/// adaptive proof cells and available work. This descriptor grants no collar
/// or endpoint completion authority.
///
/// Version 3 uses the selected ORIGINAL spatial-provider range for 1/1024
/// collar and initial endpoint supports, with the normalized bubble on that
/// range. Complete original-source comparison costs participate in immutable
/// endpoint fraction admission before any posed query. Endpoint completion
/// remains anchored to authored original STEP vertex points.
/// Native incidence/ranges remain separate; every consumer proves this recipe.
#[derive(Clone, Copy)]
pub(crate) struct OriginalMaterialRealization<'a> {
    definition: &'a Definition,
}

/// The actual implemented realization variant. A future carrier-law recipe
/// requires its complete source-only selector and shared vertex plan before it
/// can acquire a distinct identity; an unimplemented plan is not a variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaterialRealizationVariant {
    OriginalSpatialGTransport,
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
            version: 4,
            variant: MaterialRealizationVariant::OriginalSpatialGTransport,
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
            MaterialRealizationVariant::OriginalSpatialGTransport => 0,
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

impl Definition {
    /// Zero transport is a coefficient theorem, not a small residual or a
    /// source-family assertion. This narrow lane also proves every original
    /// curve endpoint equals its actual normalized topology vertex.
    pub(crate) fn material_transport_is_proved_zero(
        &self,
        work: &mut crate::GeometryWorkBudget,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<bool, crate::GeometryError> {
        crate::analysis::certify_zero_material_transport(self, work, cancel)
    }
    /// Prepares the actual retained native edge law. Original STEP operands have
    /// their own provider and must not be silently substituted for this binding.
    pub(crate) fn prepare_boundary_curve<'a>(
        &'a self,
        edge: solidcore::EdgeId,
        curve: &'a solidcore::curve::curve::Curve,
        range: (f64, f64),
        remaining_bytes: usize,
        work: &mut crate::GeometryWorkBudget,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<crate::parametric::PreparedValidatedCurve<'a>, crate::GeometryError> {
        if !self
            .edge_curve(edge)
            .is_some_and(|retained| std::ptr::eq(retained, curve))
        {
            return Err(crate::GeometryError::new(
                spiling_contracts::geometry::GeometryErrorCode::InvalidGeometry,
                "prepared native curve is not the retained edge operand",
            ));
        }
        crate::parametric::PreparedValidatedCurve::new(
            curve,
            [range.0, range.1],
            1.0,
            remaining_bytes,
            work,
            cancel,
        )
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

    /// Native face IDs in the exact source-face ordinal order.
    pub fn face_ids(&self) -> &[solidcore::FaceId] {
        &self.face_ids
    }

    /// Resolve a native face without interpreting its arena slot as source identity.
    pub fn face_ordinal(&self, id: solidcore::FaceId) -> Option<usize> {
        self.face_ids.iter().position(|&candidate| candidate == id)
    }

    /// Retained original STEP operands; use genealogy alone grants no authority.
    pub(crate) fn original_source(&self) -> &source::OriginalSourceTopology {
        &self.original_source
    }
    /// Zero-mutex borrow of a face's underlying surface.
    #[inline]
    pub fn face_surface(
        &self,
        id: solidcore::FaceId,
    ) -> Option<&solidcore::surface::surface::Surface> {
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
        solidcore::topology::edgeuse::p1::uv_trim_curve(&self.brep, id)
            .ok()
            .and_then(|(curve, _)| curve)
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
        total +=
            self.brep.vertexuses().len() * std::mem::size_of::<solidcore::topology::Vertexuse>();
        for (_, loop_) in self.brep.loops().iter() {
            total = total.saturating_add(
                loop_
                    .edges
                    .capacity()
                    .saturating_mul(std::mem::size_of::<solidcore::topology::LoopEdge>()),
            );
        }

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
        SurfaceKind::Step(_) => std::mem::size_of::<solidcore::surface::surface::p1::StepSurface>(),
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
        CurveKind::CrvOnSurf(_) => std::mem::size_of::<solidcore::curve::crv_on_surf::CrvOnSurf>(),
        CurveKind::Hyperbola(_) => std::mem::size_of::<solidcore::curve::hyperbola::Hyperbola>(),
        CurveKind::Parabola(_) => std::mem::size_of::<solidcore::curve::parabola::Parabola>(),
        CurveKind::Projected(_) => {
            std::mem::size_of::<solidcore::curve::projected_curve::ProjectedCurve>()
        }
        CurveKind::Iso(_) => std::mem::size_of::<solidcore::curve::iso_curve::IsoCurve>(),
        CurveKind::Offset(_) => std::mem::size_of::<solidcore::curve::offset_curve::OffsetCurve>(),
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
mod zero_material_transport_tests {
    use std::sync::atomic::AtomicBool;

    #[test]
    fn exact_normalized_box_endpoints_prove_zero_but_missing_receipt_cannot() {
        let cancel = AtomicBool::new(false);
        let mut definition = crate::from_step_bytes_with_cancel(
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
        definition.original_source.faces.clear();
        assert!(!matches!(
            definition.material_transport_is_proved_zero(&mut work, &cancel),
            Ok(true)
        ));
    }
}
