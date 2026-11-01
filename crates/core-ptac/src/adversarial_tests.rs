// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{
    fault_vfs::{Operation, Scope},
    model::{Candidate, hash},
    *,
};
use rusqlite::{Connection, OpenFlags, params};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const OLD_ROOT: &[u8] = b"old authoritative root";
const NEW_ROOT: &[u8] = b"new authoritative root";
const OLD_ASSET: &[u8] = b"old immutable object";
const OLD_GUARD: &[u8] = b"old allocator floor";
const NEW_GUARD: &[u8] = b"new allocator floor";
static PROCESS_TRANSITION: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn reopen_after_close(path: &Path) -> Result<std::sync::Arc<Container>> {
    let _transition = PROCESS_TRANSITION.lock().unwrap();
    Container::open_existing(path, false)
}

fn uncancelled() -> AtomicBool {
    AtomicBool::new(false)
}

fn new_asset() -> Vec<u8> {
    let mut state = 0xdead_beef_u32;
    (0..12_000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}

fn snapshot<'a>(
    root: &'a [u8],
    objects: &'a [ObjectInput<'a>],
    guard: &'a [u8],
) -> SnapshotInput<'a> {
    SnapshotInput {
        root,
        objects,
        guard,
    }
}

fn object(bytes: &[u8]) -> ObjectInput<'_> {
    ObjectInput {
        role: ObjectRole::new(1).unwrap(),
        bytes,
    }
}

fn create(path: &Path) -> Publication {
    Container::create_new(
        path,
        snapshot(OLD_ROOT, &[object(OLD_ASSET)], OLD_GUARD),
        &uncancelled(),
        |_| true,
    )
    .unwrap()
}

struct Fixture {
    _directory: tempfile::TempDir,
    path: PathBuf,
    external: PathBuf,
    sibling: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("project");
        fs::create_dir(&parent).unwrap();
        let path = parent.join("fixture.spil");
        let external = directory.path().join("external-sentinel");
        let sibling = parent.join("sibling-sentinel");
        fs::write(&external, b"external bytes must never change\0").unwrap();
        fs::write(&sibling, b"sibling bytes must never change\0").unwrap();
        Self {
            _directory: directory,
            path,
            external,
            sibling,
        }
    }

    fn sentinels_unchanged(&self) {
        assert_eq!(
            fs::read(&self.external).unwrap(),
            b"external bytes must never change\0"
        );
        assert_eq!(
            fs::read(&self.sibling).unwrap(),
            b"sibling bytes must never change\0"
        );
    }
}

