// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{Jobs, internal};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use spiling_contracts::{TRANSFER_FRAGMENT_BYTES, google::bytestream::ReadResponse, rpc::Artifact};
use tokio::sync::mpsc;
use tonic::Status;

impl Jobs {
    pub(crate) fn stream_artifact(
        &self,
        artifact: &Artifact,
        start: u64,
        end: u64,
        sender: &mpsc::Sender<Result<ReadResponse, Status>>,
    ) -> Result<(), Status> {
        if start > end || end > artifact.size_bytes {
            return Err(Status::out_of_range("invalid artifact range"));
        }
        if start == end || sender.is_closed() {
            return Ok(());
        }
        let connection = Connection::open_with_flags(
            &self.database_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(internal)?;
        let row_id: i64 = connection
            .query_row(
                "SELECT rowid FROM artifacts WHERE name=?",
                [&artifact.name],
                |row| row.get(0),
            )
            .optional()
            .map_err(internal)?
            .ok_or_else(|| Status::not_found("artifact not found"))?;
        let blob = connection
            .blob_open("main", "artifacts", "bytes", row_id, true)
            .map_err(internal)?;
        if blob.len() as u64 != artifact.size_bytes {
            return Err(Status::data_loss("artifact size changed"));
        }
        let mut offset = usize::try_from(start)
            .map_err(|_| Status::out_of_range("artifact offset exceeds address space"))?;
        let end = usize::try_from(end)
            .map_err(|_| Status::out_of_range("artifact range exceeds address space"))?;
        while offset < end && !sender.is_closed() {
            let count = (end - offset).min(TRANSFER_FRAGMENT_BYTES);
            let mut data = vec![0; count];
            blob.read_at_exact(&mut data, offset).map_err(internal)?;
            if sender.blocking_send(Ok(ReadResponse { data })).is_err() {
                break;
            }
            offset += count;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
