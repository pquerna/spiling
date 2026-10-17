// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

pub(crate) mod endpoint_genealogy;
mod loops;
pub(crate) use loops::{canonicalize_section_in_place, sample_original_face};
#[cfg(test)]
mod tests;

use crate::{
    Definition, GeometryError, GeometryWorkBudget, NativeSection, PlaneFrameMm, PlaneMm,
    check_cancel,
};
use monstertruck_modeling::{
    BoundingBox, Curve, Matrix4, Point3, Solid, Surface, builder, primitive,
};
use monstertruck_solid::{
    BooleanOperand, BooleanPreparationOptions, BooleanWorkBudget, BooleanWorkLimits,
    PreparedBooleanSource, ShapeOpsError,
};
use spiling_contracts::geometry::{
    GeometryErrorCode, MAX_SECTION_BYTES, SECTION_BOOLEAN_TOLERANCE_MM, SECTION_PLANE_TOLERANCE_MM,
    SECTION_SAMPLING_TOLERANCE_MM,
};
use std::{collections::HashSet, sync::atomic::AtomicBool};

const PARALLEL_TOLERANCE: f64 = 1e-10;

/// Shared pinned-proof, native source seed and query/output admission per job.
pub const MAX_PREPARED_SECTION_BYTES: usize = 64 * 1024 * 1024;
const MAX_QUERY_SEED_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PREPARED_SECTION_SOURCE_BYTES: usize =
    MAX_PREPARED_SECTION_BYTES - MAX_QUERY_SEED_BYTES;
// Search preparation is independent of renderer/packed-display admission.
const MAX_NATIVE_SEED_VERTICES: usize = 1_000_000;
const MAX_NATIVE_SEED_TRIANGLES: usize = 1_000_000;

/// Job-local search seeds and original trim proofs borrowing one immutable definition.
///
/// Seeds accelerate native intersections; they never certify section geometry.
/// Owned domains and explicit boundaries remain stable for borrowed query roots.
pub struct PreparedSectionSource<'a> {
    definition: &'a Definition,
    native: PreparedBooleanSource<'a, Curve, Surface>,
    originals: Option<crate::parametric::PreparedSectionOriginals<'a>>,
}

/// A query may request stricter normal accuracy, never relax the frozen policy.
pub(crate) struct SectionQuery {
    pub(crate) plane: PlaneMm,
    pub(crate) maximum_plane_error_mm: f64,
}

impl SectionQuery {
    fn standard(plane: PlaneMm) -> Self {
        Self {
            plane,
            maximum_plane_error_mm: SECTION_PLANE_TOLERANCE_MM,
        }
    }
}

/// Immutable original authority plus query-local admission, never a geometric cache.
#[derive(Clone, Copy)]
pub(crate) struct OriginalTrimSource<'s, 'd> {
    definition: &'d Definition,
    originals: Option<&'s crate::parametric::PreparedSectionOriginals<'d>>,
    vertex_plans: Option<&'s crate::analysis::PreparedSourceVertexPlans<'d>>,
}

impl<'s, 'd> OriginalTrimSource<'s, 'd> {
    pub(crate) fn standalone(definition: &'d Definition) -> Self {
        Self {
            definition,
            originals: None,
            vertex_plans: None,
        }
    }

