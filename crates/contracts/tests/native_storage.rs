use spiling_contracts::{geometry::*, native::*, project::*, rpc};

fn stats() -> ProjectStorageStats {
    ProjectStorageStats {
        current_raw_bytes: 1024,
        previous_only_raw_bytes: 512,
        reachable_payload_bytes: 1536,
        unreachable_payload_bytes: 32,
        database_bytes: 8192,
        free_page_bytes: 4096,
        journal_bytes: 0,
        snapshots: 3,
        objects: 4,
        chunks: 4,
        previous_only_payload_bytes: 512,
        scratch_bytes: 0,
        scratch_entries: 0,
        membership_rows: 6,
        mapping_rows: 4,
        retained_roots: 2,
        limits: ProjectStorageLimits {
            chunk_bytes: 262144,
            object_bytes: 16777216,
            root_bytes: 1048576,
            guard_bytes: 4096,
            snapshot_bytes: 268435456,
            database_bytes: 1073741824,
            journal_bytes: 1073741824,
            scratch_bytes: 201326592,
            scratch_entries: 8,
            snapshots: 128,
            membership_rows: 16384,
            objects: 4096,
            mapping_rows: 262144,
            chunks: 4096,
            members_per_snapshot: 128,
        },
    }
}

#[test]
fn storage_actions_are_typed_and_compaction_alone_requires_a_target() {
    let session_id = SessionId::new();
    for action in [
        StorageMaintenanceAction::Stats,
        StorageMaintenanceAction::CollectUnreachable,
        StorageMaintenanceAction::CompactToNew,
    ] {
        let target = (action == StorageMaintenanceAction::CompactToNew)
            .then(|| NativePath::from_os_str(std::ffi::OsStr::new("new.spil")).unwrap());
        let command = ProjectCommand::StorageMaintenance {
            session_id: session_id.clone(),
            base_revision: ProjectRevision(7),
            action,
            target,
        };
        let ProjectCall::StorageMaintenance(request) = project_call(command.clone(), "").unwrap()
        else {
            panic!("maintenance must not use Execute");
        };
        assert_eq!(ProjectCommand::try_from(request.clone()).unwrap(), command);
        let mut malformed = request.clone();
        malformed.action = 0;
        assert!(ProjectCommand::try_from(malformed).is_err());
        let mut malformed = request.clone();
        malformed.action = 999;
        assert!(ProjectCommand::try_from(malformed).is_err());
        let mut malformed = request;
        malformed.target = if malformed.target.is_some() {
            None
        } else {
            Some(
                NativePath::from_os_str(std::ffi::OsStr::new("unexpected.spil"))
                    .unwrap()
                    .try_into()
                    .unwrap(),
            )
        };
        assert!(ProjectCommand::try_from(malformed).is_err());
    }
}

#[test]
fn storage_measurements_and_fixed_limits_roundtrip_without_estimates() {
    let stats = stats();
    let wire: rpc::ProjectStorageStats = stats.clone().try_into().unwrap();
    assert_eq!(ProjectStorageStats::try_from(wire).unwrap(), stats);
    let mut missing_limits = wire;
    missing_limits.limits = None;
    assert!(ProjectStorageStats::try_from(missing_limits).is_err());
    let mut oversized = wire;
    oversized.scratch_entries = 9;
    assert!(ProjectStorageStats::try_from(oversized).is_err());
    for action in [
        StorageMaintenanceAction::Stats,
        StorageMaintenanceAction::CollectUnreachable,
        StorageMaintenanceAction::CompactToNew,
    ] {
        let result = JobResult::ProjectStorageMaintained {
            action,
            stats: stats.clone(),
        };
        let wire: rpc::NativeOperationResult = result.clone().try_into().unwrap();
        assert_eq!(JobResult::try_from(wire).unwrap(), result);
    }
}
