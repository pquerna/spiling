// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use std::time::Instant;

fn wait(host: &Host, predicate: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let mut receiver = host.receiver.clone();
    host.runtime.as_ref().unwrap().block_on(async {
        tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                let snapshot = receiver.borrow_and_update().clone();
                if predicate(&snapshot) {
                    return snapshot;
                }
                receiver
                    .changed()
                    .await
                    .expect("host stopped before expected state");
            }
        })
        .await
        .expect("native host deadline")
    })
}
fn idle_after(host: &Host, serial: u64) -> Snapshot {
    wait(host, |s| s.serial > serial && !s.busy && !s.picker_open)
}
fn send(host: &Host, action: Action) -> Snapshot {
    let serial = host.snapshot().serial;
    host.send(Request::Action(action)).unwrap();
    idle_after(host, serial)
}
fn native(path: &std::path::Path) -> NativePath {
    NativePath::from_os_str(path.as_os_str()).unwrap()
}
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry/independent-exporters/occt-box-mm.step")
}
fn engine() -> PathBuf {
    std::env::var_os("SPILING_TEST_ENGINE")
        .map(PathBuf::from)
        .expect("set SPILING_TEST_ENGINE")
}
fn imported(host: &Host) -> Snapshot {
    let snapshot = send(host, Action::Import(native(&fixture())));
    assert!(
        snapshot.project.as_ref().unwrap().dirty,
        "{:?}",
        snapshot.problems
    );
    assert_eq!(snapshot.scene.as_ref().unwrap().occurrence_count, 1);
    snapshot
}

#[test]
#[ignore = "actual supervised engine; requires SPILING_TEST_ENGINE"]
fn project_receipts_discard_read_only_restart_and_reap() {
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("operations"),
        },
        || {},
    )
    .unwrap();
    let initial = wait(&host, |s| s.phase != Phase::Starting);
    assert_eq!(initial.phase, Phase::Running, "{:?}", initial.problems);
    let first = imported(&host);
    let project_id = first.project.as_ref().unwrap().project_id.clone();
    let new = send(&host, Action::New);
    let id = new.confirmation.as_ref().unwrap().id;
    host.confirm(id, false).unwrap();
    let kept = wait(&host, |s| {
        s.serial > new.serial && !s.busy && s.confirmation.is_none()
    });
    assert_eq!(kept.project.as_ref().unwrap().project_id, project_id);
    assert!(kept.project.as_ref().unwrap().dirty);
    let saved_path = root.path().join("Native project – 保存");
    let saved = send(
        &host,
        Action::Save {
            target: Some(native(&saved_path)),
        },
    );
    assert!(
        !saved.project.as_ref().unwrap().dirty,
        "{:?}",
        saved.problems
    );
    assert!(saved_path.join("manifest.json").is_file());
    // Failed Save As cannot replace the attached saved route.
    let failed_path = root.path().join("not-a-directory");
    std::fs::write(&failed_path, b"occupied").unwrap();
    let failed = send(
        &host,
        Action::Save {
            target: Some(native(&failed_path)),
        },
    );
    assert!(!failed.problems.is_empty());
    let restarted = send(&host, Action::Restart);
    assert_eq!(restarted.project.as_ref().unwrap().project_id, project_id);
    assert_eq!(restarted.scene.as_ref().unwrap().occurrence_count, 1);
    assert!(
        restarted
            .project
            .as_ref()
            .unwrap()
            .path_label
            .as_ref()
            .unwrap()
            .contains("Native project")
    );
    send(&host, Action::New);
    let read_only = send(
        &host,
        Action::Open {
            path: native(&saved_path),
            read_only: true,
            recover_previous: false,
        },
    );
    assert!(read_only.project.as_ref().unwrap().read_only);
    let refused = send(&host, Action::Import(native(&fixture())));
    assert!(refused.project.as_ref().unwrap().read_only);
    assert!(!refused.project.as_ref().unwrap().dirty);
    let pid = refused.hello.as_ref().unwrap().pid;
    let closed = send(&host, Action::Close);
    assert_eq!(closed.phase, Phase::Closed);
    #[cfg(target_os = "linux")]
    assert!(
        !PathBuf::from(format!("/proc/{pid}")).exists(),
        "child still exists after normal close"
    );
    drop(host);
}

