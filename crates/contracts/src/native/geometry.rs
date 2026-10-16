// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Checked protobuf geometry boundary; packed schemas remain independent.
use super::{
    artifact_from_view, artifact_to_view, request_id, required, session_parent,
    validate_artifact_view,
};
use crate::{geometry::*, rpc};

fn checked<T>(value: Result<T, GeometryError>) -> Result<T, String> {
    value.map_err(|error| error.to_string())
}
fn offset(value: u32, maximum: u32) -> Result<u32, String> {
    if value > maximum {
        Err("page offset exceeds support limit".into())
    } else {
        Ok(value)
    }
}
fn version(value: u32) -> Result<u16, String> {
    value
        .try_into()
        .map_err(|_| "schema version exceeds u16".into())
}

impl TryFrom<rpc::KernelIdentity> for KernelIdentity {
    type Error = String;
    fn try_from(value: rpc::KernelIdentity) -> Result<Self, String> {
        let value = Self {
            name: value.name,
            version: value.version,
            revision: value.revision,
        };
        validate_kernel(&value)?;
        Ok(value)
    }
}

impl TryFrom<KernelIdentity> for rpc::KernelIdentity {
    type Error = String;
    fn try_from(value: KernelIdentity) -> Result<Self, String> {
        validate_kernel(&value)?;
        let value = Self {
            name: value.name,
            version: value.version,
            revision: value.revision,
        };
        Ok(value)
    }
}

impl TryFrom<rpc::GeometryLimits> for GeometryLimits {
    type Error = String;
    fn try_from(value: rpc::GeometryLimits) -> Result<Self, String> {
        let value = Self {
            source_bytes: value.source_bytes,
            live_source_bytes: value.live_source_bytes,
            step_entities: value.step_entities,
            native_faces: value.native_faces,
            definitions: value.definitions,
            occurrences: value.occurrences,
            display_vertices: value.display_vertices,
            display_triangles: value.display_triangles,
            scene_mesh_bytes: value.scene_mesh_bytes,
            section_bytes: value.section_bytes,
            engine_artifact_bytes: value.engine_artifact_bytes,
            geometry_chunk_bytes: value.geometry_chunk_bytes,
            display_buffer_bytes: value.display_buffer_bytes,
            loop_points: value.loop_points,
            active_jobs: value.active_jobs,
            outstanding_chunks: value.outstanding_chunks,
        };
        if value != GeometryLimits::FROZEN {
            return Err("geometry limits differ from frozen support envelope".into());
        }
        Ok(value)
    }
}

impl TryFrom<GeometryLimits> for rpc::GeometryLimits {
    type Error = String;
    fn try_from(value: GeometryLimits) -> Result<Self, String> {
        if value != GeometryLimits::FROZEN {
            return Err("geometry limits differ from frozen support envelope".into());
        }
        let value = Self {
            source_bytes: value.source_bytes,
            live_source_bytes: value.live_source_bytes,
            step_entities: value.step_entities,
            native_faces: value.native_faces,
            definitions: value.definitions,
            occurrences: value.occurrences,
            display_vertices: value.display_vertices,
            display_triangles: value.display_triangles,
            scene_mesh_bytes: value.scene_mesh_bytes,
            section_bytes: value.section_bytes,
            engine_artifact_bytes: value.engine_artifact_bytes,
            geometry_chunk_bytes: value.geometry_chunk_bytes,
            display_buffer_bytes: value.display_buffer_bytes,
            loop_points: value.loop_points,
            active_jobs: value.active_jobs,
            outstanding_chunks: value.outstanding_chunks,
        };
        Ok(value)
    }
}

impl TryFrom<rpc::SourceProvenance> for SourceProvenance {
    type Error = String;
    fn try_from(value: rpc::SourceProvenance) -> Result<Self, String> {
        let value = Self {
            source_hash: checked(SourceHash::parse(value.source_hash))?,
            source_name: value.source_name,
            source_unit: value.source_unit.try_into()?,
            uncertainty_mm: value.uncertainty_mm,
        };
        checked(value.validate())?;
        Ok(value)
    }
}

impl TryFrom<SourceProvenance> for rpc::SourceProvenance {
    type Error = String;
    fn try_from(value: SourceProvenance) -> Result<Self, String> {
        checked(value.validate())?;
        let value = Self {
            source_hash: value.source_hash.into(),
            source_name: value.source_name,
            source_unit: rpc::SourceUnit::try_from(value.source_unit)? as i32,
            uncertainty_mm: value.uncertainty_mm,
        };
        Ok(value)
    }
}

impl TryFrom<rpc::SceneSummary> for SceneSummary {
    type Error = String;
    fn try_from(value: rpc::SceneSummary) -> Result<Self, String> {
        let value = Self {
            session_id: checked(SessionId::parse(value.session_id))?,
            revision: SceneRevision(value.revision),
            definition_count: value.definition_count,
            occurrence_count: value.occurrence_count,
            bounds_mm: value.bounds_mm.map(TryInto::try_into).transpose()?,
            unique_mesh_bytes: value.unique_mesh_bytes,
        };
        checked(value.validate())?;
        Ok(value)
    }
}

impl TryFrom<SceneSummary> for rpc::SceneSummary {
    type Error = String;
    fn try_from(value: SceneSummary) -> Result<Self, String> {
        checked(value.validate())?;
        let value = Self {
            session_id: value.session_id.into(),
            revision: value.revision.get(),
            definition_count: value.definition_count,
            occurrence_count: value.occurrence_count,
            bounds_mm: value.bounds_mm.map(TryInto::try_into).transpose()?,
            unique_mesh_bytes: value.unique_mesh_bytes,
        };
        Ok(value)
    }
}

impl TryFrom<rpc::DefinitionDisplay> for DefinitionDisplay {
    type Error = String;
    fn try_from(value: rpc::DefinitionDisplay) -> Result<Self, String> {
        let value = match required(value.kind, "definition display kind")? {
            rpc::definition_display::Kind::MeshArtifactId(id) => Self::Mesh {
                artifact_id: checked(ArtifactId::new(id))?,
            },
            rpc::definition_display::Kind::Unavailable(error) => Self::Unavailable {
                error: error.try_into()?,
            },
        };
        checked(value.validate())?;
        Ok(value)
    }
}
impl TryFrom<DefinitionDisplay> for rpc::DefinitionDisplay {
    type Error = String;
    fn try_from(value: DefinitionDisplay) -> Result<Self, String> {
        checked(value.validate())?;
        let kind = match value {
            DefinitionDisplay::Mesh { artifact_id } => {
                rpc::definition_display::Kind::MeshArtifactId(artifact_id.get())
            }
            DefinitionDisplay::Unavailable { error } => {
                rpc::definition_display::Kind::Unavailable(error.try_into()?)
            }
        };
        Ok(Self { kind: Some(kind) })
    }
}

impl TryFrom<rpc::DefinitionRecord> for DefinitionRecord {
    type Error = String;
    fn try_from(value: rpc::DefinitionRecord) -> Result<Self, String> {
        let value = Self {
            definition_id: checked(DefinitionId::parse(value.definition_id))?,
            provenance: required(value.provenance, "provenance")?.try_into()?,
            face_count: value.face_count,
            bounds_mm: required(value.bounds_mm, "bounds_mm")?.try_into()?,
            display: required(value.display, "definition display")?.try_into()?,
        };
        checked(value.validate())?;
        Ok(value)
    }
}

impl TryFrom<DefinitionRecord> for rpc::DefinitionRecord {
    type Error = String;
    fn try_from(value: DefinitionRecord) -> Result<Self, String> {
        checked(value.validate())?;
        let value = Self {
            definition_id: value.definition_id.into(),
            provenance: Some(value.provenance.try_into()?),
            face_count: value.face_count,
            bounds_mm: Some(value.bounds_mm.try_into()?),
            display: Some(value.display.try_into()?),
        };
        Ok(value)
    }
}

impl TryFrom<rpc::OccurrenceRecord> for OccurrenceRecord {
    type Error = String;
    fn try_from(value: rpc::OccurrenceRecord) -> Result<Self, String> {
        let value = Self {
            occurrence_id: checked(OccurrenceId::new(value.occurrence_id))?,
            definition_id: checked(DefinitionId::parse(value.definition_id))?,
            pose: required(value.pose, "pose")?.try_into()?,
        };
        checked(value.pose.validate())?;
        Ok(value)
    }
}

impl TryFrom<OccurrenceRecord> for rpc::OccurrenceRecord {
    type Error = String;
    fn try_from(value: OccurrenceRecord) -> Result<Self, String> {
        checked(value.pose.validate())?;
        let value = Self {
            occurrence_id: value.occurrence_id.get(),
            definition_id: value.definition_id.into(),
            pose: Some(value.pose.try_into()?),
        };
        Ok(value)
    }
}

impl TryFrom<rpc::FaceRef> for FaceRef {
    type Error = String;
    fn try_from(value: rpc::FaceRef) -> Result<Self, String> {
        let value = Self {
            session_id: checked(SessionId::parse(value.session_id))?,
            scene_revision: SceneRevision(value.scene_revision),
            occurrence_id: checked(OccurrenceId::new(value.occurrence_id))?,
            definition_id: checked(DefinitionId::parse(value.definition_id))?,
            face_id: checked(FaceId::parse(value.face_id))?,
        };
        Ok(value)
    }
}