    pub(crate) fn definition(self) -> &'d Definition {
        self.definition
    }

    pub(crate) fn vertex_plans(self) -> Option<&'s crate::analysis::PreparedSourceVertexPlans<'d>> {
        self.vertex_plans
    }

    pub(crate) fn borrowing_vertex_plans(
        self,
        vertex_plans: &'s crate::analysis::PreparedSourceVertexPlans<'d>,
    ) -> Self {
        Self {
            vertex_plans: Some(vertex_plans),
            ..self
        }
    }

    /// A standalone full query owns preparation once; prepared sources already
    /// account for these immutable domains and must not prepare or debit again.
    pub(crate) fn prepare_domains_if_missing(
        self,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<crate::parametric::PreparedSectionOriginals<'d>>, GeometryError> {
        if self.originals.is_some() {
            return Ok(None);
        }
        let before = work.consumed();
        crate::parametric::PreparedSectionOriginals::prepare_trim_domains(
            self.definition,
            remaining_bytes,
            work,
            cancel,
        )
        .map(Some)
        .map_err(|error| {
            work.annotate_error(error, before, "standalone SourceOriginals preparation")
        })
    }

    pub(crate) fn borrowing_domains(
        self,
        originals: Option<&'s crate::parametric::PreparedSectionOriginals<'d>>,
    ) -> Self {
        Self {
            definition: self.definition,
            originals: self.originals.or(originals),
            vertex_plans: self.vertex_plans,
        }
    }

    pub(crate) fn face_domain(
        self,
        index: usize,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<FaceTrimQuery<'s, 'd>, GeometryError> {
        work.charge(1, cancel)?;
        let domain = match self.originals {
            Some(originals) => FaceTrimStorage::Borrowed(originals.face_domain(
                self.definition,
                index,
                work,
                cancel,
            )?),
            None => {
                FaceTrimStorage::Owned(crate::kernel::prepare_face_trim_domain_with_byte_limit(
                    self.definition,
                    index,
                    remaining_bytes,
                    work,
                    cancel,
                )?)
            }
        };
        let mut query = FaceTrimQuery {
            domain,
            byte_limit: usize::MAX,
        };
        query.restrict_retained_byte_limit(remaining_bytes, work, cancel)?;
        Ok(query)
    }
}

enum FaceTrimStorage<'s, 'd> {
    Borrowed(&'s crate::kernel::PreparedFaceTrimDomain<'d>),
    Owned(crate::kernel::PreparedFaceTrimDomain<'d>),
}

pub(crate) struct FaceTrimQuery<'s, 'd> {
    domain: FaceTrimStorage<'s, 'd>,
    byte_limit: usize,
}

impl<'d> std::ops::Deref for FaceTrimQuery<'_, 'd> {
    type Target = crate::kernel::PreparedFaceTrimDomain<'d>;
    fn deref(&self) -> &Self::Target {
        match &self.domain {
            FaceTrimStorage::Borrowed(domain) => domain,
            FaceTrimStorage::Owned(domain) => domain,
        }
    }
}

impl FaceTrimQuery<'_, '_> {
    /// Borrowed metadata is already included in the prepared source allowance.
    pub(crate) fn owned_bytes(&self) -> usize {
        match &self.domain {
            FaceTrimStorage::Borrowed(_) => 0,
            FaceTrimStorage::Owned(domain) => domain.retained_bytes(),
        }
    }

    pub(crate) fn restrict_retained_byte_limit(
        &mut self,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        let metadata_bytes = match &self.domain {
            FaceTrimStorage::Borrowed(domain) => domain.retained_bytes(),
            FaceTrimStorage::Owned(_) => 0,
        };
        let limit = remaining_bytes
            .checked_add(metadata_bytes)
            .ok_or_else(|| resource_error("original trim query capacity overflow"))?;
        if limit < self.retained_bytes() {
            return Err(resource_error(
                "original trim query metadata exceeds capacity",
            ));
        }
        self.byte_limit = self.byte_limit.min(limit);
        Ok(())
    }

    pub(crate) fn classify(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<crate::kernel::TrimClassification, GeometryError> {
        self.classify_with_byte_limit(uv, self.byte_limit, work, cancel)
    }
}

// Native and original retained_bytes already include their inline bindings.
// Count only the enclosing source's remaining fields, padding and Option tag.
fn source_header_bytes(has_originals: bool) -> usize {
    std::mem::size_of::<PreparedSectionSource<'_>>()
        - std::mem::size_of::<PreparedBooleanSource<'_, Curve, Surface>>()
        - if has_originals {
            std::mem::size_of::<crate::parametric::PreparedSectionOriginals<'_>>()
        } else {
            0
        }
}

impl<'d> PreparedSectionSource<'d> {
    pub(crate) fn definition(&self) -> &'d Definition {
        self.definition
    }

    pub(crate) fn trim_source(&self) -> OriginalTrimSource<'_, 'd> {
        OriginalTrimSource {
            definition: self.definition,
            originals: self.originals.as_ref(),
            vertex_plans: None,
        }
    }

    /// Owned seeds, original preparations and the definition's pinned query proofs.
    ///
    /// Other borrowed BREP, allocator bookkeeping and transient process RSS are
    /// excluded. Count this once per distinct definition in a job.
    pub fn retained_bytes(&self) -> usize {
        self.native
            .retained_bytes()
            .saturating_add(self.definition.retained_query_proof_bytes())
            .saturating_add(
                self.originals
                    .as_ref()
                    .map_or(0, |source| source.retained_bytes()),
            )
            .saturating_add(source_header_bytes(self.originals.is_some()))
    }

