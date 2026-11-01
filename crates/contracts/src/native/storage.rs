use super::{request_id, required, session_parent};
use crate::{project::*, rpc};

impl TryFrom<i32> for StorageMaintenanceAction {
    type Error = String;
    fn try_from(value: i32) -> Result<Self, String> {
        match rpc::StorageMaintenanceAction::try_from(value)
            .map_err(|_| "unknown storage action")?
        {
            rpc::StorageMaintenanceAction::Stats => Ok(Self::Stats),
            rpc::StorageMaintenanceAction::CollectUnreachable => Ok(Self::CollectUnreachable),
            rpc::StorageMaintenanceAction::CompactToNew => Ok(Self::CompactToNew),
            rpc::StorageMaintenanceAction::Unspecified => Err("missing storage action".into()),
        }
    }
}
impl From<StorageMaintenanceAction> for rpc::StorageMaintenanceAction {
    fn from(value: StorageMaintenanceAction) -> Self {
        match value {
            StorageMaintenanceAction::Stats => Self::Stats,
            StorageMaintenanceAction::CollectUnreachable => Self::CollectUnreachable,
            StorageMaintenanceAction::CompactToNew => Self::CompactToNew,
        }
    }
}
impl TryFrom<rpc::StorageMaintenanceRequest> for ProjectCommand {
    type Error = String;
    fn try_from(value: rpc::StorageMaintenanceRequest) -> Result<Self, String> {
        request_id(&value.request_id)?;
        let action = value.action.try_into()?;
        let target = value.target.map(TryInto::try_into).transpose()?;
        if (action == StorageMaintenanceAction::CompactToNew) != target.is_some() {
            return Err("only compact-to-new requires a new target file".into());
        }
        Ok(Self::StorageMaintenance {
            session_id: session_parent(&value.parent)?,
            base_revision: ProjectRevision(value.base_revision),
            action,
            target,
        })
    }
}
impl TryFrom<rpc::ProjectStorageStats> for ProjectStorageStats {
    type Error = String;
    fn try_from(value: rpc::ProjectStorageStats) -> Result<Self, String> {
        let limits = required(value.limits, "PTAC storage limits")?;
        let stats = Self {
            current_raw_bytes: value.current_raw_bytes,
            previous_only_raw_bytes: value.previous_only_raw_bytes,
            reachable_payload_bytes: value.reachable_payload_bytes,
            unreachable_payload_bytes: value.unreachable_payload_bytes,
            database_bytes: value.database_bytes,
            free_page_bytes: value.free_page_bytes,
            journal_bytes: value.journal_bytes,
            snapshots: value.snapshots,
            objects: value.objects,
            chunks: value.chunks,
            previous_only_payload_bytes: value.previous_only_payload_bytes,
            scratch_bytes: value.scratch_bytes,
            scratch_entries: value.scratch_entries,
            membership_rows: value.membership_rows,
            mapping_rows: value.mapping_rows,
            retained_roots: value.retained_roots,
            limits: ProjectStorageLimits {
                chunk_bytes: limits.chunk_bytes,
                object_bytes: limits.object_bytes,
                root_bytes: limits.root_bytes,
                guard_bytes: limits.guard_bytes,
                snapshot_bytes: limits.snapshot_bytes,
                database_bytes: limits.database_bytes,
                journal_bytes: limits.journal_bytes,
                scratch_bytes: limits.scratch_bytes,
                scratch_entries: limits.scratch_entries,
                snapshots: limits.snapshots,
                membership_rows: limits.membership_rows,
                objects: limits.objects,
                mapping_rows: limits.mapping_rows,
                chunks: limits.chunks,
                members_per_snapshot: limits.members_per_snapshot,
            },
        };
        stats.validate().map_err(|error| error.to_string())?;
        Ok(stats)
    }
}
impl TryFrom<ProjectStorageStats> for rpc::ProjectStorageStats {
    type Error = String;
    fn try_from(value: ProjectStorageStats) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        let limits = value.limits;
        Ok(Self {
            current_raw_bytes: value.current_raw_bytes,
            previous_only_raw_bytes: value.previous_only_raw_bytes,
            reachable_payload_bytes: value.reachable_payload_bytes,
            unreachable_payload_bytes: value.unreachable_payload_bytes,
            database_bytes: value.database_bytes,
            free_page_bytes: value.free_page_bytes,
            journal_bytes: value.journal_bytes,
            snapshots: value.snapshots,
            objects: value.objects,
            chunks: value.chunks,
            previous_only_payload_bytes: value.previous_only_payload_bytes,
            scratch_bytes: value.scratch_bytes,
            scratch_entries: value.scratch_entries,
            membership_rows: value.membership_rows,
            mapping_rows: value.mapping_rows,
            retained_roots: value.retained_roots,
            limits: Some(rpc::ProjectStorageLimits {
                chunk_bytes: limits.chunk_bytes,
                object_bytes: limits.object_bytes,
                root_bytes: limits.root_bytes,
                guard_bytes: limits.guard_bytes,
                snapshot_bytes: limits.snapshot_bytes,
                database_bytes: limits.database_bytes,
                journal_bytes: limits.journal_bytes,
                scratch_bytes: limits.scratch_bytes,
                scratch_entries: limits.scratch_entries,
                snapshots: limits.snapshots,
                membership_rows: limits.membership_rows,
                objects: limits.objects,
                mapping_rows: limits.mapping_rows,
                chunks: limits.chunks,
                members_per_snapshot: limits.members_per_snapshot,
            }),
        })
    }
}