impl TryFrom<FaceRef> for rpc::FaceRef {
    type Error = String;
    fn try_from(value: FaceRef) -> Result<Self, String> {
        let value = Self {
            session_id: value.session_id.into(),
            scene_revision: value.scene_revision.get(),
            occurrence_id: value.occurrence_id.get(),
            definition_id: value.definition_id.into(),
            face_id: value.face_id.into(),
        };
        Ok(value)
    }
}

impl TryFrom<rpc::FaceInfo> for FaceInfo {
    type Error = String;
    fn try_from(value: rpc::FaceInfo) -> Result<Self, String> {
        let value = Self {
            face_id: checked(FaceId::parse(value.face_id))?,
            surface_face_entity: value.surface_face_entity,
            source_entity_kind: value.source_entity_kind,
            source_surface_entity_kind: value.source_surface_entity_kind,
            orientation: value.orientation,
            geometry: required(value.geometry, "geometry")?.try_into()?,
        };
        checked(value.validate())?;
        Ok(value)
    }
}

impl TryFrom<FaceInfo> for rpc::FaceInfo {
    type Error = String;
    fn try_from(value: FaceInfo) -> Result<Self, String> {
        checked(value.validate())?;
        let value = Self {
            face_id: value.face_id.into(),
            surface_face_entity: value.surface_face_entity,
            source_entity_kind: value.source_entity_kind,
            source_surface_entity_kind: value.source_surface_entity_kind,
            orientation: value.orientation,
            geometry: Some(value.geometry.try_into()?),
        };
        Ok(value)
    }
}

impl TryFrom<rpc::FaceInspection> for FaceInspection {
    type Error = String;
    fn try_from(value: rpc::FaceInspection) -> Result<Self, String> {
        let value = Self {
            reference: required(value.reference, "reference")?.try_into()?,
            face: required(value.face, "face")?.try_into()?,
            pose: required(value.pose, "pose")?.try_into()?,
            provenance: required(value.provenance, "provenance")?.try_into()?,
        };
        validate_inspection(&value)?;
        Ok(value)
    }
}

impl TryFrom<FaceInspection> for rpc::FaceInspection {
    type Error = String;
    fn try_from(value: FaceInspection) -> Result<Self, String> {
        validate_inspection(&value)?;
        let value = Self {
            reference: Some(value.reference.try_into()?),
            face: Some(value.face.try_into()?),
            pose: Some(value.pose.try_into()?),
            provenance: Some(value.provenance.try_into()?),
        };
        Ok(value)
    }
}

impl TryFrom<rpc::FaceIndexRow> for FaceIndexRow {
    type Error = String;
    fn try_from(value: rpc::FaceIndexRow) -> Result<Self, String> {
        let value = Self {
            ordinal: value.ordinal,
            face_id: checked(FaceId::parse(value.face_id))?,
        };
        if value.ordinal >= MAX_NATIVE_FACES {
            return Err("face ordinal exceeds support limit".into());
        }
        Ok(value)
    }
}

impl TryFrom<FaceIndexRow> for rpc::FaceIndexRow {
    type Error = String;
    fn try_from(value: FaceIndexRow) -> Result<Self, String> {
        if value.ordinal >= MAX_NATIVE_FACES {
            return Err("face ordinal exceeds support limit".into());
        }
        let value = Self {
            ordinal: value.ordinal,
            face_id: value.face_id.into(),
        };
        Ok(value)
    }
}

impl TryFrom<rpc::PlaneFrameMm> for PlaneFrameMm {
    type Error = String;
    fn try_from(value: rpc::PlaneFrameMm) -> Result<Self, String> {
        let value = Self {
            origin_mm: required(value.origin_mm, "origin_mm")?.try_into()?,
            x_axis: required(value.x_axis, "x_axis")?.try_into()?,
            y_axis: required(value.y_axis, "y_axis")?.try_into()?,
            z_axis: required(value.z_axis, "z_axis")?.try_into()?,
        };
        validate_frame(&value)?;
        Ok(value)
    }
}

impl TryFrom<PlaneFrameMm> for rpc::PlaneFrameMm {
    type Error = String;
    fn try_from(value: PlaneFrameMm) -> Result<Self, String> {
        validate_frame(&value)?;
        let value = Self {
            origin_mm: Some(value.origin_mm.try_into()?),
            x_axis: Some(value.x_axis.try_into()?),
            y_axis: Some(value.y_axis.try_into()?),
            z_axis: Some(value.z_axis.try_into()?),
        };
        Ok(value)
    }
}

impl TryFrom<rpc::MeshChunkMetadata> for MeshChunkMetadata {
    type Error = String;
    fn try_from(value: rpc::MeshChunkMetadata) -> Result<Self, String> {
        let value = Self {
            session_id: checked(SessionId::parse(value.session_id))?,
            artifact_id: checked(ArtifactId::new(value.artifact_id))?,
            definition_id: checked(DefinitionId::parse(value.definition_id))?,
            schema_version: version(value.schema_version)?,
            mesh_profile: value.mesh_profile.try_into()?,
            chunk_index: value.chunk_index,
            chunk_count: value.chunk_count,
            byte_count: value.byte_count,
            sha256: checked(SourceHash::parse(value.sha256))?,
            face_count: value.face_count,
            bounds_mm: required(value.bounds_mm, "bounds_mm")?.try_into()?,
            carrier_deviation_mm: value.carrier_deviation_mm,
            trim_deviation_mm: required(value.trim_deviation_mm, "trim_deviation_mm")?,
            quantization_error_mm: value.quantization_error_mm,
        };
        validate_mesh_metadata(&value)?;
        Ok(value)
    }
}

impl TryFrom<MeshChunkMetadata> for rpc::MeshChunkMetadata {
    type Error = String;
    fn try_from(value: MeshChunkMetadata) -> Result<Self, String> {
        validate_mesh_metadata(&value)?;
        let value = Self {
            session_id: value.session_id.into(),
            artifact_id: value.artifact_id.get(),
            definition_id: value.definition_id.into(),
            schema_version: u32::from(value.schema_version),
            mesh_profile: rpc::DisplayProfile::try_from(value.mesh_profile)? as i32,
            chunk_index: value.chunk_index,
            chunk_count: value.chunk_count,
            byte_count: value.byte_count,
            sha256: value.sha256.into(),
            face_count: value.face_count,
            bounds_mm: Some(value.bounds_mm.try_into()?),
            carrier_deviation_mm: value.carrier_deviation_mm,
            trim_deviation_mm: Some(value.trim_deviation_mm),
            quantization_error_mm: value.quantization_error_mm,
        };
        Ok(value)
    }
}

impl TryFrom<rpc::SectionChunkMetadata> for SectionChunkMetadata {
    type Error = String;
    fn try_from(value: rpc::SectionChunkMetadata) -> Result<Self, String> {
        let value = Self {
            session_id: checked(SessionId::parse(value.session_id))?,
            artifact_id: checked(ArtifactId::new(value.artifact_id))?,
            schema_version: version(value.schema_version)?,
            chunk_index: value.chunk_index,
            chunk_count: value.chunk_count,
            byte_count: value.byte_count,
            sha256: checked(SourceHash::parse(value.sha256))?,
            first_loop_ordinal: value.first_loop_ordinal,
            loop_count: value.loop_count,
        };
        validate_section_metadata(&value)?;
        Ok(value)
    }
}

impl TryFrom<SectionChunkMetadata> for rpc::SectionChunkMetadata {
    type Error = String;
    fn try_from(value: SectionChunkMetadata) -> Result<Self, String> {
        validate_section_metadata(&value)?;
        let value = Self {
            session_id: value.session_id.into(),
            artifact_id: value.artifact_id.get(),
            schema_version: u32::from(value.schema_version),
            chunk_index: value.chunk_index,
            chunk_count: value.chunk_count,
            byte_count: value.byte_count,
            sha256: value.sha256.into(),
            first_loop_ordinal: value.first_loop_ordinal,
            loop_count: value.loop_count,
        };
        Ok(value)
    }
}

impl TryFrom<rpc::SectionLoopMetadata> for SectionLoopMetadata {
    type Error = String;
    fn try_from(value: rpc::SectionLoopMetadata) -> Result<Self, String> {
        let value = Self {
            ordinal: value.ordinal,
            occurrence_id: checked(OccurrenceId::new(value.occurrence_id))?,
            definition_id: checked(DefinitionId::parse(value.definition_id))?,
            is_hole: value.is_hole,
            point_count: value.point_count,
        };
        validate_loop(&value)?;
        Ok(value)
    }
}

impl TryFrom<SectionLoopMetadata> for rpc::SectionLoopMetadata {
    type Error = String;
    fn try_from(value: SectionLoopMetadata) -> Result<Self, String> {
        validate_loop(&value)?;
        let value = Self {
            ordinal: value.ordinal,
            occurrence_id: value.occurrence_id.get(),
            definition_id: value.definition_id.into(),
            is_hole: value.is_hole,
            point_count: value.point_count,
        };
        Ok(value)
    }
}

impl TryFrom<rpc::SectionSummary> for SectionSummary {
    type Error = String;
    fn try_from(value: rpc::SectionSummary) -> Result<Self, String> {
        let value = Self {
            session_id: checked(SessionId::parse(value.session_id))?,
            artifact_id: checked(ArtifactId::new(value.artifact_id))?,
            revision: SceneRevision(value.revision),
            schema_version: version(value.schema_version)?,
            plane: required(value.plane, "plane")?.try_into()?,
            frame: required(value.frame, "frame")?.try_into()?,
            total_loop_count: value.total_loop_count,
            chunk_count: value.chunk_count,
            total_bytes: value.total_bytes,
            sampling_tolerance_mm: value.sampling_tolerance_mm,
            boolean_tolerance_mm: value.boolean_tolerance_mm,
        };
        checked(crate::display::validate_section_summary(&value))?;
        Ok(value)
    }
}

