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
        section_task: None,
        section_cancel: Arc::new(AtomicBool::new(false)),
        display_task: None,
        display_cancel: Arc::new(AtomicBool::new(false)),
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

fn displayed(host: &Host) -> Snapshot {
    wait(host, |s| {
        !s.display_loading
            && s.display.as_ref().is_some_and(|d| {
                s.scene
                    .as_ref()
                    .is_some_and(|summary| d.same_revision(summary))
            })
    })
}
#[test]
#[ignore = "actual native scene, sections and source mappings; requires SPILING_TEST_ENGINE"]
fn checked_scene_picking_shared_occurrences_sections_and_large_origin() {
    use spiling_contracts::geometry::*;
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
    imported(&host);
    let snapshot = displayed(&host);
    let scene = snapshot.display.unwrap();
    assert_eq!(scene.definitions.len(), 1);
    assert!(!scene.definitions[0].chunks.is_empty());
    let bounds = scene.summary.bounds_mm.unwrap();
    let center = glam::DVec3::from_array(std::array::from_fn(|i| {
        (bounds.min[i] + bounds.max[i]) * 0.5
    }));
    let reference = crate::viewport::pick(&scene, center + glam::DVec3::Z * 100.0, -glam::DVec3::Z)
        .expect("ray must hit the admitted source");
    host.geometry(GeometryCommand::InspectFace {
        reference: reference.clone(),
    })
    .unwrap();
    let inspected = wait(&host, |s| {
        s.inspection
            .as_ref()
            .is_some_and(|i| i.reference == reference)
    });
    assert_eq!(inspected.inspection.unwrap().reference, reference);
    let mut pose = RigidPoseMm::IDENTITY;
    pose.translation_mm = [40.0, 0.0, 0.0];
    host.geometry(GeometryCommand::AddInstance {
        session_id: scene.summary.session_id.clone(),
        base_revision: scene.summary.revision,
        definition_id: scene.definitions[0].record.definition_id.clone(),
        pose,
    })
    .unwrap();
    wait(&host, |s| {
        s.scene.as_ref().is_some_and(|s| s.occurrence_count == 2)
    });
    let copied = displayed(&host).display.unwrap();
    assert!(
        Arc::ptr_eq(&scene.definitions[0], &copied.definitions[0]),
        "pose/occurrence edits must reuse checked bytes"
    );
    host.geometry(GeometryCommand::StartSection {
        session_id: copied.summary.session_id.clone(),
        base_revision: copied.summary.revision,
        plane: PlaneMm {
            origin_mm: center.to_array(),
            normal: [0.0, 0.0, 1.0],
        },
    })
    .unwrap();
    let section = wait(&host, |s| s.section.is_some());
    assert!(section.section.unwrap().summary.total_loop_count >= 2);
    let original = copied.occurrences[0].clone();
    host.geometry(GeometryCommand::RemoveInstance {
        session_id: copied.summary.session_id.clone(),
        base_revision: copied.summary.revision,
        occurrence_id: copied.occurrences[1].occurrence_id,
    })
    .unwrap();
    wait(&host, |s| {
        s.scene.as_ref().is_some_and(|s| s.occurrence_count == 1)
    });
    let copied = displayed(&host).display.unwrap();
    // Move the first occurrence near a large origin without changing its definition.
    let project = host.snapshot().project.unwrap();
    pose.translation_mm = [1e9, 1e9, 0.0];
    host.geometry(GeometryCommand::SetInstancePose {
        session_id: copied.summary.session_id.clone(),
        base_revision: copied.summary.revision,
        project_revision: project.revision,
        occurrence_id: original.occurrence_id,
        pose,
    })
    .unwrap();
    wait(&host, |s| {
        s.scene
            .as_ref()
            .is_some_and(|s| s.revision != copied.summary.revision)
    });
    let placed = displayed(&host).display.unwrap();
    let hit = crate::viewport::pick(
        &placed,
        center + glam::DVec3::new(1e9, 1e9, 100.0),
        -glam::DVec3::Z,
    )
    .expect("large-origin source must remain pickable in f64");
    assert_eq!(hit.occurrence_id, original.occurrence_id);
    assert!(
        host.snapshot().section.is_none(),
        "revision changes invalidate sections"
    );
    let close = send(&host, Action::Close);
    host.confirm(close.confirmation.unwrap().id, true).unwrap();
    wait(&host, |s| s.phase == Phase::Closed);
}
