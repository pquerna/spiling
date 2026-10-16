// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{PreparedCurveEnclosure, debit_native_work, native_error};
use crate::kernel::PreparedFaceTrimDomain;
use crate::section::endpoint_genealogy::SectionPlaneEndpointOrigin;
use crate::{Definition, GeometryError, GeometryWorkBudget};
use monstertruck_modeling::{
    BoundedCurve, Curve, Plane, PreparedFaceParameterDomain, PreparedFacePlaneRoots,
    PreparedValidatedCurve, ValidatedCurveOptions, ValidatedGeometryBudget, ValidatedGeometryError,
    ValidatedPlaneCoverageOptions, ValidatedPlaneCoverageQuery, ValidatedPlaneCoverageWitness,
    ValidatedPlaneTrimBoundaryBinding, ValidatedPlaneTrimEndpoint,
    ValidatedPlaneTrimEndpointBinding,
};
use monstertruck_topology::EdgeId;
use spiling_contracts::geometry::{GeometryErrorCode, SECTION_SAMPLING_TOLERANCE_MM};
use std::{collections::HashMap, hash::Hash, mem::size_of, sync::atomic::AtomicBool};

#[derive(Debug)]
pub(crate) struct SectionPlaneEndpointReceipt {
    pub(crate) origin: SectionPlaneEndpointOrigin,
    pub(crate) native: ValidatedPlaneTrimEndpoint,
}

/// Sealed original endpoint receipts, retained through sampling and integration.
#[derive(Debug)]
pub(crate) struct SectionPlaneCoverage {
    endpoints: Vec<Vec<SectionPlaneEndpointReceipt>>,
    retained_bytes: usize,
}
impl SectionPlaneCoverage {
    pub(crate) fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }
    pub(crate) fn endpoint_strata(&self, candidate: usize) -> &[SectionPlaneEndpointReceipt] {
        &self.endpoints[candidate]
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "Temporary native continuation proofs stay inline; shared original proofs are borrowed without allocation."
)]
enum OriginalCurve<'a> {
    Borrowed(&'a PreparedValidatedCurve<'static>),
    Owned(PreparedValidatedCurve<'static>),
}
struct OriginalBoundary<'a> {
    edge: EdgeId<Curve>,
    range: [f64; 2],
    scale: f64,
    curve: OriginalCurve<'a>,
}
impl OriginalBoundary<'_> {
    fn receipt(&self) -> &PreparedValidatedCurve<'_> {
        match &self.curve {
            OriginalCurve::Borrowed(receipt) => receipt,
            OriginalCurve::Owned(receipt) => receipt,
        }
    }
}
struct OriginalUse {
    boundary: usize,
    chart_error_mm: f64,
}
struct OriginalFace<'a> {
    domain: PreparedFaceTrimDomain<'a>,
    uses: Vec<OriginalUse>,
}

/// Stable immutable owners; query inventories borrow these, never each other.
pub(crate) struct PreparedSectionOriginals<'a> {
    definition: &'a Definition,
    faces: Vec<OriginalFace<'a>>,
    boundaries: Vec<OriginalBoundary<'a>>,
    retained_bytes: usize,
}
impl<'a> PreparedSectionOriginals<'a> {
    pub(crate) fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    /// Material-height/support queries borrow domains but never request plane
    /// roots. Retain the same original owner without preparing unused curves.
    pub(crate) fn prepare_trim_domains(
        definition: &'a Definition,
        byte_limit: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let mut remaining = byte_limit
            .checked_sub(size_of::<Self>())
            .ok_or_else(|| resource("native section original owner exceeds source capacity"))?;
        let mut faces = Vec::new();
        reserve(&mut faces, definition.faces.len(), &mut remaining, work, cancel)?;
        for face_index in 0..definition.faces.len() {
            let before = work.consumed();
            let domain = prepare_original_face_domain(
                definition, face_index, &mut remaining, work, cancel,
            ).map_err(|error| work.annotate_error(
                error, before, format_args!("standalone SourceOriginals source_face={face_index} trim-domain preparation"),
            ))?;
            faces.push(OriginalFace { domain, uses: Vec::new() });
        }
        Ok(Self {
            definition,
            faces,
            boundaries: Vec::new(),
            retained_bytes: byte_limit - remaining,
        })
    }

