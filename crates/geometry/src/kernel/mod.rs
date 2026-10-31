// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

pub(crate) mod source;
pub(crate) mod source_operand;

mod cylinder;
pub(crate) mod trim;
mod wall;
pub(crate) use trim::{
    PreparedFaceTrimDomain, PreparedNativeTrimSource, PreparedTrimSurface, TrimClassification,
    certified_curve2d_is_straight_regular,
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

fn prepare_finish_source<'d>(
    definition: &'d Definition,
    work: &mut crate::GeometryWorkBudget,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<PreparedNativeTrimSource<'d>, crate::GeometryError> {
    let remaining = crate::analysis::MAX_SCRATCH_ALLOCATION_BYTES
        .checked_sub(definition.retained_query_proof_bytes())
        .ok_or_else(|| {
            crate::GeometryError::new(
                spiling_contracts::geometry::GeometryErrorCode::ResourceLimit,
                "retained native proof exhausts finish source allocation cap",
            )
        })?;
    PreparedNativeTrimSource::prepare_originals(definition, remaining, work, cancel)
}

/// Exclusion-only support for the narrow plane/cylinder finish envelope.
/// A linear functional on a bounded plane or cylinder attains its extrema on
/// the spatial boundary (follow an axial generator for a cylinder). Whole
/// circles deliberately enclose all branches; this does not select a trim,
/// prove UV embedding, or mint a raw/material directional-bounds receipt.
fn finish_support_extent(
    definition: &Definition,
    direction: [f64; 3],
    work: &mut crate::GeometryWorkBudget,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<[f64; 2], crate::GeometryError> {
    use crate::{exact_coefficient::ExactCoefficient, parametric::Interval};
    use solidcraft_step::geom::{Curve, Surface};
    use spiling_contracts::geometry::GeometryErrorCode;
    let unavailable = || {
        crate::GeometryError::new(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "finish support requires original plane/cylinder faces with analytic line/circle boundaries",
        )
    };
    let remaining = crate::analysis::MAX_SCRATCH_ALLOCATION_BYTES
        .checked_sub(definition.retained_query_proof_bytes())
        .ok_or_else(|| {
            crate::GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "retained native proof exhausts finish support allocation cap",
            )
        })?;
    let coefficient = |value: &ExactCoefficient| {
        value.terms[..value.len]
            .iter()
            .fold(Interval::point(0.0), |sum, &term| {
                sum.add(Interval::point(term))
            })
    };
    let mut extent = [f64::INFINITY, f64::NEG_INFINITY];
    for face in &definition.original_source().faces {
        work.charge(1, cancel)?;
        if !matches!(
            face.carrier.as_ref(),
            Surface::Plane { .. } | Surface::Cylinder { .. }
        ) {
            return Err(unavailable());
        }
        for wire in &face.wires {
            for &index in &wire.coedges {
                work.charge(1, cancel)?;
                let edge = definition.original_source().coedge(index)?;
                let mut curve = edge.spatial_curve.as_ref();
                let mut selected_master = None;
                for depth in 0..=32 {
                    work.charge(1, cancel)?;
                    match curve {
                        Curve::Trimmed { basis, .. } => curve = basis,
                        Curve::SurfaceCurve { curve_3d, .. } => {
                            if let Some(pcurve) = source_operand::preferred_source_pcurve(curve)? {
                                selected_master = Some(pcurve);
                                break;
                            }
                            curve = curve_3d;
                        }
                        _ => break,
                    }
                    if depth == 32 {
                        return Err(unavailable());
                    }
                }
                let project = |position: [Interval; 3]| {
                    (0..3).fold(Interval::point(0.0), |sum, axis| {
                        sum.add(position[axis].mul(Interval::point(direction[axis])))
                    })
                };
                let projection = if let Some(pcurve) = selected_master {
                    let finite = source_operand::original_curve_parameter_restriction(curve)?
                        .map(|[a, b]| [a.min(b), a.max(b)]);
                    let range = match finite {
                        Some(range) => Some(range),
                        None => {
                            source_operand::original_curve_parameter_period(curve, work, cancel)?
                                .map(|period| [0.0, period.hi])
                        }
                    };
                    if let Some(range) = range {
                        let selected = source_operand::PreparedOriginalCurve::new(
                            curve, range, remaining, work, cancel,
                        )?;
                        if let Some(affine) = selected.exact_affine_coefficients(work, cancel)? {
                            let denominator = coefficient(&affine.denominator);
                            let position = project(std::array::from_fn(|axis| {
                                coefficient(&affine.position[axis])
                            }))
                            .div(denominator)?;
                            let velocity = project(std::array::from_fn(|axis| {
                                coefficient(&affine.derivative[axis])
                            }))
                            .div(denominator)?;
                            let a = position.add(velocity.mul(Interval::point(range[0])));
                            let b = position.add(velocity.mul(Interval::point(range[1])));
                            Interval {
                                lo: a.lo.min(b.lo),
                                hi: a.hi.max(b.hi),
                            }
                        } else {
                            project(selected.position_bounds(range, work, cancel)?)
                        }
                    } else {
                        let selected = source_operand::PreparedOriginalCurve::new(
                            curve,
                            [0.0, 1.0],
                            remaining,
                            work,
                            cancel,
                        )?;
                        let affine = selected.exact_affine_coefficients(work, cancel)?;
                        let (origin, velocity) = if let Some(affine) = affine {
                            let denominator = coefficient(&affine.denominator);
                            let mut origin = [Interval::point(0.0); 3];
                            let mut velocity = [Interval::point(0.0); 3];
                            for axis in 0..3 {
                                origin[axis] =
                                    coefficient(&affine.position[axis]).div(denominator)?;
                                velocity[axis] =
                                    coefficient(&affine.derivative[axis]).div(denominator)?;
                            }
                            (origin, velocity)
                        } else {
                            // A literal zero-angular-velocity LINE on a cylinder
                            // is an affine spatial generator, for every parameter.
                            let Curve::Line {
                                direction: chart_direction,
                                magnitude,
                                ..
                            } = pcurve.curve.as_ref()
                            else {
                                return Err(unavailable());
                            };
                            if !matches!(pcurve.surface.as_ref(), Surface::Cylinder { .. })
                                || chart_direction[0] != 0.0
                                || chart_direction[1] == 0.0
                                || *magnitude == 0.0
                            {
                                return Err(unavailable());
                            }
                            let jet = selected.jet([0.0, 0.0], work, cancel)?.jet;
                            (jet.position, jet.derivative)
                        };
                        let norm = velocity
                            .iter()
                            .fold(Interval::point(0.0), |sum, value| sum.add(value.square()));
                        let origin_projection = project(origin);
                        let velocity_projection = project(velocity);
                        let mut result = Interval {
                            lo: f64::INFINITY,
                            hi: f64::NEG_INFINITY,
                        };
                        for point in edge.vertex_points_mm {
                            let parameter = (0..3)
                                .fold(Interval::point(0.0), |sum, axis| {
                                    sum.add(
                                        Interval::point(point[axis])
                                            .sub(origin[axis])
                                            .mul(velocity[axis]),
                                    )
                                })
                                .div(norm)?;
                            let value = origin_projection.add(velocity_projection.mul(parameter));
                            result.lo = result.lo.min(value.lo);
                            result.hi = result.hi.max(value.hi);
                        }
                        result
                    }
                } else {
                    match curve {
                        Curve::Line { .. } => {
                            let affine = source_operand::exact_source_line_projection_coefficients(
                                curve,
                                &edge.vertex_points_mm,
                                work,
                                cancel,
                            )?
                            .ok_or_else(unavailable)?;
                            let denominator = coefficient(&affine.denominator);
                            let start = project(std::array::from_fn(|axis| {
                                coefficient(&affine.position[axis])
                            }))
                            .div(denominator)?;
                            let delta = project(std::array::from_fn(|axis| {
                                coefficient(&affine.derivative[axis])
                            }))
                            .div(denominator)?;
                            let end = start.add(delta);
                            Interval {
                                lo: start.lo.min(end.lo),
                                hi: start.hi.max(end.hi),
                            }
                        }
                        Curve::Circle { .. } => {
                            let circle = source_operand::PreparedOriginalCurve::new(
                                curve,
                                [0.0, std::f64::consts::TAU],
                                remaining,
                                work,
                                cancel,
                            )?;
                            project(circle.position_bounds(
                                [0.0, std::f64::consts::TAU],
                                work,
                                cancel,
                            )?)
                        }
                        _ => return Err(unavailable()),
                    }
                };
                extent[0] = extent[0].min(projection.lo);
                extent[1] = extent[1].max(projection.hi);
            }
        }
    }
    if !extent.iter().all(|value| value.is_finite()) {
        return Err(unavailable());
    }
    Ok(extent)
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

/// Captured native-material producer identity. Original STEP uncertainty remains
/// provenance; there is no reconstructed collar or vertex-completion recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MaterialRealizationIdentity {
    version: u32,
    source_uncertainty_bits: Option<u64>,
}
impl MaterialRealizationIdentity {
    pub(crate) fn new(definition: &Definition) -> Self {
        Self::with_source_uncertainty(definition.provenance.uncertainty_mm)
    }
    pub(crate) fn with_source_uncertainty(source_uncertainty_mm: Option<f64>) -> Self {
        Self {
            version: 16,
            source_uncertainty_bits: source_uncertainty_mm.map(f64::to_bits),
        }
    }
    /// Canonical cache hash input, not a persisted binary representation.
    pub(crate) fn identity_bytes(self) -> [u8; 53] {
        let mut bytes = [0; 53];
        bytes[..4].copy_from_slice(&self.version.to_le_bytes());
        bytes[4] = u8::from(self.source_uncertainty_bits.is_some());
        bytes[5..13].copy_from_slice(&self.source_uncertainty_bits.unwrap_or(0).to_le_bytes());
        for (chunk, value) in bytes[13..].as_chunks_mut::<8>().0.iter_mut().zip([
            KERNEL_ZONE_TOLERANCE_MM,
            KERNEL_INTERSECTION_TOLERANCE_MM,
            KERNEL_APPROXIMATION_TOLERANCE_MM,
            KERNEL_ANGULAR_TOLERANCE_DEGREES,
            KERNEL_MODEL_SIZE_ESTIMATE_MM,
        ]) {
            chunk.copy_from_slice(&value.to_bits().to_le_bytes());
        }
        bytes
    }
}

