// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Staged ByteStream transfer for content that does not fit one control frame.
//! A small message keeps its content inline; a larger one carries a
//! `StagedBlob {sha256,size_bytes}` naming the exact protobuf encoding, which
//! travels through `google.bytestream` and is checked before decoding.
use crate::rpc;
use prost::Message;
use sha2::{Digest, Sha256};

/// Room left in one control frame for everything but the staged content.
pub const STAGING_MARGIN_BYTES: usize = 4096;
/// One ByteStream write/read fragment, inside one control frame.
pub const STAGED_FRAGMENT_BYTES: usize = crate::MAX_CONTROL_BYTES as usize - STAGING_MARGIN_BYTES;
/// Staged content sizes are `u32` widths; the receiver byte-admits each one.
pub const MAX_STAGED_BLOB_BYTES: u64 = u32::MAX as u64;

/// Media type of a complete `NativeOperationResult` published as an output.
pub const NATIVE_RESULT_MEDIA_TYPE: &str = "application/x-spiling-native-result";

/// The staged-result artifact a terminal operation names, if any.
pub fn staged_native_result(
    operation: &crate::google::longrunning::Operation,
) -> Result<Option<rpc::Artifact>, String> {
    let Some(crate::google::longrunning::operation::Result::Response(any)) = &operation.result
    else {
        return Ok(None);
    };
    if any.type_url != crate::NATIVE_RESULT_TYPE {
        return Ok(None);
    }
    let result = rpc::NativeOperationResult::decode(any.value.as_slice())
        .map_err(|_| "invalid native result")?;
    match result.result {
        Some(rpc::native_operation_result::Result::Staged(artifact)) => {
            if artifact.media_type != NATIVE_RESULT_MEDIA_TYPE
                || artifact.name != format!("artifacts/{}", artifact.sha256)
            {
                return Err("invalid staged native result artifact".into());
            }
            validate_staged_blob(&rpc::StagedBlob {
                sha256: artifact.sha256.clone(),
                size_bytes: artifact.size_bytes,
            })?;
            Ok(Some(artifact))
        }
        _ => Ok(None),
    }
}

/// Install checked staged-result bytes as the operation's inline result.
pub fn install_native_result(
    operation: &mut crate::google::longrunning::Operation,
    artifact: &rpc::Artifact,
    bytes: Vec<u8>,
) -> Result<(), String> {
    verify_staged_bytes(
        &rpc::StagedBlob {
            sha256: artifact.sha256.clone(),
            size_bytes: artifact.size_bytes,
        },
        &bytes,
    )?;
    let result = rpc::NativeOperationResult::decode(bytes.as_slice())
        .map_err(|_| "staged native result is not a native result")?;
    if matches!(
        result.result,
        Some(rpc::native_operation_result::Result::Staged(_))
    ) {
        return Err("staged native result names another staged result".into());
    }
    operation.result = Some(crate::google::longrunning::operation::Result::Response(
        prost_types::Any {
            type_url: crate::NATIVE_RESULT_TYPE.into(),
            value: bytes,
        },
    ));
    Ok(())
}

/// True when this message must stage its content to fit one control frame.
pub fn exceeds_control_frame(encoded_len: usize) -> bool {
    encoded_len > crate::MAX_CONTROL_BYTES as usize - STAGING_MARGIN_BYTES
}

pub fn staged_blob(bytes: &[u8]) -> rpc::StagedBlob {
    rpc::StagedBlob {
        sha256: format!("{:x}", Sha256::digest(bytes)),
        size_bytes: bytes.len() as u64,
    }
}

pub fn validate_staged_blob(blob: &rpc::StagedBlob) -> Result<(), String> {
    if blob.size_bytes == 0
        || blob.size_bytes > MAX_STAGED_BLOB_BYTES
        || blob.sha256.len() != 64
        || !blob
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid staged blob reference".into());
    }
    Ok(())
}

/// Exact size and SHA-256 check of received staged bytes.
pub fn verify_staged_bytes(blob: &rpc::StagedBlob, bytes: &[u8]) -> Result<(), String> {
    validate_staged_blob(blob)?;
    if bytes.len() as u64 != blob.size_bytes || staged_blob(bytes).sha256 != blob.sha256 {
        return Err("staged blob size/checksum mismatch".into());
    }
    Ok(())
}

pub fn blob_resource_name(blob: &rpc::StagedBlob) -> Result<String, String> {
    validate_staged_blob(blob)?;
    Ok(format!("blobs/{}/{}", blob.sha256, blob.size_bytes))
}

pub fn upload_resource_name(upload: &uuid::Uuid, blob: &rpc::StagedBlob) -> Result<String, String> {
    validate_staged_blob(blob)?;
    Ok(format!(
        "uploads/{}/blobs/{}/{}",
        upload.hyphenated(),
        blob.sha256,
        blob.size_bytes
    ))
}