    /// Apply the same native regularity, topology and sampled-output gates.
    ///
    /// `byte_limit` is remaining query scratch/output capacity after the caller
    /// subtracts all retained prepared sources, including their pinned proofs,
    /// and any other coexisting owned query/output buffers. It only narrows the
    /// shared 64 MiB pool; the independent packed-output cap remains 16 MiB.
    pub fn section_with_byte_limit(
        &self,
        plane: PlaneMm,
        cancel: &AtomicBool,
        byte_limit: u32,
        work: &mut GeometryWorkBudget,
    ) -> Result<NativeSection, GeometryError> {
        self.section_for_query(&SectionQuery::standard(plane), cancel, byte_limit, work)
    }

    pub(crate) fn section_for_query(
        &self,
        query: &SectionQuery,
        cancel: &AtomicBool,
        byte_limit: u32,
        work: &mut GeometryWorkBudget,
    ) -> Result<NativeSection, GeometryError> {
        section_using(self.definition, query, cancel, byte_limit, Some(self), work)
    }
}

/// Prepare native search seeds within a total source allowance.
///
/// Candidate meshing uses the frozen section sampling resolution, independently
/// of the unchanged native Boolean geometric tolerance. Search seeds never
/// certify returned geometry: every crossing section proves actual native curve
/// enclosures and whole-face plane-intersection coverage before acceptance.
/// The allowance includes pinned definition proofs, reserved before growth;
/// the frozen source cap leaves independent query headroom. Native tessellation
/// is indivisible; these admission limits are not RSS limits.
pub fn prepare_section_source<'a>(
    definition: &'a Definition,
    remaining_prepared_bytes: usize,
    cancel: &AtomicBool,
    work: &mut GeometryWorkBudget,
) -> Result<PreparedSectionSource<'a>, GeometryError> {
    work.charge(1, cancel)?;
    let pinned_bytes = definition.retained_query_proof_bytes();
    let source_allowance = remaining_prepared_bytes.min(MAX_PREPARED_SECTION_SOURCE_BYTES);
    let original_allowance = source_allowance
        .checked_sub(pinned_bytes)
        .and_then(|bytes| bytes.checked_sub(source_header_bytes(true)))
        .filter(|bytes| *bytes > 0)
        .ok_or_else(|| resource_error("prepared native section capacity exhausted"))?;
    let originals =
        crate::parametric::prepare_section_originals(definition, original_allowance, work, cancel)?;
    prepare_section_source_from_originals(definition, originals, source_allowance, cancel, work)
}