fn companion(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

#[derive(Debug, PartialEq, Eq)]
struct State {
    heads: Heads,
    root: Vec<u8>,
    guard: Vec<u8>,
    members: Vec<(Member, Vec<u8>)>,
}

fn state(container: &Container, selection: Selection) -> State {
    let cancel = uncancelled();
    let capture = container.capture(selection, &cancel).unwrap();
    capture.verify_all(&cancel).unwrap();
    State {
        heads: capture.heads,
        root: capture.root.clone(),
        guard: capture.guard.clone(),
        members: capture
            .members
            .iter()
            .map(|member| {
                (
                    *member,
                    capture
                        .read_object(member.object, MAX_OBJECT_BYTES, &cancel)
                        .unwrap(),
                )
            })
            .collect(),
    }
}

fn expected_new(base: Heads, bytes: &[u8]) -> State {
    let objects = [object(bytes)];
    let candidate = Candidate::checked(snapshot(NEW_ROOT, &objects, NEW_GUARD)).unwrap();
    State {
        heads: Heads {
            generation: base.generation + 1,
            current: candidate.id,
            previous: Some(base.current),
        },
        root: NEW_ROOT.to_vec(),
        guard: NEW_GUARD.to_vec(),
        members: vec![(candidate.members[0], bytes.to_vec())],
    }
}

fn publish_new(
    container: &std::sync::Arc<Container>,
    base: Heads,
    bytes: &[u8],
) -> Result<Publication> {
    container.publish(
        base,
        Selection::Current,
        snapshot(NEW_ROOT, &[object(bytes)], NEW_GUARD),
        &uncancelled(),
        |_| true,
    )
}

#[test]
fn indexed_publish_vfs_failures_preserve_whole_graph_and_truthful_receipts() {
    let bytes = new_asset();
    let counts = {
        let scope = Scope::new();
        let fixture = Fixture::new();
        let initial = create(&fixture.path);
        scope.arm(None);
        let published = publish_new(&initial.container, initial.heads, &bytes).unwrap();
        assert_eq!(published.outcome, PublicationOutcome::PublishedConfirmed);
        let counts = Operation::ALL.map(|operation| scope.count(operation));
        scope.disarm();
        drop(published);
        drop(initial);
        counts
    };
    eprintln!("publish baseline indexed VFS counts (write,sync,truncate,delete,lock): {counts:?}");
    for operation in [
        Operation::Write,
        Operation::Sync,
        Operation::Delete,
        Operation::Lock,
    ] {
        assert!(counts[operation as usize] > 0, "unexercised {operation:?}");
    }
    for operation in Operation::ALL {
        for index in 1..=counts[operation as usize] {
            let scope = Scope::new();
            let fixture = Fixture::new();
            let initial = create(&fixture.path);
            let old = state(&initial.container, Selection::Current);
            let new = expected_new(initial.heads, &bytes);
            scope.arm(Some((operation, index)));
            let result = publish_new(&initial.container, initial.heads, &bytes);
            assert!(scope.fired(), "failure not reached: {operation:?} #{index}");
            scope.disarm();
            let receipt = result
                .as_ref()
                .ok()
                .map(|publication| (publication.outcome, publication.heads));
            drop(result);
            assert_eq!(
                std::sync::Arc::strong_count(&initial.container),
                1,
                "retained publication: {operation:?} #{index}"
            );
            drop(initial);
            let reopened = reopen_after_close(&fixture.path)
                .unwrap_or_else(|error| panic!("{operation:?} #{index}: reopen: {error}"));
            let actual = state(&reopened, Selection::Current);
            match receipt {
                None => assert_eq!(actual, old, "error receipt: {operation:?} #{index}"),
                Some((
                    PublicationOutcome::PublishedConfirmed
                    | PublicationOutcome::PublishedUnconfirmed,
                    heads,
                )) => {
                    assert_eq!(heads, new.heads, "published heads: {operation:?} #{index}");
                    assert_eq!(actual, new, "published receipt: {operation:?} #{index}");
                }
                Some((PublicationOutcome::OutcomeUnknown, _)) => {
                    assert!(
                        actual == old || actual == new,
                        "mixed outcome: {operation:?} #{index}: {actual:?}"
                    );
                }
            }
            reopened.check(&uncancelled()).unwrap();
            fixture.sentinels_unchanged();
        }
    }
}

#[test]
fn indexed_real_rollback_truncate_failure_recovers_original_graph() {
    for fail_truncate in [false, true] {
        let scope = Scope::new();
        let fixture = Fixture::new();
        let initial = create(&fixture.path);
        let expected = state(&initial.container, Selection::Current);
        drop(initial);
        let location = crate::storage_os::Location::attach(&fixture.path, false).unwrap();
        let guard = crate::vfs::Guard::new(location, true, false).unwrap();
        let fault = crate::fault_vfs::FaultVfs::for_current_thread(guard.name())
            .unwrap()
            .unwrap();
        let connection = Connection::open_with_flags_and_vfs(
            &fixture.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            fault.name(),
        )
        .unwrap();
        crate::schema::configure(&connection, false).unwrap();
        connection
            .execute_batch("PRAGMA cache_size=1; BEGIN IMMEDIATE;")
            .unwrap();
        let original_length = fs::metadata(&fixture.path).unwrap().len();
        for index in 0_u32..128 {
            let id = hash(&index.to_le_bytes());
            connection
                .execute(
                    "INSERT INTO chunks VALUES(?1,4096,0,zeroblob(4096))",
                    [&id[..]],
                )
                .unwrap();
        }
        assert!(
            fs::metadata(&fixture.path).unwrap().len() > original_length,
            "transaction did not spill to the real pager"
        );
        scope.arm(fail_truncate.then_some((Operation::Truncate, 1)));
        let rollback = connection.execute_batch("ROLLBACK");
        assert!(
            scope.count(Operation::Truncate) > 0,
            "rollback never called xTruncate"
        );
        assert_eq!(scope.fired(), fail_truncate);
        if !fail_truncate {
            rollback.unwrap();
        }
        scope.disarm();
        drop(connection);
        drop(fault);
        drop(guard);
        let reopened = reopen_after_close(&fixture.path).unwrap();
        assert_eq!(state(&reopened, Selection::Current), expected);
        reopened.check(&uncancelled()).unwrap();
        fixture.sentinels_unchanged();
    }
}

#[test]
fn hostile_catalogs_are_not_admitted_by_matching_header_versions() {
    let variants = [
        crate::schema::DDL.replace("CHECK(codec IN (0,1))", "CHECK(codec IN (0,1,2))"),
        crate::schema::DDL.replace(" REFERENCES chunks(chunk_id)", ""),
        crate::schema::DDL.replace(") STRICT;", ");"),
        crate::schema::DDL.replace(
            "CREATE INDEX snapshots_by_root ON snapshots(root_object);",
            "CREATE INDEX snapshots_by_root ON snapshots(snapshot_id);",
        ),
        format!(
            "{} CREATE VIEW foreign_view AS SELECT * FROM chunks;",
            crate::schema::DDL
        ),
        format!(
            "{} CREATE TRIGGER foreign_trigger AFTER INSERT ON chunks BEGIN DELETE FROM container; END;",
            crate::schema::DDL
        ),
        format!(
            "{} CREATE TABLE foreign_table(value BLOB);",
            crate::schema::DDL
        ),
    ];
    for (index, ddl) in variants.into_iter().enumerate() {
        let fixture = Fixture::new();
        let connection = Connection::open(&fixture.path).unwrap();
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        connection
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .unwrap();
        connection.execute_batch(&ddl).unwrap();
        drop(connection);
        let before = fs::read(&fixture.path).unwrap();
        for read_only in [false, true] {
            let error = Container::open_existing(&fixture.path, read_only).unwrap_err();
            assert!(
                matches!(&error, Error::Invalid(message) if message.contains("catalog")),
                "catalog #{index}: {error}"
            );
        }
        assert_eq!(fs::read(&fixture.path).unwrap(), before);
        fixture.sentinels_unchanged();
    }
}

#[test]
fn corrupted_selected_graphs_never_yield_partial_assets() {
    let attacks = [
        "UPDATE container SET guard_hash=zeroblob(32)",
        "UPDATE snapshots SET root_object=(SELECT object_id FROM snapshot_objects LIMIT 1)",
        "UPDATE snapshot_objects SET role=2",
        "DELETE FROM object_chunks WHERE object_id=(SELECT root_object FROM snapshots LIMIT 1)",
        "UPDATE object_chunks SET ordinal=3 WHERE object_id=(SELECT root_object FROM snapshots LIMIT 1)",
        "UPDATE objects SET raw_length=raw_length+1 WHERE object_id=(SELECT root_object FROM snapshots LIMIT 1)",
        "UPDATE chunks SET data=zeroblob(raw_length) WHERE chunk_id IN (SELECT chunk_id FROM object_chunks WHERE object_id=(SELECT root_object FROM snapshots LIMIT 1))",
    ];
    for attack in attacks {
        let fixture = Fixture::new();
        drop(create(&fixture.path));
        let connection = Connection::open(&fixture.path).unwrap();
        assert!(connection.execute(attack, []).unwrap() > 0);
        drop(connection);
        let before = fs::read(&fixture.path).unwrap();
        match Container::open_existing(&fixture.path, false) {
            Err(_) => {}
            Ok(container) => {
                assert!(
                    container
                        .capture(Selection::Current, &uncancelled())
                        .is_err(),
                    "admitted corrupt graph: {attack}"
                );
                assert!(
                    container.check(&uncancelled()).is_err(),
                    "check missed: {attack}"
                );
            }
        }
        assert_eq!(
            fs::read(&fixture.path).unwrap(),
            before,
            "corruption was rewritten: {attack}"
        );
        fixture.sentinels_unchanged();
    }
}

#[test]
fn corrupt_member_is_rejected_on_read_and_full_check() {
    let fixture = Fixture::new();
    drop(create(&fixture.path));
    let connection = Connection::open(&fixture.path).unwrap();
    connection.execute("UPDATE chunks SET data=zeroblob(raw_length) WHERE chunk_id IN (SELECT chunk_id FROM object_chunks WHERE object_id=?1)", [&ObjectId::of(OLD_ASSET).as_bytes()[..]]).unwrap();
    drop(connection);
    let container = Container::open_existing(&fixture.path, false).unwrap();
    let cancel = uncancelled();
    let capture = container.capture(Selection::Current, &cancel).unwrap();
    assert!(
        capture
            .read_object(ObjectId::of(OLD_ASSET), MAX_OBJECT_BYTES, &cancel)
            .is_err()
    );
    assert!(capture.verify_all(&cancel).is_err());
    drop(capture);
    assert!(container.check(&cancel).is_err());
}

#[test]
fn selected_member_limit_and_read_budgets_fail_closed() {
    let fixture = Fixture::new();
    let initial = create(&fixture.path);
    let cancel = uncancelled();
    let capture = initial
        .container
        .capture(Selection::Current, &cancel)
        .unwrap();
    assert!(matches!(
        capture.read_object(ObjectId::of(OLD_ASSET), OLD_ASSET.len() - 1, &cancel),
        Err(Error::ResourceLimit(_))
    ));
    assert!(matches!(
        capture.read_object(ObjectId::of(OLD_ROOT), MAX_ROOT_BYTES, &cancel),
        Err(Error::Invalid(_))
    ));
    drop(capture);
    drop(initial);
    let connection = Connection::open(&fixture.path).unwrap();
    for role in 2..=MAX_MEMBERS + 1 {
        connection
            .execute(
                "INSERT INTO snapshot_objects SELECT current_snapshot,?1,?2 FROM container",
                params![role as i64, &ObjectId::of(OLD_ASSET).as_bytes()[..]],
            )
            .unwrap();
    }
    drop(connection);
    let container = Container::open_existing(&fixture.path, false).unwrap();
    assert!(matches!(
        container.capture(Selection::Current, &cancel),
        Err(Error::ResourceLimit(_))
    ));
}

#[test]
fn input_boundary_limits_and_duplicate_roles_are_not_compatibility_defaults() {
    for root in [Vec::new(), vec![0; MAX_ROOT_BYTES + 1]] {
        assert!(matches!(
            Candidate::checked(snapshot(&root, &[], OLD_GUARD)),
            Err(Error::ResourceLimit(_))
        ));
    }
    for guard in [Vec::new(), vec![0; MAX_GUARD_BYTES + 1]] {
        assert!(matches!(
            Candidate::checked(snapshot(OLD_ROOT, &[], &guard)),
            Err(Error::ResourceLimit(_))
        ));
    }
    for bytes in [Vec::new(), vec![0; MAX_OBJECT_BYTES + 1]] {
        assert!(matches!(
            Candidate::checked(snapshot(OLD_ROOT, &[object(&bytes)], OLD_GUARD)),
            Err(Error::ResourceLimit(_))
        ));
    }
    let members: Vec<_> = (1..=MAX_MEMBERS + 1)
        .map(|role| ObjectInput {
            role: ObjectRole::new(role as u16).unwrap(),
            bytes: OLD_ASSET,
        })
        .collect();
    assert!(matches!(
        Candidate::checked(snapshot(OLD_ROOT, &members, OLD_GUARD)),
        Err(Error::ResourceLimit(_))
    ));
    assert!(
        Candidate::checked(snapshot(
            OLD_ROOT,
            &[object(OLD_ASSET), object(OLD_ASSET)],
            OLD_GUARD
        ))
        .is_err()
    );
    assert!(ObjectRole::new(0).is_err());
}

#[test]
fn codec_adversaries_reject_trailing_frames_dictionary_and_unbounded_windows() {
    let raw = vec![7; 4096];
    let encoded = crate::chunks::encode(&raw).unwrap();
    assert_eq!(encoded.codec, 1);
    assert_eq!(
        crate::chunks::decode(1, raw.len(), &encoded.data, &hash(&raw)).unwrap(),
        raw
    );
    let mut trailing = encoded.data.clone();
    trailing.push(0);
    let mut concatenated = encoded.data.clone();
    concatenated.extend_from_slice(&encoded.data);
    let mut dictionary = encoded.data.clone();
    dictionary[4] |= 1;
    let dictionary_offset = if dictionary[4] & 0x20 != 0 { 5 } else { 6 };
    dictionary.insert(dictionary_offset, 1);
    let mut no_size = zstd::stream::write::Encoder::new(Vec::new(), 3).unwrap();
    no_size.include_contentsize(false).unwrap();
    no_size.write_all(&raw).unwrap();
    let no_size = no_size.finish().unwrap();
    let attacks = [
        trailing,
        concatenated,
        dictionary,
        no_size,
        vec![0x50, 0x2a, 0x4d, 0x18, 0, 0, 0, 0],
        vec![0; 20],
    ];
    for (index, bytes) in attacks.iter().enumerate() {
        assert!(
            crate::chunks::decode(1, raw.len(), bytes, &hash(&raw)).is_err(),
            "codec attack #{index}"
        );
    }
    assert!(crate::chunks::decode(2, raw.len(), &raw, &hash(&raw)).is_err());
    assert!(crate::chunks::decode(0, raw.len() - 1, &raw, &hash(&raw)).is_err());
    assert!(crate::chunks::decode(0, raw.len(), &raw, &[0; 32]).is_err());
    assert!(crate::chunks::decode(1, raw.len() - 1, &encoded.data, &hash(&raw)).is_err());
    assert!(crate::chunks::decode(1, CHUNK_BYTES + 1, &encoded.data, &hash(&raw)).is_err());
    for (window, admitted) in [(0x40, true), (0x48, false)] {
        let payload = [11_u8; 100];
        let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0x80, window];
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        let block = ((payload.len() as u32) << 3) | 1;
        frame.extend_from_slice(&block.to_le_bytes()[..3]);
        frame.extend_from_slice(&payload);
        let decoded = crate::chunks::decode(1, payload.len(), &frame, &hash(&payload));
        assert_eq!(
            decoded.is_ok(),
            admitted,
            "window descriptor {window:#x}: {decoded:?}"
        );
    }
}

