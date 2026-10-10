// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::geometry::*;

#[derive(Debug)]
pub struct EncodedSectionChunk {
    pub metadata: SectionChunkMetadata,
    pub bytes: Vec<u8>,
}
#[derive(Debug)]
pub struct EncodedSection {
    pub summary: SectionSummary,
    pub chunks: Vec<EncodedSectionChunk>,
    pub loops: Vec<SectionLoopMetadata>,
}

fn point_body_start(loop_count: usize) -> Result<usize, GeometryError> {
    let offsets_end = checked_size(
        SECTION_HEADER_BYTES,
        loop_count
            .checked_add(1)
            .ok_or_else(|| invalid("loop count overflow"))?,
        4,
    )?;
    offsets_end
        .checked_add(7)
        .map(|v| v & !7)
        .ok_or_else(|| invalid("section alignment overflow"))
}
fn section_size(loop_count: usize, point_count: usize) -> Result<usize, GeometryError> {
    checked_size(point_body_start(loop_count)?, point_count, 24)
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Packs complete loops in deterministic occurrence order; a loop is never truncated or split.
/// The native section lane supplies canonical loop rotation and order within an occurrence.
/// Any failed push invalidates the builder, prohibiting publication of partial results.
pub struct SectionChunkBuilder {
    session: SessionId,
    artifact: ArtifactId,
    revision: SceneRevision,
    plane: PlaneMm,
    frame: PlaneFrameMm,
    chunk_limit: usize,
    failed: bool,
    chunks: Vec<EncodedSectionChunk>,
    loops: Vec<SectionLoopMetadata>,
    offsets: Vec<u32>,
    points: Vec<[f64; 3]>,
    first_loop: u32,
    total_bytes: u32,
    previous_occurrence: Option<OccurrenceId>,
    previous_definition: Option<DefinitionId>,
    holes_started: bool,
}
impl SectionChunkBuilder {
    pub fn new(
        session: SessionId,
        artifact: ArtifactId,
        revision: SceneRevision,
        plane: PlaneMm,
    ) -> Result<Self, GeometryError> {
        Self::with_chunk_limit(session, artifact, revision, plane, MAX_GEOMETRY_CHUNK_BYTES)
    }
    pub fn with_chunk_limit(
        session: SessionId,
        artifact: ArtifactId,
        revision: SceneRevision,
        plane: PlaneMm,
        chunk_limit: u32,
    ) -> Result<Self, GeometryError> {
        if !(168..=MAX_GEOMETRY_CHUNK_BYTES).contains(&chunk_limit) {
            return Err(resource("invalid section chunk cap"));
        }
        let plane = plane.normalized()?;
        let frame = PlaneFrameMm::from_plane(plane)?;
        Ok(Self {
            session,
            artifact,
            revision,
            plane,
            frame,
            chunk_limit: chunk_limit as usize,
            failed: false,
            chunks: Vec::new(),
            loops: Vec::new(),
            offsets: vec![0],
            points: Vec::new(),
            first_loop: 0,
            total_bytes: 0,
            previous_occurrence: None,
            previous_definition: None,
            holes_started: false,
        })
    }
    pub fn push_loop(
        &mut self,
        occurrence: OccurrenceId,
        definition: DefinitionId,
        is_hole: bool,
        points_world_mm: &[[f64; 3]],
    ) -> Result<(), GeometryError> {
        let origin = self.plane.origin_mm;
        self.push_loop_mapped(occurrence, definition, is_hole, points_world_mm, |point| {
            std::array::from_fn(|axis| point[axis] - origin[axis])
        })
    }

    /// Admit coordinates already relative to the plane origin, without a lossy world roundtrip.
    pub fn push_loop_relative(
        &mut self,
        occurrence: OccurrenceId,
        definition: DefinitionId,
        is_hole: bool,
        points_relative_mm: &[[f64; 3]],
    ) -> Result<(), GeometryError> {
        self.push_loop_mapped(
            occurrence,
            definition,
            is_hole,
            points_relative_mm,
            |point| point,
        )
    }

    fn push_loop_mapped(
        &mut self,
        occurrence: OccurrenceId,
        definition: DefinitionId,
        is_hole: bool,
        points: &[[f64; 3]],
        relative: impl Fn([f64; 3]) -> [f64; 3],
    ) -> Result<(), GeometryError> {
        if self.failed {
            return Err(invalid("section builder was invalidated"));
        }
        let result = self.push_loop_inner(occurrence, definition, is_hole, points, relative);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn push_loop_inner(
        &mut self,
        occurrence: OccurrenceId,
        definition: DefinitionId,
        is_hole: bool,
        points: &[[f64; 3]],
        relative: impl Fn([f64; 3]) -> [f64; 3],
    ) -> Result<(), GeometryError> {
        if points.len() < 4 {
            return Err(invalid("closed section loop needs at least four points"));
        }
        if points.len() > MAX_LOOP_POINTS as usize {
            return Err(resource("section loop exceeds point cap"));
        }
        if self
            .previous_occurrence
            .is_some_and(|previous| previous > occurrence)
        {
            return Err(invalid("section occurrences must be ordered"));
        }
        if self.previous_occurrence == Some(occurrence) {
            if self.previous_definition.as_ref() != Some(&definition)
                || (self.holes_started && !is_hole)
            {
                return Err(invalid(
                    "inconsistent definition or outer/hole section order",
                ));
            }
        } else {
            self.holes_started = false;
        }
        if is_hole && self.previous_occurrence != Some(occurrence) {
            return Err(invalid("section occurrence cannot start with a hole"));
        }
        let single = section_size(1, points.len())?;
        if single > self.chunk_limit {
            return Err(resource("complete section loop does not fit one chunk"));
        }
        let mut area = 0.0;
        for point in points {
            let point = relative(*point);
            if !finite3(point) || dot(point, self.plane.normal).abs() > SECTION_PLANE_TOLERANCE_MM {
                return Err(invalid(
                    "section point is nonfinite or off the native plane",
                ));
            }
        }
        let first = relative(points[0]);
        let last = relative(points[points.len() - 1]);
        if norm3(std::array::from_fn(|axis| first[axis] - last[axis])) > SECTION_PLANE_TOLERANCE_MM
        {
            return Err(invalid("section loop is not closed"));
        }
        for segment in points.windows(2) {
            let a = relative(segment[0]);
            let b = relative(segment[1]);
            area += dot(a, self.frame.x_axis) * dot(b, self.frame.y_axis)
                - dot(b, self.frame.x_axis) * dot(a, self.frame.y_axis);
        }
        if !area.is_finite() || area == 0.0 || (area < 0.0) != is_hole {
            return Err(invalid("section loop area or winding is invalid"));
        }
        if section_size(self.offsets.len(), self.points.len() + points.len())? > self.chunk_limit {
            self.flush()?;
        }
        let pending = section_size(self.offsets.len(), self.points.len() + points.len())?;
        if self.total_bytes as usize + pending > MAX_SECTION_BYTES as usize {
            return Err(resource("section byte budget exceeded"));
        }
        let ordinal = u32::try_from(self.loops.len())
            .map_err(|_| resource("section loop ordinal exhausted"))?;
        self.points
            .extend(points.iter().map(|point| relative(*point)));
        self.offsets.push(self.points.len() as u32);
        self.loops.push(SectionLoopMetadata {
            ordinal,
            occurrence_id: occurrence,
            definition_id: definition.clone(),
            is_hole,
            point_count: points.len() as u32,
        });
        self.previous_occurrence = Some(occurrence);
        self.previous_definition = Some(definition);
        self.holes_started |= is_hole;
        Ok(())
    }
    fn flush(&mut self) -> Result<(), GeometryError> {
        if self.offsets.len() == 1 {
            return Ok(());
        }
        let loop_count = self.offsets.len() - 1;
        let point_start = point_body_start(loop_count)?;
        let length = section_size(loop_count, self.points.len())?;
        if length > self.chunk_limit
            || self.total_bytes as usize + length > MAX_SECTION_BYTES as usize
        {
            return Err(resource("section chunk or artifact exceeds budget"));
        }
        let mut bytes = Vec::with_capacity(length);
        bytes.resize(point_start, 0);
        bytes[..4].copy_from_slice(b"SPLS");
        bytes[4..6].copy_from_slice(&SECTION_SCHEMA_VERSION.to_le_bytes());
        put_u32(&mut bytes, 8, loop_count as u32);
        put_u32(&mut bytes, 12, self.points.len() as u32);
        for axis in 0..3 {
            put_f64(&mut bytes, 16 + axis * 8, self.plane.origin_mm[axis]);
        }
        put_u32(&mut bytes, 40, self.chunks.len() as u32);
        put_u32(&mut bytes, 44, self.first_loop);
        for (index, value) in self.offsets.iter().enumerate() {
            put_u32(&mut bytes, 64 + index * 4, *value);
        }
        for point in &self.points {
            for value in point {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let metadata = SectionChunkMetadata {
            session_id: self.session.clone(),
            artifact_id: self.artifact,
            schema_version: SECTION_SCHEMA_VERSION,
            chunk_index: self.chunks.len() as u32,
            chunk_count: 0,
            byte_count: length as u32,
            sha256: SourceHash::from_bytes(&bytes),
            first_loop_ordinal: self.first_loop,
            loop_count: loop_count as u32,
        };
        self.chunks.push(EncodedSectionChunk { metadata, bytes });
        self.total_bytes += length as u32;
        self.first_loop += loop_count as u32;
        self.offsets.clear();
        self.offsets.push(0);
        self.points.clear();
        Ok(())
    }
    pub fn finish(mut self) -> Result<EncodedSection, GeometryError> {
        if self.failed {
            return Err(invalid("section artifact has invalid loops"));
        }
        self.flush()?;
        let count = self.chunks.len() as u32;
        for chunk in &mut self.chunks {
            chunk.metadata.chunk_count = count;
        }
        let summary = SectionSummary {
            session_id: self.session,
            artifact_id: self.artifact,
            revision: self.revision,
            schema_version: SECTION_SCHEMA_VERSION,
            plane: self.plane,
            frame: self.frame,
            total_loop_count: self.loops.len() as u32,
            chunk_count: count,
            total_bytes: self.total_bytes,
            sampling_tolerance_mm: SECTION_SAMPLING_TOLERANCE_MM,
            boolean_tolerance_mm: SECTION_BOOLEAN_TOLERANCE_MM,
        };
        Ok(EncodedSection {
            summary,
            chunks: self.chunks,
            loops: self.loops,
        })
    }
}

#[derive(Debug)]
pub struct ValidatedSectionChunk<'a> {
    pub plane_origin_mm: [f64; 3],
    pub chunk_index: u32,
    pub first_loop_ordinal: u32,
    pub loop_offsets: U32View<'a>,
    pub points_relative_mm: F64x3View<'a>,
}
pub fn validate_section_summary(summary: &SectionSummary) -> Result<(), GeometryError> {
    if summary.schema_version != SECTION_SCHEMA_VERSION
        || summary.total_bytes > MAX_SECTION_BYTES
        || summary.sampling_tolerance_mm != SECTION_SAMPLING_TOLERANCE_MM
        || summary.boolean_tolerance_mm != SECTION_BOOLEAN_TOLERANCE_MM
    {
        return Err(invalid("invalid section summary schema or tolerances"));
    }
    let normalized = summary.plane.normalized()?;
    if (norm3(summary.plane.normal) - 1.0).abs() > POSE_NORM_TOLERANCE {
        return Err(invalid("section summary plane is not normalized"));
    }
    let expected = PlaneFrameMm::from_plane(normalized)?;
    for (actual, wanted) in [
        summary.frame.origin_mm,
        summary.frame.x_axis,
        summary.frame.y_axis,
        summary.frame.z_axis,
    ]
    .into_iter()
    .zip([
        expected.origin_mm,
        expected.x_axis,
        expected.y_axis,
        expected.z_axis,
    ]) {
        if !finite3(actual)
            || (0..3).any(|axis| (actual[axis] - wanted[axis]).abs() > POSE_NORM_TOLERANCE)
        {
            return Err(invalid("section frame is not the canonical plane frame"));
        }
    }
    if (summary.total_loop_count == 0) != (summary.chunk_count == 0)
        || (summary.chunk_count == 0) != (summary.total_bytes == 0)
        || summary.chunk_count > summary.total_loop_count
    {
        return Err(invalid("invalid empty or nonempty section summary"));
    }
    let minimum_bytes =
        u64::from(summary.total_loop_count) * 100 + u64::from(summary.chunk_count) * 68;
    if minimum_bytes > u64::from(summary.total_bytes) {
        return Err(invalid("section summary cannot contain its declared loops"));
    }
    Ok(())
}

pub fn validate_section_chunk<'a>(
    bytes: &'a [u8],
    metadata: &SectionChunkMetadata,
    summary: &SectionSummary,
    session: &SessionId,
    artifact: ArtifactId,
    chunk_index: u32,
) -> Result<ValidatedSectionChunk<'a>, GeometryError> {
    validate_section_summary(summary)?;
    if &summary.session_id != session
        || summary.artifact_id != artifact
        || &metadata.session_id != session
        || metadata.artifact_id != artifact
        || metadata.chunk_index != chunk_index
    {
        return Err(GeometryError::new(
            GeometryErrorCode::UnknownHandle,
            "section descriptor identity mismatch",
        ));
    }
    if metadata.schema_version != SECTION_SCHEMA_VERSION
        || metadata.chunk_count != summary.chunk_count
        || chunk_index >= metadata.chunk_count
        || metadata.loop_count == 0
        || metadata
            .first_loop_ordinal
            .checked_add(metadata.loop_count)
            .is_none_or(|end| end > summary.total_loop_count)
    {
        return Err(invalid("section descriptor range or version invalid"));
    }
    if bytes.len() < SECTION_HEADER_BYTES
        || bytes.len() > MAX_GEOMETRY_CHUNK_BYTES as usize
        || bytes.len() != metadata.byte_count as usize
        || metadata.byte_count > summary.total_bytes
    {
        return Err(invalid("section byte count mismatch"));
    }
    if &bytes[..4] != b"SPLS"
        || u16_at(bytes, 4) != SECTION_SCHEMA_VERSION
        || bytes[6..8].iter().chain(&bytes[48..64]).any(|&v| v != 0)
    {
        return Err(invalid("section magic, schema or reserved bytes invalid"));
    }
    let loops = u32_at(bytes, 8) as usize;
    let points = u32_at(bytes, 12) as usize;
    if loops != metadata.loop_count as usize
        || u32_at(bytes, 40) != chunk_index
        || u32_at(bytes, 44) != metadata.first_loop_ordinal
        || points
            < loops
                .checked_mul(4)
                .ok_or_else(|| invalid("section point count overflow"))?
    {
        return Err(invalid("section header and descriptor disagree"));
    }
    let point_start = point_body_start(loops)?;
    if section_size(loops, points)? != bytes.len() {
        return Err(invalid("section exact layout length mismatch"));
    }
    let offset_end = SECTION_HEADER_BYTES + (loops + 1) * 4;
    if bytes[offset_end..point_start].iter().any(|&v| v != 0) {
        return Err(invalid("section alignment padding must be zero"));
    }
    if !metadata.sha256.matches_bytes(bytes) {
        return Err(invalid("section SHA-256 mismatch"));
    }
    let origin = std::array::from_fn(|axis| f64_at(bytes, 16 + axis * 8));
    if !finite3(origin) || origin != summary.plane.origin_mm {
        return Err(invalid("section origin mismatch"));
    }
    let offsets = U32View::new(&bytes[SECTION_HEADER_BYTES..offset_end]);
    let data = F64x3View::new(&bytes[point_start..]);
    if offsets.get(0) != Some(0) || offsets.get(loops) != Some(points as u32) {
        return Err(invalid("section offsets do not span points"));
    }
    for ordinal in 0..loops {
        let start = offsets.get(ordinal).unwrap() as usize;
        let end = offsets.get(ordinal + 1).unwrap() as usize;
        if start > end || end > points || end - start < 4 || end - start > MAX_LOOP_POINTS as usize
        {
            return Err(invalid("section offsets or loop size invalid"));
        }
        let first = data.get(start).unwrap();
        let last = data.get(end - 1).unwrap();
        if !finite3(first)
            || !finite3(last)
            || norm3(std::array::from_fn(|axis| first[axis] - last[axis]))
                > SECTION_PLANE_TOLERANCE_MM
        {
            return Err(invalid("section loop closure invalid"));
        }
        let mut previous = first;
        let mut area = 0.0;
        for index in start..end {
            let point = data.get(index).unwrap();
            let world = std::array::from_fn(|axis| origin[axis] + point[axis]);
            if !finite3(point)
                || !finite3(world)
                || dot(point, summary.plane.normal).abs() > SECTION_PLANE_TOLERANCE_MM
            {
                return Err(invalid("section coordinate is nonfinite or off plane"));
            }
            if index > start {
                area += dot(previous, summary.frame.x_axis) * dot(point, summary.frame.y_axis)
                    - dot(point, summary.frame.x_axis) * dot(previous, summary.frame.y_axis);
            }
            previous = point;
        }
        if !area.is_finite() || area == 0.0 {
            return Err(invalid("section loop area invalid"));
        }
    }
    Ok(ValidatedSectionChunk {
        plane_origin_mm: origin,
        chunk_index,
        first_loop_ordinal: metadata.first_loop_ordinal,
        loop_offsets: offsets,
        points_relative_mm: data,
    })
}

/// Requires the complete ordered descriptor/loop pages. No model-sized byte accumulator.
pub fn validate_section_manifest(
    summary: &SectionSummary,
    chunks: &[SectionChunkMetadata],
    loops: &[SectionLoopMetadata],
) -> Result<(), GeometryError> {
    validate_section_summary(summary)?;
    if chunks.len() != summary.chunk_count as usize
        || loops.len() != summary.total_loop_count as usize
    {
        return Err(invalid("section manifest page count mismatch"));
    }
    let mut next = 0_u32;
    let mut bytes = 0_u32;
    for (index, chunk) in chunks.iter().enumerate() {
        if chunk.session_id != summary.session_id
            || chunk.artifact_id != summary.artifact_id
            || chunk.schema_version != SECTION_SCHEMA_VERSION
            || chunk.chunk_index as usize != index
            || chunk.chunk_count != summary.chunk_count
            || chunk.first_loop_ordinal != next
            || chunk.loop_count == 0
            || chunk.byte_count < SECTION_HEADER_BYTES as u32
            || chunk.byte_count > MAX_GEOMETRY_CHUNK_BYTES
        {
            return Err(invalid(
                "section ranges are reordered, overlapping or inconsistent",
            ));
        }
        next = next
            .checked_add(chunk.loop_count)
            .ok_or_else(|| invalid("section range overflow"))?;
        if next as usize > loops.len() {
            return Err(invalid("section chunk loop range exceeds metadata"));
        }
        let first = chunk.first_loop_ordinal as usize;
        let points = loops[first..next as usize]
            .iter()
            .try_fold(0_usize, |count, row| {
                count
                    .checked_add(row.point_count as usize)
                    .ok_or_else(|| resource("section point count overflow"))
            })?;
        if section_size(chunk.loop_count as usize, points)? != chunk.byte_count as usize {
            return Err(invalid("section chunk size disagrees with loop metadata"));
        }
        bytes = bytes
            .checked_add(chunk.byte_count)
            .ok_or_else(|| resource("section bytes overflow"))?;
    }
    if next != summary.total_loop_count || bytes != summary.total_bytes {
        return Err(invalid("section manifest totals mismatch"));
    }
    let mut previous: Option<&SectionLoopMetadata> = None;
    for (index, row) in loops.iter().enumerate() {
        if row.ordinal as usize != index || row.point_count < 4 || row.point_count > MAX_LOOP_POINTS
        {
            return Err(invalid("section loop metadata invalid"));
        }
        if let Some(previous) = previous {
            if previous.occurrence_id > row.occurrence_id
                || (previous.occurrence_id == row.occurrence_id
                    && (previous.definition_id != row.definition_id
                        || (previous.is_hole && !row.is_hole)))
                || (previous.occurrence_id != row.occurrence_id && row.is_hole)
            {
                return Err(invalid("section loop identities or ordering invalid"));
            }
        } else if row.is_hole {
            return Err(invalid("section starts with a hole"));
        }
        previous = Some(row);
    }
    Ok(())
}

/// Compare borrowed loop lengths and winding with their artifact-wide identity rows.
pub fn validate_section_loop_metadata(
    chunk: &ValidatedSectionChunk<'_>,
    summary: &SectionSummary,
    rows: &[SectionLoopMetadata],
) -> Result<(), GeometryError> {
    validate_section_summary(summary)?;
    if chunk.plane_origin_mm != summary.plane.origin_mm || chunk.chunk_index >= summary.chunk_count
    {
        return Err(invalid("section loop view summary mismatch"));
    }
    if rows.len() + 1 != chunk.loop_offsets.len() {
        return Err(invalid("section loop page size mismatch"));
    }
    for (index, row) in rows.iter().enumerate() {
        if chunk.first_loop_ordinal.checked_add(index as u32) != Some(row.ordinal)
            || row.ordinal >= summary.total_loop_count
        {
            return Err(invalid("section loop page ordinal mismatch"));
        }
        let start = chunk.loop_offsets.get(index).unwrap() as usize;
        let end = chunk.loop_offsets.get(index + 1).unwrap() as usize;
        if start > end
            || end > chunk.points_relative_mm.len()
            || end - start < 4
            || end - start > MAX_LOOP_POINTS as usize
            || end - start != row.point_count as usize
        {
            return Err(invalid("section loop point count mismatch"));
        }
        let mut area = 0.0;
        for point in start..end - 1 {
            let a = chunk.points_relative_mm.get(point).unwrap();
            let b = chunk.points_relative_mm.get(point + 1).unwrap();
            area += dot(a, summary.frame.x_axis) * dot(b, summary.frame.y_axis)
                - dot(b, summary.frame.x_axis) * dot(a, summary.frame.y_axis);
        }
        if !area.is_finite() || area == 0.0 || (area < 0.0) != row.is_hole {
            return Err(invalid("section loop winding disagrees with hole metadata"));
        }
    }
    Ok(())
}