/// Move already admitted original domains into native section preparation.
pub(crate) fn prepare_section_source_from_originals<'a>(
    definition: &'a Definition,
    originals: Option<crate::parametric::PreparedSectionOriginals<'a>>,
    remaining_prepared_bytes: usize,
    cancel: &AtomicBool,
    work: &mut GeometryWorkBudget,
) -> Result<PreparedSectionSource<'a>, GeometryError> {
    work.charge(1, cancel)?;
    let pinned_bytes = definition.retained_query_proof_bytes();
    let source_allowance = remaining_prepared_bytes.min(MAX_PREPARED_SECTION_SOURCE_BYTES);
    let original_bytes = originals.as_ref().map_or(0, |source| source.retained_bytes());
    let header_bytes = source_header_bytes(originals.is_some());
    let native_source_bytes = source_allowance
        .checked_sub(pinned_bytes)
        .and_then(|bytes| bytes.checked_sub(header_bytes))
        .and_then(|bytes| bytes.checked_sub(original_bytes))
        .ok_or_else(|| resource_error("prepared original section capacity exhausted"))?;
    // The caller's source allowance already excludes coexisting owners/plans.
    // Preserve only the frozen independent query headroom above that allowance.
    let native_prepared_bytes = source_allowance
        .saturating_add(MAX_PREPARED_SECTION_BYTES - MAX_PREPARED_SECTION_SOURCE_BYTES)
        .checked_sub(pinned_bytes)
        .and_then(|bytes| bytes.checked_sub(original_bytes))
        .and_then(|bytes| bytes.checked_sub(header_bytes))
        .ok_or_else(|| resource_error("prepared native section proof capacity exhausted"))?;
    for face in definition.solid.face_iter() {
        work.charge(1, cancel)?;
        for _ in face.edge_iter() {
            work.charge(1, cancel)?;
        }
    }
    let limits = BooleanWorkLimits {
        max_faces: spiling_contracts::geometry::MAX_NATIVE_FACES as usize + 6,
        max_edges: 4 * spiling_contracts::geometry::MAX_STEP_ENTITIES as usize,
        max_mesh_vertices: MAX_NATIVE_SEED_VERTICES,
        max_mesh_triangles: MAX_NATIVE_SEED_TRIANGLES,
        max_triangle_pairs: 20_000_000,
        max_interference_segments: 1_000_000,
        max_output_faces: spiling_contracts::geometry::MAX_NATIVE_FACES as usize + 6,
        max_output_edges: 4 * spiling_contracts::geometry::MAX_STEP_ENTITIES as usize,
        max_prepared_bytes: native_prepared_bytes,
        max_source_prepared_bytes: native_source_bytes,
        max_other_prepared_bytes: MAX_QUERY_SEED_BYTES,
        max_intermediate_prepared_bytes: native_prepared_bytes,
    };
    let native_work =
        BooleanWorkBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
    let native = monstertruck_solid::prepare_boolean_source_counted_with_options(
        &definition.solid,
        BooleanPreparationOptions {
            geometric_tolerance: SECTION_BOOLEAN_TOLERANCE_MM,
            seed_tolerance: SECTION_SAMPLING_TOLERANCE_MM,
        },
        limits,
        cancel,
        &native_work,
    );
    crate::parametric::debit_native_work(work, native_work.consumed(), cancel)?;
    let native = native.map_err(native_error)?;
    Ok(PreparedSectionSource {
        definition,
        native,
        originals,
    })
}

pub(crate) fn section(
    definition: &Definition,
    plane: PlaneMm,
    cancel: &AtomicBool,
    byte_limit: u32,
    work: &mut GeometryWorkBudget,
) -> Result<NativeSection, GeometryError> {
    section_using(
        definition,
        &SectionQuery::standard(plane),
        cancel,
        byte_limit,
        None,
        work,
    )
}

