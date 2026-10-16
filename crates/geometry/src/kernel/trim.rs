// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native chart proofs retain their source/native chart association in the
//! immutable definition. No nearest-point reconstruction is performed here.

use super::Definition;
use crate::{GeometryError, GeometryWorkBudget, check_cancel};
use monstertruck_modeling::base::BoundedCurve;
use monstertruck_modeling::{
    Curve, Curve2D, NativeSurfaceBoundary, NaturalBoundaryJointEvidence,
    ParameterBoxClassification, Point3, PreparedFaceParameterDomain, Surface, TrimDomainError,
    TrimDomainLimits, ValidatedGeometryBudget, certified_curve2d_endpoint_coordinate,
    prepare_face_parameter_domain_with_natural_boundaries,
};
use monstertruck_topology::trimmed::{ChartCoedgeIdentity, TrimmedFace};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrimClassification {
    Inside,
    Outside,
    Boundary,
}

pub(crate) struct PreparedFaceTrimDomain<'a> {
    native: PreparedFaceParameterDomain<'a>,
    source_chart_error_mm: f64,
    certified_uv_domain: Option<[[f64; 2]; 2]>,
    definition: &'a Definition,
    face_index: usize,
}
impl PreparedFaceTrimDomain<'_> {
    /// Resolve against immutable original metadata, never a cached address.
    pub(crate) fn original_coedge(
        &self,
        coedge_index: usize,
    ) -> Result<&super::BoundaryCoedgeRef, GeometryError> {
        self.definition.face_boundaries[self.face_index]
            .wires
            .iter()
            .flatten()
            .nth(coedge_index)
            .ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original coedge index is outside its captured face",
                )
            })
    }

    pub(crate) fn original_coedge_correspondence(
        &self,
        coedge_index: usize,
    ) -> Result<&super::OriginalCoedgeCorrespondence, GeometryError> {
        let coedge = self.original_coedge(coedge_index)?;
        let proof = coedge.correspondence.as_ref().ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original coedge has no completed coefficient/forward correspondence",
            )
        })?;
        let binding = proof.binding().ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original coedge correspondence lacks normalized ownership",
            )
        })?;
        if binding.face != self.definition.face_boundaries[self.face_index].face
            || binding.edge != coedge.edge
            || binding.owner[0] != self.face_index
            || binding.source.native_orientation != coedge.orientation
            || coedge.source_chart_parameter_range
                != Some((proof.uv_range()[0], proof.uv_range()[1]))
            || !coedge.source_chart_certificate.is_certified()
        {
            return Err(GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "retained original coedge correspondence ownership changed",
            ));
        }
        Ok(proof)
    }

    /// Reevaluate unchanged native spatial and UV coefficient jets using the
    /// retained endpoint recipe. Both returned derivatives use native edge t.
    /// UV uncertainty remains separately available on the receipt.
    pub(crate) fn original_coedge_jets(
        &self,
        coedge_index: usize,
        range: [f64; 2],
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<
        (
            crate::parametric::CurveEnclosure,
            crate::parametric::ChartCurveEnclosure,
        ),
        GeometryError,
    > {
        check_cancel(cancel)?;
        let proof = self.original_coedge_correspondence(coedge_index)?;
        let binding = proof.binding().expect("validated original ownership");
        let face = &self.definition.trimmed_solid.boundaries()[0].faces()[self.face_index];
        let edge = &face.face().absolute_boundaries()[binding.owner[1]][binding.owner[2]];
        if edge.id() != binding.edge || [edge.front().id(), edge.back().id()] != binding.endpoints {
            return Err(GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original native coedge operands changed",
            ));
        }
        proof.uv_parameter_image(range)?;
        let native_range = proof.native_range();
        let spatial = edge.with_curve(|curve| {
            self.definition
                .prepare_boundary_curve(
                    edge.id(),
                    curve,
                    (native_range[0], native_range[1]),
                    remaining_bytes,
                    work,
                    cancel,
                )?
                .jet((range[0], range[1]), work, cancel)
        })?;
        Ok((
            spatial,
            self.original_coedge_uv_jet(coedge_index, range, work, cancel)?,
        ))
    }

    /// Reevaluate only the unchanged UV coefficients. Spatial consumers can
    /// reuse an already prepared original edge instead of recapturing per cell.
    pub(crate) fn original_coedge_uv_jet(
        &self,
        coedge_index: usize,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<crate::parametric::ChartCurveEnclosure, GeometryError> {
        check_cancel(cancel)?;
        let proof = self.original_coedge_correspondence(coedge_index)?;
        let binding = proof.binding().expect("validated original ownership");
        let face = &self.definition.trimmed_solid.boundaries()[0].faces()[self.face_index];
        let uv_range = proof.uv_parameter_image(range)?;
        let chart = face.trims()[binding.owner[1]][binding.owner[2]]
            .as_ref()
            .ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original coedge UV coefficients are unavailable",
                )
            })?;
        let mut native_work =
            ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
        let result = monstertruck_modeling::validated_curve2d_jet(
            chart,
            [uv_range.lo, uv_range.hi],
            &mut native_work,
            cancel,
        );
        crate::parametric::debit_native_work(work, native_work.consumed(), cancel)?;
        let jet = result.map_err(crate::parametric::native_error)?;
        let adapt = |v: monstertruck_modeling::ValidatedInterval| crate::parametric::Interval {
            lo: v.lower,
            hi: v.upper,
        };
        let slope = proof.uv_parameter_derivative();
        Ok(crate::parametric::ChartCurveEnclosure {
            position: jet.position.map(adapt),
            derivative: jet.first.map(|v| adapt(v).mul(slope)),
            second_derivative: jet.second.map(|v| adapt(v).mul(slope.square())),
        })
    }

    pub(crate) fn material_realization(&self) -> super::OriginalMaterialRealization<'_> {
        super::OriginalMaterialRealization::new(self.definition)
    }

    /// Visit every original spatial/UV knot and retained continuation boundary
    /// in native-edge coordinates without a per-cell retained vector. The
    /// callback runs under the original edge's coefficient borrow; collect
    /// boundary data only and do not recursively evaluate that edge.
    pub(crate) fn visit_original_coedge_span_boundaries(
        &self,
        coedge_index: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        visit: &mut impl FnMut(crate::parametric::Interval) -> Result<(), GeometryError>,
    ) -> Result<(), GeometryError> {
        let proof = self.original_coedge_correspondence(coedge_index)?;
        let coedge = self.original_coedge(coedge_index)?;
        let binding = proof.binding().expect("validated original ownership");
        let face = &self.definition.trimmed_solid.boundaries()[0].faces()[self.face_index];
        let edges = face.face().absolute_boundaries();
        let edge = &edges[binding.owner[1]][binding.owner[2]];
        let chart = face.trims()[binding.owner[1]][binding.owner[2]]
            .as_ref()
            .ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original coedge UV inventory is unavailable",
                )
            })?;
        let continuation = self
            .definition
            .boundary_continuations
            .get(coedge.witness)
            .and_then(Option::as_ref)
            .map(|branch| &self.definition.original_continuations[branch.prepared]);
        edge.with_curve(|curve| {
            proof.visit_span_boundaries(curve, chart, continuation, work, cancel, visit)
        })
    }

    pub(crate) fn uv_bounds(&self) -> [[f64; 2]; 2] {
        self.native.closed_parameter_box()
    }
    pub(crate) fn native(&self) -> &PreparedFaceParameterDomain<'_> {
        &self.native
    }
    pub(crate) fn source_chart_error_mm(&self) -> f64 {
        self.source_chart_error_mm
    }
    pub(crate) fn certified_uv_domain(&self) -> Option<[[f64; 2]; 2]> {
        self.certified_uv_domain
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        self.native.retained_bytes()
    }

    pub(crate) fn boundary_parameter_intervals(
        &self,
        coedge_index: usize,
    ) -> Result<impl Iterator<Item = [f64; 2]> + '_, GeometryError> {
        self.native
            .boundary_parameter_intervals(coedge_index)
            .map_err(trim_error)
    }

    pub(crate) fn boundary_position_enclosure(
        &self,
        coedge_index: usize,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[f64; 2]; 2], GeometryError> {
        check_cancel(cancel)?;
        let mut native_work =
            ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
        let result =
            self.native
                .boundary_position_enclosure(coedge_index, range, &mut native_work, cancel);
        crate::parametric::debit_native_work(work, native_work.consumed(), cancel)?;
        let mut position = result
            .map_err(trim_error)?
            .map(|value| [value.lower, value.upper]);
        if let Some(domain) = self.certified_uv_domain {
            for axis in 0..2 {
                position[axis][0] = position[axis][0].max(domain[axis][0]);
                position[axis][1] = position[axis][1].min(domain[axis][1]);
                if position[axis][0] > position[axis][1] {
                    return Err(GeometryError::new(
                        GeometryErrorCode::ParameterCorrespondenceUnavailable,
                        "source trim tube does not meet its independently certified carrier domain",
                    ));
                }
            }
        }
        Ok(position)
    }

    pub(crate) fn classify(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<TrimClassification, GeometryError> {
        self.classify_with_byte_limit(uv, usize::MAX, work, cancel)
    }

    pub(crate) fn classify_with_byte_limit(
        &self,
        uv: [[f64; 2]; 2],
        metadata_inclusive_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<TrimClassification, GeometryError> {
        check_cancel(cancel)?;
        let mut native_work =
            ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
        let result = self.native.classify_parameter_box_with_byte_limit(
            uv,
            metadata_inclusive_bytes,
            &mut native_work,
            cancel,
        );
        crate::parametric::debit_native_work(work, native_work.consumed(), cancel)?;
        Ok(classification(result.map_err(trim_error)?))
    }
}

fn classification(value: ParameterBoxClassification) -> TrimClassification {
    match value {
        ParameterBoxClassification::Inside => TrimClassification::Inside,
        ParameterBoxClassification::Outside => TrimClassification::Outside,
        ParameterBoxClassification::Boundary => TrimClassification::Boundary,
    }
}
fn trim_error(error: TrimDomainError) -> GeometryError {
    let code = match error {
        TrimDomainError::Geometry(error) => return crate::parametric::native_error(error),
        TrimDomainError::ResourceLimit => GeometryErrorCode::ResourceLimit,
        TrimDomainError::MissingExactTrim { .. } => {
            GeometryErrorCode::ParameterCorrespondenceUnavailable
        }
        TrimDomainError::InvalidChart | TrimDomainError::UnclosedChart => {
            GeometryErrorCode::InvalidGeometry
        }
        TrimDomainError::UnsupportedSeam => GeometryErrorCode::AnalysisIndeterminate,
    };
    GeometryError::new(
        code,
        format!("native exact trim-domain analysis: {error:?}"),
    )
}

/// Whole original homogeneous branch test, never an endpoint/chord fit.
pub(crate) fn certified_curve2d_is_straight_regular(
    curve: &Curve2D,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    check_cancel(cancel)?;
    let mut native =
        ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
    let result = monstertruck_modeling::certified_curve2d_is_straight_regular(
        curve,
        range,
        &mut native,
        cancel,
    );
    crate::parametric::debit_native_work(work, native.consumed(), cancel)?;
    result.map_err(trim_error)
}

/// Authorize only an ORIGINAL source chart's selected natural boundary. Exact
/// endpoint-coordinate identities establish the branch; interval nearness is
/// used solely to discard ordinary/small UV joints, never as the authority.
fn original_natural_boundaries(
    trimmed: &TrimmedFace<Point3, Curve, Surface, Curve2D>,
    identity: &super::FaceBoundaryRefs,
    remaining_bytes: usize,
    budget: &mut ValidatedGeometryBudget,
    cancel: &AtomicBool,
) -> Result<Vec<NaturalBoundaryJointEvidence>, TrimDomainError> {
    use monstertruck_io::step::load::step_geometry::SurfaceParameterMap;
    let mut evidence = Vec::new();
    if !matches!(identity.parameter.conversion, SurfaceParameterMap::Identity) {
        return Ok(evidence);
    }
    let Some(domain) = identity.parameter.native_uv_domain else {
        return Ok(evidence);
    };
    let face = trimmed.face();
    let boundaries = [
        (0, domain[0][0], NativeSurfaceBoundary::UFront),
        (0, domain[0][1], NativeSurfaceBoundary::UBack),
        (1, domain[1][0], NativeSurfaceBoundary::VFront),
        (1, domain[1][1], NativeSurfaceBoundary::VBack),
    ];
    if face.absolute_boundaries().len() != identity.wires.len() {
        return Err(TrimDomainError::InvalidChart);
    }
    for (wire_index, wire) in face.absolute_boundaries().iter().enumerate() {
        if wire.len() != identity.wires[wire_index].len() {
            return Err(TrimDomainError::InvalidChart);
        }
        for (index, current) in wire.iter().enumerate() {
            budget.charge(1, cancel)?;
            let next_index = if index + 1 == wire.len() {
                0
            } else {
                index + 1
            };
            let next = &wire[next_index];
            let a = &identity.wires[wire_index][index];
            let b = &identity.wires[wire_index][next_index];
            if !a.source_chart_declared || !b.source_chart_declared {
                continue;
            }
            if a.edge != current.id()
                || b.edge != next.id()
                || a.orientation != current.orientation()
                || b.orientation != next.orientation()
                || current.back().id() != next.front().id()
            {
                return Err(TrimDomainError::InvalidChart);
            }
            let first = trimmed.trims()[wire_index][index].as_ref().ok_or(
                TrimDomainError::MissingExactTrim {
                    wire_index,
                    coedge_index: index,
                },
            )?;
            let second = trimmed.trims()[wire_index][next_index].as_ref().ok_or(
                TrimDomainError::MissingExactTrim {
                    wire_index,
                    coedge_index: next_index,
                },
            )?;
            let first_range = first.range_tuple();
            let second_range = second.range_tuple();
            if a.source_chart_parameter_range != Some(first_range)
                || b.source_chart_parameter_range != Some(second_range)
            {
                return Err(TrimDomainError::InvalidChart);
            }
            let ta = if current.orientation() {
                first_range.1
            } else {
                first_range.0
            };
            let tb = if next.orientation() {
                second_range.0
            } else {
                second_range.1
            };
            let p = monstertruck_modeling::validated_curve2d_jet(first, [ta; 2], budget, cancel)?
                .position;
            let q = monstertruck_modeling::validated_curve2d_jet(second, [tb; 2], budget, cancel)?
                .position;
            let mut selected = None;
            for (axis, value, boundary) in boundaries {
                let variable = 1 - axis;
                // A genuine pole connector has distinct variable-coordinate
                // endpoint intervals. Ordinary equal/tiny joints stay ordinary.
                if !(p[variable].upper < q[variable].lower || q[variable].upper < p[variable].lower)
                {
                    continue;
                }
                if certified_curve2d_endpoint_coordinate(first, ta, axis, value, budget, cancel)?
                    && certified_curve2d_endpoint_coordinate(
                        second, tb, axis, value, budget, cancel,
                    )?
                {
                    if selected.is_some() {
                        return Err(TrimDomainError::InvalidChart);
                    }
                    selected = Some(boundary);
                }
            }
            let Some(boundary) = selected else {
                continue;
            };
            let use_identity = |coedge_index: usize| ChartCoedgeIdentity {
                face_id: face.id(),
                wire_index,
                coedge_index,
                edge_id: wire[coedge_index].id(),
                orientation: wire[coedge_index].orientation() == face.orientation(),
            };
            let (incoming, outgoing) = if face.orientation() {
                (use_identity(index), use_identity(next_index))
            } else {
                (use_identity(next_index), use_identity(index))
            };
            if evidence.len() == evidence.capacity() {
                let capacity = evidence.capacity().saturating_mul(2).max(4);
                if capacity > remaining_bytes / std::mem::size_of::<NaturalBoundaryJointEvidence>()
                {
                    return Err(TrimDomainError::ResourceLimit);
                }
                budget.charge(1, cancel)?;
                evidence
                    .try_reserve_exact(capacity - evidence.len())
                    .map_err(|_| TrimDomainError::ResourceLimit)?;
            }
            evidence.push(NaturalBoundaryJointEvidence {
                vertex_id: current.back().id(),
                incoming,
                outgoing,
                boundary,
                // This is source-vertex numerical correspondence, NOT license
                // to fill a display-sized physical gap. Euclidean reserve is
                // below the existing 1e-7 mm source/coedge budget.
                max_vertex_deviation: [5.0e-8; 3],
            });
        }
    }
    Ok(evidence)
}

/// Preparation acquires the native face's surface mutex: call this before any
/// `with_surface` callback for the same face. The returned UV domain retains no
/// surface lock and can be reused while inspecting a separately borrowed surface.
pub(crate) fn prepare_face_trim_domain_with_byte_limit<'a>(
    definition: &'a Definition,
    face_index: usize,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PreparedFaceTrimDomain<'a>, GeometryError> {
    check_cancel(cancel)?;
    let face = definition
        .trimmed_solid
        .boundaries()
        .first()
        .and_then(|shell| shell.faces().get(face_index))
        .ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::UnknownHandle,
                "native trim face index is outside captured definition",
            )
        })?;
    let identity = definition.face_boundaries.get(face_index).ok_or_else(|| {
        GeometryError::new(
            GeometryErrorCode::KernelFailure,
            "native trim face is missing source correspondence",
        )
    })?;
    if face.face().id() != identity.face {
        return Err(GeometryError::new(
            GeometryErrorCode::KernelFailure,
            "native trim chart disagrees with captured source face identity",
        ));
    }
    let mut margin_uv = [0.0_f64; 2];
    let mut source_chart_error_mm = 0.0_f64;
    for (wire_index, coedge_index, coedge) in
        identity
            .wires
            .iter()
            .enumerate()
            .flat_map(|(wire_index, wire)| {
                wire.iter()
                    .enumerate()
                    .map(move |(coedge_index, coedge)| (wire_index, coedge_index, coedge))
            })
    {
        work.charge(1, cancel)?;
        if !coedge.source_chart_certificate.is_certified() {
            let source_use = definition
                .source_topology
                .faces
                .get(face_index)
                .and_then(|face| face.wires.get(wire_index))
                .and_then(|wire| wire.coedges.get(coedge_index));
            return Err(GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                format!(
                    "original source trim retained, but whole-coedge native chart correspondence is unresolved: source={} face={face_index} source_face={:?} wire={wire_index} coedge={coedge_index} source_edge={:?} native_edge={:?}",
                    definition.id().as_str(),
                    definition
                        .faces
                        .get(face_index)
                        .map(|face| face.face_id.as_str()),
                    source_use.and_then(|use_| use_.source_edge),
                    source_use.map(|use_| use_.native_edge_index),
                ),
            ));
        }
        if !coedge.source_chart_error_mm.is_finite() || coedge.source_chart_error_mm < 0.0 {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "source/native chart physical uncertainty is invalid",
            ));
        }
        source_chart_error_mm = source_chart_error_mm.max(coedge.source_chart_error_mm);
        for (maximum, margin) in margin_uv.iter_mut().zip(coedge.source_chart_margin_uv) {
            if !margin.is_finite() || margin < 0.0 {
                return Err(GeometryError::new(
                    GeometryErrorCode::ConditioningFailure,
                    "source/native chart uncertainty is invalid",
                ));
            }
            *maximum = maximum.max(margin);
        }
    }
    let mut native_work =
        ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
    let result = (|| {
        let evidence = original_natural_boundaries(
            face,
            identity,
            remaining_bytes.min(32 * 1024 * 1024),
            &mut native_work,
            cancel,
        )?;
        let evidence_bytes = evidence
            .capacity()
            .checked_mul(std::mem::size_of::<NaturalBoundaryJointEvidence>())
            .ok_or(TrimDomainError::ResourceLimit)?;
        let native_bytes = remaining_bytes
            .min(32 * 1024 * 1024)
            .checked_sub(evidence_bytes)
            .ok_or(TrimDomainError::ResourceLimit)?;
        let mut prepared = prepare_face_parameter_domain_with_natural_boundaries(
            face,
            margin_uv,
            &evidence,
            TrimDomainLimits {
                max_coedges: 2 * spiling_contracts::geometry::MAX_STEP_ENTITIES as usize,
                max_subdivisions: 1_000_000,
                max_retained_bytes: native_bytes,
            },
            &mut native_work,
            cancel,
        )?;
        if let Some(bounds) = identity.parameter.native_uv_domain {
            prepared.restrict_parameter_domain(bounds, &mut native_work, cancel)?;
        }
        Ok(prepared)
    })();
    crate::parametric::debit_native_work(work, native_work.consumed(), cancel)?;
    Ok(PreparedFaceTrimDomain {
        native: result.map_err(trim_error)?,
        source_chart_error_mm,
        certified_uv_domain: identity.parameter.native_uv_domain,
        definition,
        face_index,
    })
}

