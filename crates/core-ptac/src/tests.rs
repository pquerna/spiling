// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use std::sync::atomic::AtomicBool;

fn input(root: &[u8]) -> SnapshotInput<'_> {
    SnapshotInput {
        root,
        objects: &[],
        guard: b"independent guard",
    }
}

#[test]
#[ignore = "explicit bounded storage workload measurement"]
fn bounded_storage_workload_measurements() {
    use std::time::Instant;
    for (name, count, length, shared) in [
        ("shared-small", 128, 4096, true),
        ("max-object", 1, MAX_OBJECT_BYTES, false),
        ("large-combined-capture", 9, MAX_OBJECT_BYTES, false),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("workload.spil");
        let mut random = 0x759a_312f_48bc_d167_u64;
        let assets: Vec<Vec<u8>> = (0..count)
            .map(|index| {
                if shared {
                    vec![(index % 16) as u8; length]
                } else {
                    (0..length)
                        .map(|_| {
                            random ^= random << 13;
                            random ^= random >> 7;
                            random ^= random << 17;
                            (random >> 32) as u8
                        })
                        .collect()
                }
            })
            .collect();
        let objects: Vec<_> = assets
            .iter()
            .enumerate()
            .map(|(index, bytes)| ObjectInput {
                role: ObjectRole::new(index as u16 + 1).unwrap(),
                bytes,
            })
            .collect();
        let candidate = |root| SnapshotInput {
            root,
            objects: &objects,
            guard: b"workload guard",
        };
        let cancel = AtomicBool::new(false);
        let start = Instant::now();
        let initial =
            Container::create_new(&path, candidate(b"initial root"), &cancel, |_| true).unwrap();
        eprintln!("{name} create_us={}", start.elapsed().as_micros());
        drop(initial);
        let start = Instant::now();
        let container = Container::open_existing(&path, false).unwrap();
        eprintln!("{name} open_us={}", start.elapsed().as_micros());
        let start = Instant::now();
        let capture = container.capture(Selection::Current, &cancel).unwrap();
        capture.verify_all(&cancel).unwrap();
        let heads = capture.heads;
        drop(capture);
        eprintln!("{name} capture_verify_us={}", start.elapsed().as_micros());
        let start = Instant::now();
        let unchanged = container
            .publish(
                heads,
                Selection::Current,
                candidate(b"initial root"),
                &cancel,
                |_| true,
            )
            .unwrap();
        assert_eq!(unchanged.heads.current, heads.current);
        eprintln!("{name} unchanged_save_us={}", start.elapsed().as_micros());
        let start = Instant::now();
        let changed = container
            .publish(
                unchanged.heads,
                Selection::Current,
                candidate(b"changed root"),
                &cancel,
                |_| true,
            )
            .unwrap();
        assert_eq!(changed.heads.previous, Some(heads.current));
        eprintln!("{name} changed_save_us={}", start.elapsed().as_micros());
        let start = Instant::now();
        container.collect_unreachable(&cancel, || true).unwrap();
        eprintln!("{name} collect_us={}", start.elapsed().as_micros());
        let start = Instant::now();
        let copy = container
            .compact_to_new(&temporary.path().join("compact.spil"), &cancel, |_| true)
            .unwrap();
        assert_eq!(copy.heads.current, changed.heads.current);
        assert_eq!(copy.heads.previous, changed.heads.previous);
        assert_eq!(copy.outcome, PublicationOutcome::PublishedConfirmed);
        eprintln!("{name} compact_us={}", start.elapsed().as_micros());
        eprintln!("{name} stats={:?}", container.stats(&cancel).unwrap());
    }
}

#[test]
fn retained_capacity_refuses_without_implicit_collection_then_explicit_gc_progresses() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("capacity.spil");
    let cancel = AtomicBool::new(false);
    let receipt = Container::create_new(&path, input(b"initial"), &cancel, |_| true).unwrap();
    let container = receipt.container;
    let mut heads = receipt.heads;
    for index in 1..MAX_SNAPSHOTS {
        let root = format!("bounded root {index}");
        heads = container
            .publish(
                heads,
                Selection::Current,
                input(root.as_bytes()),
                &cancel,
                |_| true,
            )
            .unwrap()
            .heads;
    }
    let failure = container
        .publish(
            heads,
            Selection::Current,
            input(b"one beyond retained capacity"),
            &cancel,
            |_| true,
        )
        .unwrap_err();
    assert!(matches!(failure, Error::ResourceLimit(_)));
    assert!(failure.to_string().contains("collect"));
    assert_eq!(container.stats(&cancel).unwrap().snapshots, MAX_SNAPSHOTS);
    assert_eq!(
        container
            .capture(Selection::Current, &cancel)
            .unwrap()
            .heads,
        heads
    );
    container.collect_unreachable(&cancel, || true).unwrap();
    assert_eq!(container.stats(&cancel).unwrap().snapshots, 2);
    let next = container
        .publish(
            heads,
            Selection::Current,
            input(b"capacity reclaimed explicitly"),
            &cancel,
            |_| true,
        )
        .unwrap();
    assert_eq!(next.outcome, PublicationOutcome::PublishedConfirmed);
}