fn section_using(
    definition: &Definition,
    query: &SectionQuery,
    cancel: &AtomicBool,
    byte_limit: u32,
    prepared: Option<&PreparedSectionSource<'_>>,
    work: &mut GeometryWorkBudget,
) -> Result<NativeSection, GeometryError> {
    work.charge(1, cancel)?;
    let plane = query.plane;
    if !query.maximum_plane_error_mm.is_finite()
        || query.maximum_plane_error_mm <= 0.0
        || query.maximum_plane_error_mm > SECTION_PLANE_TOLERANCE_MM
    {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "section plane error must narrow the frozen sampling allowance",
        ));
    }
    if let Some(source) = prepared
        && !std::ptr::eq(source.definition, definition)
    {
        return Err(kernel_error(
            "prepared native section source belongs to a different immutable definition",
        ));
    }
    let packed_byte_limit = byte_limit.min(MAX_SECTION_BYTES);
    let mut budget = loops::SectionBudget::with_limit(packed_byte_limit)?;
    // Packed output does not own native preparation admission. Direct queries
    // use the shared frozen pool and reserve pinned proofs here; prepared
    // callers already charged every retained source, including those proofs.
    let pinned_bytes = definition.retained_query_proof_bytes();
    let query_byte_limit = match prepared {
        Some(source) => MAX_PREPARED_SECTION_BYTES
            .checked_sub(source.retained_bytes())
            .map(|bytes| bytes.min(byte_limit as usize)),
        None => MAX_PREPARED_SECTION_BYTES.checked_sub(pinned_bytes),
    }
    .ok_or_else(|| resource_error("native section proof exceeds query capacity"))?;
    if definition.id().allocated_bytes() > query_byte_limit {
        return Err(resource_error(
            "native section certificate exceeds query capacity",
        ));
    }
    let plane = plane.normalized()?;
    let frame = PlaneFrameMm::from_plane(plane)?;
    let mut result = NativeSection {
        plane,
        frame,
        loops: Vec::new(),
        loop_source_faces: Vec::new(),
        sampling_tolerance_mm: SECTION_SAMPLING_TOLERANCE_MM,
        boolean_tolerance_mm: SECTION_BOOLEAN_TOLERANCE_MM,
        certificate: crate::NativeSectionCertificate {
            definition_id: definition.id().clone(),
            authority: crate::SectionAuthority::NativeIntersection,
            maximum_chord_error_mm: 0.0,
            maximum_plane_error_mm: 0.0,
        },
    };
    let query_remaining = query_byte_limit
        .checked_sub(result.retained_bytes())
        .ok_or_else(|| resource_error("native section certificate exceeds query capacity"))?;
    // Scratch already excludes the pinned proof and result certificate.
    let regularity = crate::analysis::section_regularity(
        prepared.map_or_else(
            || OriginalTrimSource::standalone(definition),
            |source| source.trim_source(),
        ),
        &plane,
        query_remaining,
        work,
        cancel,
    );
    if matches!(regularity?, crate::analysis::SectionRegularity::Empty) {
        return Ok(result);
    }

    // Native regularity analysis establishes crossing; all certified AABB
    // corners establish a safe finite clipping box, including interior extrema.
    let (min, max) = clip_bounds(definition, &frame)?;
    let clip: Solid = primitive::cuboid(BoundingBox::from_iter([
        Point3::new(min[0], min[1], min[2]),
        Point3::new(max[0], max[1], max[2]),
    ]));
    let clip = builder::transformed(&clip, frame_to_definition(&frame));
    for shell in clip.boundaries() {
        shell
            .check_solid_boundary()
            .map_err(|_| kernel_error("invalid native clipping cuboid"))?;
    }
    let mut clip_face = None;
    for face in clip.face_iter() {
        let candidate = face.with_surface(|surface| -> Result<_, GeometryError> {
            let Surface::Plane(carrier) = surface else {
                return Ok(None);
            };
            Ok(
                if parallel(carrier.normal().into(), plane.normal)
                    && signed_distance(carrier.origin().into(), &plane)?.abs()
                        <= SECTION_PLANE_TOLERANCE_MM
                {
                    Some(*carrier)
                } else {
                    None
                },
            )
        })?;
        if let Some(carrier) = candidate {
            clip_face = Some((face.id(), carrier));
            break;
        }
    }
    let (clip_face, clip_plane) =
        clip_face.ok_or_else(|| kernel_error("native clipping cuboid has no plane-side face"))?;
    check_cancel(cancel)?;
    let mut owned = None;
    let source = match prepared {
        Some(source) => source,
        None => {
            // The constructor accepts a total source allowance, whereas
            // query_remaining already excludes the pinned proof. Restore that
            // proof once, while preserving the frozen native clip headroom.
            let source_allowance = query_remaining
                .checked_sub(MAX_QUERY_SEED_BYTES)
                .and_then(|bytes| bytes.checked_add(pinned_bytes))
                .ok_or_else(|| resource_error("native section query seed capacity exhausted"))?;
            owned.insert(prepare_section_source(
                definition,
                source_allowance,
                cancel,
                work,
            )?)
        }
    };
    work.charge(1, cancel)?;
    let native_query_remaining = if prepared.is_none() {
        query_remaining
            .checked_sub(source.retained_bytes().saturating_sub(pinned_bytes))
            .ok_or_else(|| resource_error("native section source exceeds query capacity"))?
    } else {
        query_remaining
    };
    let native_work =
        BooleanWorkBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
    let cut = monstertruck_solid::and_prepared_with_provenance_counted_with_query_bytes(
        &source.native,
        &clip,
        cancel,
        &native_work,
        native_query_remaining,
    );
    crate::parametric::debit_native_work(work, native_work.consumed(), cancel)?;
    let cut = cut.map_err(native_error)?;
    check_cancel(cancel)?;
    for shell in cut.solid.boundaries() {
        check_cancel(cancel)?;
        shell
            .check_solid_boundary()
            .map_err(|_| kernel_error("native section boolean returned invalid solid topology"))?;
    }
    let mut cap_faces = HashSet::new();
    let scratch_limit = native_query_remaining.min(loops::MAX_CURVE_CAPTURE_BYTES);
    let mut cap_bytes = 0;
    for provenance in &cut.face_origins {
        work.charge(1, cancel)?;
        if provenance.source.operand == BooleanOperand::Other
            && provenance.source.face_id == clip_face
        {
            cap_bytes = insert_cap_face(&mut cap_faces, provenance.output_face, scratch_limit)?;
        }
    }
    let capture_limit = scratch_limit
        .checked_sub(cap_bytes)
        .ok_or_else(|| resource_error("native cap index exceeds query capacity"))?;
    let captures =
        loops::CapturedCurves::collect(definition, &cut, &cap_faces, capture_limit, work, cancel)
            .map_err(|mut error| {
            error.message = format!(
                "native section / collect original curve owners: {}",
                error.message
            );
            error
        })?;
    let originals = source.originals.as_ref().ok_or_else(|| {
        GeometryError::new(
            spiling_contracts::geometry::GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "native section requires whole-coedge source/chart correspondence",
        )
    })?;
    let roots = crate::parametric::prepare_section_plane_roots(
        originals,
        &clip_plane,
        captures.remaining_bytes(),
        work,
        cancel,
    )?;
    let root_bytes = roots.retained_bytes();
    let (prepared_curves, remaining_bytes) = captures
        .prepare(definition, &clip_plane, &roots, root_bytes, work, cancel)
        .map_err(|mut error| {
            error.message = format!(
                "native section / prepare original curve roots: {}",
                error.message
            );
            error
        })?;
    let capture_bytes = capture_limit - remaining_bytes - root_bytes;
    // Full original-root coverage precedes sampling: sealed endpoint extensions
    // must spend the same frozen chord/plane and signed-area allowances.
    let coverage = crate::parametric::certify_section_plane_coverage(
        definition,
        &roots,
        &prepared_curves,
        captures.endpoint_origins(),
        remaining_bytes,
        work,
        cancel,
    )
    .map_err(|mut error| {
        error.message = format!(
            "native section / complete original plane coverage: {}",
            error.message
        );
        error
    })?;
    let coverage_bytes = coverage.retained_bytes();
    // Inventories are query-local and can be released once every endpoint is sealed.
    drop(roots);
    let remaining_bytes = remaining_bytes
        .checked_sub(coverage_bytes)
        .and_then(|bytes| bytes.checked_add(root_bytes))
        .ok_or_else(|| resource_error("native endpoint receipts exceed query capacity"))?;

    let mut boundaries = Vec::new();
    let mut sampling_work = loops::SamplingWork::new(work, remaining_bytes);
    // The native loop budget narrows to remaining job cache/output capacity.
    for face in cut.solid.face_iter() {
        check_cancel(cancel)?;
        // Boolean genealogy, not carrier resemblance or coincidence, owns cap
        // identity. Nearby source faces must never become generated boundaries.
        if !cap_faces.contains(&face.id()) {
            continue;
        }
        let (normal, origin): ([f64; 3], [f64; 3]) =
            face.with_surface(|surface| match surface {
                Surface::Plane(carrier) => Ok((carrier.normal().into(), carrier.origin().into())),
                _ => Err(kernel_error(
                    "native clipping-plane descendant is not planar",
                )),
            })?;
        if !parallel(normal, plane.normal)
            || signed_distance(origin, &plane)?.abs() > SECTION_PLANE_TOLERANCE_MM
        {
            return Err(kernel_error(
                "native cap provenance disagrees with clipping plane",
            ));
        }
        // Source coplanar faces were rejected before the Boolean. Generated
        // cap genealogy is private and never manufactures a source FaceId.
        // Absolute wires and the raw carrier normal share an orientation.
        // Their un-inverted signs preserve outer/hole classification even on
        // an inverted face, without cloning/inverting boundary topology.
        let face_sign = dot(normal, plane.normal);
        let mut outer_count = 0;
        for wire in face.absolute_boundaries() {
            check_cancel(cancel)?;
            sampling_work.reserve_loop(&mut boundaries)?;
            let boundary = loops::sample_wire(
                wire,
                &loops::WireSampling {
                    frame: &frame,
                    maximum_plane_error_mm: query.maximum_plane_error_mm,
                    face_sign,
                    budget: &budget,
                    captures: &captures,
                    prepared: &prepared_curves,
                    coverage: &coverage,
                },
                &mut sampling_work,
                cancel,
            )
            .map_err(|mut error| {
                error.message =
                    format!("native section / sample native cap wire: {}", error.message);
                error
            })?;
            if !boundary.is_hole() {
                outer_count += 1;
            }
            budget.push_loop(boundary.point_count())?;
            boundaries.push(boundary);
            check_cancel(cancel)?;
        }
        if outer_count == 0 {
            return Err(kernel_error("native section cap has no outer boundary"));
        }
    }
    let [maximum_chord_error_mm, maximum_plane_error_mm] = sampling_work.error_bounds();
    let remaining_bytes = sampling_work.remaining_bytes();
    drop(cut);
    drop(cap_faces);
    result.certificate.maximum_chord_error_mm = maximum_chord_error_mm;
    result.certificate.maximum_plane_error_mm = maximum_plane_error_mm;
    drop(prepared_curves);
    drop(captures);
    drop(coverage);
    let transfer_remaining = remaining_bytes
        .checked_add(cap_bytes)
        .and_then(|bytes| bytes.checked_add(capture_bytes))
        .and_then(|bytes| bytes.checked_add(coverage_bytes))
        .ok_or_else(|| resource_error("native section transfer byte overflow"))?;
    if boundaries.is_empty() {
        return Err(kernel_error(
            "native crossing section produced no planar cap",
        ));
    }
    check_cancel(cancel)?;
    loops::sort_loops(&mut boundaries, &frame, work, cancel)?;
    // Canonical ordering can change complete-loop chunk boundaries. Recheck
    // the actual sorted packing budget before returning the owned result.
    let mut sorted_budget = loops::SectionBudget::with_limit(packed_byte_limit)?;
    for boundary in &boundaries {
        check_cancel(cancel)?;
        sorted_budget.push_loop(boundary.point_count())?;
    }
    boundaries
        .len()
        .checked_mul(
            std::mem::size_of::<crate::SectionLoopMm>() + std::mem::size_of::<Vec<usize>>(),
        )
        .filter(|bytes| *bytes <= transfer_remaining)
        .ok_or_else(|| resource_error("native section loop transfer exceeds query capacity"))?;
    result
        .loops
        .try_reserve_exact(boundaries.len())
        .map_err(|_| resource_error("native section loop transfer allocation failed"))?;
    result
        .loops
        .capacity()
        .checked_mul(std::mem::size_of::<crate::SectionLoopMm>())
        .filter(|bytes| *bytes <= transfer_remaining)
        .ok_or_else(|| resource_error("native section loop transfer exceeds query capacity"))?;
    let lineage_remaining = transfer_remaining
        .checked_sub(
            result
                .loops
                .capacity()
                .checked_mul(std::mem::size_of::<crate::SectionLoopMm>())
                .ok_or_else(|| resource_error("native slice lineage transfer byte overflow"))?,
        )
        .ok_or_else(|| resource_error("native slice lineage transfer exceeds query capacity"))?;
    boundaries
        .len()
        .checked_mul(std::mem::size_of::<Vec<usize>>())
        .filter(|bytes| *bytes <= lineage_remaining)
        .ok_or_else(|| resource_error("native slice lineage transfer exceeds query capacity"))?;
    result
        .loop_source_faces
        .try_reserve_exact(boundaries.len())
        .map_err(|_| resource_error("native slice lineage transfer allocation failed"))?;
    result
        .loop_source_faces
        .capacity()
        .checked_mul(std::mem::size_of::<Vec<usize>>())
        .filter(|bytes| *bytes <= lineage_remaining)
        .ok_or_else(|| resource_error("native slice lineage transfer exceeds query capacity"))?;
    // Both metadata arrays coexist until into_iter releases the canonical one.
    // Point buffers move without duplication.
    for boundary in boundaries {
        let (native, source_faces) = boundary.into_parts();
        result.loops.push(native);
        result.loop_source_faces.push(source_faces);
    }
    Ok(result)
}