    pub(crate) fn face_domain(
        &self,
        definition: &Definition,
        face_index: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<&PreparedFaceTrimDomain<'a>, GeometryError> {
        work.charge(1, cancel)?;
        if !std::ptr::eq(self.definition, definition) {
            return Err(unresolved(
                "original trim owner belongs to another immutable definition",
            ));
        }
        let face = self
            .faces
            .get(face_index)
            .ok_or_else(|| unresolved("original trim owner lost its source face"))?;
        let identity = definition
            .face_boundaries
            .get(face_index)
            .ok_or_else(|| unresolved("original trim owner lacks source face correspondence"))?;
        if face.domain.native().face().id() != identity.face {
            return Err(unresolved(
                "original trim owner disagrees with immutable face identity",
            ));
        }
        Ok(&face.domain)
    }

    fn original_use(
        &self,
        origin: &SectionPlaneEndpointOrigin,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(usize, &OriginalUse), GeometryError> {
        work.charge(1, cancel)?;
        if origin.shell_index != 0 {
            return Err(unresolved(
                "native section endpoint requires the admitted original source shell",
            ));
        }
        let face = self
            .faces
            .get(origin.face_index)
            .ok_or_else(|| unresolved("native section endpoint lost its original source face"))?;
        let identity = origin.identity;
        let mut ordinal = identity.coedge_index;
        for wire in face
            .domain
            .native()
            .face()
            .absolute_boundaries()
            .iter()
            .take(identity.wire_index)
        {
            work.charge(1, cancel)?;
            ordinal = ordinal
                .checked_add(wire.len())
                .ok_or_else(|| resource("native section original coedge ordinal overflow"))?;
        }
        let coedge = face
            .domain
            .native()
            .coedges()
            .get(ordinal)
            .ok_or_else(|| unresolved("native section endpoint original coedge is absent"))?;
        let actual = coedge.identity;
        if actual.face_id != identity.face_id
            || actual.edge_id != identity.edge_id
            || actual.wire_index != identity.wire_index
            || actual.coedge_index != identity.coedge_index
            || actual.orientation != identity.orientation
        {
            return Err(unresolved(
                "native section endpoint original oriented coedge identity changed",
            ));
        }
        let use_ = face.uses.get(ordinal).ok_or_else(|| {
            unresolved("native section endpoint lost its retained boundary owner")
        })?;
        Ok((ordinal, use_))
    }
}

fn resource(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, message)
}
fn unresolved(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::AnalysisIndeterminate, message)
}
fn reserve<T>(
    values: &mut Vec<T>,
    capacity: usize,
    remaining: &mut usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    let previous = values.capacity();
    if capacity <= previous {
        return Ok(());
    }
    (capacity - previous)
        .checked_mul(size_of::<T>())
        .filter(|bytes| *bytes <= *remaining)
        .ok_or_else(|| resource("native section preparation exceeds retained capacity"))?;
    values
        .try_reserve_exact(capacity - values.len())
        .map_err(|_| resource("native section preparation allocation failed"))?;
    let growth = (values.capacity() - previous)
        .checked_mul(size_of::<T>())
        .ok_or_else(|| resource("native section preparation byte overflow"))?;
    *remaining = remaining
        .checked_sub(growth)
        .ok_or_else(|| resource("native section preparation exceeds retained capacity"))?;
    Ok(())
}
fn reserve_registry<K: Eq + Hash, V>(
    values: &mut HashMap<K, V>,
    capacity: usize,
    remaining: &mut usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    if capacity == 0 {
        return Ok(());
    }
    let entry_bytes = 2 * (size_of::<(K, V)>() + 32);
    capacity
        .checked_next_power_of_two()
        .and_then(|count| count.checked_mul(2))
        .map(|count| count.max(8))
        .and_then(|count| count.checked_mul(entry_bytes))
        .filter(|bytes| *bytes <= *remaining)
        .ok_or_else(|| resource("native section original registry exceeds capacity"))?;
    values
        .try_reserve(capacity)
        .map_err(|_| resource("native section original registry allocation failed"))?;
    let bytes = values
        .capacity()
        .checked_mul(entry_bytes)
        .ok_or_else(|| resource("native section original registry byte overflow"))?;
    *remaining = remaining
        .checked_sub(bytes)
        .ok_or_else(|| resource("native section original registry exceeds capacity"))?;
    Ok(())
}
fn explicit_source(
    mut curve: &Curve,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for _ in 0..=monstertruck_modeling::MAX_NATIVE_GEOMETRY_NESTING {
        work.charge(1, cancel)?;
        match curve {
            Curve::IntersectionCurve(_) => return Ok(false),
            Curve::TrimmedCurve(wrapper) => curve = wrapper.entity().curve(),
            _ => return Ok(true),
        }
    }
    Err(resource(
        "native section original boundary nesting exceeds frozen cap",
    ))
}