#[cfg(test)]
mod retained_receipt_tests {
    use super::*;

    fn original_box(name: &str) -> Definition {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry")
            .join(name);
        crate::import_step(&std::fs::read(path).unwrap(), &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn retained_receipt_survives_normalization_and_resolves_original_jets() {
        let cancel = AtomicBool::new(false);
        for name in ["box-mm.step", "box-inch.step"] {
            let definition = original_box(name);
            let mut work = GeometryWorkBudget::new(10_000_000);
            for face_index in 0..definition.faces.len() {
                let domain = prepare_face_trim_domain_with_byte_limit(
                    &definition,
                    face_index,
                    32 * 1024 * 1024,
                    &mut work,
                    &cancel,
                )
                .unwrap();
                let refs = &definition.face_boundaries[face_index];
                for (index, coedge) in refs.wires.iter().flatten().enumerate() {
                    let proof = domain.original_coedge_correspondence(index).unwrap();
                    let binding = proof.binding().unwrap();
                    assert_eq!(binding.face, refs.face);
                    assert_eq!(binding.edge, coedge.edge);
                    assert_eq!(binding.owner[0], face_index);
                    assert_eq!(binding.source.native_edge_index, coedge.witness);
                    assert_eq!(binding.source.native_orientation, coedge.orientation);
                    assert_eq!(
                        binding.source,
                        definition.source_topology.faces[face_index].wires[binding.owner[1]]
                            .coedges[binding.owner[2]],
                    );
                    assert!(binding.source.source_edge.is_some());
                    assert!(proof.position_error_mm() <= 1.0e-7);
                    assert!(proof.relative_derivative_error_bound() <= 0.25);
                    assert_eq!(proof.margin_uv(), coedge.source_chart_margin_uv);
                    let [a, b] = proof.native_range();
                    let (spatial, chart) = domain
                        .original_coedge_jets(index, [a, a], 32 * 1024 * 1024, &mut work, &cancel)
                        .unwrap();
                    let uv = domain
                        .original_coedge_uv_jet(index, [a, a], &mut work, &cancel)
                        .unwrap();
                    assert_eq!(
                        chart.position.map(|v| [v.lo, v.hi]),
                        uv.position.map(|v| [v.lo, v.hi])
                    );
                    assert!(
                        spatial
                            .position
                            .iter()
                            .all(|v| v.lo.is_finite() && v.hi.is_finite())
                    );
                    let end = proof.uv_parameter_image([b, b]).unwrap();
                    assert!(end.lo <= proof.uv_range()[1] && proof.uv_range()[1] <= end.hi);
                    assert!(proof.uv_parameter_image([a - (b - a), b]).is_err());
                    let mut saw_front = false;
                    let mut saw_back = false;
                    domain
                        .visit_original_coedge_span_boundaries(
                            index,
                            &mut work,
                            &cancel,
                            &mut |boundary| {
                                saw_front |= boundary.lo <= a && a <= boundary.hi;
                                saw_back |= boundary.lo <= b && b <= boundary.hi;
                                assert!(a <= boundary.lo && boundary.hi <= b);
                                Ok(())
                            },
                        )
                        .unwrap();
                    assert!(saw_front && saw_back);
                }
                let recipe = domain.material_realization();
                assert_eq!(recipe.collar_parameter_fraction(), 1.0 / 1024.0);
                assert_eq!(recipe.endpoint_support_fraction(), 1.0 / 1024.0);
            }
            let receipts = definition
                .face_boundaries
                .iter()
                .flat_map(|face| &face.wires)
                .map(Vec::capacity)
                .sum::<usize>();
            assert!(
                definition.retained_query_proof_bytes()
                    >= receipts * std::mem::size_of::<super::super::BoundaryCoedgeRef>()
            );
        }
    }

    #[test]
    fn inverse_qualification_without_retained_receipt_refuses_material_authority() {
        let mut definition = original_box("box-mm.step");
        let coedge = &mut definition.face_boundaries[0].wires[0][0];
        coedge.correspondence = None;
        assert!(coedge.source_chart_certificate.is_certified());
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000_000);
        let domain = prepare_face_trim_domain_with_byte_limit(
            &definition,
            0,
            32 * 1024 * 1024,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(
            domain.original_coedge_correspondence(0).err().unwrap().code,
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
        );
        assert_eq!(
            domain
                .original_coedge_uv_jet(0, [0.0, 0.0], &mut work, &cancel)
                .err()
                .unwrap()
                .code,
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
        );
    }
}