#[test]
fn checked_in_sqlite_golden_is_inspected_without_the_creator() {
    let golden = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/profile-v1.spil");
    if std::env::var_os("SPILING_REGENERATE_PTAC_GOLDEN").is_some() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("golden.spil");
        let receipt = Container::create_new(
            &source,
            SnapshotInput {
                root: b"PTAC golden root\n",
                objects: &[ObjectInput {
                    role: ObjectRole::new(1).unwrap(),
                    bytes: b"independent exact asset bytes\n",
                }],
                guard: b"PTAC golden guard\n",
            },
            &AtomicBool::new(false),
            |_| true,
        )
        .unwrap();
        drop(receipt);
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::copy(source, &golden).unwrap();
    }
    let oracle =
        rusqlite::Connection::open_with_flags(&golden, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let application: u32 = oracle
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .unwrap();
    assert_eq!(application, 0x50544143);
    let version: u32 = oracle
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 1);
    let profile: String = oracle
        .query_row("SELECT profile FROM container", [], |row| row.get(0))
        .unwrap();
    assert_eq!(profile, "spiling.project");
    let mut statement = oracle
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap();
    let tables = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
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
    drop(statement);
    let codec: u32 = oracle
        .query_row("SELECT max(codec) FROM chunks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(codec, 0);
    let asset: Vec<u8> = oracle
        .query_row(
            "SELECT data FROM chunks WHERE data=?1",
            [b"independent exact asset bytes\n".as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(asset, b"independent exact asset bytes\n");
    drop(oracle);
    let container = Container::open_existing(&golden, true).unwrap();
    let cancel = AtomicBool::new(false);
    container.check(&cancel).unwrap();
    let capture = container.capture(Selection::Current, &cancel).unwrap();
    assert_eq!(capture.root, b"PTAC golden root\n");
    assert_eq!(capture.guard, b"PTAC golden guard\n");
    assert!(capture.heads.previous().is_none());
}

#[test]
fn collection_and_compaction_retain_exact_two_roots_and_guard() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("source.spil");
    let cancel = AtomicBool::new(false);
    let receipt = Container::create_new(&path, input(b"one"), &cancel, |_| true).unwrap();
    let container = receipt.container;
    let mut heads = receipt.heads;
    for root in [b"two".as_slice(), b"three", b"four"] {
        heads = container
            .publish(heads, Selection::Current, input(root), &cancel, |_| true)
            .unwrap()
            .heads;
    }
    let before = container.stats(&cancel).unwrap();
    assert_eq!(before.snapshots, 4);
    assert!(before.unreachable_payload_bytes > 0);
    assert!(matches!(
        container.collect_unreachable(&cancel, || false),
        Err(Error::Cancelled)
    ));
    assert_eq!(container.stats(&cancel).unwrap(), before);
    let compact_path = temporary.path().join("copy.spil");
    let compact = container
        .compact_to_new(&compact_path, &cancel, |_| true)
        .unwrap();
    assert_eq!(compact.heads.current(), heads.current());
    assert_eq!(compact.heads.previous(), heads.previous());
    assert_eq!(compact.container.stats(&cancel).unwrap().snapshots, 2);
    assert!(container.compact_to_new(&path, &cancel, |_| true).is_err());
    container.collect_unreachable(&cancel, || true).unwrap();
    let after = container.stats(&cancel).unwrap();
    assert_eq!(after.snapshots, 2);
    assert_eq!(after.unreachable_payload_bytes, 0);
    assert_eq!(
        container
            .capture(Selection::Current, &cancel)
            .unwrap()
            .heads,
        heads
    );
    let previous = compact
        .container
        .capture(Selection::Previous, &cancel)
        .unwrap();
    assert_eq!(previous.root, b"three");
    assert_eq!(previous.guard, b"independent guard");
}

#[test]
fn first_publication_is_complete_exclusive_and_classifies_late_failures() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("new.spil");
    let cancel = AtomicBool::new(false);
    let cancelled = Container::create_new(&path, input(b"one"), &cancel, |stage| {
        stage != SaveStage::BeforePublish
    });
    assert!(matches!(cancelled, Err(Error::Cancelled)));
    assert!(!path.exists());
    let receipt =
        Container::create_new_retaining(&path, input(b"two"), input(b"one"), &cancel, |stage| {
            stage != SaveStage::Published
        })
        .unwrap();
    assert_eq!(receipt.outcome, PublicationOutcome::PublishedUnconfirmed);
    assert!(receipt.error.is_some());
    assert_eq!(
        receipt
            .container
            .capture(Selection::Current, &cancel)
            .unwrap()
            .root,
        b"two"
    );
    assert_eq!(
        receipt
            .container
            .capture(Selection::Previous, &cancel)
            .unwrap()
            .root,
        b"one"
    );
    drop(receipt);
    let original = std::fs::read(&path).unwrap();
    assert!(Container::create_new(&path, input(b"overwrite"), &cancel, |_| true).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(&path).unwrap().nlink(), 1);
    }
}

