// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Checked domain adapters over the one authenticated gRPC connection.
use super::{ClientError, EngineClient, EngineRpc};
use prost::Message;
use spiling_contracts::{
    ArtifactView, MAX_CONTROL_BYTES,
    display::{validate_mesh_chunk, validate_section_chunk, validate_section_summary},
    geometry::{
        ArtifactChunkMetadata, ArtifactSummary, GeometryCommand, GeometryError, GeometryErrorCode,
        GeometryResponse, JobError, MAX_GEOMETRY_CHUNK_BYTES,
    },
    manufacturing::{
        MAX_MANUFACTURING_BUNDLE_BYTES, ManufacturingArtifactRecord, ManufacturingBundle,
        ManufacturingCommand, ManufacturingError, ManufacturingErrorCode, ManufacturingResponse,
        decode_bundle,
    },
    native::{self, GeometryCall, ManufacturingCall, NativeOperationView, ProjectCall},
    project::{ProjectCommand, ProjectResponse},
    rpc,
};
use uuid::Uuid;

fn domain_error(error: JobError) -> ClientError {
    match error {
        JobError::Geometry { error } => ClientError::Geometry(error),
        JobError::Project { error } => ClientError::Project(error),
        JobError::Manufacturing { error } => ClientError::Manufacturing(error),
    }
}
pub(crate) fn rpc_error(status: tonic::Status) -> ClientError {
    if status.details().is_empty() {
        return ClientError::Rpc(status);
    }
    match spiling_contracts::google::rpc::Status::decode(status.details()) {
        Ok(details)
            if details.code == status.code() as i32 && details.message == status.message() =>
        {
            match native::domain_error(&details) {
                Ok(error) => domain_error(error),
                Err(error) => ClientError::Protocol(error),
            }
        }
        Ok(_) => {
            ClientError::Protocol("structured RPC error disagrees with canonical status".into())
        }
        Err(error) => ClientError::Protocol(format!("invalid structured RPC error: {error}")),
    }
}
fn bounded<T: Message>(message: T) -> Result<T, ClientError> {
    if message.encoded_len() > MAX_CONTROL_BYTES as usize {
        return Err(ClientError::InvalidRequest(
            "native control exceeds limit".into(),
        ));
    }
    Ok(message)
}
fn corrupt_manufacturing(message: &str) -> ClientError {
    ManufacturingError::new(ManufacturingErrorCode::CorruptArtifact, message).into()
}
impl EngineRpc {
    pub async fn geometry(
        &self,
        command: GeometryCommand,
    ) -> Result<GeometryResponse, ClientError> {
        self.geometry_with_request_id(command, &Uuid::new_v4().to_string())
            .await
    }
    pub async fn geometry_with_request_id(
        &self,
        command: GeometryCommand,
        request_id: &str,
    ) -> Result<GeometryResponse, ClientError> {
        match self.geometry_exchange(command, request_id).await {
            Err(ClientError::Geometry(error)) => Ok(GeometryResponse::Error { error }),
            result => result,
        }
    }
    async fn geometry_exchange(
        &self,
        command: GeometryCommand,
        request_id: &str,
    ) -> Result<GeometryResponse, ClientError> {
        let validation = match &command {
            GeometryCommand::ImportPart {
                source,
                initial_pose,
                ..
            } => source.validate().and_then(|()| initial_pose.validate()),
            GeometryCommand::AddInstance { pose, .. }
            | GeometryCommand::SetInstancePose { pose, .. } => pose.validate(),
            GeometryCommand::StartSection { plane, .. } => plane.validate(),
            _ => Ok(()),
        };
        if let Err(error) = validation {
            return Ok(GeometryResponse::Error { error });
        }
        let call =
            native::geometry_call(command, request_id).map_err(ClientError::InvalidRequest)?;
        let mut client = rpc::geometry_client::GeometryClient::new(self.channel.clone())
            .max_decoding_message_size(MAX_CONTROL_BYTES as usize)
            .max_encoding_message_size(MAX_CONTROL_BYTES as usize);
        match call {
            GeometryCall::Execute(request) => {
                let reply = client
                    .execute(self.request(bounded(request)?))
                    .await
                    .map_err(rpc_error)?
                    .into_inner();
                let response = GeometryResponse::try_from(reply).map_err(ClientError::Protocol)?;
                match response {
                    GeometryResponse::ProjectError { error } => Err(ClientError::Project(error)),
                    response => Ok(response),
                }
            }
            GeometryCall::ImportPart(request) => {
                let operation = client
                    .import_part(self.request(bounded(request)?))
                    .await
                    .map_err(rpc_error)?
                    .into_inner();
                Ok(GeometryResponse::OperationAccepted {
                    operation: native::native_operation_view(&operation)
                        .map_err(ClientError::Protocol)?,
                })
            }
            GeometryCall::StartSection(request) => {
                let operation = client
                    .start_section(self.request(bounded(request)?))
                    .await
                    .map_err(rpc_error)?
                    .into_inner();
                Ok(GeometryResponse::OperationAccepted {
                    operation: native::native_operation_view(&operation)
                        .map_err(ClientError::Protocol)?,
                })
            }
        }
    }
    pub async fn project(&self, command: ProjectCommand) -> Result<ProjectResponse, ClientError> {
        self.project_with_request_id(command, &Uuid::new_v4().to_string())
            .await
    }
    pub async fn project_with_request_id(
        &self,
        command: ProjectCommand,
        request_id: &str,
    ) -> Result<ProjectResponse, ClientError> {
        match &command {
            ProjectCommand::Open { path, .. } => path.validate(),
            ProjectCommand::Save {
                target: Some(path), ..
            } => path.validate(),
            _ => Ok(()),
        }
        .map_err(|error| ClientError::InvalidRequest(error.to_string()))?;
        let call =
            native::project_call(command, request_id).map_err(ClientError::InvalidRequest)?;
        let mut client = rpc::projects_client::ProjectsClient::new(self.channel.clone())
            .max_decoding_message_size(MAX_CONTROL_BYTES as usize)
            .max_encoding_message_size(MAX_CONTROL_BYTES as usize);
        match call {
            ProjectCall::Execute(request) => {
                let reply = client
                    .execute(self.request(bounded(request)?))
                    .await
                    .map_err(rpc_error)?
                    .into_inner();
                match ProjectResponse::try_from(reply).map_err(ClientError::Protocol)? {
                    ProjectResponse::Error { error } => Err(error.into()),
                    response => Ok(response),
                }
            }
            ProjectCall::Open(request) => {
                let operation = client
                    .open(self.request(bounded(request)?))
                    .await
                    .map_err(rpc_error)?
                    .into_inner();
                Ok(ProjectResponse::OperationAccepted {
                    operation: native::native_operation_view(&operation)
                        .map_err(ClientError::Protocol)?,
                })
            }
            ProjectCall::Save(request) => {
                let operation = client
                    .save(self.request(bounded(request)?))
                    .await
                    .map_err(rpc_error)?
                    .into_inner();
                Ok(ProjectResponse::OperationAccepted {
                    operation: native::native_operation_view(&operation)
                        .map_err(ClientError::Protocol)?,
                })
            }
        }
    }
    pub async fn manufacturing(
        &self,
        command: ManufacturingCommand,
    ) -> Result<ManufacturingResponse, ClientError> {
        self.manufacturing_with_request_id(command, &Uuid::new_v4().to_string())
            .await
    }
    pub async fn manufacturing_with_request_id(
        &self,
        command: ManufacturingCommand,
        request_id: &str,
    ) -> Result<ManufacturingResponse, ClientError> {
        if let ManufacturingCommand::SetIntent { intent, .. } = &command {
            intent.validate()?;
        }
        let call =
            native::manufacturing_call(command, request_id).map_err(ClientError::InvalidRequest)?;
        let mut client = rpc::manufacturing_client::ManufacturingClient::new(self.channel.clone())
            .max_decoding_message_size(MAX_CONTROL_BYTES as usize)
            .max_encoding_message_size(MAX_CONTROL_BYTES as usize);
        match call {
            ManufacturingCall::Execute(request) => {
                let reply = client
                    .execute(self.request(bounded(request)?))
                    .await
                    .map_err(rpc_error)?
                    .into_inner();
                match ManufacturingResponse::try_from(reply).map_err(ClientError::Protocol)? {
                    ManufacturingResponse::Error { error } => Err(error.into()),
                    response => Ok(response),
                }
            }
            ManufacturingCall::Compile(request) => {
                let operation = client
                    .compile(self.request(bounded(request)?))
                    .await
                    .map_err(rpc_error)?
                    .into_inner();
                Ok(ManufacturingResponse::OperationAccepted {
                    operation: native::native_operation_view(&operation)
                        .map_err(ClientError::Protocol)?,
                })
            }
            ManufacturingCall::Verify(request) => {
                let operation = client
                    .verify(self.request(bounded(request)?))
                    .await
                    .map_err(rpc_error)?
                    .into_inner();
                Ok(ManufacturingResponse::OperationAccepted {
                    operation: native::native_operation_view(&operation)
                        .map_err(ClientError::Protocol)?,
                })
            }
        }
    }
    pub async fn native_operation(
        &self,
        name: impl Into<String>,
    ) -> Result<NativeOperationView, ClientError> {
        let name = name.into();
        let operation = self
            .get_operation(name.clone())
            .await
            .map_err(|error| match error {
                ClientError::Rpc(status) => rpc_error(status),
                error => error,
            })?;
        if operation.name != name {
            return Err(ClientError::Protocol(
                "operation response name mismatch".into(),
            ));
        }
        native::native_operation_view(&operation).map_err(ClientError::Protocol)
    }