impl Definition {
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

/// Native query coincidence tolerance; individual BREP entity tolerances remain
/// separate captured import data, not an independent original-source proof.
pub const KERNEL_ZONE_TOLERANCE_MM: f64 = 1.0e-6;
/// Native surface/curve intersection tolerance.
pub const KERNEL_INTERSECTION_TOLERANCE_MM: f64 = 2.0e-6;
/// Native approximation tolerance.
pub const KERNEL_APPROXIMATION_TOLERANCE_MM: f64 = 5.0e-7;
const KERNEL_ANGULAR_TOLERANCE_DEGREES: f64 = 1.0e-6;
const KERNEL_MODEL_SIZE_ESTIMATE_MM: f64 = 100.0;

/// Constructs a deterministic thread-isolated `solidcore::Kernel` context with Spiling mm tolerances.
pub fn make_kernel_context() -> solidcore::Kernel {
    let tolerances = solidcore::Tolerances {
        zone_tol_3d: KERNEL_ZONE_TOLERANCE_MM,
        xsect_tol_3d: KERNEL_INTERSECTION_TOLERANCE_MM,
        approx_tol_3d: KERNEL_APPROXIMATION_TOLERANCE_MM,
        ang_tol_deg: KERNEL_ANGULAR_TOLERANCE_DEGREES,
        model_size_estimate: KERNEL_MODEL_SIZE_ESTIMATE_MM,
        ..solidcore::Tolerances::default()
    };

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
mod finish_support_tests {
    use super::*;
    use solidcraft_step::geom::{Associated, Curve, PreferredRep, Surface};
    use std::{
        fmt::Write,
        sync::{Arc, atomic::AtomicBool},
    };

    #[test]
    fn cylinder_certificate_cannot_ignore_another_faces_preferred_master_extent() {
        for preferred in [PreferredRep::PcurveS1, PreferredRep::PcurveS2] {
            let cancel = AtomicBool::new(false);
            let mut definition = crate::from_step_bytes_with_cancel(
                include_bytes!(
                    "../../../../fixtures/geometry/independent-exporters/occt-cylinder-mm.step"
                ),
                &cancel,
            )
            .unwrap();
            let side = definition
                .faces
                .iter()
                .position(|face| {
                    matches!(
                        face.geometry.analytic,
                        Some(
                            spiling_contracts::geometry::AnalyticSurfaceDescription::Cylinder { .. }
                        )
                    )
                })
                .unwrap();
            let side_id = definition.faces[side].face_id.clone();
            let other = definition
                .original_source
                .faces
                .iter()
                .enumerate()
                .find(|(index, _)| *index != side)
                .unwrap()
                .1;
            let index = other.wires[0].coedges[0];
            let original =
                Arc::make_mut(&mut definition.original_source.coedges[index].spatial_curve);
            let Curve::SurfaceCurve {
                master, associated, ..
            } = original
            else {
                panic!("independent cap retains its original SURFACE_CURVE");
            };
            *master = preferred;
            let selected = match preferred {
                PreferredRep::PcurveS1 => 0,
                PreferredRep::PcurveS2 => 1,
                _ => unreachable!(),
            };
            let Associated::Pcurve(pcurve) = &mut associated[selected] else {
                panic!("declared master is an original PCURVE");
            };
            match Arc::make_mut(&mut pcurve.surface) {
                Surface::Plane { position } | Surface::Cylinder { position, .. } => {
                    position.origin[2] += 16.0;
                }
                _ => panic!("analytic original carrier"),
            }
            let support = finish_support_extent(
                &definition,
                [0.0, 0.0, 1.0],
                &mut crate::GeometryWorkBudget::new(1_000_000),
                &cancel,
            )
            .unwrap();
            assert!(
                support[1] >= 16.0,
                "selected master must not use the unchanged 3D reference"
            );
            assert_eq!(
                definition
                    .vertical_cylinder_face(&side_id)
                    .unwrap_err()
                    .code,
                spiling_contracts::geometry::GeometryErrorCode::UnsupportedGeometry,
            );
        }
    }

    #[test]
    fn source_z_rotated_affine_bounds_preserve_support_projection_correlation() {
        let source = include_str!("../../../../fixtures/geometry/box-mm.step");
        let mut rotated = String::new();
        for line in source.lines() {
            let mut line = line.to_owned();
            if line.contains("CARTESIAN_POINT(") || line.contains("DIRECTION(") {
                let start = line.rfind('(').unwrap() + 1;
                let end = start + line[start..].find(')').unwrap();
                let values: Vec<f64> = line[start..end]
                    .split(',')
                    .map(|value| value.trim().parse().unwrap())
                    .collect();
                if let [x, y, z] = values.as_slice() {
                    let tuple = format!("{},{},{}", 0.6 * x - 0.8 * y, 0.8 * x + 0.6 * y, z);
                    line.replace_range(start..end, &tuple);
                }
            }
            writeln!(rotated, "{line}").unwrap();
        }
        let definition = crate::from_step_bytes(rotated.as_bytes()).unwrap();
        // The independent box is [0, 20] × [0, 10] × [0, 8]. Projecting
        // its rotated affine edges must retain correlation before hulling;
        // projecting their world-axis boxes invents material outside a wall.
        let cancel = AtomicBool::new(false);
        let mut work = crate::GeometryWorkBudget::new(1_000_000);
        for (direction, expected) in [
            ([0.6, 0.8, 0.0], [0.0, 20.0]),
            ([-0.6, -0.8, 0.0], [-20.0, 0.0]),
            ([-0.8, 0.6, 0.0], [0.0, 10.0]),
            ([0.8, -0.6, 0.0], [-10.0, 0.0]),
        ] {
            let actual = finish_support_extent(&definition, direction, &mut work, &cancel).unwrap();
            for (bound, expected) in actual.into_iter().zip(expected) {
                assert!((bound - expected).abs() <= 1e-7);
            }
        }
    }
}