#[test]
fn ordered_chunk_closure_rejects_swaps_missing_tail_and_reused_corruption() {
    for attack in ["swap", "missing-tail", "reused-corruption"] {
        let fixture = Fixture::new();
        let mut bytes = vec![23; CHUNK_BYTES];
        bytes.extend_from_slice(&[49; 17]);
        let initial = Container::create_new(
            &fixture.path,
            snapshot(OLD_ROOT, &[object(&bytes)], OLD_GUARD),
            &uncancelled(),
            |_| true,
        )
        .unwrap();
        let base = initial.heads;
        drop(initial);
        let connection = Connection::open(&fixture.path).unwrap();
        let id = ObjectId::of(&bytes);
        match attack {
            "swap" => {
                connection
                    .execute(
                        "UPDATE object_chunks SET ordinal=ordinal+2 WHERE object_id=?1",
                        [&id.as_bytes()[..]],
                    )
                    .unwrap();
                connection
                    .execute(
                        "UPDATE object_chunks SET ordinal=3-ordinal WHERE object_id=?1",
                        [&id.as_bytes()[..]],
                    )
                    .unwrap();
            }
            "missing-tail" => {
                assert_eq!(
                    connection
                        .execute(
                            "DELETE FROM object_chunks WHERE object_id=?1 AND ordinal=1",
                            [&id.as_bytes()[..]]
                        )
                        .unwrap(),
                    1
                );
            }
            "reused-corruption" => {
                assert_eq!(connection.execute("UPDATE chunks SET data=zeroblob(length(data)) WHERE chunk_id IN (SELECT chunk_id FROM object_chunks WHERE object_id=?1 AND ordinal=0)", [&id.as_bytes()[..]]).unwrap(), 1);
            }
            _ => unreachable!(),
        }
        drop(connection);
        let container = Container::open_existing(&fixture.path, false).unwrap();
        let cancel = uncancelled();
        let capture = container.capture(Selection::Current, &cancel).unwrap();
        assert!(
            capture.read_object(id, bytes.len(), &cancel).is_err(),
            "{attack}"
        );
        drop(capture);
        assert!(
            container
                .publish(
                    base,
                    Selection::Current,
                    snapshot(NEW_ROOT, &[object(&bytes)], NEW_GUARD),
                    &cancel,
                    |_| true
                )
                .is_err(),
            "corrupt reuse was trusted: {attack}"
        );
        assert!(container.check(&cancel).is_err());
    }
}

#[test]
fn corruption_of_previous_never_causes_silent_current_or_previous_fallback() {
    let fixture = Fixture::new();
    let initial = create(&fixture.path);
    let publication = publish_new(&initial.container, initial.heads, &new_asset()).unwrap();
    drop(publication);
    drop(initial);
    let connection = Connection::open(&fixture.path).unwrap();
    assert_eq!(connection.execute(
        "UPDATE chunks SET data=zeroblob(raw_length) WHERE chunk_id IN (SELECT chunk_id FROM object_chunks WHERE object_id=(SELECT root_object FROM snapshots WHERE snapshot_id=(SELECT previous_snapshot FROM container)))", [],
    ).unwrap(), 1);
    drop(connection);
    let container = Container::open_existing(&fixture.path, false).unwrap();
    assert_eq!(state(&container, Selection::Current).root, NEW_ROOT);
    assert!(
        container
            .capture(Selection::Previous, &uncancelled())
            .is_err()
    );
    assert!(container.check(&uncancelled()).is_err());
}

