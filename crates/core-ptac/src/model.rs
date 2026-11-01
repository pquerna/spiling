// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{Error, Result, invalid, limit};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};

pub const CHUNK_BYTES: usize = 256 * 1024;
pub const MAX_OBJECT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ROOT_BYTES: usize = 1024 * 1024;
pub const MAX_GUARD_BYTES: usize = 4096;
pub const MAX_MEMBERS: usize = 128;
pub const MAX_SNAPSHOT_BYTES: usize = 160 * 1024 * 1024;
pub const MAX_DATABASE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_JOURNAL_BYTES: u64 = 2 * MAX_DATABASE_BYTES;
pub const PROFILE: &str = "spiling.project";
pub const APPLICATION_ID: u32 = 0x50544143;
pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_SNAPSHOTS: u64 = 512;
pub const MAX_MEMBERSHIP_ROWS: u64 = 16384;
pub const MAX_OBJECTS: u64 = 4096;
pub const MAX_MAPPING_ROWS: u64 = 262144;
pub const MAX_CHUNKS: u64 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageLimits {
    pub chunk_bytes: u64,
    pub object_bytes: u64,
    pub root_bytes: u64,
    pub guard_bytes: u64,
    pub snapshot_bytes: u64,
    pub database_bytes: u64,
    pub journal_bytes: u64,
    pub scratch_bytes: u64,
    pub scratch_entries: u64,
    pub snapshots: u64,
    pub membership_rows: u64,
    pub objects: u64,
    pub mapping_rows: u64,
    pub chunks: u64,
    pub members_per_snapshot: u64,
}

pub const STORAGE_LIMITS: StorageLimits = StorageLimits {
    chunk_bytes: CHUNK_BYTES as u64,
    object_bytes: MAX_OBJECT_BYTES as u64,
    root_bytes: MAX_ROOT_BYTES as u64,
    guard_bytes: MAX_GUARD_BYTES as u64,
    snapshot_bytes: MAX_SNAPSHOT_BYTES as u64,
    database_bytes: MAX_DATABASE_BYTES,
    journal_bytes: MAX_JOURNAL_BYTES,
    scratch_bytes: 192 * 1024 * 1024,
    scratch_entries: 8,
    snapshots: MAX_SNAPSHOTS,
    membership_rows: MAX_MEMBERSHIP_ROWS,
    objects: MAX_OBJECTS,
    mapping_rows: MAX_MAPPING_ROWS,
    chunks: MAX_CHUNKS,
    members_per_snapshot: MAX_MEMBERS as u64,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId([u8; 32]);

impl ObjectId {
    pub fn of(bytes: &[u8]) -> Self {
        Self(hash(bytes))
    }
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SnapshotId(pub(crate) [u8; 32]);

impl SnapshotId {
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjectRole(u16);

impl ObjectRole {
    pub fn new(value: u16) -> Result<Self> {
        if value == 0 {
            return Err(invalid("zero object role"));
        }
        Ok(Self(value))
    }
    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Member {
    pub role: ObjectRole,
    pub object: ObjectId,
}

#[derive(Clone, Copy, Debug)]
pub struct ObjectInput<'a> {
    pub role: ObjectRole,
    pub bytes: &'a [u8],
}

#[derive(Clone, Copy, Debug)]
pub struct SnapshotInput<'a> {
    pub root: &'a [u8],
    pub objects: &'a [ObjectInput<'a>],
    pub guard: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heads {
    pub(crate) generation: i64,
    pub(crate) current: SnapshotId,
    pub(crate) previous: Option<SnapshotId>,
}

impl Heads {
    pub const fn generation(self) -> i64 {
        self.generation
    }
    pub const fn current(self) -> SnapshotId {
        self.current
    }
    pub const fn previous(self) -> Option<SnapshotId> {
        self.previous
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Current,
    Previous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveStage {
    Staged,
    BeforeCommit,
    Committed,
    BeforePublish,
    Published,
}

#[derive(Debug)]
pub(crate) struct Candidate<'a> {
    pub root: ObjectId,
    pub members: Vec<Member>,
    pub objects: BTreeMap<ObjectId, &'a [u8]>,
    pub id: SnapshotId,
    pub guard: &'a [u8],
}

impl<'a> Candidate<'a> {
    pub fn checked(input: SnapshotInput<'a>) -> Result<Self> {
        if input.root.is_empty() || input.root.len() > MAX_ROOT_BYTES {
            return Err(limit("root must contain 1..1 MiB"));
        }
        if input.guard.is_empty() || input.guard.len() > MAX_GUARD_BYTES {
            return Err(limit("guard must contain 1..4096 bytes"));
        }
        if input.objects.len() > MAX_MEMBERS {
            return Err(limit("snapshot membership"));
        }
        let root = ObjectId::of(input.root);
        let mut objects = BTreeMap::from([(root, input.root)]);
        let mut members = Vec::with_capacity(input.objects.len());
        for input_object in input.objects {
            if input_object.bytes.is_empty() || input_object.bytes.len() > MAX_OBJECT_BYTES {
                return Err(limit("object must contain 1..16 MiB"));
            }
            let object = ObjectId::of(input_object.bytes);
            objects.insert(object, input_object.bytes);
            members.push(Member {
                role: input_object.role,
                object,
            });
        }
        members.sort_unstable();
        if members.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(invalid("duplicate snapshot member"));
        }
        let total = objects
            .values()
            .try_fold(0usize, |total, bytes| total.checked_add(bytes.len()))
            .ok_or_else(|| limit("snapshot byte arithmetic"))?;
        if total > MAX_SNAPSHOT_BYTES {
            return Err(limit("snapshot raw bytes"));
        }
        Ok(Self {
            root,
            id: snapshot_id(root, &members),
            members,
            objects,
            guard: input.guard,
        })
    }
}

pub(crate) fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub(crate) fn snapshot_id(root: ObjectId, members: &[Member]) -> SnapshotId {
    let mut digest = Sha256::new();
    digest.update(b"spiling:ptac:snapshot:v1\0");
    digest.update(root.0);
    digest.update((members.len() as u32).to_le_bytes());
    for member in members {
        digest.update(member.role.0.to_le_bytes());
        digest.update(member.object.0);
    }
    SnapshotId(digest.finalize().into())
}

pub(crate) fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

pub(crate) fn digest_bytes(bytes: &[u8]) -> Result<[u8; 32]> {
    bytes
        .try_into()
        .map_err(|_| invalid("digest is not 32 bytes"))
}