#[test]
#[ignore = "actual supervised engine; requires SPILING_TEST_ENGINE"]
#[cfg(target_os = "linux")]
fn engine_death_is_bounded_and_restart_does_not_claim_unsaved_recovery() {
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("operations"),
        },
        || {},
    )
    .unwrap();
    wait(&host, |s| s.phase == Phase::Running);
    let state = imported(&host);
    let pid = state.hello.as_ref().unwrap().pid;
    let started = Instant::now();
    assert!(
        std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    let failed = wait(&host, |s| s.phase == Phase::Interrupted);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(!failed.problems.is_empty());
    assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
    let restarted = send(&host, Action::Restart);
    assert_eq!(restarted.scene.as_ref().unwrap().occurrence_count, 0);
    assert!(!restarted.project.as_ref().unwrap().dirty);
    assert_eq!(send(&host, Action::Close).phase, Phase::Closed);
}

#[test]
fn native_path_limit_and_problem_history_are_bounded() {
    let root = tempfile::tempdir().unwrap();
    let (updates, _) = watch::channel(Snapshot::default());
    let mut actor = Actor {
        config: Config {
            engine: root.path().join("missing"),
            store: root.path().join("store"),
        },
        updates,
        wake: Arc::new(|| {}),
        state: Snapshot::default(),
        client: None,
        pending: None,
        confirmation: None,
        route: None,
        deferred: None,
        dialog: None,
        nonce: 0,
        executing: false,
    };
    for i in 0..20 {
        actor.problem(format!("{i}:{}", "界".repeat(PROBLEM_BYTES)));
    }
    assert_eq!(actor.state.problems.len(), HISTORY_ROWS);
    assert!(
        actor
            .state
            .problems
            .iter()
            .all(|p| p.len() <= PROBLEM_BYTES)
    );
    assert!(selected_action(Picker::Import, &PathBuf::from("a".repeat(100_000))).is_err());
}

#[test]
#[ignore = "actual supervised engine; requires SPILING_TEST_ENGINE"]
fn cancellation_observes_terminal_outcome_before_another_mutation() {
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("operations"),
        },
        || {},
    )
    .unwrap();
    wait(&host, |s| s.phase == Phase::Running);
    host.send(Request::Action(Action::Import(native(&fixture()))))
        .unwrap();
    let active = wait(&host, |s| {
        s.active_operation || s.jobs.back().is_some_and(|j| j.status.is_terminal())
    });
    if active.active_operation {
        host.command(Command::Cancel).unwrap();
    }
    let outcome = wait(&host, |s| {
        !s.busy && s.jobs.back().is_some_and(|j| j.status.is_terminal())
    });
    let status = outcome.jobs.back().unwrap().status;
    assert!(
        matches!(status, JobStatus::Completed | JobStatus::Cancelled),
        "{:?}",
        outcome.problems
    );
    if status == JobStatus::Cancelled {
        assert_eq!(outcome.scene.as_ref().unwrap().occurrence_count, 0);
    }
    if outcome.project.as_ref().unwrap().dirty {
        let requested = send(&host, Action::Close);
        host.confirm(requested.confirmation.unwrap().id, true)
            .unwrap();
        wait(&host, |s| s.phase == Phase::Closed);
    } else {
        assert_eq!(send(&host, Action::Close).phase, Phase::Closed);
    }
}

#[test]
#[ignore = "actual ELF mismatch fault injection; requires SPILING_TEST_ENGINE"]
#[cfg(target_os = "linux")]
fn startup_build_mismatch_is_visible_and_reaps_the_child() {
    let root = tempfile::tempdir().unwrap();
    let mismatched = root.path().join("mismatched-engine");
    let mut bytes = std::fs::read(engine()).unwrap();
    let expected = spiling_contracts::ENGINE_BUILD_ID.as_bytes();
    let mut replacements = 0;
    for offset in 0..bytes.len() - expected.len() {
        if &bytes[offset..offset + expected.len()] == expected {
            bytes[offset] = b'9';
            replacements += 1;
        }
    }
    assert!(
        replacements > 0,
        "engine identity must exist in the built ELF"
    );
    std::fs::write(&mismatched, bytes).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&mismatched, std::fs::Permissions::from_mode(0o700)).unwrap();
    let started = Instant::now();
    let host = Host::new(
        Config {
            engine: mismatched.clone(),
            store: root.path().join("operations"),
        },
        || {},
    )
    .unwrap();
    let failed = wait(&host, |s| s.phase == Phase::Interrupted);
    assert!(started.elapsed() < Duration::from_secs(15));
    assert!(
        failed.problems.iter().any(|p| p.contains("mismatch")),
        "{:?}",
        failed.problems
    );
    assert!(failed.hello.is_none());
    for process in std::fs::read_dir("/proc").unwrap().flatten() {
        assert_ne!(
            std::fs::read_link(process.path().join("exe")).ok().as_ref(),
            Some(&mismatched),
            "mismatched child leaked"
        );
    }
    assert_eq!(send(&host, Action::Close).phase, Phase::Closed);
}
