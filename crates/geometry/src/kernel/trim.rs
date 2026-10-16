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
}
impl PreparedFaceTrimDomain<'_> {
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
    })
}