impl TryFrom<SectionSummary> for rpc::SectionSummary {
    type Error = String;
    fn try_from(value: SectionSummary) -> Result<Self, String> {
        checked(crate::display::validate_section_summary(&value))?;
        let value = Self {
            session_id: value.session_id.into(),
            artifact_id: value.artifact_id.get(),
            revision: value.revision.get(),
            schema_version: u32::from(value.schema_version),
            plane: Some(value.plane.try_into()?),
            frame: Some(value.frame.try_into()?),
            total_loop_count: value.total_loop_count,
            chunk_count: value.chunk_count,
            total_bytes: value.total_bytes,
            sampling_tolerance_mm: value.sampling_tolerance_mm,
            boolean_tolerance_mm: value.boolean_tolerance_mm,
        };
        Ok(value)
    }
}

impl TryFrom<i32> for SourceUnit {
    type Error = String;
    fn try_from(value: i32) -> Result<Self, String> {
        match rpc::SourceUnit::try_from(value).map_err(|_| "unknown SourceUnit".to_string())? {
            rpc::SourceUnit::Millimetre => Ok(Self::Millimetre),
            rpc::SourceUnit::Metre => Ok(Self::Metre),
            rpc::SourceUnit::Inch => Ok(Self::Inch),
            _ => Err("unspecified SourceUnit".into()),
        }
    }
}
impl TryFrom<SourceUnit> for rpc::SourceUnit {
    type Error = String;
    fn try_from(value: SourceUnit) -> Result<Self, String> {
        Ok(match value {
            SourceUnit::Millimetre => Self::Millimetre,
            SourceUnit::Metre => Self::Metre,
            SourceUnit::Inch => Self::Inch,
        })
    }
}

impl TryFrom<i32> for DisplayProfile {
    type Error = String;
    fn try_from(value: i32) -> Result<Self, String> {
        match rpc::DisplayProfile::try_from(value)
            .map_err(|_| "unknown DisplayProfile".to_string())?
        {
            rpc::DisplayProfile::MeshTrimmedMm005V2 => Ok(Self::MeshTrimmedMm005V2),
            _ => Err("unspecified DisplayProfile".into()),
        }
    }
}
impl TryFrom<DisplayProfile> for rpc::DisplayProfile {
    type Error = String;
    fn try_from(value: DisplayProfile) -> Result<Self, String> {
        Ok(match value {
            DisplayProfile::MeshTrimmedMm005V2 => Self::MeshTrimmedMm005V2,
        })
    }
}

impl TryFrom<i32> for ScenePageKind {
    type Error = String;
    fn try_from(value: i32) -> Result<Self, String> {
        match rpc::ScenePageKind::try_from(value)
            .map_err(|_| "unknown ScenePageKind".to_string())?
        {
            rpc::ScenePageKind::Definitions => Ok(Self::Definitions),
            rpc::ScenePageKind::Occurrences => Ok(Self::Occurrences),
            _ => Err("unspecified ScenePageKind".into()),
        }
    }
}
impl TryFrom<ScenePageKind> for rpc::ScenePageKind {
    type Error = String;
    fn try_from(value: ScenePageKind) -> Result<Self, String> {
        Ok(match value {
            ScenePageKind::Definitions => Self::Definitions,
            ScenePageKind::Occurrences => Self::Occurrences,
        })
    }
}

impl TryFrom<i32> for ArtifactPageKind {
    type Error = String;
    fn try_from(value: i32) -> Result<Self, String> {
        match rpc::ArtifactPageKind::try_from(value)
            .map_err(|_| "unknown ArtifactPageKind".to_string())?
        {
            rpc::ArtifactPageKind::Chunks => Ok(Self::Chunks),
            rpc::ArtifactPageKind::Loops => Ok(Self::Loops),
            _ => Err("unspecified ArtifactPageKind".into()),
        }
    }
}
impl TryFrom<ArtifactPageKind> for rpc::ArtifactPageKind {
    type Error = String;
    fn try_from(value: ArtifactPageKind) -> Result<Self, String> {
        Ok(match value {
            ArtifactPageKind::Chunks => Self::Chunks,
            ArtifactPageKind::Loops => Self::Loops,
        })
    }
}