#[cfg(unix)]
#[test]
fn linked_main_and_symlinked_ancestry_are_not_alternate_attachments() {
    use std::os::unix::fs::symlink;
    for attack in ["symlink", "hardlink", "ancestor"] {
        let fixture = Fixture::new();
        drop(create(&fixture.path));
        let before = fs::read(&fixture.path).unwrap();
        let alias = fixture._directory.path().join("alias.spil");
        let target = match attack {
            "symlink" => {
                symlink(&fixture.path, &alias).unwrap();
                alias
            }
            "hardlink" => {
                fs::hard_link(&fixture.path, &alias).unwrap();
                alias
            }
            _ => {
                let alias = fixture._directory.path().join("alias-parent");
                symlink(fixture.path.parent().unwrap(), &alias).unwrap();
                alias.join("fixture.spil")
            }
        };
        for read_only in [false, true] {
            assert!(
                Container::open_existing(&target, read_only).is_err(),
                "{attack}"
            );
        }
        assert_eq!(fs::read(&fixture.path).unwrap(), before);
        fixture.sentinels_unchanged();
    }
}

#[test]
fn namespace_vfs_rejects_attach_and_temp_file_bypass_without_touching_sentinels() {
    let fixture = Fixture::new();
    drop(create(&fixture.path));
    let before = fs::read(&fixture.path).unwrap();
    let location = crate::storage_os::Location::attach(&fixture.path, false).unwrap();
    let guard = crate::vfs::Guard::new(location, true, false).unwrap();
    let connection = Connection::open_with_flags_and_vfs(
        &fixture.path,
        OpenFlags::SQLITE_OPEN_READ_WRITE,
        guard.name(),
    )
    .unwrap();
    crate::schema::configure(&connection, false).unwrap();
    for target in [&fixture.external, &fixture.sibling] {
        assert!(
            connection
                .execute(
                    "ATTACH DATABASE ?1 AS foreign_database",
                    [target.to_str().unwrap()]
                )
                .is_err()
        );
        fixture.sentinels_unchanged();
    }
    let foreign_scratch = fixture.path.parent().unwrap().join("foreign-scratch");
    assert!(
        connection
            .execute("VACUUM INTO ?1", [foreign_scratch.to_str().unwrap()])
            .is_err()
    );
    assert!(!foreign_scratch.exists());
    unsafe {
        let name = std::ffi::CString::new(guard.name()).unwrap();
        let vfs = rusqlite::ffi::sqlite3_vfs_find(name.as_ptr());
        assert!(!vfs.is_null());
        let file = rusqlite::ffi::sqlite3_malloc64((*vfs).szOsFile as u64)
            .cast::<rusqlite::ffi::sqlite3_file>();
        assert!(!file.is_null());
        std::ptr::write_bytes(file.cast::<u8>(), 0, (*vfs).szOsFile as usize);
        let result = (*vfs).xOpen.unwrap()(
            vfs,
            std::ptr::null(),
            file,
            rusqlite::ffi::SQLITE_OPEN_TEMP_DB
                | rusqlite::ffi::SQLITE_OPEN_READWRITE
                | rusqlite::ffi::SQLITE_OPEN_CREATE,
            std::ptr::null_mut(),
        );
        assert_eq!(result, rusqlite::ffi::SQLITE_CANTOPEN);
        assert!((*file).pMethods.is_null());
        rusqlite::ffi::sqlite3_free(file.cast());
    }
    drop(connection);
    assert_eq!(fs::read(&fixture.path).unwrap(), before);
    fixture.sentinels_unchanged();
}

#[test]
fn predecision_cancellation_stale_generation_and_live_capture_preserve_state() {
    let fixture = Fixture::new();
    let initial = create(&fixture.path);
    let old = state(&initial.container, Selection::Current);
    let bytes = new_asset();
    for stage in [SaveStage::Staged, SaveStage::BeforeCommit] {
        let result = initial.container.publish(
            initial.heads,
            Selection::Current,
            snapshot(NEW_ROOT, &[object(&bytes)], NEW_GUARD),
            &uncancelled(),
            |observed| observed != stage,
        );
        assert!(matches!(result, Err(Error::Cancelled)), "{stage:?}");
        assert_eq!(state(&initial.container, Selection::Current), old);
    }
    let cancel = AtomicBool::new(true);
    assert!(matches!(
        initial.container.publish(
            initial.heads,
            Selection::Current,
            snapshot(NEW_ROOT, &[object(&bytes)], NEW_GUARD),
            &cancel,
            |_| true
        ),
        Err(Error::Cancelled)
    ));
    let capture_cancel = uncancelled();
    let capture = initial
        .container
        .capture(Selection::Current, &capture_cancel)
        .unwrap();
    let started = Instant::now();
    assert!(matches!(
        publish_new(&initial.container, initial.heads, &bytes),
        Err(Error::Locked)
    ));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(capture.root, OLD_ROOT);
    assert_eq!(
        capture
            .read_object(ObjectId::of(OLD_ASSET), MAX_OBJECT_BYTES, &capture_cancel)
            .unwrap(),
        OLD_ASSET
    );
    drop(capture);
    let published = publish_new(&initial.container, initial.heads, &bytes).unwrap();
    assert!(matches!(
        publish_new(&initial.container, initial.heads, b"stale asset"),
        Err(Error::Conflict)
    ));
    assert_eq!(
        state(&initial.container, Selection::Current),
        expected_new(initial.heads, &bytes)
    );
    assert_eq!(
        state(&initial.container, Selection::Previous).root,
        OLD_ROOT
    );
    assert_eq!(published.heads.generation(), initial.heads.generation() + 1);
}

#[test]
fn postdecision_cancellation_and_failed_ack_are_not_reported_as_rollback() {
    for failed_ack in [false, true] {
        let fixture = Fixture::new();
        let initial = create(&fixture.path);
        let cancel = uncancelled();
        let bytes = new_asset();
        let published = initial
            .container
            .publish(
                initial.heads,
                Selection::Current,
                snapshot(NEW_ROOT, &[object(&bytes)], NEW_GUARD),
                &cancel,
                |stage| {
                    if stage == SaveStage::BeforeCommit {
                        cancel.store(true, Ordering::Release);
                    }
                    !(failed_ack && stage == SaveStage::Committed)
                },
            )
            .unwrap();
        assert_eq!(
            published.outcome,
            if failed_ack {
                PublicationOutcome::PublishedUnconfirmed
            } else {
                PublicationOutcome::PublishedConfirmed
            }
        );
        assert_eq!(published.error.is_some(), failed_ack);
        assert_eq!(
            state(&initial.container, Selection::Current),
            expected_new(initial.heads, &bytes)
        );
    }
}

