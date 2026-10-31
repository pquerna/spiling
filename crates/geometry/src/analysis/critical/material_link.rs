// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native BREP material incidence and continuous boundary qualification.
//! Native chart normalization is the kernel's edgeuse law. Original STEP
//! coefficients, transport recipes and reconstructed endpoint cones do not
//! participate in any material receipt.

use super::super::source_incidence::CoedgeUse;
use super::*;
use crate::parametric::{CurveEnclosure, SurfaceFirstEnclosure};

mod atlas;
mod coverage;
mod fiber;
mod radial_product;
mod vertex_fan;
#[cfg(test)]
mod wall;
pub(super) use coverage::{ClosedRadialLinks, MAX_CORRECTION_BYTES, native_face_point_is_retained};
pub(in crate::analysis) use radial_product::{
    BoundaryCell, BoundaryVisit, ClosedRadialMaterialProduct, RadialRegionKind,
    certify_closed_radial_material_product,
    certify_closed_radial_material_product_with_boundary_visitor,
};
pub(in crate::analysis) use vertex_fan::{
    qualify_native_common_carrier_vertex, qualify_native_vertex,
};
#[cfg(test)]
pub(crate) use wall::prove_source_wall_component;

pub(in crate::analysis) const UV_EMBEDDING_STACK_BYTES: usize = {
    let cell = crate::parametric::SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES + 32 * 1024;
    let trace = crate::kernel::trim::NATIVE_TRACE_QUERY_SCRATCH_BYTES;
    if cell > trace { cell } else { trace }
};

pub(super) fn resource() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "native material boundary exhausts remaining scratch allowance",
    )
}
fn uv_box(uv: [Interval; 2]) -> [[f64; 2]; 2] {
    uv.map(|axis| [axis.lo, axis.hi])
}
fn first_normal(jet: &SurfaceFirstEnclosure, orientation: bool) -> [Interval; 3] {
    let n = cross(jet.du, jet.dv);
    if orientation { n } else { n.map(Interval::neg) }
}

pub(super) struct MaterialLinkCell {
    pub position: [Interval; 3],
    pub effective_tolerance_mm: f64,
    pub convex_link: bool,
    outward: [Interval; 3],
    second_outward: [Interval; 3],
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
pub(in crate::analysis) enum MaterialLinkRefusal {
    NativeOwnership,
    NativeChartRank,
    NativeTraceTolerance(GeometryError),
    RadialProjection,
    OutwardRadialDot,
}
impl std::fmt::Debug for MaterialLinkRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NativeOwnership => formatter.write_str("NativeOwnership"),
            Self::NativeChartRank => formatter.write_str("NativeChartRank"),
            Self::NativeTraceTolerance(error) => formatter
                .debug_tuple("NativeTraceTolerance")
                .field(error)
                .finish(),
            Self::RadialProjection => formatter.write_str("RadialProjection"),
            Self::OutwardRadialDot => formatter.write_str("OutwardRadialDot"),
        }
    }
}