fn parse_blob_suffix(rest: &str) -> Result<rpc::StagedBlob, String> {
    let mut parts = rest.split('/');
    let (Some("blobs"), Some(sha256), Some(size), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("invalid staged blob resource name".into());
    };
    let size_bytes: u64 = size
        .parse()
        .map_err(|_| "invalid staged blob resource size")?;
    if size_bytes.to_string() != size {
        return Err("noncanonical staged blob resource size".into());
    }
    let blob = rpc::StagedBlob {
        sha256: sha256.into(),
        size_bytes,
    };
    validate_staged_blob(&blob)?;
    Ok(blob)
}

/// `blobs/{sha256}/{size}` download names.
pub fn parse_blob_resource(name: &str) -> Result<rpc::StagedBlob, String> {
    parse_blob_suffix(name)
}

/// `uploads/{uuid}/blobs/{sha256}/{size}` upload names.
pub fn parse_upload_resource(name: &str) -> Result<(uuid::Uuid, rpc::StagedBlob), String> {
    let rest = name
        .strip_prefix("uploads/")
        .ok_or("invalid staged upload resource name")?;
    let (upload, rest) = rest
        .split_once('/')
        .ok_or("invalid staged upload resource name")?;
    let parsed = uuid::Uuid::parse_str(upload).map_err(|_| "invalid staged upload identity")?;
    if parsed.is_nil() || parsed.hyphenated().to_string() != upload {
        return Err("noncanonical staged upload identity".into());
    }
    Ok((parsed, parse_blob_suffix(rest)?))
}

/// A control message that may instead carry its complete encoding staged.
pub trait StagedMessage: Message + Default + Sized {
    fn staged(&self) -> Option<&rpc::StagedBlob>;
    fn from_staged(blob: rpc::StagedBlob) -> Self;
}

impl StagedMessage for rpc::ProjectRequest {
    fn staged(&self) -> Option<&rpc::StagedBlob> {
        match &self.command {
            Some(rpc::project_request::Command::Staged(blob)) => Some(blob),
            _ => None,
        }
    }
    fn from_staged(blob: rpc::StagedBlob) -> Self {
        Self {
            command: Some(rpc::project_request::Command::Staged(blob)),
            ..Default::default()
        }
    }
}

impl StagedMessage for rpc::ProjectReply {
    fn staged(&self) -> Option<&rpc::StagedBlob> {
        match &self.response {
            Some(rpc::project_reply::Response::Staged(blob)) => Some(blob),
            _ => None,
        }
    }
    fn from_staged(blob: rpc::StagedBlob) -> Self {
        Self {
            response: Some(rpc::project_reply::Response::Staged(blob)),
        }
    }
}

impl StagedMessage for rpc::ManufacturingRequest {
    fn staged(&self) -> Option<&rpc::StagedBlob> {
        match &self.command {
            Some(rpc::manufacturing_request::Command::Staged(blob)) => Some(blob),
            _ => None,
        }
    }
    fn from_staged(blob: rpc::StagedBlob) -> Self {
        Self {
            command: Some(rpc::manufacturing_request::Command::Staged(blob)),
            ..Default::default()
        }
    }
}

impl StagedMessage for rpc::ManufacturingReply {
    fn staged(&self) -> Option<&rpc::StagedBlob> {
        match &self.result {
            Some(rpc::manufacturing_reply::Result::Staged(blob)) => Some(blob),
            _ => None,
        }
    }
    fn from_staged(blob: rpc::StagedBlob) -> Self {
        Self {
            result: Some(rpc::manufacturing_reply::Result::Staged(blob)),
        }
    }
}

/// A staged message carries nothing else: its outer fields are defaults.
pub fn validate_staged_envelope<M: StagedMessage>(message: &M) -> Result<(), String> {
    let blob = message.staged().ok_or("message is not staged")?;
    validate_staged_blob(blob)?;
    if message.encode_to_vec() != M::from_staged(blob.clone()).encode_to_vec() {
        return Err("staged message carries other fields".into());
    }
    Ok(())
}

/// Decode received staged bytes as the complete message, never nested staging.
pub fn decode_staged<M: StagedMessage>(blob: &rpc::StagedBlob, bytes: &[u8]) -> Result<M, String> {
    verify_staged_bytes(blob, bytes)?;
    let message = M::decode(bytes).map_err(|_| "staged bytes are not the expected message")?;
    if message.staged().is_some() {
        return Err("staged message names another staged message".into());
    }
    Ok(message)
}