fn validate_kernel(value: &KernelIdentity) -> Result<(), String> {
    if [&value.name, &value.version, &value.revision]
        .into_iter()
        .any(|s| s.is_empty() || s.len() > MAX_DISPLAY_LABEL_BYTES as usize)
    {
        return Err("invalid kernel identity".into());
    }
    Ok(())
}
fn validate_inspection(value: &FaceInspection) -> Result<(), String> {
    checked(value.face.validate())?;
    checked(value.pose.validate())?;
    checked(value.provenance.validate())?;
    if value.reference.face_id != value.face.face_id {
        return Err("inspection face does not match reference".into());
    }
    Ok(())
}
fn validate_frame(value: &PlaneFrameMm) -> Result<(), String> {
    if !finite3(value.origin_mm)
        || [value.x_axis, value.y_axis, value.z_axis]
            .into_iter()
            .any(|a| !finite3(a) || (norm3(a) - 1.0).abs() > POSE_NORM_TOLERANCE)
    {
        return Err("nonfinite or nonunit plane frame".into());
    }
    let cross = [
        value.x_axis[1] * value.y_axis[2] - value.x_axis[2] * value.y_axis[1],
        value.x_axis[2] * value.y_axis[0] - value.x_axis[0] * value.y_axis[2],
        value.x_axis[0] * value.y_axis[1] - value.x_axis[1] * value.y_axis[0],
    ];
    let dot = (0..3)
        .map(|i| value.x_axis[i] * value.y_axis[i])
        .sum::<f64>();
    if dot.abs() > POSE_NORM_TOLERANCE
        || (0..3).any(|i| (cross[i] - value.z_axis[i]).abs() > POSE_NORM_TOLERANCE)
    {
        return Err("plane frame is not right-handed orthonormal".into());
    }
    Ok(())
}
fn validate_mesh_metadata(value: &MeshChunkMetadata) -> Result<(), String> {
    checked(value.validate())
}
fn validate_section_metadata(value: &SectionChunkMetadata) -> Result<(), String> {
    if value.schema_version != crate::display::SECTION_SCHEMA_VERSION
        || value.chunk_count == 0
        || value.chunk_count > MAX_SECTION_BYTES / 168
        || value.chunk_index >= value.chunk_count
        || value.loop_count == 0
        || value
            .first_loop_ordinal
            .checked_add(value.loop_count)
            .is_none_or(|end| end > MAX_SECTION_BYTES / 100)
        || value.byte_count > MAX_GEOMETRY_CHUNK_BYTES
        || u64::from(value.byte_count) < 68 + 100 * u64::from(value.loop_count)
    {
        return Err("invalid section descriptor counts, bytes or schema".into());
    }
    Ok(())
}
fn validate_loop(value: &SectionLoopMetadata) -> Result<(), String> {
    if value.ordinal >= MAX_SECTION_BYTES / 100
        || !(4..=MAX_LOOP_POINTS).contains(&value.point_count)
    {
        return Err("invalid section loop ordinal or point count".into());
    }
    Ok(())
}
impl TryFrom<rpc::FaceDomainSummary> for FaceDomainSummary {
    type Error = String;
    fn try_from(value: rpc::FaceDomainSummary) -> Result<Self, String> {
        checked(Self::new(
            value.trim_loop_count,
            value.inner_loop_count,
            value.periodic_u,
            value.periodic_v,
            value.has_singular_boundary,
        ))
    }
}
impl TryFrom<FaceDomainSummary> for rpc::FaceDomainSummary {
    type Error = String;
    fn try_from(value: FaceDomainSummary) -> Result<Self, String> {
        checked(value.validate())?;
        Ok(Self {
            trim_loop_count: value.trim_loop_count,
            inner_loop_count: value.inner_loop_count,
            periodic_u: value.periodic_u,
            periodic_v: value.periodic_v,
            has_singular_boundary: value.has_singular_boundary,
        })
    }
}
impl TryFrom<rpc::FaceGeometryInfo> for FaceGeometryInfo {
    type Error = String;
    fn try_from(value: rpc::FaceGeometryInfo) -> Result<Self, String> {
        let family = match rpc::SurfaceFamily::try_from(value.family)
            .map_err(|_| "unknown surface family")?
        {
            rpc::SurfaceFamily::Plane => SurfaceFamily::Plane,
            rpc::SurfaceFamily::Cylinder => SurfaceFamily::Cylinder,
            rpc::SurfaceFamily::Cone => SurfaceFamily::Cone,
            rpc::SurfaceFamily::Sphere => SurfaceFamily::Sphere,
            rpc::SurfaceFamily::Torus => SurfaceFamily::Torus,
            rpc::SurfaceFamily::BSpline => SurfaceFamily::BSpline,
            rpc::SurfaceFamily::Nurbs => SurfaceFamily::Nurbs,
            rpc::SurfaceFamily::LinearExtrusion => SurfaceFamily::LinearExtrusion,
            rpc::SurfaceFamily::Revolution => SurfaceFamily::Revolution,
            rpc::SurfaceFamily::TSpline => SurfaceFamily::TSpline,
            rpc::SurfaceFamily::Unspecified => return Err("unspecified surface family".into()),
        };
        checked(Self::new(
            family,
            value.analytic.map(TryInto::try_into).transpose()?,
            required(value.domain, "face domain")?.try_into()?,
        ))
    }
}
impl TryFrom<FaceGeometryInfo> for rpc::FaceGeometryInfo {
    type Error = String;
    fn try_from(value: FaceGeometryInfo) -> Result<Self, String> {
        checked(value.validate())?;
        let family = match value.family {
            SurfaceFamily::Plane => rpc::SurfaceFamily::Plane,
            SurfaceFamily::Cylinder => rpc::SurfaceFamily::Cylinder,
            SurfaceFamily::Cone => rpc::SurfaceFamily::Cone,
            SurfaceFamily::Sphere => rpc::SurfaceFamily::Sphere,
            SurfaceFamily::Torus => rpc::SurfaceFamily::Torus,
            SurfaceFamily::BSpline => rpc::SurfaceFamily::BSpline,
            SurfaceFamily::Nurbs => rpc::SurfaceFamily::Nurbs,
            SurfaceFamily::LinearExtrusion => rpc::SurfaceFamily::LinearExtrusion,
            SurfaceFamily::Revolution => rpc::SurfaceFamily::Revolution,
            SurfaceFamily::TSpline => rpc::SurfaceFamily::TSpline,
        };
        Ok(Self {
            family: family as i32,
            analytic: value.analytic.map(TryInto::try_into).transpose()?,
            domain: Some(value.domain.try_into()?),
        })
    }
}
impl TryFrom<rpc::AnalyticSurfaceDescription> for AnalyticSurfaceDescription {
    type Error = String;
    fn try_from(value: rpc::AnalyticSurfaceDescription) -> Result<Self, String> {
        use rpc::analytic_surface_description::Kind;
        let value = match required(value.kind, "analytic surface kind")? {
            Kind::Plane(v) => Self::Plane {
                origin_mm: required(v.origin_mm, "origin_mm")?.try_into()?,
                normal: required(v.normal, "normal")?.try_into()?,
            },
            Kind::Cylinder(v) => Self::Cylinder {
                axis_origin_mm: required(v.axis_origin_mm, "axis_origin_mm")?.try_into()?,
                axis_direction: required(v.axis_direction, "axis_direction")?.try_into()?,
                radius_mm: v.radius_mm,
            },
            Kind::Cone(v) => Self::Cone {
                frame: required(v.frame, "cone frame")?.try_into()?,
                reference_radius_mm: v.reference_radius_mm,
                semi_angle_radians: v.semi_angle_radians,
            },
            Kind::Sphere(v) => Self::Sphere {
                frame: required(v.frame, "sphere frame")?.try_into()?,
                radius_mm: v.radius_mm,
            },
            Kind::Torus(v) => Self::Torus {
                frame: required(v.frame, "torus frame")?.try_into()?,
                major_radius_mm: v.major_radius_mm,
                minor_radius_mm: v.minor_radius_mm,
                torus_kind: match rpc::TorusKind::try_from(v.kind)
                    .map_err(|_| "unknown torus kind")?
                {
                    rpc::TorusKind::RegularRing => TorusKind::RegularRing,
                    rpc::TorusKind::Horn => TorusKind::Horn,
                    rpc::TorusKind::Spindle => TorusKind::Spindle,
                    rpc::TorusKind::Unspecified => return Err("unspecified torus kind".into()),
                },
            },
        };
        checked(value.validate())?;
        Ok(value)
    }
}
impl TryFrom<AnalyticSurfaceDescription> for rpc::AnalyticSurfaceDescription {
    type Error = String;
    fn try_from(value: AnalyticSurfaceDescription) -> Result<Self, String> {
        checked(value.validate())?;
        use rpc::analytic_surface_description::Kind;
        let kind = match value {
            AnalyticSurfaceDescription::Plane { origin_mm, normal } => {
                Kind::Plane(rpc::PlaneSurfaceDescription {
                    origin_mm: Some(origin_mm.try_into()?),
                    normal: Some(normal.try_into()?),
                })
            }
            AnalyticSurfaceDescription::Cylinder {
                axis_origin_mm,
                axis_direction,
                radius_mm,
            } => Kind::Cylinder(rpc::CylinderSurfaceDescription {
                axis_origin_mm: Some(axis_origin_mm.try_into()?),
                axis_direction: Some(axis_direction.try_into()?),
                radius_mm,
            }),
            AnalyticSurfaceDescription::Cone {
                frame,
                reference_radius_mm,
                semi_angle_radians,
            } => Kind::Cone(rpc::ConeSurfaceDescription {
                frame: Some(frame.try_into()?),
                reference_radius_mm,
                semi_angle_radians,
            }),
            AnalyticSurfaceDescription::Sphere { frame, radius_mm } => {
                Kind::Sphere(rpc::SphereSurfaceDescription {
                    frame: Some(frame.try_into()?),
                    radius_mm,
                })
            }
            AnalyticSurfaceDescription::Torus {
                frame,
                major_radius_mm,
                minor_radius_mm,
                torus_kind,
            } => Kind::Torus(rpc::TorusSurfaceDescription {
                frame: Some(frame.try_into()?),
                major_radius_mm,
                minor_radius_mm,
                kind: match torus_kind {
                    TorusKind::RegularRing => rpc::TorusKind::RegularRing,
                    TorusKind::Horn => rpc::TorusKind::Horn,
                    TorusKind::Spindle => rpc::TorusKind::Spindle,
                } as i32,
            }),
        };
        Ok(Self { kind: Some(kind) })
    }
}
impl TryFrom<rpc::ArtifactSummary> for ArtifactSummary {
    type Error = String;
    fn try_from(value: rpc::ArtifactSummary) -> Result<Self, String> {
        let value = match required(value.kind, "artifact summary kind")? {
            rpc::artifact_summary::Kind::Mesh(v) => Self::Mesh {
                artifact_id: checked(ArtifactId::new(v.artifact_id))?,
                definition_id: checked(DefinitionId::parse(v.definition_id))?,
                chunk_count: v.chunk_count,
                total_bytes: v.total_bytes,
            },
            rpc::artifact_summary::Kind::Section(v) => Self::Section {
                summary: v.try_into()?,
            },
        };
        validate_artifact_summary(&value)?;
        Ok(value)
    }
}
impl TryFrom<ArtifactSummary> for rpc::ArtifactSummary {
    type Error = String;
    fn try_from(value: ArtifactSummary) -> Result<Self, String> {
        validate_artifact_summary(&value)?;
        let kind = match value {
            ArtifactSummary::Mesh {
                artifact_id,
                definition_id,
                chunk_count,
                total_bytes,
            } => rpc::artifact_summary::Kind::Mesh(rpc::MeshArtifactSummary {
                artifact_id: artifact_id.get(),
                definition_id: definition_id.into(),
                chunk_count,
                total_bytes,
            }),
            ArtifactSummary::Section { summary } => {
                rpc::artifact_summary::Kind::Section(summary.try_into()?)
            }
        };
        Ok(Self { kind: Some(kind) })
    }
}
fn validate_artifact_summary(value: &ArtifactSummary) -> Result<(), String> {
    match value {
        ArtifactSummary::Mesh {
            chunk_count,
            total_bytes,
            ..
        } => {
            if *chunk_count == 0
                || *total_bytes > MAX_SCENE_MESH_BYTES
                || u64::from(*total_bytes)
                    < u64::from(*chunk_count) * crate::display::MESH_HEADER_BYTES as u64
            {
                return Err("invalid mesh artifact summary".into());
            }
        }
        ArtifactSummary::Section { summary } => {
            checked(crate::display::validate_section_summary(summary))?
        }
    }
    Ok(())
}
impl TryFrom<rpc::ArtifactChunkMetadata> for ArtifactChunkMetadata {
    type Error = String;
    fn try_from(value: rpc::ArtifactChunkMetadata) -> Result<Self, String> {
        Ok(match required(value.kind, "chunk kind")? {
            rpc::artifact_chunk_metadata::Kind::Mesh(v) => Self::Mesh {
                metadata: v.try_into()?,
            },
            rpc::artifact_chunk_metadata::Kind::Section(v) => Self::Section {
                metadata: v.try_into()?,
            },
        })
    }
}
impl TryFrom<ArtifactChunkMetadata> for rpc::ArtifactChunkMetadata {
    type Error = String;
    fn try_from(value: ArtifactChunkMetadata) -> Result<Self, String> {
        Ok(Self {
            kind: Some(match value {
                ArtifactChunkMetadata::Mesh { metadata } => {
                    rpc::artifact_chunk_metadata::Kind::Mesh(metadata.try_into()?)
                }
                ArtifactChunkMetadata::Section { metadata } => {
                    rpc::artifact_chunk_metadata::Kind::Section(metadata.try_into()?)
                }
            }),
        })
    }
}