#[test]
fn cancellation_is_effective_until_but_not_after_decision() {
    use std::sync::atomic::Ordering;
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("cancel.spil");
    let cancel = AtomicBool::new(false);
    let receipt = Container::create_new(&path, input(b"one"), &cancel, |stage| {
        if stage == SaveStage::BeforePublish {
            cancel.store(true, Ordering::Release);
        }
        true
    })
    .unwrap();
    assert_eq!(receipt.outcome, PublicationOutcome::PublishedConfirmed);
    cancel.store(false, Ordering::Release);
    let container = receipt.container;
    let current = container.capture(Selection::Current, &cancel).unwrap();
    let base = current.heads;
    assert!(matches!(
        container.capture(Selection::Current, &cancel),
        Err(Error::Locked)
    ));
    assert!(matches!(
        container.publish(base, Selection::Current, input(b"two"), &cancel, |_| true),
        Err(Error::Locked)
    ));
    drop(current);
    assert!(matches!(
        container.publish(
            base,
            Selection::Current,
            input(b"two"),
            &cancel,
            |stage| stage != SaveStage::BeforeCommit
        ),
        Err(Error::Cancelled)
    ));
    assert_eq!(
        container
            .capture(Selection::Current, &cancel)
            .unwrap()
            .heads,
        base
    );
    let published = container
        .publish(base, Selection::Current, input(b"two"), &cancel, |stage| {
            if stage == SaveStage::BeforeCommit {
                cancel.store(true, Ordering::Release);
            }
            true
        })
        .unwrap();
    assert_eq!(published.outcome, PublicationOutcome::PublishedConfirmed);
    cancel.store(false, Ordering::Release);
    assert!(matches!(
        container.publish(base, Selection::Current, input(b"three"), &cancel, |_| true),
        Err(Error::Conflict)
    ));
}

#[test]
fn real_create_publish_capture_and_previous() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("example.spil");
    let cancel = AtomicBool::new(false);
    let bytes = vec![42; CHUNK_BYTES + 1];
    let objects = [ObjectInput {
        role: ObjectRole::new(1).unwrap(),
        bytes: &bytes,
    }];
    let first = Container::create_new(
        &path,
        SnapshotInput {
            root: b"first",
            objects: &objects,
            guard: b"guard",
        },
        &cancel,
        |_| true,
    )
    .unwrap();
    assert_eq!(first.outcome, PublicationOutcome::PublishedConfirmed);
    let capture = first
        .container
        .capture(Selection::Current, &cancel)
        .unwrap();
    assert_eq!(capture.root, b"first");
    assert_eq!(
        capture
            .read_object(ObjectId::of(&bytes), bytes.len(), &cancel)
            .unwrap(),
        bytes
    );
    let base = capture.heads;
    drop(capture);
    first
        .container
        .publish(
            base,
            Selection::Current,
            SnapshotInput {
                root: b"second",
                objects: &objects,
                guard: b"later",
            },
            &cancel,
            |_| true,
        )
        .unwrap();
    assert_eq!(
        first
            .container
            .capture(Selection::Previous, &cancel)
            .unwrap()
            .root,
        b"first"
    );
    first.container.check(&cancel).unwrap();
    drop(first);
    let reopened = Container::open_existing(&path, false).unwrap();
    assert_eq!(
        reopened.capture(Selection::Current, &cancel).unwrap().root,
        b"second"
    );
}