#[test]
fn cancelled_create_and_exclusive_publication_leave_foreign_targets_untouched() {
    for stage in [SaveStage::Staged, SaveStage::BeforePublish] {
        let fixture = Fixture::new();
        let result = Container::create_new(
            &fixture.path,
            snapshot(OLD_ROOT, &[], OLD_GUARD),
            &uncancelled(),
            |observed| observed != stage,
        );
        assert!(
            matches!(result, Err(Error::Cancelled)),
            "{stage:?}: {result:?}"
        );
        assert!(!fixture.path.exists());
        fixture.sentinels_unchanged();
    }
    let fixture = Fixture::new();
    fs::write(&fixture.path, b"foreign target is not a project").unwrap();
    assert!(
        Container::create_new(
            &fixture.path,
            snapshot(OLD_ROOT, &[], OLD_GUARD),
            &uncancelled(),
            |_| true
        )
        .is_err()
    );
    assert_eq!(
        fs::read(&fixture.path).unwrap(),
        b"foreign target is not a project"
    );
    let fixture = Fixture::new();
    let result = Container::create_new(
        &fixture.path,
        snapshot(OLD_ROOT, &[], OLD_GUARD),
        &uncancelled(),
        |stage| {
            if stage == SaveStage::BeforePublish {
                fs::write(&fixture.path, b"racing foreign target").unwrap();
            }
            true
        },
    );
    assert!(result.is_err());
    assert_eq!(fs::read(&fixture.path).unwrap(), b"racing foreign target");
}

#[test]
fn actual_read_only_attachment_never_publishes() {
    let fixture = Fixture::new();
    let initial = create(&fixture.path);
    let reader = Container::open_existing(&fixture.path, true).unwrap();
    assert!(reader.read_only());
    assert!(matches!(
        publish_new(&reader, initial.heads, &new_asset()),
        Err(Error::ReadOnly)
    ));
    assert_eq!(
        state(&reader, Selection::Current),
        state(&initial.container, Selection::Current)
    );
    assert!(matches!(
        Container::open_existing(&fixture.path, false),
        Err(Error::Locked)
    ));
}

#[test]
fn sqlite_execution_scope_interrupts_and_unregisters_bounded_work() {
    let connection = Connection::open_in_memory().unwrap();
    let query = "WITH RECURSIVE work(value) AS (VALUES(1) UNION ALL SELECT value+1 FROM work WHERE value<1000000000) SELECT sum(value) FROM work";
    for cancel in [AtomicBool::new(true), uncancelled()] {
        let started = Instant::now();
        let execution = crate::execution::Execution::new(&connection, &cancel);
        let result = connection.query_row(query, [], |row| row.get::<_, i64>(0));
        assert!(
            matches!(result, Err(rusqlite::Error::SqliteFailure(error, _)) if error.code == rusqlite::ErrorCode::OperationInterrupted)
        );
        assert!(started.elapsed() < Duration::from_secs(35));
        drop(execution);
        assert_eq!(
            connection
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}

#[test]
fn wal_shm_and_foreign_headers_refuse_without_touching_any_bytes() {
    for suffix in ["-wal", "-shm"] {
        let fixture = Fixture::new();
        drop(create(&fixture.path));
        let before = fs::read(&fixture.path).unwrap();
        let sidecar = companion(&fixture.path, suffix);
        fs::write(&sidecar, b"foreign recovery bytes").unwrap();
        for read_only in [true, false] {
            assert!(Container::open_existing(&fixture.path, read_only).is_err());
        }
        assert_eq!(fs::read(&fixture.path).unwrap(), before);
        assert_eq!(fs::read(sidecar).unwrap(), b"foreign recovery bytes");
        fixture.sentinels_unchanged();
    }
    for offset in [0, 16, 18, 19, 60, 68] {
        let fixture = Fixture::new();
        drop(create(&fixture.path));
        let mut bytes = fs::read(&fixture.path).unwrap();
        bytes[offset] ^= 0xff;
        fs::write(&fixture.path, &bytes).unwrap();
        fs::write(
            companion(&fixture.path, "-journal"),
            b"not our recovery journal",
        )
        .unwrap();
        assert!(
            Container::open_existing(&fixture.path, false).is_err(),
            "header byte {offset}"
        );
        assert_eq!(fs::read(&fixture.path).unwrap(), bytes);
        assert_eq!(
            fs::read(companion(&fixture.path, "-journal")).unwrap(),
            b"not our recovery journal"
        );
    }
}

#[cfg(unix)]
fn superjournal_bytes(name: &Path, database: &[u8]) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    let name = name.as_os_str().as_bytes();
    let mut journal = vec![0; 512];
    journal[..8].copy_from_slice(&[0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7]);
    journal[8..12].copy_from_slice(&1_u32.to_be_bytes());
    journal[16..20].copy_from_slice(&((database.len() / 4096) as u32).to_be_bytes());
    journal[20..24].copy_from_slice(&512_u32.to_be_bytes());
    journal[24..28].copy_from_slice(&4096_u32.to_be_bytes());
    journal.extend_from_slice(&1_u32.to_be_bytes());
    journal.extend_from_slice(&database[..4096]);
    let mut checksum = 0_u32;
    let mut offset = 4096 - 200;
    loop {
        checksum = checksum.wrapping_add(database[offset] as u32);
        if offset < 200 {
            break;
        }
        offset -= 200;
    }
    journal.extend_from_slice(&checksum.to_be_bytes());
    journal.extend_from_slice(&262145_u32.to_be_bytes());
    journal.extend_from_slice(name);
    journal.extend_from_slice(&(name.len() as u32).to_be_bytes());
    journal.extend_from_slice(
        &name
            .iter()
            .fold(0_u32, |sum, byte| sum.wrapping_add(*byte as u32))
            .to_be_bytes(),
    );
    journal.extend_from_slice(&[0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7]);
    journal
}

#[cfg(unix)]
#[test]
fn crafted_superjournals_cannot_escape_or_exploit_the_missing_wal_probe() {
    for target in ["external", "sibling", "missing-wal"] {
        let fixture = Fixture::new();
        drop(create(&fixture.path));
        let database = fs::read(&fixture.path).unwrap();
        let missing_wal = companion(&fixture.path, "-wal");
        let name = match target {
            "external" => &fixture.external,
            "sibling" => &fixture.sibling,
            _ => &missing_wal,
        };
        let journal = superjournal_bytes(name, &database);
        let journal_path = companion(&fixture.path, "-journal");
        fs::write(&journal_path, &journal).unwrap();
        for read_only in [true, false] {
            assert!(
                Container::open_existing(&fixture.path, read_only).is_err(),
                "superjournal target {target}, read_only={read_only}"
            );
            assert_eq!(fs::read(&fixture.path).unwrap(), database);
            assert_eq!(fs::read(&journal_path).unwrap(), journal);
            fixture.sentinels_unchanged();
            assert!(!missing_wal.exists());
        }
    }
}

#[cfg(unix)]
#[test]
fn symlink_hardlink_and_nonregular_recovery_names_are_never_followed() {
    use std::os::unix::fs::symlink;
    for attack in [
        "external-symlink",
        "sibling-symlink",
        "hardlink",
        "directory",
        "oversize",
        "fifo",
    ] {
        let fixture = Fixture::new();
        drop(create(&fixture.path));
        let database = fs::read(&fixture.path).unwrap();
        let journal = companion(&fixture.path, "-journal");
        match attack {
            "external-symlink" => symlink(&fixture.external, &journal).unwrap(),
            "sibling-symlink" => symlink(&fixture.sibling, &journal).unwrap(),
            "hardlink" => fs::hard_link(&fixture.external, &journal).unwrap(),
            "directory" => fs::create_dir(&journal).unwrap(),
            "oversize" => fs::File::create(&journal)
                .unwrap()
                .set_len(MAX_JOURNAL_BYTES + 1)
                .unwrap(),
            "fifo" => {
                use std::{ffi::CString, os::unix::ffi::OsStrExt};
                let name = CString::new(journal.as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            }
            _ => unreachable!(),
        }
        let started = Instant::now();
        for read_only in [true, false] {
            assert!(
                Container::open_existing(&fixture.path, read_only).is_err(),
                "{attack}"
            );
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "blocked on {attack}"
        );
        assert_eq!(fs::read(&fixture.path).unwrap(), database);
        fixture.sentinels_unchanged();
        assert!(fs::symlink_metadata(journal).is_ok());
    }
}

#[cfg(unix)]
#[test]
fn substituted_main_parent_and_writer_lease_invalidate_attached_authority() {
    for attack in ["main", "parent", "lease"] {
        let fixture = Fixture::new();
        let initial = create(&fixture.path);
        match attack {
            "main" => {
                fs::rename(&fixture.path, companion(&fixture.path, ".displaced")).unwrap();
                fs::write(&fixture.path, b"replacement must not be opened").unwrap();
            }
            "parent" => {
                let parent = fixture.path.parent().unwrap();
                fs::rename(parent, parent.with_extension("displaced")).unwrap();
                fs::create_dir(parent).unwrap();
                fs::write(&fixture.path, b"replacement must not be opened").unwrap();
            }
            "lease" => {
                let lease = companion(&fixture.path, ".lock");
                assert!(lease.exists(), "writer lease naming contract changed");
                fs::rename(&lease, companion(&lease, ".displaced")).unwrap();
                fs::write(&lease, b"foreign lease").unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            initial
                .container
                .capture(Selection::Current, &uncancelled())
                .is_err(),
            "{attack}"
        );
        assert!(
            publish_new(&initial.container, initial.heads, &new_asset()).is_err(),
            "{attack}"
        );
        assert_eq!(
            fs::read(&fixture.external).unwrap(),
            b"external bytes must never change\0"
        );
        if attack != "parent" {
            fixture.sentinels_unchanged();
        }
    }
}

struct ChildProcess(Child);

impl ChildProcess {
    fn start(path: &Path, mode: &str) -> Self {
        let transition = PROCESS_TRANSITION.lock().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "adversarial_tests::subprocess_worker",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("SPILING_PTAC_CHILD_MODE", mode)
            .env("SPILING_PTAC_CHILD_PATH", path)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        drop(transition);
        let stdout = child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) if line.contains("PTAC_CHILD_READY") => {
                        let _ = sender.send(());
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });
        let mut process = Self(child);
        if let Err(error) = receiver.recv_timeout(Duration::from_secs(15)) {
            let _ = process.0.kill();
            panic!("subprocess did not reach {mode}: {error}");
        }
        process
    }

    fn kill(&mut self) {
        self.0.kill().unwrap();
        let status = self.0.wait().unwrap();
        assert!(!status.success());
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(libc::SIGKILL));
        }
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn child_ready() -> ! {
    println!("PTAC_CHILD_READY");
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn subprocess_worker() {
    let Some(mode) = std::env::var_os("SPILING_PTAC_CHILD_MODE") else {
        return;
    };
    let path = PathBuf::from(std::env::var_os("SPILING_PTAC_CHILD_PATH").unwrap());
    #[cfg(target_os = "linux")]
    if mode == "stage-emfile" {
        let parent = crate::storage_os::Parent::open(&path).unwrap();
        let highest = fs::read_dir("/proc/self/fd")
            .unwrap()
            .map(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
            })
            .max()
            .unwrap();
        let mut original = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut original) },
            0
        );
        let limited = libc::rlimit {
            rlim_cur: highest + 3,
            rlim_max: original.rlim_max,
        };
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limited) }, 0);
        let result = crate::storage_os::Stage::new(&parent);
        assert_eq!(
            unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &original) },
            0
        );
        assert!(
            matches!(result, Err(Error::Io(ref error)) if error.raw_os_error() == Some(libc::EMFILE))
        );
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
        drop(crate::storage_os::Stage::new(&parent).unwrap());
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
        child_ready();
    }
    let container = Container::open_existing(&path, false).unwrap();
    if mode == "writer" {
        child_ready();
    }
    let base = state(&container, Selection::Current).heads;
    let bytes = new_asset();
    let target = if mode == "before-commit" {
        SaveStage::BeforeCommit
    } else {
        SaveStage::Committed
    };
    container
        .publish(
            base,
            Selection::Current,
            snapshot(NEW_ROOT, &[object(&bytes)], NEW_GUARD),
            &uncancelled(),
            |stage| {
                if stage == target {
                    child_ready();
                }
                true
            },
        )
        .unwrap();
    panic!("subprocess failed to reach requested commit boundary");
}

