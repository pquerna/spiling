// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Staged ByteStream transfers for messages larger than one control frame.
//! Uploads (`uploads/{uuid}/blobs/{sha256}/{size}`) are byte-admitted against
//! fresh memory headroom, checked by exact size and SHA-256 and consumed by the
//! request naming them. Downloads (`blobs/{sha256}/{size}`) are released after
//! one complete read; unread downloads may be released to admit a new one, and
//! reading one then fails explicitly so the client repeats the request.
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use spiling_contracts::google::bytestream::{ReadResponse, WriteRequest, WriteResponse};
use spiling_contracts::native::transfer::{self, StagedMessage};
use spiling_contracts::rpc::StagedBlob;
use std::collections::BTreeMap;
use std::sync::Arc;
use tonic::Status;

type Key = (String, u64);

fn key(blob: &StagedBlob) -> Key {
    (blob.sha256.clone(), blob.size_bytes)
}

#[derive(Default)]
struct State {
    uploads: BTreeMap<Key, Arc<[u8]>>,
    downloads: BTreeMap<Key, Arc<[u8]>>,
    download_bytes: u64,
}

#[derive(Clone, Default)]
pub struct Transfers {
    state: Arc<Mutex<State>>,
}

fn admit(size: u64) -> Result<(), Status> {
    let growth = crate::resources::transfer_growth()
        .map_err(|error| Status::resource_exhausted(error.message))?;
    if size > growth as u64 {
        return Err(Status::resource_exhausted(format!(
            "staged transfer of {size} bytes exceeds the engine's free memory admission of {growth} bytes"
        )));
    }
    Ok(())
}

impl Transfers {
    /// Receive one complete upload. Every fragment is checked in order.
    pub async fn write(
        &self,
        mut stream: tonic::Streaming<WriteRequest>,
    ) -> Result<WriteResponse, Status> {
        let first = stream
            .message()
            .await?
            .ok_or_else(|| Status::invalid_argument("empty staged upload"))?;
        let (_, blob) = transfer::parse_upload_resource(&first.resource_name)
            .map_err(Status::invalid_argument)?;
        admit(blob.size_bytes)?;
        let size = usize::try_from(blob.size_bytes)
            .map_err(|_| Status::resource_exhausted("staged upload exceeds address width"))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| Status::resource_exhausted("staged upload allocation failed"))?;
        let name = first.resource_name.clone();
        let mut hash = Sha256::new();
        let mut next = Some(first);
        let mut finished = false;
        while let Some(request) = next.take() {
            if finished {
                return Err(Status::invalid_argument(
                    "staged upload continues after finish",
                ));
            }
            if !request.resource_name.is_empty() && request.resource_name != name {
                return Err(Status::invalid_argument("staged upload changed resource"));
            }
            if request.write_offset != bytes.len() as i64 {
                return Err(Status::invalid_argument(
                    "staged upload offset is not contiguous",
                ));
            }
            if request.data.len() > transfer::STAGED_FRAGMENT_BYTES
                || bytes.len() + request.data.len() > size
            {
                return Err(Status::invalid_argument(
                    "staged upload exceeds its declared size",
                ));
            }
            hash.update(&request.data);
            bytes.extend_from_slice(&request.data);
            finished = request.finish_write;
            next = stream.message().await?;
        }
        if !finished || bytes.len() != size || format!("{:x}", hash.finalize()) != blob.sha256 {
            return Err(Status::data_loss("staged upload size/checksum mismatch"));
        }
        self.state
            .lock()
            .uploads
            .insert(key(&blob), Arc::from(bytes.into_boxed_slice()));
        Ok(WriteResponse {
            committed_size: blob.size_bytes as i64,
        })
    }

    /// Consume an upload named by a request.
    fn take_upload(&self, blob: &StagedBlob) -> Result<Arc<[u8]>, Status> {
        self.state.lock().uploads.remove(&key(blob)).ok_or_else(|| {
            Status::failed_precondition(
                "staged upload is not present; upload it before the request",
            )
        })
    }

    /// Replace a staged request with its checked complete message.
    pub fn resolve<M: StagedMessage>(&self, message: M) -> Result<M, Status> {
        let Some(blob) = message.staged().cloned() else {
            return Ok(message);
        };
        transfer::validate_staged_envelope(&message).map_err(Status::invalid_argument)?;
        let bytes = self.take_upload(&blob)?;
        transfer::decode_staged(&blob, &bytes).map_err(Status::invalid_argument)
    }

    /// Replace a staged alternative intent with its checked inline content.
    pub fn resolve_alternative(
        &self,
        request: &mut spiling_contracts::rpc::EvaluateManufacturingAlternativeRequest,
    ) -> Result<(), Status> {
        let Some(blob) = transfer::alternative_staged_intent(request).cloned() else {
            return Ok(());
        };
        transfer::validate_staged_blob(&blob).map_err(Status::invalid_argument)?;
        let bytes = self.take_upload(&blob)?;
        transfer::set_alternative_intent(request, &blob, &bytes).map_err(Status::invalid_argument)
    }

    /// Stage a complete reply when it would not fit one control frame.
    pub fn stage_reply<M: StagedMessage>(&self, reply: M) -> Result<M, Status> {
        if !transfer::exceeds_control_frame(reply.encoded_len()) {
            return Ok(reply);
        }
        let bytes = reply.encode_to_vec();
        drop(reply);
        let blob = transfer::staged_blob(&bytes);
        transfer::validate_staged_blob(&blob).map_err(Status::resource_exhausted)?;
        if admit(blob.size_bytes).is_err() {
            // Unread older downloads are reproducible by repeating their
            // request; release them before refusing this one.
            let mut state = self.state.lock();
            state.downloads.clear();
            state.download_bytes = 0;
            drop(state);
            admit(blob.size_bytes)?;
        }
        let mut state = self.state.lock();
        if state
            .downloads
            .insert(key(&blob), Arc::from(bytes.into_boxed_slice()))
            .is_none()
        {
            state.download_bytes += blob.size_bytes;
        }
        drop(state);
        Ok(M::from_staged(blob))
    }

    /// Stream one staged download, releasing it after a complete read.
    pub fn read(
        &self,
        name: &str,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<Result<ReadResponse, Status>>, Status> {
        let blob = transfer::parse_blob_resource(name).map_err(Status::invalid_argument)?;
        let bytes = self
            .state
            .lock()
            .downloads
            .get(&key(&blob))
            .cloned()
            .ok_or_else(|| {
                Status::not_found("staged download is no longer retained; repeat the request")
            })?;
        let start = usize::try_from(offset)
            .ok()
            .filter(|start| *start <= bytes.len())
            .ok_or_else(|| Status::out_of_range("staged read offset exceeds size"))?;
        let end = if limit == 0 {
            bytes.len()
        } else {
            start
                .saturating_add(
                    usize::try_from(limit)
                        .map_err(|_| Status::invalid_argument("negative read limit"))?,
                )
                .min(bytes.len())
        };
        if end == bytes.len() {
            let mut state = self.state.lock();
            if state.downloads.remove(&key(&blob)).is_some() {
                state.download_bytes -= blob.size_bytes;
            }
        }
        Ok(bytes[start..end]
            .chunks(transfer::STAGED_FRAGMENT_BYTES)
            .map(|chunk| {
                Ok(ReadResponse {
                    data: chunk.to_vec(),
                })
            })
            .collect())
    }
}