    /// Immutable ByteStream retrieval followed by checked packed-layout validation.
    pub async fn read_geometry_chunk(
        &self,
        descriptor: &ArtifactView,
        expected: &ArtifactChunkMetadata,
        summary: &ArtifactSummary,
    ) -> Result<Vec<u8>, ClientError> {
        let size = native::validate_artifact_view(descriptor, u64::from(MAX_GEOMETRY_CHUNK_BYTES))
            .map_err(ClientError::Protocol)?;
        let (count, hash, media_type) = match expected {
            ArtifactChunkMetadata::Mesh { metadata } => (
                metadata.byte_count,
                &metadata.sha256,
                "application/x-spiling-mesh",
            ),
            ArtifactChunkMetadata::Section { metadata } => (
                metadata.byte_count,
                &metadata.sha256,
                "application/x-spiling-section",
            ),
        };
        if size != u64::from(count)
            || size == 0
            || size > u64::from(MAX_GEOMETRY_CHUNK_BYTES)
            || descriptor.sha256 != hash.as_str()
            || descriptor.media_type != media_type
        {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "geometry resource does not match chunk metadata",
            )
            .into());
        }
        match (expected, summary) {
            (
                ArtifactChunkMetadata::Mesh { metadata },
                ArtifactSummary::Mesh {
                    artifact_id,
                    definition_id,
                    chunk_count,
                    total_bytes,
                },
            ) if metadata.artifact_id == *artifact_id
                && metadata.definition_id == *definition_id
                && metadata.chunk_count == *chunk_count
                && metadata.chunk_index < *chunk_count
                && count <= *total_bytes => {}
            (ArtifactChunkMetadata::Section { metadata }, ArtifactSummary::Section { summary })
                if metadata.session_id == summary.session_id
                    && metadata.artifact_id == summary.artifact_id
                    && metadata.chunk_count == summary.chunk_count
                    && metadata.chunk_index < summary.chunk_count
                    && count <= summary.total_bytes =>
            {
                validate_section_summary(summary)?;
            }
            _ => {
                return Err(GeometryError::new(
                    GeometryErrorCode::InvalidGeometry,
                    "geometry chunk and artifact summary disagree",
                )
                .into());
            }
        }
        let bytes = self
            .read_bounded_artifact(
                &descriptor.name,
                size,
                &descriptor.sha256,
                u64::from(MAX_GEOMETRY_CHUNK_BYTES),
            )
            .await?;
        match (expected, summary) {
            (
                ArtifactChunkMetadata::Mesh { metadata },
                ArtifactSummary::Mesh {
                    artifact_id,
                    definition_id,
                    ..
                },
            ) => {
                validate_mesh_chunk(
                    &bytes,
                    metadata,
                    &metadata.session_id,
                    *artifact_id,
                    definition_id,
                    metadata.chunk_index,
                )?;
            }
            (ArtifactChunkMetadata::Section { metadata }, ArtifactSummary::Section { summary }) => {
                validate_section_chunk(
                    &bytes,
                    metadata,
                    summary,
                    &summary.session_id,
                    summary.artifact_id,
                    metadata.chunk_index,
                )?;
            }
            _ => unreachable!("checked artifact kind"),
        }
        Ok(bytes)
    }

    /// Schema/integrity validation is not independent emitted-program replay.
    pub async fn fetch_manufacturing_bundle(
        &self,
        record: &ManufacturingArtifactRecord,
        descriptor: &ArtifactView,
    ) -> Result<ManufacturingBundle, ClientError> {
        if record.byte_count == 0 {
            return Err(corrupt_manufacturing("manufacturing artifact is empty"));
        }
        if record.byte_count > MAX_MANUFACTURING_BUNDLE_BYTES {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "manufacturing artifact exceeds bundle byte limit",
            )
            .into());
        }
        record.validate()?;
        let size =
            native::validate_artifact_view(descriptor, u64::from(MAX_MANUFACTURING_BUNDLE_BYTES))
                .map_err(|error| corrupt_manufacturing(&error))?;
        if size != u64::from(record.byte_count)
            || descriptor.sha256 != record.hash.as_str()
            || descriptor.media_type != "application/x-spiling-manufacturing-bundle"
        {
            return Err(corrupt_manufacturing(
                "manufacturing descriptor does not match artifact record",
            ));
        }
        let bytes = self
            .read_bounded_artifact(
                &descriptor.name,
                size,
                &descriptor.sha256,
                u64::from(MAX_MANUFACTURING_BUNDLE_BYTES),
            )
            .await
            .map_err(|error| match error {
                ClientError::Protocol(message) => corrupt_manufacturing(&message),
                ClientError::Rpc(status) => rpc_error(status),
                error => error,
            })?;
        decode_manufacturing_artifact(&bytes, record)
    }
}