#[cfg(target_os = "linux")]
#[test]
fn failed_stage_attachment_cleans_its_owned_directory_before_retry() {
    let temporary = tempfile::tempdir().unwrap();
    let mut child = ChildProcess::start(temporary.path(), "stage-emfile");
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 0);
}

#[test]
fn subprocess_lifetime_writer_lease_survives_readonly_opens_and_dies_with_owner() {
    let fixture = Fixture::new();
    drop(create(&fixture.path));
    let mut child = ChildProcess::start(&fixture.path, "writer");
    for _ in 0..3 {
        let started = Instant::now();
        assert!(matches!(
            Container::open_existing(&fixture.path, false),
            Err(Error::Locked)
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
        let reader = Container::open_existing(&fixture.path, true).unwrap();
        assert_eq!(state(&reader, Selection::Current).root, OLD_ROOT);
        drop(reader);
    }
    child.kill();
    let writer = Container::open_existing(&fixture.path, false).unwrap();
    assert_eq!(state(&writer, Selection::Current).root, OLD_ROOT);
    fixture.sentinels_unchanged();
}

#[test]
fn subprocess_sigkill_before_and_after_commit_reopens_exact_old_or_new_state() {
    for mode in ["before-commit", "after-commit"] {
        let fixture = Fixture::new();
        let initial = create(&fixture.path);
        let old = state(&initial.container, Selection::Current);
        let new = expected_new(initial.heads, &new_asset());
        drop(initial);
        let mut child = ChildProcess::start(&fixture.path, mode);
        child.kill();
        let reopened = Container::open_existing(&fixture.path, false).unwrap();
        assert_eq!(
            state(&reopened, Selection::Current),
            if mode == "before-commit" { old } else { new }
        );
        if mode == "after-commit" {
            let previous = state(&reopened, Selection::Previous);
            assert_eq!(previous.root, OLD_ROOT);
            assert_eq!(previous.members[0].1, OLD_ASSET);
        }
        reopened.check(&uncancelled()).unwrap();
        fixture.sentinels_unchanged();
    }
}

fn golden_digest(text: &str) -> [u8; 32] {
    assert_eq!(text.len(), 64);
    std::array::from_fn(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap())
}

#[test]
fn independent_golden_snapshot_preimage_and_sqlite_profile_are_stable() {
    let root_digest =
        golden_digest("7e43b4d22a268ccba8b129401305db1c84a16e3ed6b1986afd82228187f0a91e");
    let first_digest =
        golden_digest("17f6deef78ba85b0778dd8c6681b0dd7fc71e036641f457678426ffa365a2dc2");
    let second_digest =
        golden_digest("dd89bfcaf3ae1a8571d31ecbedaf5525bd5d0eba89b0ea3827ddc004b7191262");
    let snapshot_digest =
        golden_digest("0588ac451b3c4c3910eecfb80dd37c755a2ea8ebf751be4fe5ab4a3ada22e96d");
    assert_eq!(hash(b"golden root"), root_digest);
    assert_eq!(hash(b"golden object one"), first_digest);
    assert_eq!(hash(b"golden object two"), second_digest);
    let fixture = Fixture::new();
    let objects = [
        ObjectInput {
            role: ObjectRole::new(19).unwrap(),
            bytes: b"golden object two",
        },
        ObjectInput {
            role: ObjectRole::new(7).unwrap(),
            bytes: b"golden object one",
        },
    ];
    let initial = Container::create_new(
        &fixture.path,
        snapshot(b"golden root", &objects, b"golden guard"),
        &uncancelled(),
        |_| true,
    )
    .unwrap();
    assert_eq!(initial.heads.current().as_bytes(), &snapshot_digest);
    let captured = state(&initial.container, Selection::Current);
    assert_eq!(captured.members[0].0.role.get(), 7);
    assert_eq!(captured.members[1].0.role.get(), 19);
    drop(initial);
    let connection =
        Connection::open_with_flags(&fixture.path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA application_id", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        0x5054_4143
    );
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row("PRAGMA page_size", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        4096
    );
    assert_eq!(
        connection
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "delete"
    );
    let singleton: (String, i64, Vec<u8>, Option<Vec<u8>>, Vec<u8>) = connection
        .query_row(
            "SELECT profile,generation,current_snapshot,previous_snapshot,guard FROM container",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        singleton,
        (
            "spiling.project".into(),
            1,
            snapshot_digest.to_vec(),
            None,
            b"golden guard".to_vec()
        )
    );
    let tables = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        tables,
        [
            "chunks",
            "container",
            "object_chunks",
            "objects",
            "snapshot_objects",
            "snapshots"
        ]
    );
    for table in tables {
        let strict = connection
            .query_row(
                "SELECT strict FROM pragma_table_list WHERE name=?1",
                [&table],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(strict, 1, "{table}");
    }
    for (digest, bytes) in [
        (root_digest, &b"golden root"[..]),
        (first_digest, &b"golden object one"[..]),
        (second_digest, &b"golden object two"[..]),
    ] {
        let stored: (i64, i64, Vec<u8>) = connection
            .query_row(
                "SELECT raw_length,codec,data FROM chunks WHERE chunk_id=?1",
                [&digest[..]],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(stored, (bytes.len() as i64, 0, bytes.to_vec()));
        let chunk: (i64, Vec<u8>) = connection
            .query_row(
                "SELECT ordinal,chunk_id FROM object_chunks WHERE object_id=?1",
                [&digest[..]],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(chunk, (0, digest.to_vec()));
    }
    drop(connection);
    let database = fs::read(&fixture.path).unwrap();
    assert_eq!(&database[..16], b"SQLite format 3\0");
    assert_eq!(&database[16..20], &[0x10, 0, 1, 1]);
    assert_eq!(&database[60..64], &[0, 0, 0, 1]);
    assert_eq!(&database[68..72], b"PTAC");
    assert_eq!(database.len() % 4096, 0);
}

#[test]
fn bounded_deterministic_file_mutations_refuse_or_preserve_the_complete_logical_state() {
    let original = Fixture::new();
    let initial = create(&original.path);
    let expected = state(&initial.container, Selection::Current);
    drop(initial);
    let original_bytes = fs::read(&original.path).unwrap();
    let mut corpus = vec![original_bytes.clone()];
    for offset in [
        0, 15, 16, 17, 18, 19, 20, 21, 24, 27, 28, 31, 32, 35, 36, 39, 40, 43, 44, 47, 48, 51, 52,
        55, 56, 59, 60, 63, 64, 67, 68, 71, 72, 75, 92, 95, 96, 99, 100, 101, 102, 103, 104, 105,
    ] {
        let mut bytes = original_bytes.clone();
        bytes[offset] ^= 0xff;
        corpus.push(bytes);
    }
    for page in 1..original_bytes.len() / 4096 {
        for offset in [page * 4096, page * 4096 + 100, (page + 1) * 4096 - 1] {
            let mut bytes = original_bytes.clone();
            bytes[offset] ^= 0x80;
            corpus.push(bytes);
        }
    }
    for length in [0, 1, 15, 99, 100, 4095, 4096, original_bytes.len() - 1] {
        corpus.push(original_bytes[..length].to_vec());
    }
    let mut appended = original_bytes.clone();
    appended.extend_from_slice(&[0; 4096]);
    corpus.push(appended);
    assert!(corpus.len() <= 128);
    let mut random = fuzz_seed();
    for _ in 0..fuzz_cases() {
        let mut bytes = original_bytes.clone();
        let offset = fuzz_next(&mut random) as usize % bytes.len();
        bytes[offset] ^= 1 << (fuzz_next(&mut random) % 8);
        corpus.push(bytes);
    }
    let mut refused = 0;
    let mut admitted = 0;
    for (index, bytes) in corpus.iter().enumerate() {
        let fixture = Fixture::new();
        fs::write(&fixture.path, bytes).unwrap();
        match reopen_after_close(&fixture.path) {
            Err(_) => refused += 1,
            Ok(container) => {
                if container.check(&uncancelled()).is_err() {
                    refused += 1;
                } else {
                    assert_eq!(
                        state(&container, Selection::Current),
                        expected,
                        "accepted mutation #{index}"
                    );
                    admitted += 1;
                }
                drop(container);
            }
        }
        assert_eq!(
            fs::read(&fixture.path).unwrap(),
            *bytes,
            "mutation was rewritten #{index}"
        );
        fixture.sentinels_unchanged();
    }
    assert_eq!(admitted + refused, corpus.len());
    assert!(admitted > 0);
    assert!(refused > 0);
    eprintln!(
        "bounded file mutation corpus: {} cases, {admitted} identical logical states, {refused} refusals",
        corpus.len()
    );
}

#[test]
fn bounded_codec_mutations_never_change_successfully_decoded_identity() {
    let random = new_asset();
    let mut mutations = 0;
    let mut seed = fuzz_seed();
    for length in [1, 63, 64, 65, 255, 256, 4096, CHUNK_BYTES - 1, CHUNK_BYTES] {
        for compressible in [false, true] {
            let raw: Vec<_> = (0..length)
                .map(|index| {
                    if compressible {
                        17
                    } else {
                        random[index % random.len()]
                    }
                })
                .collect();
            let encoded = crate::chunks::encode(&raw).unwrap();
            assert_eq!(
                crate::chunks::decode(encoded.codec as i64, length, &encoded.data, &hash(&raw))
                    .unwrap(),
                raw
            );
            for offset in [0, encoded.data.len() / 2, encoded.data.len() - 1] {
                for mask in [1, 0x80] {
                    let mut bytes = encoded.data.clone();
                    bytes[offset] ^= mask;
                    if let Ok(decoded) =
                        crate::chunks::decode(encoded.codec as i64, length, &bytes, &hash(&raw))
                    {
                        assert_eq!(decoded, raw);
                    }
                    mutations += 1;
                }
            }
            for _ in 0..fuzz_cases().div_ceil(18) {
                let mut bytes = encoded.data.clone();
                let offset = fuzz_next(&mut seed) as usize % bytes.len();
                bytes[offset] ^= 1 << (fuzz_next(&mut seed) % 8);
                if let Ok(decoded) =
                    crate::chunks::decode(encoded.codec as i64, length, &bytes, &hash(&raw))
                {
                    assert_eq!(decoded, raw);
                }
            }
        }
    }
    assert_eq!(mutations, 108);
}

fn fuzz_cases() -> usize {
    std::env::var("SPILING_PTAC_FUZZ_CASES")
        .map(|value| value.parse::<usize>().expect("integer fuzz case count"))
        .unwrap_or(0)
        .min(4096)
}

fn fuzz_seed() -> u64 {
    std::env::var("SPILING_PTAC_FUZZ_SEED")
        .map(|value| value.parse::<u64>().expect("integer fuzz seed"))
        .unwrap_or(0x5a17_c9e2_147b_80d3)
}

fn fuzz_next(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
    *state
}

#[test]
fn mutation_fuzz_snapshot_preimages_and_row_mappings_are_bounded() {
    use sha2::{Digest, Sha256};
    let mut random = fuzz_seed();
    for case in 0..32 + fuzz_cases() {
        let root: Vec<u8> = (0..64)
            .map(|_| (fuzz_next(&mut random) >> 32) as u8)
            .collect();
        let assets: Vec<Vec<u8>> = (0..fuzz_next(&mut random) as usize % 16)
            .map(|_| {
                (0..32)
                    .map(|_| (fuzz_next(&mut random) >> 32) as u8)
                    .collect()
            })
            .collect();
        let inputs: Vec<_> = assets
            .iter()
            .enumerate()
            .rev()
            .map(|(index, bytes)| ObjectInput {
                role: ObjectRole::new(index as u16 + 1).unwrap(),
                bytes,
            })
            .collect();
        let candidate = Candidate::checked(snapshot(&root, &inputs, OLD_GUARD)).unwrap();
        let mut oracle = Sha256::new();
        oracle.update(b"spiling:ptac:snapshot:v1\0");
        oracle.update(Sha256::digest(&root));
        oracle.update((assets.len() as u32).to_le_bytes());
        for (index, bytes) in assets.iter().enumerate() {
            oracle.update((index as u16 + 1).to_le_bytes());
            oracle.update(Sha256::digest(bytes));
        }
        assert_eq!(
            candidate.id.as_bytes(),
            &<[u8; 32]>::from(oracle.finalize())
        );
        let fixture = Fixture::new();
        drop(create(&fixture.path));
        let connection = Connection::open(&fixture.path).unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys=OFF; PRAGMA ignore_check_constraints=ON;")
            .unwrap();
        match case % 3 {
            0 => {
                connection
                    .execute(
                        "UPDATE object_chunks SET ordinal=?1 WHERE object_id=?2",
                        params![
                            1 + (fuzz_next(&mut random) >> 1) as i64 % 1024,
                            &ObjectId::of(OLD_ASSET).as_bytes()[..]
                        ],
                    )
                    .unwrap();
            }
            1 => {
                connection
                    .execute(
                        "UPDATE object_chunks SET chunk_id=?1 WHERE object_id=?2",
                        params![&root[..32], &ObjectId::of(OLD_ASSET).as_bytes()[..]],
                    )
                    .unwrap();
            }
            _ => {
                connection
                    .execute(
                        "UPDATE objects SET raw_length=?1 WHERE object_id=?2",
                        params![
                            -1 - (fuzz_next(&mut random) >> 1) as i64,
                            &ObjectId::of(OLD_ASSET).as_bytes()[..]
                        ],
                    )
                    .unwrap();
            }
        }
        drop(connection);
        let before = fs::read(&fixture.path).unwrap();
        if let Ok(container) = reopen_after_close(&fixture.path) {
            assert!(container.check(&uncancelled()).is_err());
            if let Ok(capture) = container.capture(Selection::Current, &uncancelled()) {
                assert!(capture.verify_all(&uncancelled()).is_err());
            }
        }
        assert_eq!(fs::read(&fixture.path).unwrap(), before);
        fixture.sentinels_unchanged();
    }
}

#[test]
fn deterministic_save_state_machine_matches_two_root_model() {
    let fixture = Fixture::new();
    let initial = create(&fixture.path);
    let container = initial.container;
    let mut heads = initial.heads;
    let mut current = OLD_ROOT.to_vec();
    let mut previous: Option<Vec<u8>> = None;
    let mut guard = OLD_GUARD.to_vec();
    let mut random = 0x98ab_34cd_u32;
    let mut exercised = [0; 6];
    for step in 0..48 {
        random ^= random << 13;
        random ^= random >> 17;
        random ^= random << 5;
        let operation = random as usize % 6;
        exercised[operation] += 1;
        let root = if operation == 5 {
            current.clone()
        } else {
            format!("model root {step}").into_bytes()
        };
        let next_guard = format!("allocator floor {step}").into_bytes();
        let base = if operation == 3 {
            Heads {
                generation: heads.generation - 1,
                ..heads
            }
        } else {
            heads
        };
        let retain = if operation == 4 {
            Selection::Previous
        } else {
            Selection::Current
        };
        let result = container.publish(
            base,
            retain,
            snapshot(&root, &[object(OLD_ASSET)], &next_guard),
            &uncancelled(),
            |stage| {
                !matches!(
                    (operation, stage),
                    (1, SaveStage::Staged) | (2, SaveStage::BeforeCommit)
                )
            },
        );
        match operation {
            1 | 2 => assert!(matches!(result, Err(Error::Cancelled))),
            3 => assert!(matches!(result, Err(Error::Conflict))),
            4 if previous.is_none() => assert!(matches!(result, Err(Error::NoPrevious))),
            _ => {
                let publication = result.unwrap();
                assert_eq!(publication.outcome, PublicationOutcome::PublishedConfirmed);
                assert_eq!(publication.heads.generation(), heads.generation() + 1);
                let retained = if retain == Selection::Current {
                    current.clone()
                } else {
                    previous.clone().unwrap()
                };
                if root != retained {
                    previous = Some(retained);
                }
                current = root;
                guard = next_guard;
                heads = publication.heads;
            }
        }
        let actual = state(&container, Selection::Current);
        assert_eq!(actual.heads, heads, "step {step}, operation {operation}");
        assert_eq!(actual.root, current, "step {step}, operation {operation}");
        assert_eq!(actual.guard, guard, "step {step}, operation {operation}");
        assert_eq!(actual.members[0].1, OLD_ASSET);
        if let Some(previous) = &previous {
            assert_eq!(&state(&container, Selection::Previous).root, previous);
        } else {
            assert!(matches!(
                container.capture(Selection::Previous, &uncancelled()),
                Err(Error::NoPrevious)
            ));
        }
        container.check(&uncancelled()).unwrap();
    }
    assert!(
        exercised.iter().all(|count| *count > 0),
        "missing state-machine branch: {exercised:?}"
    );
}

#[test]
fn exhausted_generation_refuses_without_changing_root_guard_or_members() {
    let fixture = Fixture::new();
    drop(create(&fixture.path));
    let connection = Connection::open(&fixture.path).unwrap();
    connection
        .execute("UPDATE container SET generation=?1", [i64::MAX])
        .unwrap();
    drop(connection);
    let container = reopen_after_close(&fixture.path).unwrap();
    let expected = state(&container, Selection::Current);
    assert_eq!(expected.heads.generation(), i64::MAX);
    assert!(matches!(
        publish_new(&container, expected.heads, &new_asset()),
        Err(Error::ResourceLimit(_))
    ));
    assert_eq!(state(&container, Selection::Current), expected);
    container.check(&uncancelled()).unwrap();
}
