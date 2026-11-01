// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Bounded immutable objects and atomic two-root SQLite containers.
//! Application contracts, project mutation and program verification are separate authorities.
mod chunks;
mod execution;
mod model;
mod schema;
mod sqlite;
mod storage_os;
mod vfs;

pub use model::*;
pub use sqlite::{Capture, Container, Publication, PublicationOutcome, StorageStats};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported PTAC representation: {0}")]
    Unsupported(String),
    #[error("invalid PTAC container: {0}")]
    Invalid(String),
    #[error("PTAC resource limit: {0}")]
    ResourceLimit(String),
    #[error("project writer is already attached")]
    Locked,
    #[error("PTAC attachment is read-only")]
    ReadOnly,
    #[error("PTAC root changed since capture")]
    Conflict,
    #[error("no previous saved snapshot")]
    NoPrevious,
    #[error("PTAC operation cancelled before publication")]
    Cancelled,
    #[error("PTAC storage I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("PTAC SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

fn limit(message: impl Into<String>) -> Error {
    Error::ResourceLimit(message.into())
}

#[cfg(test)]
mod adversarial_tests;
#[cfg(test)]
mod fault_vfs;
#[cfg(test)]
mod tests;
