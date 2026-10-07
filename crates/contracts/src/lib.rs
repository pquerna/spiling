// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Protobuf engine authority and generated native-shell view types.

use prost::Message;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub mod google {
    // Upstream generated documentation has list indentation Clippy cannot infer.
    #[allow(clippy::doc_lazy_continuation)]
    pub mod bytestream {
        tonic::include_proto!("google.bytestream");
    }
    pub mod api {
        tonic::include_proto!("google.api");
    }
    pub mod rpc {
        tonic::include_proto!("google.rpc");
    }
    pub mod longrunning {
        tonic::include_proto!("google.longrunning");
    }
}
pub mod spiling {
    pub mod engine {
        tonic::include_proto!("spiling.engine");
    }
}
pub use spiling::engine as rpc;

pub const MAX_CONTROL_BYTES: u32 = 65_536;
pub const MAX_BINARY_BYTES: u32 = 4_194_304;
pub const MAX_ARTIFACT_BYTES: u32 = 262_144;
pub const TRANSFER_FRAGMENT_BYTES: usize = 16_384;
pub const TRIANGLE_SCHEMA_VERSION: u16 = 1;
pub const TRIANGLE_HEADER_BYTES: usize = 16;
pub const TRIANGLE_VERTEX_COUNT: u32 = 3;
pub const TRIANGLE_INDEX_COUNT: u32 = 3;
pub const METADATA_TYPE: &str = "type.googleapis.com/spiling.engine.DiagnosticMetadata";
pub const RESULT_TYPE: &str = "type.googleapis.com/spiling.engine.DiagnosticResult";

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Hello {
    pub instance_id: String,
    pub engine_build: String,
    pub kernel: String,
    pub geometry_capabilities: Vec<String>,
    pub max_control_bytes: u32,
    pub max_binary_bytes: u32,
    pub pid: u32,
}
impl From<rpc::EngineInfo> for Hello {
    fn from(info: rpc::EngineInfo) -> Self {
        Self {
            instance_id: info.instance_id,
            engine_build: info.engine_build,
            kernel: info.kernel,
            geometry_capabilities: info.geometry_capabilities,
            max_control_bytes: info.max_message_bytes,
            max_binary_bytes: info.max_artifact_bytes,
            pid: info.pid,
        }
    }
}

/// Bounded startup IPC only. Authentication is provided on stdin, never stdout.
#[derive(Debug, Serialize, Deserialize)]
pub struct StartupInfo {
    pub endpoint: String,
    pub instance_id: String,
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct ArtifactView {
    pub name: String,
    pub size_bytes: String,
    pub sha256: String,
    pub media_type: String,
}
impl From<&rpc::Artifact> for ArtifactView {
    fn from(value: &rpc::Artifact) -> Self {
        Self {
            name: value.name.clone(),
            size_bytes: value.size_bytes.to_string(),
            sha256: value.sha256.clone(),
            media_type: value.media_type.clone(),
        }
    }
}
#[derive(Debug, Clone, Serialize, TS)]
pub struct OperationView {
    pub name: String,
    pub done: bool,
    pub state_version: String,
    pub state: String,
    pub create_time: Option<String>,
    pub update_time: Option<String>,
    pub end_time: Option<String>,
    pub phase: String,
    pub completed_units: u32,
    pub total_units: u32,
    pub input_revision: String,
    pub input_digest: String,
    pub outputs: Vec<ArtifactView>,
    pub error_code: Option<i32>,
    pub error_message: Option<String>,
}

pub fn metadata(
    operation: &google::longrunning::Operation,
) -> Result<rpc::DiagnosticMetadata, String> {
    let any = operation
        .metadata
        .as_ref()
        .ok_or("operation metadata missing")?;
    if any.type_url != METADATA_TYPE {
        return Err("unexpected metadata type".into());
    }
    rpc::DiagnosticMetadata::decode(any.value.as_slice()).map_err(|e| e.to_string())
}
pub fn operation_view(operation: &google::longrunning::Operation) -> Result<OperationView, String> {
    let meta = metadata(operation)?;
    let state =
        rpc::DiagnosticState::try_from(meta.state).map_err(|_| "unknown diagnostic state")?;
    let terminal = matches!(
        state,
        rpc::DiagnosticState::Succeeded
            | rpc::DiagnosticState::Failed
            | rpc::DiagnosticState::Cancelled
            | rpc::DiagnosticState::Interrupted
    );
    if state == rpc::DiagnosticState::Unspecified
        || terminal != operation.done
        || operation.done != operation.result.is_some()
        || meta.state_version == 0
        || !(1..=64).contains(&meta.total_units)
        || meta.completed_units > meta.total_units
        || meta.outputs.len() != meta.completed_units as usize
    {
        return Err("inconsistent operation snapshot".into());
    }
    if let Some(google::longrunning::operation::Result::Response(any)) = &operation.result {
        if state != rpc::DiagnosticState::Succeeded || any.type_url != RESULT_TYPE {
            return Err("unexpected operation result".into());
        }
        let result =
            rpc::DiagnosticResult::decode(any.value.as_slice()).map_err(|e| e.to_string())?;
        if result.artifacts != meta.outputs || meta.completed_units != meta.total_units {
            return Err("inconsistent completed outputs".into());
        }
    }
    if let Some(google::longrunning::operation::Result::Error(error)) = &operation.result
        && (state == rpc::DiagnosticState::Succeeded || error.code == 0)
    {
        return Err("inconsistent operation error".into());
    }
    let error = match &operation.result {
        Some(google::longrunning::operation::Result::Error(error)) => Some(error),
        _ => None,
    };
    Ok(OperationView {
        name: operation.name.clone(),
        done: operation.done,
        state_version: meta.state_version.to_string(),
        state: state.as_str_name().to_lowercase(),
        create_time: meta.create_time.map(|t| t.to_string()),
        update_time: meta.update_time.map(|t| t.to_string()),
        end_time: meta.end_time.map(|t| t.to_string()),
        phase: meta.phase,
        completed_units: meta.completed_units,
        total_units: meta.total_units,
        input_revision: meta.input_revision,
        input_digest: meta.input_digest,
        outputs: meta.outputs.iter().map(ArtifactView::from).collect(),
        error_code: error.map(|e| e.code),
        error_message: error.map(|e| e.message.clone()),
    })
}

/// Explicit little-endian schema, never Rust memory layout or CAD geometry.
pub fn synthetic_triangle() -> [u8; 64] {
    let mut bytes = [0; 64];
    bytes[..4].copy_from_slice(b"SPLT");
    bytes[4..6].copy_from_slice(&TRIANGLE_SCHEMA_VERSION.to_le_bytes());
    bytes[8..12].copy_from_slice(&TRIANGLE_VERTEX_COUNT.to_le_bytes());
    bytes[12..16].copy_from_slice(&TRIANGLE_INDEX_COUNT.to_le_bytes());
    let positions: [f32; 9] = [-0.75, -0.6, 0.0, 0.75, -0.6, 0.0, 0.0, 0.75, 0.0];
    for (index, value) in positions.iter().enumerate() {
        bytes[16 + index * 4..20 + index * 4].copy_from_slice(&value.to_le_bytes());
    }
    for (index, value) in [0_u32, 1, 2].iter().enumerate() {
        bytes[52 + index * 4..56 + index * 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}