/// Native edgeuse ownership and separately qualified normalized-gauge positional
/// correspondence. Each query encloses both complete oriented carrier images.
pub(in crate::analysis) struct PairedMaterialLink<'a, 'd> {
    first: &'a PreparedFaceTrimDomain<'d>,
    second: &'a PreparedFaceTrimDomain<'d>,
    uses: [CoedgeUse; 2],
    range: [f64; 2],
    tolerance: f64,
    source_charts: [Option<source_chart::SphereChart>; 2],
}
impl<'a, 'd> PairedMaterialLink<'a, 'd> {
    pub fn new(
        definition: &Definition,
        first_domain: &'a PreparedFaceTrimDomain<'d>,
        first: CoedgeUse,
        second_domain: &'a PreparedFaceTrimDomain<'d>,
        second: CoedgeUse,
        range: [f64; 2],
        remaining: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        work.charge(1, cancel)?;
        if remaining < UV_EMBEDDING_STACK_BYTES {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                format!(
                    "native paired-link scratch exceeds the live allocation allowance: required_scratch_bytes={UV_EMBEDDING_STACK_BYTES}, available_bytes={remaining}",
                ),
            ));
        }
        if !std::ptr::eq(first_domain.definition(), definition)
            || !std::ptr::eq(second_domain.definition(), definition)
            || !first_domain.is_native_material()
            || !second_domain.is_native_material()
            || first.orientation == second.orientation
        {
            return Ok(None);
        }
        let (Some(a), Some(b)) = (
            first_domain.coedges().get(first.coedge),
            second_domain.coedges().get(second.coedge),
        ) else {
            return Ok(None);
        };
        if a.edge != b.edge
            || a.native_range != b.native_range
            || a.edgeuse != first.edgeuse
            || b.edgeuse != second.edgeuse
            || a.identity().wire_index != first.wire
            || a.identity().coedge_index != first.index
            || b.identity().wire_index != second.wire
            || b.identity().coedge_index != second.index
            || range[0] < a.native_range[0]
            || range[1] > a.native_range[1]
        {
            return Ok(None);
        }
        // Native carrier identity/orientation is plane-independent and immutable
        // for this borrowed pair. Reuse it through every adaptive cell instead
        // of rebuilding the same cofactors for each normal query.
        let source_charts = [
            source_chart::SphereChart::new(
                &first_domain.surface_operand(),
                first_domain,
                work,
                cancel,
            )?,
            source_chart::SphereChart::new(
                &second_domain.surface_operand(),
                second_domain,
                work,
                cancel,
            )?,
        ];
        Ok(Some(Self {
            first: first_domain,
            second: second_domain,
            uses: [first, second],
            range,
            tolerance: first_domain
                .effective_tolerance_mm()
                .max(second_domain.effective_tolerance_mm()),
            source_charts,
        }))
    }
    pub(in crate::analysis) fn normals(
        &self,
        range: [f64; 2],
        curve: &CurveEnclosure,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<
        Result<([Interval; 3], [Interval; 3], [Interval; 3]), MaterialLinkRefusal>,
        GeometryError,
    > {
        if range[0] < self.range[0] || range[1] > self.range[1] {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "native paired-edge query leaves captured restriction",
            ));
        }
        let mut normals = [[Interval::point(0.0); 3]; 2];
        let mut position = curve.position;
        for (slot, (domain, use_)) in [(self.first, self.uses[0]), (self.second, self.uses[1])]
            .into_iter()
            .enumerate()
        {
            let uv = match domain.derived_native_coedge_uv_image(
                use_.coedge,
                range,
                self.tolerance,
                work,
                cancel,
            )? {
                Some(image) => image,
                None => {
                    // No whole-branch derived map supplies authority here.
                    // Prove this local fixed pairing, preserving height-based
                    // refinement of irrelevant unqualified endpoint ranges.
                    match domain.qualify_native_coedge_trace(
                        use_.coedge,
                        range,
                        self.tolerance,
                        UV_EMBEDDING_STACK_BYTES,
                        work,
                        cancel,
                    ) {
                        Ok(()) => {}
                        Err(error)
                            if matches!(
                                error.code,
                                GeometryErrorCode::ParameterCorrespondenceUnavailable
                                    | GeometryErrorCode::AnalysisIndeterminate
                                    | GeometryErrorCode::ConditioningFailure
                            ) =>
                        {
                            return Ok(Err(MaterialLinkRefusal::NativeTraceTolerance(error)));
                        }
                        Err(error) => return Err(error),
                    }
                    domain.coedges()[use_.coedge]
                        .native_chart_jet(range, work, cancel)?
                        .position
                }
            };
            let surface = domain.surface_operand();
            let jet = crate::parametric::surface_piecewise_first_enclosure(
                &surface,
                uv_box(uv),
                work,
                cancel,
            )?;
            normals[slot] = match self.source_charts[slot].as_ref() {
                Some(sphere) => sphere.normal(jet.position)?,
                None => first_normal(&jet, domain.orientation()),
            };
            if length(normals[slot])?.lo <= 0.0 {
                return Ok(Err(MaterialLinkRefusal::NativeChartRank));
            }
            position = std::array::from_fn(|axis| position[axis].hull(jet.position[axis]));
        }
        Ok(Ok((position, normals[0], normals[1])))
    }
    pub(super) fn swept_enclosure(
        &self,
        range: [f64; 2],
        curve: &CurveEnclosure,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 3], GeometryError> {
        // Each complete UV branch is independently covered over the complete
        // native range even without a linked-gauge theorem. Such a swept image
        // supplies ONLY position exclusion; normals require proved correspondence.
        let mut position = curve.position;
        for (domain, use_) in [(self.first, self.uses[0]), (self.second, self.uses[1])] {
            let uv = match domain.derived_native_coedge_uv_image(
                use_.coedge,
                range,
                self.tolerance,
                work,
                cancel,
            )? {
                Some(image) => image,
                None => {
                    domain.coedges()[use_.coedge]
                        .native_chart_jet(range, work, cancel)?
                        .position
                }
            };
            let image = crate::parametric::surface_position_enclosure(
                &domain.surface_operand(),
                uv_box(uv),
                work,
                cancel,
            )?;
            position = std::array::from_fn(|axis| position[axis].hull(image[axis]));
        }
        Ok(position)
    }
    pub(super) fn cell(
        &self,
        range: [f64; 2],
        curve: &CurveEnclosure,
        prism: &SamplePrism,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Result<MaterialLinkCell, MaterialLinkRefusal>, GeometryError> {
        let (position, a, b) = match self.normals(range, curve, work, cancel)? {
            Ok(value) => value,
            Err(reason) => return Ok(Err(reason)),
        };
        let Some((radial, _)) = prism.radial_projection_frame(position)? else {
            return Ok(Err(MaterialLinkRefusal::RadialProjection));
        };
        if dot(a, radial).lo <= 0.0 || dot(b, radial).lo <= 0.0 {
            return Ok(Err(MaterialLinkRefusal::OutwardRadialDot));
        }
        let tangent = if self.uses[0].orientation {
            curve.derivative
        } else {
            curve.derivative.map(Interval::neg)
        };
        let convex = dot(b, cross(a, tangent)).hi <= 0.0;
        Ok(Ok(MaterialLinkCell {
            position,
            effective_tolerance_mm: self.tolerance,
            convex_link: convex,
            outward: a,
            second_outward: b,
        }))
    }
}

/// Conservative fallback when a certified collar has no finite positive
/// source-only projection rectangle. The whole-material coefficient bounds
/// enclose every closed height subset; this is neither a tight slab extremum
/// nor a void, membership or distance certificate.
pub(crate) fn native_boundary_projection_cover(
    source: MaterialSourceView<'_, '_>,
    matrix: [[Interval; 3]; 3],
    pose: spiling_contracts::geometry::RigidPoseMm,
    mut xy: [[f64; 2]; 2],
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[f64; 2]; 2], GeometryError> {
    let position = super::super::placed::material_position_bounds(source, remaining, work, cancel)?;
    for axis in 0..2 {
        work.charge(1, cancel)?;
        let world = dot(matrix[axis], position).add(Interval::point(pose.translation_mm[axis]));
        Interval::new(world.lo, world.hi)?;
        xy[axis][0] = xy[axis][0].min(world.lo);
        xy[axis][1] = xy[axis][1].max(world.hi);
    }
    Ok(xy)
}