#[derive(Debug, Clone)]
pub enum GeometryCall {
    Execute(rpc::GeometryRequest),
    ImportPart(rpc::ImportPartRequest),
    StartSection(rpc::StartSectionRequest),
    PrepareBedPose(rpc::PrepareBedPoseRequest),
}
impl TryFrom<rpc::AddInstanceRequest> for GeometryCommand {
    type Error = String;
    fn try_from(value: rpc::AddInstanceRequest) -> Result<Self, String> {
        Ok(Self::AddInstance {
            session_id: checked(SessionId::parse(value.session_id))?,
            base_revision: SceneRevision(value.base_revision),
            definition_id: checked(DefinitionId::parse(value.definition_id))?,
            pose: required(value.pose, "pose")?.try_into()?,
        })
    }
}
impl TryFrom<rpc::SetInstancePoseRequest> for GeometryCommand {
    type Error = String;
    fn try_from(value: rpc::SetInstancePoseRequest) -> Result<Self, String> {
        Ok(Self::SetInstancePose {
            session_id: checked(SessionId::parse(value.session_id))?,
            base_revision: SceneRevision(value.base_revision),
            project_revision: crate::project::ProjectRevision(value.project_revision),
            occurrence_id: checked(OccurrenceId::new(value.occurrence_id))?,
            pose: required(value.pose, "pose")?.try_into()?,
        })
    }
}
impl TryFrom<rpc::RemoveInstanceRequest> for GeometryCommand {
    type Error = String;
    fn try_from(value: rpc::RemoveInstanceRequest) -> Result<Self, String> {
        Ok(Self::RemoveInstance {
            session_id: checked(SessionId::parse(value.session_id))?,
            base_revision: SceneRevision(value.base_revision),
            occurrence_id: checked(OccurrenceId::new(value.occurrence_id))?,
        })
    }
}
impl TryFrom<rpc::GetSceneRequest> for GeometryCommand {
    type Error = String;
    fn try_from(value: rpc::GetSceneRequest) -> Result<Self, String> {
        Ok(Self::GetScene {
            session_id: checked(SessionId::parse(value.session_id))?,
        })
    }
}
impl TryFrom<rpc::GetScenePageRequest> for GeometryCommand {
    type Error = String;
    fn try_from(value: rpc::GetScenePageRequest) -> Result<Self, String> {
        Ok(Self::GetScenePage {
            session_id: checked(SessionId::parse(value.session_id))?,
            revision: SceneRevision(value.revision),
            kind: value.kind.try_into()?,
            offset: offset(value.offset, MAX_OCCURRENCES)?,
        })
    }
}
impl TryFrom<rpc::GetFaceIndexPageRequest> for GeometryCommand {
    type Error = String;
    fn try_from(value: rpc::GetFaceIndexPageRequest) -> Result<Self, String> {
        Ok(Self::GetFaceIndexPage {
            session_id: checked(SessionId::parse(value.session_id))?,
            definition_id: checked(DefinitionId::parse(value.definition_id))?,
            offset: offset(value.offset, MAX_NATIVE_FACES)?,
        })
    }
}
impl TryFrom<rpc::InspectFaceRequest> for GeometryCommand {
    type Error = String;
    fn try_from(value: rpc::InspectFaceRequest) -> Result<Self, String> {
        Ok(Self::InspectFace {
            reference: required(value.reference, "reference")?.try_into()?,
        })
    }
}
impl TryFrom<rpc::GetArtifactPageRequest> for GeometryCommand {
    type Error = String;
    fn try_from(value: rpc::GetArtifactPageRequest) -> Result<Self, String> {
        Ok(Self::GetArtifactPage {
            session_id: checked(SessionId::parse(value.session_id))?,
            artifact_id: checked(ArtifactId::new(value.artifact_id))?,
            kind: value.kind.try_into()?,
            offset: offset(value.offset, MAX_SCENE_MESH_BYTES / 64)?,
        })
    }
}
impl TryFrom<rpc::ReleaseArtifactRequest> for GeometryCommand {
    type Error = String;
    fn try_from(value: rpc::ReleaseArtifactRequest) -> Result<Self, String> {
        Ok(Self::ReleaseArtifact {
            session_id: checked(SessionId::parse(value.session_id))?,
            artifact_id: checked(ArtifactId::new(value.artifact_id))?,
        })
    }
}
impl TryFrom<rpc::GeometryRequest> for GeometryCommand {
    type Error = String;
    fn try_from(value: rpc::GeometryRequest) -> Result<Self, String> {
        match required(value.command, "geometry command")? {
            rpc::geometry_request::Command::AddInstance(v) => v.try_into(),
            rpc::geometry_request::Command::SetInstancePose(v) => v.try_into(),
            rpc::geometry_request::Command::RemoveInstance(v) => v.try_into(),
            rpc::geometry_request::Command::GetScene(v) => v.try_into(),
            rpc::geometry_request::Command::GetScenePage(v) => v.try_into(),
            rpc::geometry_request::Command::GetFaceIndexPage(v) => v.try_into(),
            rpc::geometry_request::Command::InspectFace(v) => v.try_into(),
            rpc::geometry_request::Command::GetArtifactPage(v) => v.try_into(),
            rpc::geometry_request::Command::ReleaseArtifact(v) => v.try_into(),
        }
    }
}
impl TryFrom<rpc::ImportPartRequest> for GeometryCommand {
    type Error = String;
    fn try_from(value: rpc::ImportPartRequest) -> Result<Self, String> {
        request_id(&value.request_id)?;
        Ok(Self::ImportPart {
            session_id: session_parent(&value.parent)?,
            base_revision: SceneRevision(value.base_revision),
            source: required(value.source, "source")?.try_into()?,
            initial_pose: required(value.initial_pose, "initial_pose")?.try_into()?,
        })
    }
}
impl TryFrom<rpc::StartSectionRequest> for GeometryCommand {
    type Error = String;
    fn try_from(value: rpc::StartSectionRequest) -> Result<Self, String> {
        request_id(&value.request_id)?;
        Ok(Self::StartSection {
            session_id: session_parent(&value.parent)?,
            base_revision: SceneRevision(value.base_revision),
            plane: required(value.plane, "plane")?.try_into()?,
        })
    }
}
pub fn geometry_call(command: GeometryCommand, id: &str) -> Result<GeometryCall, String> {
    request_id(id)?;
    let command = match command {
        GeometryCommand::ImportPart {
            session_id,
            base_revision,
            source,
            initial_pose,
        } => {
            return Ok(GeometryCall::ImportPart(rpc::ImportPartRequest {
                parent: format!("sessions/{session_id}", session_id = session_id.as_str()),
                request_id: id.into(),
                base_revision: base_revision.get(),
                source: Some(source.try_into()?),
                initial_pose: Some(initial_pose.try_into()?),
            }));
        }
        GeometryCommand::StartSection {
            session_id,
            base_revision,
            plane,
        } => {
            return Ok(GeometryCall::StartSection(rpc::StartSectionRequest {
                parent: format!("sessions/{session_id}", session_id = session_id.as_str()),
                request_id: id.into(),
                base_revision: base_revision.get(),
                plane: Some(plane.try_into()?),
            }));
        }
        GeometryCommand::PrepareBedPose {
            session_id,
            base_revision,
            project_revision,
            occurrence_id,
            expected_definition_id,
            expected_pose,
            envelope_mm,
            center_xy,
            contact_tolerance_mm,
        } => {
            super::bed_pose::validate_request(expected_pose, envelope_mm, contact_tolerance_mm)?;
            return Ok(GeometryCall::PrepareBedPose(rpc::PrepareBedPoseRequest {
                parent: format!("sessions/{}", session_id.as_str()),
                request_id: id.into(),
                base_revision: base_revision.get(),
                project_revision: project_revision.0,
                occurrence_id: occurrence_id.get(),
                expected_definition_id: expected_definition_id.into(),
                expected_pose: Some(expected_pose.try_into()?),
                envelope_mm: Some(envelope_mm.try_into()?),
                center_xy,
                contact_tolerance_mm,
            }));
        }
        GeometryCommand::AddInstance {
            session_id,
            base_revision,
            definition_id,
            pose,
        } => rpc::geometry_request::Command::AddInstance(rpc::AddInstanceRequest {
            session_id: session_id.into(),
            base_revision: base_revision.get(),
            definition_id: definition_id.into(),
            pose: Some(pose.try_into()?),
        }),
        GeometryCommand::SetInstancePose {
            session_id,
            base_revision,
            project_revision,
            occurrence_id,
            pose,
        } => rpc::geometry_request::Command::SetInstancePose(rpc::SetInstancePoseRequest {
            session_id: session_id.into(),
            base_revision: base_revision.get(),
            project_revision: project_revision.0,
            occurrence_id: occurrence_id.get(),
            pose: Some(pose.try_into()?),
        }),
        GeometryCommand::RemoveInstance {
            session_id,
            base_revision,
            occurrence_id,
        } => rpc::geometry_request::Command::RemoveInstance(rpc::RemoveInstanceRequest {
            session_id: session_id.into(),
            base_revision: base_revision.get(),
            occurrence_id: occurrence_id.get(),
        }),
        GeometryCommand::GetScene { session_id } => {
            rpc::geometry_request::Command::GetScene(rpc::GetSceneRequest {
                session_id: session_id.into(),
            })
        }
        GeometryCommand::GetScenePage {
            session_id,
            revision,
            kind,
            offset: offset_value,
        } => rpc::geometry_request::Command::GetScenePage(rpc::GetScenePageRequest {
            session_id: session_id.into(),
            revision: revision.get(),
            kind: rpc::ScenePageKind::try_from(kind)? as i32,
            offset: offset(offset_value, MAX_OCCURRENCES)?,
        }),
        GeometryCommand::GetFaceIndexPage {
            session_id,
            definition_id,
            offset: offset_value,
        } => rpc::geometry_request::Command::GetFaceIndexPage(rpc::GetFaceIndexPageRequest {
            session_id: session_id.into(),
            definition_id: definition_id.into(),
            offset: offset(offset_value, MAX_NATIVE_FACES)?,
        }),
        GeometryCommand::InspectFace { reference } => {
            rpc::geometry_request::Command::InspectFace(rpc::InspectFaceRequest {
                reference: Some(reference.try_into()?),
            })
        }
        GeometryCommand::GetArtifactPage {
            session_id,
            artifact_id,
            kind,
            offset: offset_value,
        } => rpc::geometry_request::Command::GetArtifactPage(rpc::GetArtifactPageRequest {
            session_id: session_id.into(),
            artifact_id: artifact_id.get(),
            kind: rpc::ArtifactPageKind::try_from(kind)? as i32,
            offset: offset(offset_value, MAX_SCENE_MESH_BYTES / 64)?,
        }),
        GeometryCommand::ReleaseArtifact {
            session_id,
            artifact_id,
        } => rpc::geometry_request::Command::ReleaseArtifact(rpc::ReleaseArtifactRequest {
            session_id: session_id.into(),
            artifact_id: artifact_id.get(),
        }),
    };
    Ok(GeometryCall::Execute(rpc::GeometryRequest {
        command: Some(command),
    }))
}
impl TryFrom<GeometryCommand> for rpc::GeometryRequest {
    type Error = String;
    fn try_from(value: GeometryCommand) -> Result<Self, String> {
        match geometry_call(value, "")? {
            GeometryCall::Execute(request) => Ok(request),
            _ => Err("long geometry commands require their dedicated RPC".into()),
        }
    }
}
impl TryFrom<GeometryCommand> for rpc::ImportPartRequest {
    type Error = String;
    fn try_from(value: GeometryCommand) -> Result<Self, String> {
        match geometry_call(value, "")? {
            GeometryCall::ImportPart(request) => Ok(request),
            _ => Err("expected import_part command".into()),
        }
    }
}
impl TryFrom<GeometryCommand> for rpc::StartSectionRequest {
    type Error = String;
    fn try_from(value: GeometryCommand) -> Result<Self, String> {
        match geometry_call(value, "")? {
            GeometryCall::StartSection(request) => Ok(request),
            _ => Err("expected start_section command".into()),
        }
    }
}