fn insert_cap_face(
    faces: &mut HashSet<monstertruck_topology::FaceId<Surface>>,
    face: monstertruck_topology::FaceId<Surface>,
    remaining_bytes: usize,
) -> Result<usize, GeometryError> {
    const SLOT_BYTES: usize =
        2 * (std::mem::size_of::<monstertruck_topology::FaceId<Surface>>() + 32);
    if !faces.contains(&face) {
        if faces.len() == faces.capacity() {
            faces
                .len()
                .checked_add(1)
                .and_then(usize::checked_next_power_of_two)
                .and_then(|capacity| capacity.checked_mul(2))
                .map(|capacity| capacity.max(8))
                .and_then(|capacity| capacity.checked_mul(SLOT_BYTES))
                .filter(|bytes| *bytes <= remaining_bytes)
                .ok_or_else(|| resource_error("native cap index growth exceeds query capacity"))?;
            faces
                .try_reserve(1)
                .map_err(|_| resource_error("native cap index allocation failed"))?;
        }
        faces
            .capacity()
            .checked_mul(SLOT_BYTES)
            .filter(|bytes| *bytes <= remaining_bytes)
            .ok_or_else(|| resource_error("native cap index exceeds query capacity"))?;
        faces.insert(face);
    }
    faces
        .capacity()
        .checked_mul(SLOT_BYTES)
        .ok_or_else(|| resource_error("native cap index byte overflow"))
}