/// The inline intent of an alternative-evaluation start, moved out for staging.
pub fn take_alternative_intent(
    request: &mut rpc::EvaluateManufacturingAlternativeRequest,
) -> Option<Vec<u8>> {
    use rpc::evaluate_manufacturing_alternative_request::Construction;
    match request.construction.take() {
        Some(Construction::Intent(intent)) => Some(intent.encode_to_vec()),
        other => {
            request.construction = other;
            None
        }
    }
}
pub fn set_alternative_staged(
    request: &mut rpc::EvaluateManufacturingAlternativeRequest,
    blob: rpc::StagedBlob,
) {
    request.construction =
        Some(rpc::evaluate_manufacturing_alternative_request::Construction::IntentBlob(blob));
}
/// The staged intent reference of an alternative-evaluation start, if any.
pub fn alternative_staged_intent(
    request: &rpc::EvaluateManufacturingAlternativeRequest,
) -> Option<&rpc::StagedBlob> {
    match &request.construction {
        Some(rpc::evaluate_manufacturing_alternative_request::Construction::IntentBlob(blob)) => {
            Some(blob)
        }
        _ => None,
    }
}
/// Install checked staged bytes as the alternative's inline intent.
pub fn set_alternative_intent(
    request: &mut rpc::EvaluateManufacturingAlternativeRequest,
    blob: &rpc::StagedBlob,
    bytes: &[u8],
) -> Result<(), String> {
    verify_staged_bytes(blob, bytes)?;
    let intent = rpc::ManufacturingIntent::decode(bytes)
        .map_err(|_| "staged manufacturing intent is not a protobuf intent")?;
    request.construction =
        Some(rpc::evaluate_manufacturing_alternative_request::Construction::Intent(intent));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_resource_names_are_canonical_and_checked() {
        let bytes = b"intent".to_vec();
        let blob = staged_blob(&bytes);
        let upload = uuid::Uuid::new_v4();
        let name = upload_resource_name(&upload, &blob).unwrap();
        assert_eq!(
            parse_upload_resource(&name).unwrap(),
            (upload, blob.clone())
        );
        let download = blob_resource_name(&blob).unwrap();
        assert_eq!(parse_blob_resource(&download).unwrap(), blob);
        verify_staged_bytes(&blob, &bytes).unwrap();
        assert!(verify_staged_bytes(&blob, b"intenT").is_err());
        for bad in [
            format!("blobs/{}/06", blob.sha256),
            format!("blobs/{}/0", blob.sha256),
            format!("blobs/{}/6/x", blob.sha256),
            format!("blobs/{}/6", blob.sha256.to_uppercase()),
            format!("uploads/{}/blobs/{}/6", uuid::Uuid::nil(), blob.sha256),
        ] {
            assert!(
                parse_blob_resource(&bad).is_err() && parse_upload_resource(&bad).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn whole_messages_stage_exactly_and_never_nest() {
        let request = rpc::ManufacturingRequest {
            parent: "sessions/x".into(),
            command: Some(rpc::manufacturing_request::Command::Get(
                rpc::GetManufacturingRequest {},
            )),
            ..Default::default()
        };
        let bytes = request.encode_to_vec();
        let blob = staged_blob(&bytes);
        let staged = rpc::ManufacturingRequest::from_staged(blob.clone());
        validate_staged_envelope(&staged).unwrap();
        let mut decorated = staged.clone();
        decorated.parent = "sessions/x".into();
        assert!(validate_staged_envelope(&decorated).is_err());
        assert_eq!(
            decode_staged::<rpc::ManufacturingRequest>(&blob, &bytes).unwrap(),
            request
        );
        assert!(decode_staged::<rpc::ManufacturingRequest>(&blob, b"x").is_err());
        let nested = staged.encode_to_vec();
        assert!(
            decode_staged::<rpc::ManufacturingRequest>(&staged_blob(&nested), &nested).is_err()
        );

        let intent = rpc::ManufacturingIntent::try_from(
            crate::manufacturing::decode_intent(include_bytes!(
                "../../../../fixtures/manufacturing/complete-sparse.intent.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let mut alternative = rpc::EvaluateManufacturingAlternativeRequest {
            construction: Some(
                rpc::evaluate_manufacturing_alternative_request::Construction::Intent(
                    intent.clone(),
                ),
            ),
            ..Default::default()
        };
        let bytes = take_alternative_intent(&mut alternative).unwrap();
        let blob = staged_blob(&bytes);
        set_alternative_staged(&mut alternative, blob.clone());
        assert_eq!(alternative_staged_intent(&alternative), Some(&blob));
        set_alternative_intent(&mut alternative, &blob, &bytes).unwrap();
        assert_eq!(
            alternative.construction,
            Some(rpc::evaluate_manufacturing_alternative_request::Construction::Intent(intent))
        );
    }
}