pub(crate) fn decode_manufacturing_artifact(
    bytes: &[u8],
    record: &ManufacturingArtifactRecord,
) -> Result<ManufacturingBundle, ClientError> {
    if bytes.len() != record.byte_count as usize || !record.hash.matches_bytes(bytes) {
        return Err(corrupt_manufacturing(
            "manufacturing bundle size or hash mismatch",
        ));
    }
    let bundle = decode_bundle(bytes)?;
    bundle.validate_record(record)?;
    Ok(bundle)
}

impl EngineClient {
    pub async fn geometry(
        &self,
        command: GeometryCommand,
    ) -> Result<GeometryResponse, ClientError> {
        self.rpc.geometry(command).await
    }
    pub async fn project(&self, command: ProjectCommand) -> Result<ProjectResponse, ClientError> {
        self.rpc.project(command).await
    }
    pub async fn manufacturing(
        &self,
        command: ManufacturingCommand,
    ) -> Result<ManufacturingResponse, ClientError> {
        self.rpc.manufacturing(command).await
    }
    pub async fn native_operation(
        &self,
        name: impl Into<String>,
    ) -> Result<NativeOperationView, ClientError> {
        self.rpc.native_operation(name).await
    }
    pub async fn cancel_operation(&self, name: impl Into<String>) -> Result<(), ClientError> {
        self.rpc.cancel_operation(name).await
    }
    pub async fn read_geometry_chunk(
        &self,
        descriptor: &ArtifactView,
        expected: &ArtifactChunkMetadata,
        summary: &ArtifactSummary,
    ) -> Result<Vec<u8>, ClientError> {
        self.rpc
            .read_geometry_chunk(descriptor, expected, summary)
            .await
    }
    pub async fn fetch_manufacturing_bundle(
        &self,
        record: &ManufacturingArtifactRecord,
        descriptor: &ArtifactView,
    ) -> Result<ManufacturingBundle, ClientError> {
        self.rpc
            .fetch_manufacturing_bundle(record, descriptor)
            .await
    }
}