fn page_end(length: usize, next: Option<u32>, cap: u32, maximum: u32) -> Result<(), String> {
    if length > cap as usize
        || next.is_some_and(|n| length != cap as usize || n < cap || n > maximum)
    {
        return Err("invalid bounded page or next offset".into());
    }
    Ok(())
}
fn sequential(ordinals: impl Iterator<Item = u32>, next: Option<u32>) -> Result<(), String> {
    let mut previous: Option<u32> = None;
    for ordinal in ordinals {
        if previous.is_some_and(|p| p.checked_add(1) != Some(ordinal)) {
            return Err("page ordinals are not contiguous".into());
        }
        previous = Some(ordinal);
    }
    if next.is_some_and(|n| previous.and_then(|p| p.checked_add(1)) != Some(n)) {
        return Err("next offset does not follow page".into());
    }
    Ok(())
}
fn validate_response(value: &GeometryResponse) -> Result<(), String> {
    match value {
        GeometryResponse::Scene { summary } | GeometryResponse::SceneChanged { summary } => {
            checked(summary.validate())?
        }
        GeometryResponse::ScenePage {
            definitions,
            occurrences,
            next_offset,
        } => {
            if !definitions.is_empty() && !occurrences.is_empty() {
                return Err("scene page mixes record kinds".into());
            }
            page_end(
                definitions.len() + occurrences.len(),
                *next_offset,
                SCENE_PAGE_SIZE,
                MAX_OCCURRENCES,
            )?;
            if definitions.len() > MAX_DEFINITIONS as usize
                || next_offset.is_some_and(|n| !definitions.is_empty() && n > MAX_DEFINITIONS)
            {
                return Err("definition page exceeds support envelope".into());
            }
            let mut ids = std::collections::BTreeSet::new();
            for definition in definitions {
                checked(definition.validate())?;
                if !ids.insert(&definition.definition_id) {
                    return Err("duplicate definition in page".into());
                }
            }
            let mut ids = std::collections::BTreeSet::new();
            for occurrence in occurrences {
                checked(occurrence.pose.validate())?;
                if !ids.insert(occurrence.occurrence_id) {
                    return Err("duplicate occurrence in page".into());
                }
            }
        }
        GeometryResponse::FaceIndexPage { faces, next_offset } => {
            page_end(faces.len(), *next_offset, FACE_PAGE_SIZE, MAX_NATIVE_FACES)?;
            sequential(faces.iter().map(|f| f.ordinal), *next_offset)?;
            let mut ids = std::collections::BTreeSet::new();
            for face in faces {
                if face.ordinal >= MAX_NATIVE_FACES || !ids.insert(&face.face_id) {
                    return Err("invalid or duplicate face index".into());
                }
            }
        }
        GeometryResponse::FaceInspection { inspection } => validate_inspection(inspection)?,
        GeometryResponse::ArtifactPage {
            summary,
            chunks,
            loops,
            resources,
            next_offset,
        } => {
            validate_artifact_summary(summary)?;
            if !chunks.is_empty() && !loops.is_empty() {
                return Err("artifact page mixes chunks and loops".into());
            }
            if chunks.len() != resources.len() {
                return Err("artifact resources do not correspond to chunks".into());
            }
            let (count, bytes, loop_count) = match summary {
                ArtifactSummary::Mesh {
                    chunk_count,
                    total_bytes,
                    ..
                } => {
                    if !loops.is_empty() {
                        return Err("mesh artifact has section loops".into());
                    }
                    (*chunk_count, *total_bytes, 0)
                }
                ArtifactSummary::Section { summary } => (
                    summary.chunk_count,
                    summary.total_bytes,
                    summary.total_loop_count,
                ),
            };
            page_end(
                chunks.len() + loops.len(),
                *next_offset,
                ARTIFACT_PAGE_SIZE,
                count.max(loop_count),
            )?;
            if !chunks.is_empty() {
                sequential(
                    chunks.iter().map(|chunk| match chunk {
                        ArtifactChunkMetadata::Mesh { metadata } => metadata.chunk_index,
                        ArtifactChunkMetadata::Section { metadata } => metadata.chunk_index,
                    }),
                    *next_offset,
                )?;
                let last = match chunks.last().expect("nonempty chunks") {
                    ArtifactChunkMetadata::Mesh { metadata } => metadata.chunk_index,
                    ArtifactChunkMetadata::Section { metadata } => metadata.chunk_index,
                };
                if next_offset.is_some() != (last.checked_add(1).is_some_and(|end| end < count)) {
                    return Err("chunk page continuation differs from summary".into());
                }
            }
            if let Some(last) = loops.last() {
                sequential(loops.iter().map(|l| l.ordinal), *next_offset)?;
                if next_offset.is_some()
                    != (last
                        .ordinal
                        .checked_add(1)
                        .is_some_and(|end| end < loop_count))
                {
                    return Err("loop page continuation differs from summary".into());
                }
            }
            let mut names = std::collections::BTreeSet::new();
            let mut page_bytes = 0u64;
            let mut mesh_session: Option<&SessionId> = None;
            let mut section_loop_end = None;
            for (chunk, resource) in chunks.iter().zip(resources) {
                let (byte_count, sha256) = match (summary, chunk) {
                    (
                        ArtifactSummary::Mesh {
                            artifact_id,
                            definition_id,
                            chunk_count,
                            ..
                        },
                        ArtifactChunkMetadata::Mesh { metadata },
                    ) => {
                        validate_mesh_metadata(metadata)?;
                        if metadata.artifact_id != *artifact_id
                            || metadata.definition_id != *definition_id
                            || metadata.chunk_count != *chunk_count
                            || mesh_session.is_some_and(|s| s != &metadata.session_id)
                        {
                            return Err("mesh chunk differs from artifact summary".into());
                        }
                        mesh_session = Some(&metadata.session_id);
                        (metadata.byte_count, &metadata.sha256)
                    }
                    (
                        ArtifactSummary::Section { summary },
                        ArtifactChunkMetadata::Section { metadata },
                    ) => {
                        validate_section_metadata(metadata)?;
                        let end = metadata
                            .first_loop_ordinal
                            .checked_add(metadata.loop_count)
                            .ok_or("section loop range overflows")?;
                        if metadata.session_id != summary.session_id
                            || metadata.artifact_id != summary.artifact_id
                            || metadata.chunk_count != summary.chunk_count
                            || end > summary.total_loop_count
                            || section_loop_end.is_some_and(|e| e != metadata.first_loop_ordinal)
                        {
                            return Err("section chunk differs from artifact summary".into());
                        }
                        section_loop_end = Some(end);
                        (metadata.byte_count, &metadata.sha256)
                    }
                    _ => return Err("chunk kind differs from artifact kind".into()),
                };
                let size = validate_artifact_view(resource, u64::from(MAX_GEOMETRY_CHUNK_BYTES))?;
                if size != u64::from(byte_count)
                    || resource.sha256 != sha256.as_str()
                    || !names.insert(&resource.name)
                {
                    return Err("artifact resource does not match packed chunk".into());
                }
                page_bytes += u64::from(byte_count);
            }
            if page_bytes > u64::from(bytes) {
                return Err("chunk page exceeds total artifact bytes".into());
            }
            for metadata in loops {
                validate_loop(metadata)?;
                if metadata.ordinal >= loop_count {
                    return Err("loop ordinal exceeds artifact summary".into());
                }
            }
        }
        GeometryResponse::Released {} => {}
        GeometryResponse::OperationAccepted { .. } => {
            return Err("long geometry RPC returns Operation, not GeometryReply".into());
        }
        GeometryResponse::Error { .. } | GeometryResponse::ProjectError { .. } => {
            return Err("domain failures require canonical RPC Status".into());
        }
    }
    Ok(())
}
impl TryFrom<rpc::GeometryReply> for GeometryResponse {
    type Error = String;
    fn try_from(value: rpc::GeometryReply) -> Result<Self, String> {
        use rpc::geometry_reply::Response;
        let value = match required(value.response, "geometry reply response")? {
            Response::SceneChanged(summary) => Self::SceneChanged {
                summary: summary.try_into()?,
            },
            Response::Scene(summary) => Self::Scene {
                summary: summary.try_into()?,
            },
            Response::ScenePage(v) => {
                page_end(
                    v.definitions.len() + v.occurrences.len(),
                    v.next_offset,
                    SCENE_PAGE_SIZE,
                    MAX_OCCURRENCES,
                )?;
                Self::ScenePage {
                    definitions: v
                        .definitions
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                    occurrences: v
                        .occurrences
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                    next_offset: v.next_offset,
                }
            }
            Response::FaceIndexPage(v) => {
                page_end(
                    v.faces.len(),
                    v.next_offset,
                    FACE_PAGE_SIZE,
                    MAX_NATIVE_FACES,
                )?;
                Self::FaceIndexPage {
                    faces: v
                        .faces
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                    next_offset: v.next_offset,
                }
            }
            Response::FaceInspection(inspection) => Self::FaceInspection {
                inspection: inspection.try_into()?,
            },
            Response::ArtifactPage(v) => {
                page_end(
                    v.chunks.len() + v.loops.len(),
                    v.next_offset,
                    ARTIFACT_PAGE_SIZE,
                    MAX_SCENE_MESH_BYTES / 64,
                )?;
                if v.resources.len() != v.chunks.len() {
                    return Err("artifact resource count differs from chunks".into());
                }
                Self::ArtifactPage {
                    summary: required(v.summary, "artifact summary")?.try_into()?,
                    chunks: v
                        .chunks
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                    loops: v
                        .loops
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                    resources: v
                        .resources
                        .into_iter()
                        .map(|resource| {
                            artifact_to_view(resource, u64::from(MAX_GEOMETRY_CHUNK_BYTES))
                        })
                        .collect::<Result<_, _>>()?,
                    next_offset: v.next_offset,
                }
            }
            Response::Released(()) => Self::Released {},
        };
        validate_response(&value)?;
        Ok(value)
    }
}
impl TryFrom<GeometryResponse> for rpc::GeometryReply {
    type Error = String;
    fn try_from(value: GeometryResponse) -> Result<Self, String> {
        validate_response(&value)?;
        use rpc::geometry_reply::Response;
        let response = match value {
            GeometryResponse::SceneChanged { summary } => {
                Response::SceneChanged(summary.try_into()?)
            }
            GeometryResponse::Scene { summary } => Response::Scene(summary.try_into()?),
            GeometryResponse::ScenePage {
                definitions,
                occurrences,
                next_offset,
            } => Response::ScenePage(rpc::ScenePage {
                definitions: definitions
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                occurrences: occurrences
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                next_offset,
            }),
            GeometryResponse::FaceIndexPage { faces, next_offset } => {
                Response::FaceIndexPage(rpc::FaceIndexPage {
                    faces: faces
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                    next_offset,
                })
            }
            GeometryResponse::FaceInspection { inspection } => {
                Response::FaceInspection(inspection.try_into()?)
            }
            GeometryResponse::ArtifactPage {
                summary,
                chunks,
                loops,
                resources,
                next_offset,
            } => Response::ArtifactPage(rpc::ArtifactPage {
                summary: Some(summary.try_into()?),
                chunks: chunks
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                loops: loops
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
                resources: resources
                    .into_iter()
                    .map(|resource| {
                        artifact_from_view(resource, u64::from(MAX_GEOMETRY_CHUNK_BYTES))
                    })
                    .collect::<Result<_, _>>()?,
                next_offset,
            }),
            GeometryResponse::Released {} => Response::Released(()),
            GeometryResponse::OperationAccepted { .. }
            | GeometryResponse::Error { .. }
            | GeometryResponse::ProjectError { .. } => {
                return Err("geometry reply must contain a short RPC success".into());
            }
        };
        Ok(Self {
            response: Some(response),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_geometry_command_uses_checked_typed_dispatch() {
        let session = SessionId::new();
        let definition = DefinitionId::parse("a".repeat(64)).unwrap();
        let occurrence = OccurrenceId::new(1).unwrap();
        let artifact = ArtifactId::new(1).unwrap();
        let revision = SceneRevision(7);
        let plane = PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        };
        let reference = FaceRef {
            session_id: session.clone(),
            scene_revision: revision,
            occurrence_id: occurrence,
            definition_id: definition.clone(),
            face_id: FaceId::from_step_entity(9007199254740993).unwrap(),
        };
        let commands = [
            GeometryCommand::ImportPart {
                session_id: session.clone(),
                base_revision: revision,
                source: NativePath::UnixBytes { hex: "2fff".into() },
                initial_pose: RigidPoseMm::IDENTITY,
            },
            GeometryCommand::AddInstance {
                session_id: session.clone(),
                base_revision: revision,
                definition_id: definition.clone(),
                pose: RigidPoseMm::IDENTITY,
            },
            GeometryCommand::SetInstancePose {
                session_id: session.clone(),
                base_revision: revision,
                project_revision: crate::project::ProjectRevision(7),
                occurrence_id: occurrence,
                pose: RigidPoseMm::IDENTITY,
            },
            GeometryCommand::RemoveInstance {
                session_id: session.clone(),
                base_revision: revision,
                occurrence_id: occurrence,
            },
            GeometryCommand::GetScene {
                session_id: session.clone(),
            },
            GeometryCommand::GetScenePage {
                session_id: session.clone(),
                revision,
                kind: ScenePageKind::Definitions,
                offset: 0,
            },
            GeometryCommand::GetScenePage {
                session_id: session.clone(),
                revision,
                kind: ScenePageKind::Occurrences,
                offset: 0,
            },
            GeometryCommand::GetFaceIndexPage {
                session_id: session.clone(),
                definition_id: definition,
                offset: 0,
            },
            GeometryCommand::InspectFace { reference },
            GeometryCommand::StartSection {
                session_id: session.clone(),
                base_revision: revision,
                plane,
            },
            GeometryCommand::GetArtifactPage {
                session_id: session.clone(),
                artifact_id: artifact,
                kind: ArtifactPageKind::Chunks,
                offset: 0,
            },
            GeometryCommand::GetArtifactPage {
                session_id: session.clone(),
                artifact_id: artifact,
                kind: ArtifactPageKind::Loops,
                offset: 0,
            },
            GeometryCommand::ReleaseArtifact {
                session_id: session,
                artifact_id: artifact,
            },
        ];
        let id = SessionId::new();
        for command in commands {
            let decoded = match geometry_call(command.clone(), id.as_str()).unwrap() {
                GeometryCall::Execute(request) => GeometryCommand::try_from(request).unwrap(),
                GeometryCall::ImportPart(request) => {
                    assert_eq!(request.request_id, id.as_str());
                    GeometryCommand::try_from(request).unwrap()
                }
                GeometryCall::StartSection(request) => {
                    assert_eq!(request.request_id, id.as_str());
                    GeometryCommand::try_from(request).unwrap()
                }
                GeometryCall::PrepareBedPose(request) => {
                    assert_eq!(request.request_id, id.as_str());
                    GeometryCommand::try_from(request).unwrap()
                }
            };
            assert_eq!(decoded, command);
        }
        assert!(
            geometry_call(
                GeometryCommand::GetScene {
                    session_id: SessionId::new()
                },
                "invalid"
            )
            .is_err()
        );
    }

    #[test]
    fn definition_display_roundtrip_requires_explicit_mesh_or_bounded_refusal() {
        for display in [
            DefinitionDisplay::Mesh {
                artifact_id: ArtifactId::new(1).unwrap(),
            },
            DefinitionDisplay::Unavailable {
                error: GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "source chart unavailable for display",
                ),
            },
        ] {
            let wire = rpc::DefinitionDisplay::try_from(display.clone()).unwrap();
            assert_eq!(DefinitionDisplay::try_from(wire).unwrap(), display);
        }
        assert!(DefinitionDisplay::try_from(rpc::DefinitionDisplay { kind: None }).is_err());
        assert!(
            DefinitionDisplay::try_from(rpc::DefinitionDisplay {
                kind: Some(rpc::definition_display::Kind::MeshArtifactId(0)),
            })
            .is_err()
        );
        assert!(
            DefinitionDisplay::try_from(rpc::DefinitionDisplay {
                kind: Some(rpc::definition_display::Kind::Unavailable(
                    rpc::DomainError {
                        code: "unknown".into(),
                        message: "refusal".into(),
                    }
                )),
            })
            .is_err()
        );
        assert!(
            DefinitionDisplay::try_from(rpc::DefinitionDisplay {
                kind: Some(rpc::definition_display::Kind::Unavailable(
                    rpc::DomainError {
                        code: "unsupported_operation".into(),
                        message: "x".repeat(MAX_ERROR_MESSAGE_BYTES as usize + 1),
                    }
                )),
            })
            .is_err()
        );
        assert!(
            rpc::DefinitionDisplay::try_from(DefinitionDisplay::Unavailable {
                error: GeometryError {
                    code: GeometryErrorCode::UnsupportedOperation,
                    message: "x".repeat(MAX_ERROR_MESSAGE_BYTES as usize + 1)
                },
            })
            .is_err()
        );
        assert!(
            DefinitionRecord::try_from(rpc::DefinitionRecord {
                definition_id: "a".repeat(64),
                provenance: Some(rpc::SourceProvenance {
                    source_hash: "b".repeat(64),
                    source_name: "part".into(),
                    source_unit: rpc::SourceUnit::Millimetre as i32,
                    uncertainty_mm: None,
                }),
                face_count: 1,
                bounds_mm: Some(AabbMm::new([0.0; 3], [1.0; 3]).unwrap().try_into().unwrap()),
                display: None,
            })
            .is_err()
        );
    }

    #[test]
    fn face_geometry_roundtrip_preserves_encoding_family_domains_and_singularities() {
        let frame = PlaneFrameMm::from_plane(PlaneMm {
            origin_mm: [1.0, 2.0, 3.0],
            normal: [0.0, 0.0, 1.0],
        })
        .unwrap();
        let descriptions = [
            AnalyticSurfaceDescription::Plane {
                origin_mm: frame.origin_mm,
                normal: frame.z_axis,
            },
            AnalyticSurfaceDescription::Cylinder {
                axis_origin_mm: frame.origin_mm,
                axis_direction: frame.z_axis,
                radius_mm: 2.0,
            },
            AnalyticSurfaceDescription::Cone {
                frame,
                reference_radius_mm: 2.0,
                semi_angle_radians: -0.2,
            },
            AnalyticSurfaceDescription::Sphere {
                frame,
                radius_mm: 3.0,
            },
            AnalyticSurfaceDescription::Torus {
                frame,
                major_radius_mm: 3.0,
                minor_radius_mm: 3.0,
                torus_kind: TorusKind::Horn,
            },
        ];
        for description in descriptions {
            let face = FaceInfo {
                face_id: FaceId::from_step_entity(9_007_199_254_740_993).unwrap(),
                surface_face_entity: "9007199254740994".into(),
                source_entity_kind: "ADVANCED_FACE".into(),
                source_surface_entity_kind: "B_SPLINE_SURFACE_WITH_KNOTS".into(),
                orientation: false,
                geometry: FaceGeometryInfo::new(
                    SurfaceFamily::Nurbs,
                    Some(description),
                    FaceDomainSummary::new(2, 1, true, false, true).unwrap(),
                )
                .unwrap(),
            };
            let wire = rpc::FaceInfo::try_from(face.clone()).unwrap();
            assert_eq!(FaceInfo::try_from(wire.clone()).unwrap(), face);
            let mut malformed = wire;
            malformed.geometry.as_mut().unwrap().family = i32::MAX;
            assert!(FaceInfo::try_from(malformed).is_err());
        }
        assert!(
            FaceGeometryInfo::try_from(rpc::FaceGeometryInfo {
                family: rpc::SurfaceFamily::Plane as i32,
                analytic: None,
                domain: None,
            })
            .is_err()
        );
        assert!(
            rpc::FaceGeometryInfo::try_from(FaceGeometryInfo {
                family: SurfaceFamily::Sphere,
                analytic: Some(AnalyticSurfaceDescription::Plane {
                    origin_mm: [0.0; 3],
                    normal: [0.0, 0.0, 1.0]
                }),
                domain: FaceDomainSummary::new(1, 0, false, false, false).unwrap(),
            })
            .is_err()
        );
        assert!(
            AnalyticSurfaceDescription::try_from(rpc::AnalyticSurfaceDescription { kind: None })
                .is_err()
        );
    }

    #[test]
    fn reply_rejects_domain_errors_and_invalid_wire_values() {
        assert!(GeometryResponse::try_from(rpc::GeometryReply { response: None }).is_err());
        let error = GeometryResponse::Error {
            error: GeometryError::new(GeometryErrorCode::InvalidGeometry, "invalid"),
        };
        assert!(rpc::GeometryReply::try_from(error).is_err());
        let summary = SceneSummary {
            session_id: SessionId::new(),
            revision: SceneRevision::ZERO,
            definition_count: 0,
            occurrence_count: 0,
            bounds_mm: None,
            unique_mesh_bytes: 0,
        };
        let response = GeometryResponse::Scene { summary };
        assert_eq!(
            GeometryResponse::try_from(rpc::GeometryReply::try_from(response.clone()).unwrap())
                .unwrap(),
            response
        );
        assert!(
            GeometryCommand::try_from(rpc::GetScenePageRequest {
                session_id: SessionId::new().into(),
                revision: 0,
                kind: 0,
                offset: 0
            })
            .is_err()
        );
        assert!(
            AnalyticSurfaceDescription::try_from(rpc::AnalyticSurfaceDescription {
                kind: Some(rpc::analytic_surface_description::Kind::Plane(
                    rpc::PlaneSurfaceDescription {
                        origin_mm: Some(rpc::Vec3 {
                            x: f64::NAN,
                            y: 0.0,
                            z: 0.0
                        }),
                        normal: Some(rpc::Vec3 {
                            x: 0.0,
                            y: 0.0,
                            z: 1.0
                        })
                    }
                ))
            })
            .is_err()
        );
        assert!(
            SectionLoopMetadata::try_from(rpc::SectionLoopMetadata {
                ordinal: 0,
                occurrence_id: 1,
                definition_id: "a".repeat(64),
                is_hole: false,
                point_count: MAX_LOOP_POINTS + 1
            })
            .is_err()
        );
    }

    #[test]
    fn artifact_pages_require_matching_resources_and_metadata() {
        let hash = SourceHash::parse("b".repeat(64)).unwrap();
        let artifact = ArtifactId::new(1).unwrap();
        let definition = DefinitionId::parse("a".repeat(64)).unwrap();
        let metadata = MeshChunkMetadata {
            session_id: SessionId::new(),
            artifact_id: artifact,
            definition_id: definition.clone(),
            schema_version: crate::display::MESH_SCHEMA_VERSION,
            mesh_profile: DisplayProfile::MeshTrimmedMm005V2,
            chunk_index: 0,
            chunk_count: 1,
            byte_count: 256,
            sha256: hash.clone(),
            face_count: 1,
            bounds_mm: AabbMm::new([0.0; 3], [1.0; 3]).unwrap(),
            carrier_deviation_mm: 0.0,
            trim_deviation_mm: 0.0,
            quantization_error_mm: 0.0,
        };
        let mut achieved = metadata.clone();
        achieved.carrier_deviation_mm = 0.01;
        achieved.trim_deviation_mm = 0.005;
        achieved.quantization_error_mm = 0.02;
        let wire = rpc::MeshChunkMetadata::try_from(achieved.clone()).unwrap();
        assert_eq!(MeshChunkMetadata::try_from(wire.clone()).unwrap(), achieved);
        assert_eq!(
            serde_json::from_value::<MeshChunkMetadata>(serde_json::to_value(&achieved).unwrap())
                .unwrap(),
            achieved
        );
        let mut absent_trim = wire.clone();
        absent_trim.trim_deviation_mm = None;
        assert!(MeshChunkMetadata::try_from(absent_trim).is_err());
        let mut old_profile = wire.clone();
        old_profile.mesh_profile = 1;
        assert!(MeshChunkMetadata::try_from(old_profile).is_err());
        let mut over_budget = achieved.clone();
        over_budget.trim_deviation_mm = 0.02;
        assert!(rpc::MeshChunkMetadata::try_from(over_budget.clone()).is_err());
        assert!(
            serde_json::from_value::<MeshChunkMetadata>(serde_json::to_value(over_budget).unwrap())
                .is_err()
        );
        let mut missing = serde_json::to_value(achieved).unwrap();
        missing.as_object_mut().unwrap().remove("trim_deviation_mm");
        assert!(serde_json::from_value::<MeshChunkMetadata>(missing).is_err());
        let response = GeometryResponse::ArtifactPage {
            summary: ArtifactSummary::Mesh {
                artifact_id: artifact,
                definition_id: definition,
                chunk_count: 1,
                total_bytes: 256,
            },
            chunks: vec![ArtifactChunkMetadata::Mesh { metadata }],
            loops: vec![],
            next_offset: None,
            resources: vec![crate::ArtifactView {
                name: format!("artifacts/{}", hash.as_str()),
                size_bytes: "256".into(),
                sha256: hash.into(),
                media_type: "application/octet-stream".into(),
            }],
        };
        let wire = rpc::GeometryReply::try_from(response.clone()).unwrap();
        assert_eq!(GeometryResponse::try_from(wire.clone()).unwrap(), response);
        let mut wrong_size = wire.clone();
        if let Some(rpc::geometry_reply::Response::ArtifactPage(page)) = &mut wrong_size.response {
            page.resources[0].size_bytes += 1;
        }
        assert!(GeometryResponse::try_from(wrong_size).is_err());
        let mut missing_resource = wire.clone();
        if let Some(rpc::geometry_reply::Response::ArtifactPage(page)) =
            &mut missing_resource.response
        {
            page.resources.clear();
        }
        assert!(GeometryResponse::try_from(missing_resource).is_err());
        let mut wrong_schema = wire;
        if let Some(rpc::geometry_reply::Response::ArtifactPage(page)) = &mut wrong_schema.response
            && let Some(rpc::artifact_chunk_metadata::Kind::Mesh(metadata)) =
                &mut page.chunks[0].kind
        {
            metadata.schema_version = u32::MAX;
        }
        assert!(GeometryResponse::try_from(wrong_schema).is_err());
    }
}
