use super::{ProjectError, invalid};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageMaintenanceAction {
    Stats,
    CollectUnreachable,
    CompactToNew,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectStorageLimits {
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

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectStorageStats {
    pub current_raw_bytes: u64,
    pub previous_only_raw_bytes: u64,
    pub reachable_payload_bytes: u64,
    pub unreachable_payload_bytes: u64,
    pub database_bytes: u64,
    pub free_page_bytes: u64,
    pub journal_bytes: u64,
    pub snapshots: u64,
    pub objects: u64,
    pub chunks: u64,
    pub previous_only_payload_bytes: u64,
    pub scratch_bytes: u64,
    pub scratch_entries: u64,
    pub membership_rows: u64,
    pub mapping_rows: u64,
    pub retained_roots: u64,
    pub limits: ProjectStorageLimits,
}

impl ProjectStorageStats {
    pub fn validate(&self) -> Result<(), ProjectError> {
        let limits = &self.limits;
        if [
            limits.chunk_bytes,
            limits.object_bytes,
            limits.root_bytes,
            limits.guard_bytes,
            limits.snapshot_bytes,
            limits.database_bytes,
            limits.journal_bytes,
            limits.scratch_bytes,
            limits.scratch_entries,
            limits.snapshots,
            limits.membership_rows,
            limits.objects,
            limits.mapping_rows,
            limits.chunks,
            limits.members_per_snapshot,
        ]
        .contains(&0)
            || self.database_bytes > limits.database_bytes
            || self.journal_bytes > limits.journal_bytes
            || self.scratch_bytes > limits.scratch_bytes
            || self.scratch_entries > limits.scratch_entries
            || self.snapshots > limits.snapshots
            || self.objects > limits.objects
            || self.chunks > limits.chunks
            || self.membership_rows > limits.membership_rows
            || self.mapping_rows > limits.mapping_rows
            || !(1..=2).contains(&self.retained_roots)
            || self.retained_roots > self.snapshots
            || self.free_page_bytes > self.database_bytes
            || self.previous_only_payload_bytes > self.reachable_payload_bytes
        {
            return Err(invalid("invalid PTAC storage statistics or limits"));
        }
        Ok(())
    }
}
