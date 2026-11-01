use rusqlite::{Connection, OpenFlags, params};
use sha2::{Digest, Sha256};
use spiling_contracts::{geometry::SourceHash, project::ProjectManifest};
use std::{path::Path, sync::atomic::AtomicBool};

pub fn inspect_and_remove_interrupted_scratch(parent: &Path) -> usize {
    let mut removed = 0;
    for entry in std::fs::read_dir(parent).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".spiling-stage-") || name.starts_with(".spiling-chunks-") {
            let metadata = std::fs::symlink_metadata(entry.path()).unwrap();
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() {
                std::fs::remove_dir_all(entry.path()).unwrap();
            } else {
                assert!(metadata.is_file());
                std::fs::remove_file(entry.path()).unwrap();
            }
            removed += 1;
        }
    }
    removed
}

pub fn copy_project(source: &Path, destination: &Path) {
    let connection = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    connection
        .execute("VACUUM INTO ?1", [destination.to_str().unwrap()])
        .unwrap();
}

pub fn manifest(path: &Path) -> ProjectManifest {
    spiling_core::storage::open(path, true, false, &AtomicBool::new(false))
        .unwrap()
        .manifest
}

pub fn write_manifest(path: &Path, manifest: &ProjectManifest) {
    write_root(path, &serde_json::to_vec(manifest).unwrap(), &[]);
}

fn object(connection: &Connection, bytes: &[u8]) -> Vec<u8> {
    let object_id = Sha256::digest(bytes).to_vec();
    connection
        .execute(
            "INSERT OR IGNORE INTO objects VALUES(?1,?2,?3)",
            params![
                object_id,
                i64::try_from(bytes.len()).unwrap(),
                i64::try_from(bytes.len().div_ceil(262144)).unwrap()
            ],
        )
        .unwrap();
    for (ordinal, chunk) in bytes.chunks(262144).enumerate() {
        let chunk_id = Sha256::digest(chunk).to_vec();
        connection
            .execute(
                "INSERT OR IGNORE INTO chunks VALUES(?1,?2,0,?3)",
                params![chunk_id, i64::try_from(chunk.len()).unwrap(), chunk],
            )
            .unwrap();
        connection
            .execute(
                "INSERT OR IGNORE INTO object_chunks VALUES(?1,?2,?3)",
                params![object_id, i64::try_from(ordinal).unwrap(), chunk_id],
            )
            .unwrap();
    }
    object_id
}

pub fn write_root(path: &Path, bytes: &[u8], replacements: &[(SourceHash, Vec<u8>)]) {
    let mut connection = Connection::open(path).unwrap();
    connection.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    let transaction = connection.transaction().unwrap();
    let mut members = transaction
        .prepare(
            "SELECT role,object_id FROM snapshot_objects
             WHERE snapshot_id=(SELECT current_snapshot FROM container)",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, u16>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for (old_hash, replacement) in replacements {
        let replacement_id = object(&transaction, replacement);
        let mut replaced = false;
        for (_, member) in &mut members {
            if hash_hex(member) == old_hash.as_str() {
                *member = replacement_id.clone();
                replaced = true;
            }
        }
        assert!(replaced, "replacement must identify a current member");
    }
    members.sort();
    let root = object(&transaction, bytes);
    let mut digest = Sha256::new();
    digest.update(b"spiling:ptac:snapshot:v1\0");
    digest.update(&root);
    digest.update((members.len() as u32).to_le_bytes());
    for (role, member) in &members {
        digest.update(role.to_le_bytes());
        digest.update(member);
    }
    let snapshot = digest.finalize().to_vec();
    transaction
        .execute(
            "INSERT INTO snapshots VALUES(?1,?2)",
            params![snapshot, root],
        )
        .unwrap();
    for (role, member) in members {
        transaction
            .execute(
                "INSERT INTO snapshot_objects VALUES(?1,?2,?3)",
                params![snapshot, role, member],
            )
            .unwrap();
    }
    transaction
        .execute("UPDATE container SET current_snapshot=?1", [snapshot])
        .unwrap();
    transaction.commit().unwrap();
}

fn hash_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn corrupt_root(path: &Path, previous: bool) {
    let connection = Connection::open(path).unwrap();
    let root: Vec<u8> = connection
        .query_row(
            if previous {
                "SELECT root_object FROM snapshots
                 WHERE snapshot_id=(SELECT previous_snapshot FROM container)"
            } else {
                "SELECT root_object FROM snapshots
                 WHERE snapshot_id=(SELECT current_snapshot FROM container)"
            },
            [],
            |row| row.get(0),
        )
        .unwrap();
    corrupt_object_id(&connection, &root);
}

pub fn corrupt_object(path: &Path, hash: &SourceHash) {
    let connection = Connection::open(path).unwrap();
    let object_id = connection
        .prepare("SELECT object_id FROM objects")
        .unwrap()
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .unwrap()
        .map(Result::unwrap)
        .find(|object_id| hash_hex(object_id) == hash.as_str())
        .unwrap();
    corrupt_object_id(&connection, &object_id);
}

fn corrupt_object_id(connection: &Connection, object_id: &[u8]) {
    assert_eq!(
        connection
            .execute(
                "UPDATE chunks SET data=zeroblob(length(data)) WHERE chunk_id=(
                     SELECT chunk_id FROM object_chunks WHERE object_id=?1
                     ORDER BY ordinal DESC LIMIT 1)",
                [object_id],
            )
            .unwrap(),
        1
    );
}

pub fn remove_source_membership(path: &Path) {
    let connection = Connection::open(path).unwrap();
    assert!(
        connection
            .execute(
                "DELETE FROM snapshot_objects WHERE role=1
                 AND snapshot_id=(SELECT current_snapshot FROM container)",
                [],
            )
            .unwrap()
            > 0
    );
}

pub fn heads(path: &Path) -> (i64, Vec<u8>, Option<Vec<u8>>) {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row(
            "SELECT generation,current_snapshot,previous_snapshot FROM container",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
}

pub fn guard(path: &Path) -> Vec<u8> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row("SELECT guard FROM container", [], |row| row.get(0))
        .unwrap()
}