fn clip_bounds(
    definition: &Definition,
    frame: &PlaneFrameMm,
) -> Result<([f64; 3], [f64; 3]), GeometryError> {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for mask in 0..8 {
        let corner = std::array::from_fn(|axis| {
            if mask & (1 << axis) == 0 {
                definition.bounds.min[axis]
            } else {
                definition.bounds.max[axis]
            }
        });
        let local = in_frame(corner, frame)?;
        for axis in 0..3 {
            min[axis] = min[axis].min(local[axis]);
            max[axis] = max[axis].max(local[axis]);
        }
    }
    let pad = 1.0_f64.max(100.0 * SECTION_BOOLEAN_TOLERANCE_MM);
    let min = [min[0] - pad, min[1] - pad, 0.0];
    let max = [max[0] + pad, max[1] + pad, max[2] + pad];
    if (0..3).any(|axis| !min[axis].is_finite() || !max[axis].is_finite() || min[axis] >= max[axis])
    {
        return Err(kernel_error(
            "section clipping extents are not finite and ordered",
        ));
    }
    Ok((min, max))
}

fn frame_to_definition(frame: &PlaneFrameMm) -> Matrix4 {
    let [x, y, z, o] = [frame.x_axis, frame.y_axis, frame.z_axis, frame.origin_mm];
    Matrix4::new(
        x[0], x[1], x[2], 0.0, y[0], y[1], y[2], 0.0, z[0], z[1], z[2], 0.0, o[0], o[1], o[2], 1.0,
    )
}

