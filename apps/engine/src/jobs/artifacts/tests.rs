// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tonic::Code;

async fn stored_artifact() -> (tempfile::TempDir, Jobs, Artifact, Vec<u8>) {
    let directory = tempfile::tempdir().unwrap();
    let jobs = Jobs::open(directory.path()).unwrap();
    let bytes = (0..TRANSFER_FRAGMENT_BYTES * 16 + 37)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let artifact = Artifact {
        name: format!("artifacts/{sha256}"),
        sha256,
        size_bytes: bytes.len() as u64,
        media_type: "application/octet-stream".into(),
    };
    let name = artifact.name.clone();
    let media_type = artifact.media_type.clone();
    let stored = bytes.clone();
    jobs.with(move |inner| {
        inner
            .db
            .execute("INSERT INTO artifacts VALUES(?,?)", params![name, stored])
            .map_err(internal)?;
        inner
            .db
            .execute(
                "INSERT INTO artifact_types VALUES(?,?)",
                params![name, media_type],
            )
            .map_err(internal)?;
        Ok(())
    })
    .await
    .unwrap();
    (directory, jobs, artifact, bytes)
}

#[tokio::test]
async fn incremental_reads_match_exact_ranges_and_survive_reopen() {
    let (directory, jobs, artifact, bytes) = stored_artifact().await;
    drop(jobs);
    let jobs = Jobs::open(directory.path()).unwrap();
    for (start, end) in [
        (0, bytes.len()),
        (TRANSFER_FRAGMENT_BYTES - 3, TRANSFER_FRAGMENT_BYTES * 3 + 5),
        (bytes.len() - 1, bytes.len()),
        (bytes.len(), bytes.len()),
    ] {
        let (sender, mut receiver) = mpsc::channel(2);
        let reader = jobs.clone();
        let descriptor = artifact.clone();
        let task = tokio::task::spawn_blocking(move || {
            reader.stream_artifact(&descriptor, start as u64, end as u64, &sender)
        });
        let mut received = Vec::new();
        while let Some(fragment) = receiver.recv().await {
            let data = fragment.unwrap().data;
            assert!(!data.is_empty());
            assert!(data.len() <= TRANSFER_FRAGMENT_BYTES);
            received.extend_from_slice(&data);
        }
        task.await.unwrap().unwrap();
        assert_eq!(received, bytes[start..end]);
    }
}

#[tokio::test]
async fn stalled_read_does_not_block_publication_and_drop_releases_snapshot() {
    let (_directory, jobs, artifact, _) = stored_artifact().await;
    let (sender, receiver) = mpsc::channel(2);
    let probe = sender.clone();
    let reader = jobs.clone();
    let descriptor = artifact.clone();
    let task = tokio::task::spawn_blocking(move || {
        reader.stream_artifact(&descriptor, 0, descriptor.size_bytes, &sender)
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while probe.capacity() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!task.is_finished());
    let name = artifact.name.clone();
    let visible = tokio::time::timeout(Duration::from_secs(5), async {
        jobs.with(|inner| {
            inner
                .db
                .execute(
                    "INSERT INTO artifacts VALUES(?,?)",
                    params!["artifacts/later", vec![7u8; 64]],
                )
                .map_err(internal)?;
            Ok(())
        })
        .await?;
        jobs.artifact(name).await
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(visible, artifact);
    drop(receiver);
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    jobs.with(|inner| {
        let busy: i64 = inner
            .db
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
            .map_err(internal)?;
        assert_eq!(busy, 0);
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn invalid_ranges_and_changed_or_missing_artifacts_refuse() {
    let (_directory, jobs, artifact, _) = stored_artifact().await;
    let mut changed = artifact.clone();
    changed.size_bytes += 1;
    let mut missing = artifact.clone();
    missing.name = "artifacts/missing".into();
    for (descriptor, start, end, expected) in [
        (artifact.clone(), 2, 1, Code::OutOfRange),
        (
            artifact.clone(),
            0,
            artifact.size_bytes + 1,
            Code::OutOfRange,
        ),
        (changed, 0, 1, Code::DataLoss),
        (missing, 0, 1, Code::NotFound),
    ] {
        let (sender, _receiver) = mpsc::channel(2);
        let reader = jobs.clone();
        let error = tokio::task::spawn_blocking(move || {
            reader.stream_artifact(&descriptor, start, end, &sender)
        })
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error.code(), expected);
    }
}