fn prepare_original_face_domain<'a>(
    definition: &'a Definition,
    face_index: usize,
    remaining: &mut usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PreparedFaceTrimDomain<'a>, GeometryError> {
    work.charge(1, cancel)?;
    let domain = crate::kernel::prepare_face_trim_domain_with_byte_limit(
        definition, face_index, *remaining, work, cancel,
    )?;
    // The domain's inline binding already lives in the precharged face vector.
    let domain_heap = domain
        .retained_bytes()
        .checked_sub(size_of::<PreparedFaceParameterDomain<'_>>())
        .ok_or_else(|| unresolved("native section original domain omitted inline storage"))?;
    *remaining = remaining
        .checked_sub(domain_heap)
        .ok_or_else(|| resource("native section original domains exceed source capacity"))?;
    Ok(domain)
}

/// Freeze each original trim domain and distinct boundary preparation once.
/// Profile-only operations retain their existing admission: missing whole-chart
/// correspondence is refused only if a query actually requests native coverage.
pub(crate) fn prepare_section_originals<'a>(
    definition: &'a Definition,
    byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<PreparedSectionOriginals<'a>>, GeometryError> {
    for face in &definition.face_boundaries {
        for wire in &face.wires {
            for use_ in wire {
                work.charge(1, cancel)?;
                if !use_.source_chart_certificate.is_certified() {
                    return Ok(None);
                }
            }
        }
    }
    let mut remaining = byte_limit
        .checked_sub(size_of::<PreparedSectionOriginals<'_>>())
        .ok_or_else(|| resource("native section original owner exceeds source capacity"))?;
    let mut faces = Vec::new();
    reserve(
        &mut faces,
        definition.faces.len(),
        &mut remaining,
        work,
        cancel,
    )?;
    let coedge_count = definition
        .face_boundaries
        .iter()
        .try_fold(0usize, |count, face| {
            face.wires
                .iter()
                .try_fold(count, |count, wire| count.checked_add(wire.len()))
        })
        .ok_or_else(|| resource("native section original coedge count overflow"))?;
    let mut boundaries = Vec::new();
    reserve(&mut boundaries, coedge_count, &mut remaining, work, cancel)?;
    let mut registry = HashMap::new();
    reserve_registry(&mut registry, coedge_count, &mut remaining, work, cancel)?;
    let registry_bytes =
        registry.capacity() * 2 * (size_of::<((EdgeId<Curve>, u64, u64), usize)>() + 32);
    for face_index in 0..definition.faces.len() {
        let domain = prepare_original_face_domain(
            definition, face_index, &mut remaining, work, cancel,
        )?;
        let face = domain.native().face();
        let mut uses = Vec::new();
        reserve(
            &mut uses,
            domain.native().coedges().len(),
            &mut remaining,
            work,
            cancel,
        )?;
        for coedge in domain.native().coedges() {
            work.charge(1, cancel)?;
            let identity = coedge.identity;
            let edge = face
                .absolute_boundaries()
                .get(identity.wire_index)
                .and_then(|wire| wire.get(identity.coedge_index))
                .filter(|edge| edge.id() == identity.edge_id)
                .ok_or_else(|| unresolved("native section original boundary identity changed"))?;
            let range = edge.with_curve(BoundedCurve::range_tuple);
            let key = (identity.edge_id, range.0.to_bits(), range.1.to_bits());
            let boundary = if let Some(&index) = registry.get(&key) {
                index
            } else {
                let (curve, scale) = if let Some(prepared) =
                    definition.original_boundary_curve(identity.edge_id, range, work, cancel)?
                {
                    let receipt = prepared.original_receipt().ok_or_else(|| {
                        unresolved("native section original boundary lost its immutable receipt")
                    })?;
                    (OriginalCurve::Borrowed(receipt), prepared.scale_to_mm())
                } else {
                    let prepared = edge.with_curve(|curve| {
                        if !explicit_source(curve, work, cancel)? {
                            return Err(unresolved("native section implicit original boundary has no retained source receipt"));
                        }
                        let mut native_work = ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
                        let bytes = monstertruck_modeling::curve_owned_bytes(curve, remaining, &mut native_work, cancel);
                        debit_native_work(work, native_work.consumed(), cancel)?;
                        bytes.map_err(native_error)?;
                        work.charge(1, cancel)?;
                        let owned = curve.clone();
                        let mut native_work = ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
                        let result = monstertruck_modeling::prepared_validated_curve_owned(owned, [range.0, range.1],
                            &ValidatedCurveOptions { max_retained_bytes: remaining, ..ValidatedCurveOptions::default() },
                            &mut native_work, cancel);
                        debit_native_work(work, native_work.consumed(), cancel)?;
                        result.map_err(native_error)
                    })?;
                    let heap = prepared
                        .retained_bytes()
                        .checked_sub(size_of::<PreparedValidatedCurve<'static>>())
                        .ok_or_else(|| {
                            unresolved("native section original curve omitted inline storage")
                        })?;
                    remaining = remaining.checked_sub(heap).ok_or_else(|| {
                        resource("native section original curves exceed source capacity")
                    })?;
                    (OriginalCurve::Owned(prepared), 1.0)
                };
                let index = boundaries.len();
                boundaries.push(OriginalBoundary {
                    edge: identity.edge_id,
                    range: [range.0, range.1],
                    scale,
                    curve,
                });
                registry.insert(key, index);
                index
            };
            let source_use = definition
                .face_boundaries
                .get(face_index)
                .filter(|source| source.face == identity.face_id)
                .and_then(|source| source.wires.get(identity.wire_index))
                .and_then(|wire| wire.get(identity.coedge_index))
                .filter(|source| {
                    source.edge == identity.edge_id
                        && (source.orientation == face.orientation()) == identity.orientation
                })
                .ok_or_else(|| {
                    unresolved(
                        "native section original boundary lost its source/chart correspondence",
                    )
                })?;
            uses.push(OriginalUse {
                boundary,
                chart_error_mm: source_use.source_chart_error_mm,
            });
        }
        faces.push(OriginalFace { domain, uses });
    }
    drop(registry);
    remaining = remaining
        .checked_add(registry_bytes)
        .ok_or_else(|| resource("native section original registry byte overflow"))?;
    Ok(Some(PreparedSectionOriginals {
        definition,
        faces,
        boundaries,
        retained_bytes: byte_limit - remaining,
    }))
}

/// Query-local inventory borrows stable original owners and the exact clip plane.
pub(crate) struct PreparedSectionPlaneRoots<'a> {
    originals: &'a PreparedSectionOriginals<'a>,
    plane: &'a Plane,
    roots: Vec<PreparedFacePlaneRoots<'a>>,
    retained_bytes: usize,
}
impl PreparedSectionPlaneRoots<'_> {
    pub(crate) fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }
    pub(crate) fn endpoint_root_chart(
        &self,
        origin: &SectionPlaneEndpointOrigin,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[f64; 2], GeometryError> {
        work.charge(1, cancel)?;
        let (ordinal, _) = self.originals.original_use(origin, work, cancel)?;
        let mut native_work =
            ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
        let result = self.roots[origin.face_index].root_chart_for_cut(
            ordinal,
            origin.source_cut_parameter,
            &mut native_work,
            cancel,
        );
        debit_native_work(work, native_work.consumed(), cancel)?;
        Ok(result
            .map_err(native_error)?
            .map(|interval| interval.midpoint()))
    }
}
pub(crate) fn prepare_section_plane_roots<'a>(
    originals: &'a PreparedSectionOriginals<'a>,
    plane: &'a Plane,
    byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PreparedSectionPlaneRoots<'a>, GeometryError> {
    let byte_limit = byte_limit.min(32 * 1024 * 1024);
    let mut remaining = byte_limit
        .checked_sub(size_of::<PreparedSectionPlaneRoots<'_>>())
        .ok_or_else(|| resource("native section root owner exceeds query capacity"))?;
    let mut roots = Vec::new();
    reserve(
        &mut roots,
        originals.faces.len(),
        &mut remaining,
        work,
        cancel,
    )?;
    for (face_index, face) in originals.faces.iter().enumerate() {
        let mut face_remaining = remaining;
        let mut bindings = Vec::new();
        reserve(
            &mut bindings,
            face.uses.len(),
            &mut face_remaining,
            work,
            cancel,
        )?;
        for (coedge_index, use_) in face.uses.iter().enumerate() {
            work.charge(1, cancel)?;
            let boundary = &originals.boundaries[use_.boundary];
            bindings.push(ValidatedPlaneTrimBoundaryBinding {
                coedge_index,
                original_edge_id: boundary.edge,
                boundary: boundary.receipt(),
                boundary_range: boundary.range,
                boundary_scale: boundary.scale,
                source_chart_error_mm: use_.chart_error_mm,
            });
        }
        let mut native_work =
            ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
        let result = monstertruck_modeling::prepared_face_plane_roots(
            face.domain.native(),
            plane,
            &bindings,
            face_remaining,
            &mut native_work,
            cancel,
        );
        debit_native_work(work, native_work.consumed(), cancel)?;
        let inventory = result.map_err(|cause| {
            let mut error = native_error(cause);
            error.message = format!(
                "plane coverage face {face_index} / original root preparation: {}",
                error.message
            );
            error
        })?;
        let heap = inventory
            .retained_bytes()
            .checked_sub(size_of::<PreparedFacePlaneRoots<'_>>())
            .ok_or_else(|| unresolved("native section root inventory omitted inline storage"))?;
        remaining = remaining
            .checked_sub(heap)
            .ok_or_else(|| resource("native section root inventories exceed query capacity"))?;
        roots.push(inventory);
    }
    Ok(PreparedSectionPlaneRoots {
        originals,
        plane,
        roots,
        retained_bytes: byte_limit - remaining,
    })
}

/// Coverage always reruns actual candidate association, interior continuation and
/// endpoint sealing. The original roots alone never authorize a section.
pub(crate) fn certify_section_plane_coverage(
    definition: &Definition,
    roots: &PreparedSectionPlaneRoots<'_>,
    curves: &[PreparedCurveEnclosure<'_>],
    origins: &[SectionPlaneEndpointOrigin],
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SectionPlaneCoverage, GeometryError> {
    work.charge(1, cancel)?;
    if !std::ptr::eq(definition, roots.originals.definition) {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "native section root inventory belongs to a different immutable definition",
        ));
    }
    let plane = roots.plane;
    let byte_limit = remaining_bytes.min(32 * 1024 * 1024);
    curves
        .len()
        .checked_mul(size_of::<&PreparedValidatedCurve<'_>>())
        .filter(|bytes| *bytes <= 4 * 1024 * 1024 && *bytes <= byte_limit)
        .ok_or_else(|| resource("native section coverage references exceed scratch capacity"))?;
    let mut remaining = byte_limit;
    let mut counts = Vec::new();
    reserve(&mut counts, curves.len(), &mut remaining, work, cancel)?;
    counts.resize(curves.len(), 0usize);
    for origin in origins {
        work.charge(1, cancel)?;
        if origin.shell_index != 0 || origin.face_index >= definition.faces.len() {
            return Err(unresolved(
                "original endpoint genealogy refers to an absent source-face use",
            ));
        }
        let count = counts.get_mut(origin.candidate_index).ok_or_else(|| {
            unresolved("original endpoint genealogy refers to an absent section candidate")
        })?;
        *count = count
            .checked_add(1)
            .ok_or_else(|| resource("native section endpoint count overflow"))?;
    }
    let mut endpoints = Vec::new();
    reserve(&mut endpoints, curves.len(), &mut remaining, work, cancel)?;
    for &count in &counts {
        let mut receipts = Vec::new();
        reserve(&mut receipts, count, &mut remaining, work, cancel)?;
        endpoints.push(receipts);
    }
    let count_bytes = counts.capacity() * size_of::<usize>();
    drop(counts);
    remaining += count_bytes;
    let retained_bytes = byte_limit - remaining;
    let mut candidates = Vec::new();
    reserve(&mut candidates, curves.len(), &mut remaining, work, cancel)?;
    if candidates.capacity() * size_of::<&PreparedValidatedCurve<'_>>() > 4 * 1024 * 1024 {
        return Err(resource(
            "native section coverage references exceed scratch capacity",
        ));
    }
    for curve in curves {
        work.charge(1, cancel)?;
        candidates.push(curve.native_receipt());
    }
    for (face_index, face) in roots.originals.faces.iter().enumerate() {
        work.charge(1, cancel)?;
        let mut face_remaining = remaining;
        let mut binding_count = 0usize;
        for origin in origins {
            work.charge(1, cancel)?;
            if origin.face_index == face_index {
                binding_count += 1;
            }
        }
        let mut bindings = Vec::new();
        reserve(
            &mut bindings,
            binding_count,
            &mut face_remaining,
            work,
            cancel,
        )?;
        let mut binding_origins = HashMap::new();
        reserve_registry(
            &mut binding_origins,
            binding_count,
            &mut face_remaining,
            work,
            cancel,
        )?;
        for (origin_index, origin) in origins.iter().enumerate() {
            work.charge(1, cancel)?;
            if origin.face_index != face_index {
                continue;
            }
            let (coedge_index, use_) = roots.originals.original_use(origin, work, cancel)?;
            let key = (
                origin.candidate_index,
                origin.endpoint_is_back,
                coedge_index,
                origin.identity.edge_id,
            );
            if binding_origins.insert(key, origin_index).is_some() {
                return Err(unresolved(
                    "native section endpoint has ambiguous original binding",
                ));
            }
            let boundary = &roots.originals.boundaries[use_.boundary];
            bindings.push(ValidatedPlaneTrimEndpointBinding {
                coedge_index,
                original_edge_id: boundary.edge,
                boundary: boundary.receipt(),
                boundary_range: boundary.range,
                boundary_scale: boundary.scale,
                candidate_index: origin.candidate_index,
                endpoint_is_back: origin.endpoint_is_back,
                source_cut_parameter: origin.source_cut_parameter,
                source_chart_error_mm: use_.chart_error_mm,
            });
        }
        let mut native_work =
            ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
        let result = monstertruck_modeling::validated_face_plane_coverage(
            face.domain.native(),
            &ValidatedPlaneCoverageQuery {
                plane,
                candidates: &candidates,
                endpoint_bindings: &bindings,
                prepared_roots: Some(&roots.roots[face_index]),
            },
            &ValidatedPlaneCoverageOptions {
                maximum_endpoint_extension_mm: if bindings.is_empty() {
                    0.0
                } else {
                    SECTION_SAMPLING_TOLERANCE_MM
                },
                max_retained_bytes: face_remaining,
                ..ValidatedPlaneCoverageOptions::default()
            },
            &mut native_work,
            cancel,
        );
        debit_native_work(work, native_work.consumed(), cancel)?;
        let result = result.map_err(|cause| {
            let mut error = native_error(cause);
            if let ValidatedGeometryError::PlaneCoverageUnresolved {
                witness: Some(ValidatedPlaneCoverageWitness::Endpoint(witness)), ..
            } = cause
                && let Ok(candidate_index) = usize::try_from(witness.candidate_index)
                && let Some(candidate) = candidates.get(candidate_index)
            {
                // Failure context reads retained records, never proposes another root.
                let source_index = usize::from(witness.source_index);
                let end_tubes = [candidate.pieces().first(), candidate.pieces().last()].map(|piece| {
                    piece.and_then(|piece| {
                        let uv = piece.source_parameters?.get(source_index).copied()?;
                        Some((piece.parameter, uv.map(|value| [value.lower, value.upper])))
                    })
                });
                let failed_coedge = usize::try_from(witness.coedge_index).ok();
                let end_bindings = [false, true].map(|back| {
                    bindings.iter().find(|binding| binding.candidate_index == candidate_index
                        && binding.endpoint_is_back == back
                        && (back != witness.endpoint_is_back
                            || Some(binding.coedge_index) == failed_coedge)).map(|binding| {
                        let use_ = face.domain.native().coedges().get(binding.coedge_index).map(|coedge| {
                            let linear_uv = match coedge.curve {
                                monstertruck_modeling::Curve2D::Line(line) => Some([
                                    [line.0.x, line.0.y], [line.1.x, line.1.y],
                                ]),
                                _ => None,
                            };
                            (coedge.identity.wire_index, coedge.identity.coedge_index,
                                coedge.identity.orientation, coedge.oriented_range, linear_uv)
                        });
                        (binding.coedge_index, binding.source_cut_parameter,
                            binding.boundary_range, binding.source_chart_error_mm, use_)
                    })
                });
                let original_ends_present = [end_bindings[0].is_some(), end_bindings[1].is_some()];
                error.message = format!(
                    "plane coverage face {face_index} / endpoint context: domain={:?}, face_orientation={}, \
                     original_origin_present(front/back)={original_ends_present:?}, \
                     front/back(parameter,sourceUV)={end_tubes:?}, \
                     front/back(coedge,source_cut,source_range,chart_error,\
                     use(wire,coedge,material_orientation,oriented_range,linearUV))={end_bindings:?}; native: {}",
                    face.domain.native().closed_parameter_box(),
                    face.domain.native().face().orientation(), error.message,
                );
            } else {
                error.message = format!("plane coverage face {face_index} / whole-domain roots: {}", error.message);
            }
            error
        })?;
        if result.endpoint_strata.len() > binding_count {
            return Err(unresolved(
                "native section coverage returned unbound endpoint strata",
            ));
        }
        for native in result.endpoint_strata {
            work.charge(1, cancel)?;
            let key = (
                native.candidate_index(),
                native.endpoint_is_back(),
                native.coedge_index(),
                native.original_edge_id(),
            );
            let origin_index = binding_origins.remove(&key).ok_or_else(|| {
                unresolved("native section endpoint receipt lost its unique original binding")
            })?;
            let origin = origins[origin_index];
            let receipts = endpoints.get_mut(origin.candidate_index).ok_or_else(|| {
                unresolved("native section endpoint receipt refers to an absent candidate")
            })?;
            if receipts.len() == receipts.capacity() {
                return Err(resource(
                    "native section endpoint receipts exceed reserved coexistence capacity",
                ));
            }
            receipts.push(SectionPlaneEndpointReceipt { origin, native });
        }
    }
    Ok(SectionPlaneCoverage {
        endpoints,
        retained_bytes,
    })
}