pub(super) fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn parallel(a: [f64; 3], b: [f64; 3]) -> bool {
    let cross = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    dot(cross, cross) <= PARALLEL_TOLERANCE * PARALLEL_TOLERANCE
}

fn signed_distance(point: [f64; 3], plane: &PlaneMm) -> Result<f64, GeometryError> {
    let value = dot(
        std::array::from_fn(|axis| point[axis] - plane.origin_mm[axis]),
        plane.normal,
    );
    if !value.is_finite() {
        return Err(kernel_error("nonfinite native section signed distance"));
    }
    Ok(value)
}

fn in_frame(point: [f64; 3], frame: &PlaneFrameMm) -> Result<[f64; 3], GeometryError> {
    let offset = std::array::from_fn(|axis| point[axis] - frame.origin_mm[axis]);
    let local = [
        dot(offset, frame.x_axis),
        dot(offset, frame.y_axis),
        dot(offset, frame.z_axis),
    ];
    if local.iter().any(|value| !value.is_finite()) {
        return Err(kernel_error(
            "nonfinite native section plane-frame coordinate",
        ));
    }
    Ok(local.map(|value| if value == 0.0 { 0.0 } else { value }))
}

fn kernel_error(message: impl AsRef<str>) -> GeometryError {
    GeometryError::new(GeometryErrorCode::KernelFailure, message)
}

fn native_error(error: ShapeOpsError) -> GeometryError {
    let code = match &error {
        ShapeOpsError::Cancelled => GeometryErrorCode::Cancelled,
        ShapeOpsError::WorkLimitExceeded { .. } => GeometryErrorCode::ResourceLimit,
        _ => GeometryErrorCode::NativeIntersectionFailure,
    };
    GeometryError::new(code, format!("native section boolean failed: {error}"))
}

fn resource_error(message: &'static str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, message)
}
