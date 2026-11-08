// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use std::time::Instant;
fn layout_id(host: &Host) -> spiling_contracts::project::PrintLayoutId {
    host.snapshot()
        .project
        .expect("observed project")
        .layout_id
        .clone()
}

fn wait(host: &Host, predicate: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let mut receiver = host.receiver.clone();
    host.runtime.as_ref().unwrap().block_on(async {
        tokio::time::timeout(Duration::from_secs(180), async {
            let mut previous = String::new();
            loop {
                let snapshot = receiver.borrow_and_update().clone();
                if std::env::var_os("SPILING_HOST_TRACE").is_some() && previous != snapshot.message
                {
                    eprintln!(
                        "host: {} busy={} loading={} review={} problems={:?}",
                        snapshot.message,
                        snapshot.busy,
                        snapshot.manufacturing.loading,
                        snapshot.manufacturing.review.is_some(),
                        snapshot.problems
                    );
                    previous = snapshot.message.clone();
                }
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
        .unwrap_or_else(|_| {
            let s = host.snapshot();
            panic!(
                "native host deadline: {} busy={} loading={} problems={:?}",
                s.message, s.busy, s.manufacturing.loading, s.problems
            )
        })
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
fn compute_preferences_apply_on_guarded_restart_only() {
    use spiling_contracts::compute::{ComputeBackend, ComputeSettings};
    let root = tempfile::tempdir().unwrap();
    let settings = ComputeSettings {
        default_backend: ComputeBackend::Cpu,
        fea_backend: None,
    };
    let host = Host::with_compute(
        Config {
            engine: engine(),
            store: root.path().join("operations"),
        },
        settings,
        || {},
    )
    .unwrap();
    let initial = wait(&host, |state| state.phase != Phase::Starting);
    assert_eq!(initial.phase, Phase::Running, "{:?}", initial.problems);
    let initial_hello = initial.hello.as_ref().unwrap();
    assert_eq!(initial_hello.compute.settings, settings);
    assert_eq!(
        initial_hello.compute.default_device,
        initial_hello.compute.fea_device
    );
    let next = ComputeSettings {
        fea_backend: Some(ComputeBackend::Cpu),
        ..settings
    };
    host.compute_settings(next).unwrap();
    let dirty = imported(&host);
    let original_project = dirty.project.as_ref().unwrap().project_id.clone();
    assert_eq!(dirty.hello.as_ref().unwrap().compute.settings, settings);
    host.command(Command::Restart).unwrap();
    let pending = wait(&host, |state| state.confirmation.is_some());
    host.confirm(pending.confirmation.as_ref().unwrap().id, false)
        .unwrap();
    let kept = wait(&host, |state| {
        state.serial > pending.serial && !state.busy && state.confirmation.is_none()
    });
    assert_eq!(kept.project.as_ref().unwrap().project_id, original_project);
    assert_eq!(
        kept.hello.as_ref().unwrap().instance_id,
        initial_hello.instance_id
    );
    assert_eq!(kept.hello.as_ref().unwrap().compute.settings, settings);
    host.command(Command::Restart).unwrap();
    let pending = wait(&host, |state| state.confirmation.is_some());
    host.confirm(pending.confirmation.as_ref().unwrap().id, true)
        .unwrap();
    let restarted = wait(&host, |state| {
        state.phase == Phase::Running
            && !state.busy
            && state
                .hello
                .as_ref()
                .is_some_and(|hello| hello.instance_id != initial_hello.instance_id)
    });
    assert_eq!(restarted.hello.as_ref().unwrap().compute.settings, next);
    assert!(!restarted.project.as_ref().unwrap().dirty);
    assert_eq!(restarted.scene.as_ref().unwrap().occurrence_count, 0);
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
    let saved_path = root.path().join("Native project – 保存.spil");
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
    assert!(saved_path.is_file());
    // Failed Save As cannot replace the attached saved route.
    let failed_path = root.path().join("occupied.spil");
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
fn drop_reaps_child_with_native_observation_in_flight() {
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("operations"),
        },
        || {},
    )
    .unwrap();
    let ready = wait(&host, |s| s.phase == Phase::Running);
    let pid = ready.hello.as_ref().unwrap().pid;
    let curved = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step");
    host.send(Request::Action(Action::Import(native(&curved))))
        .unwrap();
    wait(&host, |s| s.active_operation || !s.jobs.is_empty());
    let started = Instant::now();
    drop(host);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
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
    assert_ne!(
        restarted.hello.as_ref().unwrap().session_id,
        state.hello.as_ref().unwrap().session_id
    );
    // A late old observation must never interrupt work in the replacement session.
    let current = imported(&host);
    assert_eq!(current.phase, Phase::Running);
    assert_eq!(current.scene.as_ref().unwrap().occurrence_count, 1);
    let closing = send(&host, Action::Close);
    host.confirm(closing.confirmation.unwrap().id, true)
        .unwrap();
    wait(&host, |s| s.phase == Phase::Closed);
}

#[test]
fn native_path_limit_and_problem_history_are_bounded() {
    let root = tempfile::tempdir().unwrap();
    let (updates, _) = watch::channel(Snapshot::default());
    let mut actor = Actor {
        compute_settings: Default::default(),
        library_location: None,
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
        optical_task: None,
        stock_task: None,
        diagnostic_task: None,
        diagnostic_paused: false,
        image_task: None,
        review_task: None,
        alternative_task: None,
        preserve_current_for_option_edit: false,
        export_task: None,
        section_task: None,
        section_cancel: Arc::new(AtomicBool::new(false)),
        display_task: None,
        display_cancel: Arc::new(AtomicBool::new(false)),
    };
    eprintln!(
        "Native request future bytes: {}",
        std::mem::size_of_val(&actor.request(Request::Command(Command::Restart)))
    );
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
    actor.route = Some(Route {
        path: native(&root.path().join("original.spil")),
        read_only: false,
        uncertain: false,
    });
    let attempted = native(&root.path().join("attempted.spil"));
    actor.pending = Some(Pending {
        name: "operations/unresolved-save".into(),
        label: "Save",
        receipt: Receipt::Save(Some(Route {
            path: attempted.clone(),
            read_only: false,
            uncertain: false,
        })),
        paused: false,
    });
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        actor
            .disconnect("Unresolved save disconnected".into())
            .await;
        assert_eq!(actor.route.as_ref().unwrap().path, attempted);
        assert!(actor.route.as_ref().unwrap().uncertain);
        actor.reopen().await;
    });
    assert!(actor.pending.is_none());
    assert!(actor.state.message.contains("explicitly inspect or reopen"));
}

#[test]
#[ignore = "actual postpublication engine exit; requires SPILING_TEST_ENGINE"]
#[cfg(unix)]
fn unresolved_save_exit_suppresses_restart_reopen_for_save_and_save_as() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    for (stage, save_as) in [("after_commit", false), ("target_published", true)] {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original.spil");
        let seed = Host::new(
            Config {
                engine: engine(),
                store: root.path().join("seed-operations"),
            },
            || {},
        )
        .unwrap();
        wait(&seed, |snapshot| snapshot.phase == Phase::Running);
        let imported = imported(&seed);
        let project_id = imported.project.as_ref().unwrap().project_id.clone();
        let saved = send(
            &seed,
            Action::Save {
                target: Some(native(&original)),
            },
        );
        assert!(
            !saved.project.as_ref().unwrap().dirty,
            "{:?}",
            saved.problems
        );
        send(&seed, Action::Close);
        drop(seed);

        symlink(
            engine().canonicalize().unwrap(),
            root.path().join("real-engine"),
        )
        .unwrap();
        let wrapper = root.path().join("fault-engine");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nexport SPILING_PROJECT_FAULT={stage}\nexec \"$(dirname \"$0\")/real-engine\" \"$@\"\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let host = Host::new(
            Config {
                engine: wrapper,
                store: root.path().join("fault-operations"),
            },
            || {},
        )
        .unwrap();
        wait(&host, |snapshot| snapshot.phase == Phase::Running);
        let opened = send(
            &host,
            Action::Open {
                path: native(&original),
                read_only: false,
                recover_previous: false,
            },
        );
        assert_eq!(opened.project.as_ref().unwrap().project_id, project_id);
        let display = displayed(&host).display.unwrap();
        host.geometry(GeometryCommand::AddInstance {
            session_id: display.summary.session_id.clone(),
            base_revision: display.summary.revision,
            definition_id: display.definitions[0].record.definition_id.clone(),
            pose: RigidPoseMm::IDENTITY,
        })
        .unwrap();
        wait(&host, |snapshot| {
            !snapshot.busy
                && snapshot
                    .scene
                    .as_ref()
                    .is_some_and(|scene| scene.occurrence_count == 2)
        });
        let attempted = if save_as {
            root.path().join("attempted.spil")
        } else {
            original
        };
        host.send(Request::Action(Action::Save {
            target: save_as.then(|| native(&attempted)),
        }))
        .unwrap();
        wait(&host, |snapshot| snapshot.phase == Phase::Interrupted);
        assert!(attempted.is_file());
        let restarted = send(&host, Action::Restart);
        assert_eq!(restarted.phase, Phase::Running);
        assert_ne!(restarted.project.as_ref().unwrap().project_id, project_id);
        assert_eq!(restarted.scene.as_ref().unwrap().occurrence_count, 0);
        assert!(restarted.message.contains("explicitly inspect or reopen"));
        let inspected = send(
            &host,
            Action::Open {
                path: native(&attempted),
                read_only: true,
                recover_previous: false,
            },
        );
        assert_eq!(inspected.project.as_ref().unwrap().project_id, project_id);
        assert_eq!(inspected.scene.as_ref().unwrap().occurrence_count, 2);
        send(&host, Action::Close);
    }
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
    let focus = scene
        .focus_target(Some(&reference), Some(reference.occurrence_id))
        .unwrap();
    assert!(focus.valid());
    assert!(
        focus
            .normal
            .is_some_and(|normal| normal.dot(glam::DVec3::Z) > 0.9)
    );
    assert!((focus.bounds_mm.max[2] - focus.bounds_mm.min[2]).abs() < 0.01);
    assert!(scene.focus_target(Some(&reference), None).is_err());
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
        copied
            .focus_target(Some(&reference), Some(reference.occurrence_id))
            .is_err()
    );
    let original_part = scene
        .focus_target(None, Some(reference.occurrence_id))
        .unwrap();
    let copy_id = copied
        .occurrences
        .iter()
        .find(|item| item.occurrence_id != reference.occurrence_id)
        .unwrap()
        .occurrence_id;
    let copy_part = copied.focus_target(None, Some(copy_id)).unwrap();
    assert!((copy_part.center() - original_part.center() - glam::DVec3::X * 40.0).length() < 1e-10);
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
    let section = section.section.unwrap();
    assert!(section.summary.total_loop_count >= 2);
    let all = crate::analysis::ContourPreview::load(&section, None).unwrap();
    let scoped =
        crate::analysis::ContourPreview::load(&section, Some(copied.occurrences[0].occurrence_id))
            .unwrap();
    assert!(all.matches(&section, None));
    assert!(scoped.matches(&section, Some(copied.occurrences[0].occurrence_id)));
    let mut cut = crate::analysis::SectionView {
        enabled: true,
        offset_mm: center.z,
        ..Default::default()
    };
    let plane = cut.plane(None);
    let ray_origin = center + glam::DVec3::Z * 100.0;
    assert!(
        crate::viewport::pick_clipped(&copied, ray_origin, -glam::DVec3::Z, &cut, plane, None)
            .is_none(),
        "synthetic cap must not pick the backing native face"
    );
    cut.flipped = true;
    assert!(
        crate::viewport::pick_clipped(&copied, ray_origin, -glam::DVec3::Z, &cut, plane, None)
            .is_some(),
        "retained outer face remains native pickable"
    );
    cut.selected_only = true;
    assert!(
        crate::viewport::pick_clipped(
            &copied,
            ray_origin,
            -glam::DVec3::Z,
            &cut,
            plane,
            Some(copied.occurrences[1].occurrence_id)
        )
        .is_none(),
        "scope excludes the original occurrence"
    );
    cut.section_only = true;
    assert!(
        crate::viewport::pick_clipped(&copied, ray_origin, -glam::DVec3::Z, &cut, plane, None)
            .is_none()
    );
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

#[test]
#[ignore = "actual supervised manufacturing engine; requires SPILING_TEST_ENGINE"]
fn manufacturing_review_export_stale_edit_and_source_independent_reopen() {
    use spiling_contracts::{manufacturing::*, process::ProcessEdit, surface::SurfaceScope};
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
    let source = root.path().join("occt-rounded-housing-mm.step");
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"),
        &source,
    )
    .unwrap();
    send(&host, Action::Import(native(&source)));
    let s = wait(&host, |s| {
        s.display
            .as_ref()
            .is_some_and(|d| d.same_revision(s.scene.as_ref().unwrap()))
    });
    let intent_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/manufacturing/complete-sparse.intent.json");
    let loaded = send(&host, Action::LoadIntent(native(&intent_path)));
    let mut intent = loaded
        .manufacturing
        .loaded
        .as_ref()
        .unwrap()
        .intent
        .as_ref()
        .clone();
    intent.recipe.seam_policy = SeamPolicy::Distributed;
    let intent = Arc::new(intent);
    let h = s.hello.as_ref().unwrap();
    let serial = host.snapshot().serial;
    host.manufacturing(ManufacturingCommand::SetIntent {
        layout_id: layout_id(&host),
        session_id: h.session_id.clone(),
        base_revision: s.project.as_ref().unwrap().revision,
        intent: intent.clone(),
    })
    .unwrap();
    let s = idle_after(&host, serial);
    assert_eq!(
        s.manufacturing.intent.as_deref(),
        Some(intent.as_ref()),
        "{:?}",
        s.problems
    );
    let d = s.display.as_ref().unwrap();
    let o = &d.occurrences[0];
    let scene = s.scene.as_ref().unwrap();
    let serial = s.serial;
    host.geometry(GeometryCommand::PrepareBedPose {
        session_id: h.session_id.clone(),
        base_revision: scene.revision,
        project_revision: s.project.as_ref().unwrap().revision,
        occurrence_id: o.occurrence_id,
        expected_definition_id: o.definition_id.clone(),
        expected_pose: o.pose,
        envelope_mm: intent.printer.build_envelope,
        center_xy: true,
        contact_tolerance_mm: spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM,
    })
    .unwrap();
    let s = idle_after(&host, serial);
    let p = s.bed_proposal.as_ref().expect("native placement proposal");
    let serial = s.serial;
    host.geometry(GeometryCommand::SetInstancePose {
        session_id: p.session_id.clone(),
        base_revision: p.scene_revision,
        project_revision: p.project_revision,
        occurrence_id: p.occurrence_id,
        pose: p.pose,
    })
    .unwrap();
    let s = idle_after(&host, serial);
    let captured = s.project.as_ref().unwrap().revision;
    let sparse = ProjectCommand::EditProcess {
        session_id: h.session_id.clone(),
        base_revision: captured,
        edit: ProcessEdit::SetSparseFraction {
            scope: SurfaceScope::Occurrence {
                occurrence_id: o.occurrence_id,
                expected_definition_id: o.definition_id.clone(),
            },
            fraction: 0.3,
        },
    };
    let serial = s.serial;
    host.project(sparse.clone()).unwrap();
    let s = idle_after(&host, serial);
    assert!(
        s.project.as_ref().unwrap().revision > captured,
        "{:?}",
        s.problems
    );
    let revision = s.project.as_ref().unwrap().revision;
    // Reusing the same stale edit cannot overwrite the accepted process mutation.
    let serial = s.serial;
    host.project(sparse).unwrap();
    let s = idle_after(&host, serial);
    assert_eq!(s.project.as_ref().unwrap().revision, revision);
    assert!(!s.problems.is_empty());
    let serial = s.serial;
    host.manufacturing(ManufacturingCommand::Preflight {
        layout_id: layout_id(&host),
        session_id: h.session_id.clone(),
        base_revision: revision,
    })
    .unwrap();
    let s = idle_after(&host, serial);
    assert_eq!(
        s.manufacturing
            .preflight
            .as_ref()
            .expect("preflight")
            .disposition,
        PreflightDisposition::Admitted,
        "{:?}",
        s.manufacturing.preflight
    );
    let serial = s.serial;
    host.manufacturing(ManufacturingCommand::Compile {
        layout_id: layout_id(&host),
        session_id: h.session_id.clone(),
        base_revision: revision,
    })
    .unwrap();
    let s = idle_after(&host, serial);
    assert!(s.manufacturing.artifact.is_some(), "{:?}", s.problems);
    fn inspect(host: &Host) -> Snapshot {
        let s = host.snapshot();
        let serial = s.serial;
        host.manufacturing(ManufacturingCommand::Inspect {
            layout_id: layout_id(host),
            session_id: s.hello.as_ref().unwrap().session_id.clone(),
            base_revision: s.project.as_ref().unwrap().revision,
            output: VerificationOutput::ReportAndMotion,
        })
        .unwrap();
        wait(host, |s| {
            s.serial > serial
                && !s.busy
                && !s.manufacturing.loading
                && s.manufacturing.review.is_some()
        })
    }
    let s = inspect(&host);
    let r = s.manufacturing.review.as_ref().unwrap();
    assert!(r.capture.report.verified);
    assert!(r.motion.is_some());
    // Exercise the production controls against a real checked job. Widget focus
    // must be read before input_mut, or this draw deadlocks the native UI.
    let ctx = egui::Context::default();
    let mut browser = crate::layers::Browser::default();
    let mut selection = crate::replay::View::default();
    let mut draw = |events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(100.0, 500.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                browser.rail(ui, &mut selection, r);
            },
        );
        output.textures_delta.clear();
    };
    draw(vec![]);
    draw(vec![
        egui::Event::PointerMoved(egui::pos2(50.0, 200.0)),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, -1.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    assert_eq!(selection.layer, Some(0));
    assert_eq!(
        selection.layer_end,
        Some((r.bundle.plan.layers.len() - 2) as u16)
    );
    let replay = r.display.as_ref().unwrap();
    assert!(!replay.batches.is_empty());
    assert_eq!(
        r.bundle.provenance.intent.recipe.seam_policy,
        SeamPolicy::Distributed
    );
    assert!(!replay.seams.is_empty());
    let mut selected_seam = None;
    let mut seam_view = crate::replay::View {
        seams: true,
        ..Default::default()
    };
    ctx.run_ui(egui::RawInput::default(), |ui| {
        crate::ui::seam_review(
            ui,
            &replay.seams,
            &mut selected_seam,
            &mut seam_view,
            crate::replay::seams::SectionClip::default(),
            replay.origin,
            &r.bundle.provenance.intent.recipe.seam_policy,
        );
    })
    .drop_without_applying_deltas();
    assert_eq!(selected_seam, Some(replay.seams[0].first_event));
    assert_eq!(
        host.snapshot().project.as_ref().unwrap().revision,
        s.project.as_ref().unwrap().revision
    );
    assert!(
        replay
            .batches
            .iter()
            .filter(|batch| batch.seam)
            .all(|batch| batch.points
                && batch
                    .context_ranges(seam_view)
                    .iter()
                    .all(std::ops::Range::is_empty))
    );
    assert!(
        replay
            .batches
            .iter()
            .filter(|b| b.kind == 11 || b.kind == 12)
            .all(|b| b.points)
    );
    assert!(replay.batches.iter().all(|b| b.kind != 14));
    let destination = root.path().join("output");
    let s = send(&host, Action::Export(native(&destination)));
    assert!(s.manufacturing.last_export.is_some(), "{:?}", s.problems);
    let program = std::fs::read(destination.join("program.gcode")).unwrap();
    send(&host, Action::Export(native(&destination)));
    assert_eq!(
        std::fs::read(destination.join("program.gcode")).unwrap(),
        program
    );
    let serial = host.snapshot().serial;
    host.command(Command::Undo).unwrap();
    let s = idle_after(&host, serial);
    assert!(s.manufacturing.capture.is_none());
    let serial = s.serial;
    host.command(Command::Redo).unwrap();
    idle_after(&host, serial);
    let project = root.path().join("saved");
    let s = send(
        &host,
        Action::Save {
            target: Some(native(&project)),
        },
    );
    assert!(!s.project.as_ref().unwrap().dirty, "{:?}", s.problems);
    let project_id = s.project.as_ref().unwrap().project_id.clone();
    send(&host, Action::Close);
    drop(host);
    std::fs::remove_file(source).unwrap();
    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("reopened-operations"),
        },
        || {},
    )
    .unwrap();
    wait(&host, |s| s.phase == Phase::Running);
    let s = send(
        &host,
        Action::Open {
            path: native(&project),
            read_only: true,
            recover_previous: false,
        },
    );
    assert_eq!(s.project.as_ref().unwrap().project_id, project_id);
    assert!(s.project.as_ref().unwrap().read_only);
    let s = inspect(&host);
    assert!(
        s.manufacturing
            .review
            .as_ref()
            .unwrap()
            .capture
            .report
            .verified
    );
    let s = send(
        &host,
        Action::Export(native(&root.path().join("reopened-output"))),
    );
    assert!(s.manufacturing.last_export.is_some(), "{:?}", s.problems);
    assert_eq!(
        std::fs::read(root.path().join("reopened-output/program.gcode")).unwrap(),
        program
    );
    assert_eq!(send(&host, Action::Close).phase, Phase::Closed);
}

fn project_edit(
    host: &Host,
    build: impl FnOnce(spiling_contracts::geometry::SessionId, ProjectRevision) -> ProjectCommand,
) -> Snapshot {
    let s = host.snapshot();
    let serial = s.serial;
    host.project(build(
        s.hello.as_ref().unwrap().session_id.clone(),
        s.project.as_ref().unwrap().revision,
    ))
    .unwrap();
    idle_after(host, serial)
}
fn manufacture_control(
    host: &Host,
    build: impl FnOnce(spiling_contracts::geometry::SessionId, ProjectRevision) -> ManufacturingCommand,
) -> Snapshot {
    let s = host.snapshot();
    let serial = s.serial;
    host.manufacturing(build(
        s.hello.as_ref().unwrap().session_id.clone(),
        s.project.as_ref().unwrap().revision,
    ))
    .unwrap();
    idle_after(host, serial)
}

fn inspect_current_program(host: &Host) -> Snapshot {
    manufacture_control(host, |session_id, base_revision| {
        ManufacturingCommand::Inspect {
            layout_id: layout_id(host),
            session_id,
            base_revision,
            output: spiling_contracts::manufacturing::VerificationOutput::ReportAndMotion,
        }
    });
    wait(host, |snapshot| {
        !snapshot.manufacturing.loading && snapshot.manufacturing.review.is_some()
    })
}

fn evaluate_alternative_program(
    host: &Host,
    intent: Arc<spiling_contracts::manufacturing::ManufacturingIntent>,
    context: spiling_contracts::geometry::SourceHash,
) -> Snapshot {
    let revision = host.snapshot().project.as_ref().unwrap().revision;
    let submitted = manufacture_control(host, |session_id, base_revision| {
        ManufacturingCommand::EvaluateAlternative {
            layout_id: layout_id(host),
            session_id,
            base_revision,
            expected_context_hash: context,
            intent: intent.clone(),
        }
    });
    assert!(
        submitted
            .manufacturing
            .alternative
            .as_ref()
            .is_some_and(|evaluation| {
                evaluation.captured_revision == revision && evaluation.intent == intent
            }),
        "Alternative evaluation failed: {:?}",
        submitted.problems
    );
    let loaded = wait(host, |snapshot| {
        !snapshot.busy && !snapshot.manufacturing.alternative_loading
    });
    assert!(
        loaded
            .manufacturing
            .alternative_review
            .as_ref()
            .is_some_and(|review| review.capture.observed_revision == revision),
        "Alternative retrieval failed: {:?}",
        loaded.problems
    );
    loaded
}

#[test]
#[ignore = "actual supervised alternative workflow; requires SPILING_TEST_ENGINE"]
fn manufacturing_alternative_keep_inspect_reopen_evaluate_use_and_current_only_export() {
    use spiling_contracts::{geometry::SourceHash, manufacturing::*, project::ManufacturingOption};
    let root = tempfile::tempdir().unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("operations"),
        },
        || {},
    )
    .unwrap();
    let empty = wait(&host, |snapshot| snapshot.phase == Phase::Running);
    let serial = empty.serial;
    host.command(Command::PinAlternativeReference { nonce: 0 })
        .unwrap();
    let missing = idle_after(&host, serial);
    assert!(missing.manufacturing.reference_review.is_none());
    assert_eq!(missing.project, empty.project);
    assert!(!missing.problems.is_empty());
    let source = root.path().join("housing.step");
    std::fs::copy(
        repo.join("fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"),
        &source,
    )
    .unwrap();
    send(&host, Action::Import(native(&source)));
    let original = Arc::new(
        decode_intent(
            &std::fs::read(repo.join("fixtures/manufacturing/complete-sparse.intent.json"))
                .unwrap(),
        )
        .unwrap(),
    );
    manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::SetIntent {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            intent: original.clone(),
        }
    });
    place_on_bed(&host, &original);
    manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::Preflight {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
        }
    });
    manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::Compile {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
        }
    });
    let baseline = inspect_current_program(&host);
    let reference = baseline.manufacturing.review.as_ref().unwrap().clone();
    let serial = baseline.serial;
    host.command(Command::PinAlternativeReference {
        nonce: reference.capture.nonce,
    })
    .unwrap();
    idle_after(&host, serial);
    let file = root.path().join("Alternatives — 保存.spil");
    let baseline = send(
        &host,
        Action::Save {
            target: Some(native(&file)),
        },
    );
    assert!(!baseline.project.as_ref().unwrap().dirty);
    let context = baseline.manufacturing.context_hash.clone().unwrap();
    let mut candidate = (*original).clone();
    candidate.recipe.seam_policy = SeamPolicy::Distributed;
    let candidate = Arc::new(candidate);
    let evaluated = evaluate_alternative_program(&host, candidate.clone(), context.clone());
    assert_eq!(evaluated.project, baseline.project);
    assert_eq!(
        evaluated.manufacturing.intent,
        baseline.manufacturing.intent
    );
    assert_eq!(
        evaluated.manufacturing.artifact,
        baseline.manufacturing.artifact
    );
    assert!(Arc::ptr_eq(
        evaluated.manufacturing.review.as_ref().unwrap(),
        &reference
    ));
    let evaluation = evaluated
        .manufacturing
        .alternative
        .as_ref()
        .unwrap()
        .clone();
    assert!(evaluation.report.verified);
    assert!(evaluation.changed_inputs_complete);
    assert!(
        evaluation
            .changed_inputs
            .iter()
            .any(|change| change.path.contains("seam_policy"))
    );
    let candidate_review = evaluated
        .manufacturing
        .alternative_review
        .as_ref()
        .unwrap()
        .clone();
    assert!(!candidate_review.display.as_ref().unwrap().seams.is_empty());
    assert_ne!(reference.bundle.program, candidate_review.bundle.program);
    let exported = root.path().join("Current-only");
    let snapshot = send(&host, Action::Export(native(&exported)));
    assert_eq!(
        std::fs::read_to_string(exported.join("program.gcode")).unwrap(),
        reference.bundle.program
    );
    assert_eq!(snapshot.project, baseline.project);
    let option = ManufacturingOption::new(
        "Distributed starts — 試行".into(),
        context.clone(),
        candidate.clone(),
    )
    .unwrap();
    let kept = project_edit(&host, |session_id, base_revision| {
        ProjectCommand::RetainManufacturingOption {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            option: option.clone(),
        }
    });
    assert!(kept.project.as_ref().unwrap().dirty);
    assert_eq!(kept.manufacturing.artifact, baseline.manufacturing.artifact);
    assert_eq!(kept.manufacturing.intent, baseline.manufacturing.intent);
    assert_eq!(kept.manufacturing.context_hash, Some(context.clone()));
    assert!(kept.manufacturing.review.is_some());
    assert!(Arc::ptr_eq(
        kept.manufacturing.reference_review.as_ref().unwrap(),
        &reference
    ));
    assert_eq!(
        kept.manufacturing
            .alternative
            .as_ref()
            .unwrap()
            .captured_revision,
        evaluation.captured_revision
    );
    assert_ne!(
        kept.project.as_ref().unwrap().revision,
        evaluation.captured_revision
    );
    let inspected = project_edit(&host, |session_id, base_revision| {
        ProjectCommand::GetManufacturingOption {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            id: option.id.clone(),
        }
    });
    assert_eq!(inspected.project, kept.project);
    assert_eq!(
        inspected
            .manufacturing
            .inspected_option
            .as_ref()
            .unwrap()
            .option,
        option
    );
    let reevaluated = evaluate_alternative_program(&host, candidate.clone(), context.clone());
    assert_eq!(
        reevaluated
            .manufacturing
            .alternative
            .as_ref()
            .unwrap()
            .captured_revision,
        kept.project.as_ref().unwrap().revision
    );
    let candidate_review = reevaluated
        .manufacturing
        .alternative_review
        .as_ref()
        .unwrap()
        .clone();
    let applied = project_edit(&host, |session_id, base_revision| {
        ProjectCommand::ApplyManufacturingOption {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            id: option.id.clone(),
        }
    });
    assert_eq!(applied.manufacturing.intent, Some(candidate.clone()));
    let receipt = applied
        .manufacturing
        .applied_settings
        .as_ref()
        .expect("confirmed Use receipt");
    assert_eq!(
        receipt.base_revision,
        reevaluated.project.as_ref().unwrap().revision
    );
    assert_eq!(receipt.revision, applied.project.as_ref().unwrap().revision);
    assert_eq!(receipt.intent, candidate);
    let serial = applied.serial;
    host.command(Command::Undo).unwrap();
    let undone = idle_after(&host, serial);
    assert_eq!(undone.manufacturing.intent, Some(original.clone()));
    let serial = undone.serial;
    host.command(Command::Redo).unwrap();
    let redone = idle_after(&host, serial);
    assert_eq!(redone.manufacturing.intent, Some(candidate.clone()));
    let serial = redone.serial;
    host.command(Command::Undo).unwrap();
    let restored = idle_after(&host, serial);
    assert_eq!(restored.manufacturing.intent, Some(original.clone()));
    assert_eq!(
        restored.project.as_ref().unwrap().manufacturing_options,
        vec![option.info()]
    );
    let stale = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::EvaluateAlternative {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            expected_context_hash: SourceHash::from_bytes(b"wrong context"),
            intent: candidate.clone(),
        }
    });
    assert_eq!(stale.project, restored.project);
    assert!(Arc::ptr_eq(
        stale.manufacturing.alternative_review.as_ref().unwrap(),
        &candidate_review
    ));
    assert_eq!(stale.manufacturing.intent, baseline.manufacturing.intent);
    send(&host, Action::Save { target: None });
    assert_eq!(send(&host, Action::Close).phase, Phase::Closed);
    drop(host);
    std::fs::remove_file(&source).unwrap();

    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("read-only-operations"),
        },
        || {},
    )
    .unwrap();
    wait(&host, |snapshot| snapshot.phase == Phase::Running);
    let reopened = send(
        &host,
        Action::Open {
            path: native(&file),
            read_only: true,
            recover_previous: false,
        },
    );
    assert!(reopened.project.as_ref().unwrap().read_only);
    assert!(reopened.manufacturing.alternative.is_none());
    assert!(reopened.manufacturing.review.is_none());
    assert_eq!(
        reopened.project.as_ref().unwrap().manufacturing_options,
        vec![option.info()]
    );
    let inspected = project_edit(&host, |session_id, base_revision| {
        ProjectCommand::GetManufacturingOption {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            id: option.id.clone(),
        }
    });
    assert_eq!(inspected.project, reopened.project);
    let fresh =
        evaluate_alternative_program(&host, option.intent.clone(), option.context_hash.clone());
    assert_eq!(fresh.project, reopened.project);
    for command in [
        ProjectCommand::ApplyManufacturingOption {
            layout_id: fresh.project.as_ref().unwrap().layout_id.clone(),
            session_id: fresh.hello.as_ref().unwrap().session_id.clone(),
            base_revision: fresh.project.as_ref().unwrap().revision,
            id: option.id.clone(),
        },
        ProjectCommand::RemoveManufacturingOption {
            layout_id: fresh.project.as_ref().unwrap().layout_id.clone(),
            session_id: fresh.hello.as_ref().unwrap().session_id.clone(),
            base_revision: fresh.project.as_ref().unwrap().revision,
            id: option.id.clone(),
        },
        ProjectCommand::RetainManufacturingOption {
            layout_id: fresh.project.as_ref().unwrap().layout_id.clone(),
            session_id: fresh.hello.as_ref().unwrap().session_id.clone(),
            base_revision: fresh.project.as_ref().unwrap().revision,
            option: option.clone(),
        },
    ] {
        let serial = host.snapshot().serial;
        host.project(command).unwrap();
        let refused = idle_after(&host, serial);
        assert_eq!(refused.project, reopened.project);
        assert_eq!(refused.manufacturing.intent, Some(original.clone()));
    }
    assert_eq!(send(&host, Action::Close).phase, Phase::Closed);
    drop(host);

    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("use-operations"),
        },
        || {},
    )
    .unwrap();
    wait(&host, |snapshot| snapshot.phase == Phase::Running);
    send(
        &host,
        Action::Open {
            path: native(&file),
            read_only: false,
            recover_previous: false,
        },
    );
    let current = inspect_current_program(&host);
    let reference = current.manufacturing.review.as_ref().unwrap().clone();
    let serial = current.serial;
    host.command(Command::PinAlternativeReference {
        nonce: reference.capture.nonce,
    })
    .unwrap();
    idle_after(&host, serial);
    evaluate_alternative_program(&host, option.intent.clone(), option.context_hash.clone());
    let applied = project_edit(&host, |session_id, base_revision| {
        ProjectCommand::ApplyManufacturingOption {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            id: option.id.clone(),
        }
    });
    assert_eq!(applied.manufacturing.intent, Some(candidate.clone()));
    assert!(applied.manufacturing.capture.is_none());
    assert!(applied.manufacturing.artifact.is_none());
    assert!(Arc::ptr_eq(
        applied.manufacturing.reference_review.as_ref().unwrap(),
        &reference
    ));
    assert!(applied.project.as_ref().unwrap().dirty);
    manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::Preflight {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
        }
    });
    manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::Compile {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
        }
    });
    let current = inspect_current_program(&host);
    assert_eq!(
        current
            .manufacturing
            .review
            .as_ref()
            .unwrap()
            .bundle
            .provenance
            .intent,
        *candidate
    );
    assert!(
        current.manufacturing.display_bytes()
            >= reference.display.as_ref().unwrap().owned_bytes
                + current
                    .manufacturing
                    .review
                    .as_ref()
                    .unwrap()
                    .display
                    .as_ref()
                    .unwrap()
                    .owned_bytes
                + current
                    .manufacturing
                    .alternative_review
                    .as_ref()
                    .unwrap()
                    .display
                    .as_ref()
                    .unwrap()
                    .owned_bytes
    );
    let output = root.path().join("Used-Current");
    send(&host, Action::Export(native(&output)));
    assert_ne!(
        std::fs::read_to_string(output.join("program.gcode")).unwrap(),
        reference.bundle.program
    );
    let removed = project_edit(&host, |session_id, base_revision| {
        ProjectCommand::RemoveManufacturingOption {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            id: option.id.clone(),
        }
    });
    assert!(
        removed
            .project
            .as_ref()
            .unwrap()
            .manufacturing_options
            .is_empty()
    );
    assert_eq!(removed.manufacturing.intent, Some(candidate));
    assert_eq!(
        removed.manufacturing.artifact,
        current.manufacturing.artifact
    );
    let serial = removed.serial;
    host.command(Command::Undo).unwrap();
    let undone = idle_after(&host, serial);
    assert_eq!(
        undone.project.as_ref().unwrap().manufacturing_options,
        vec![option.info()]
    );
    let serial = undone.serial;
    host.command(Command::DiscardAlternative).unwrap();
    let discarded = idle_after(&host, serial);
    assert!(discarded.manufacturing.reference_review.is_none());
    assert!(discarded.manufacturing.alternative.is_none());
    assert_eq!(discarded.project, undone.project);
    let closed = send(&host, Action::Close);
    if let Some(confirmation) = closed.confirmation {
        host.confirm(confirmation.id, true).unwrap();
        wait(&host, |snapshot| snapshot.phase == Phase::Closed);
    }
}
fn place_on_bed(
    host: &Host,
    intent: &spiling_contracts::manufacturing::ManufacturingIntent,
) -> Snapshot {
    let s = wait(host, |s| {
        s.display
            .as_ref()
            .is_some_and(|d| d.same_revision(s.scene.as_ref().unwrap()))
    });
    let o = &s.display.as_ref().unwrap().occurrences[0];
    let scene = s.scene.as_ref().unwrap();
    let serial = s.serial;
    host.geometry(GeometryCommand::PrepareBedPose {
        session_id: scene.session_id.clone(),
        base_revision: scene.revision,
        project_revision: s.project.as_ref().unwrap().revision,
        occurrence_id: o.occurrence_id,
        expected_definition_id: o.definition_id.clone(),
        expected_pose: o.pose,
        envelope_mm: intent.printer.build_envelope,
        center_xy: true,
        contact_tolerance_mm: spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM,
    })
    .unwrap();
    let s = idle_after(host, serial);
    let p = s.bed_proposal.as_ref().expect("native bed proposal");
    let serial = s.serial;
    host.geometry(GeometryCommand::SetInstancePose {
        session_id: p.session_id.clone(),
        base_revision: p.scene_revision,
        project_revision: p.project_revision,
        occurrence_id: p.occurrence_id,
        pose: p.pose,
    })
    .unwrap();
    idle_after(host, serial)
}
#[test]
#[ignore = "actual supervised engine; requires SPILING_TEST_ENGINE"]
fn finish_layers_canonical_image_preview_and_recovery() {
    use spiling_contracts::{geometry::AnalyticSurfaceDescription, manufacturing::*, surface::*};
    let root = tempfile::tempdir().unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("ops"),
        },
        || {},
    )
    .unwrap();
    wait(&host, |s| s.phase == Phase::Running);
    let source = root.path().join("box-mm.step");
    std::fs::copy(repo.join("fixtures/geometry/box-mm.step"), &source).unwrap();
    send(&host, Action::Import(native(&source)));
    let s = wait(&host, |s| {
        s.display
            .as_ref()
            .is_some_and(|d| d.same_revision(s.scene.as_ref().unwrap()))
    });
    let scene = s.display.as_ref().unwrap();
    let o = &scene.occurrences[0];
    let definition = scene.definition(&o.definition_id).unwrap();
    let mut selected = None;
    for ordinal in 0..definition.faces.len() {
        let reference = scene.reference(o, ordinal).unwrap();
        let serial = host.snapshot().serial;
        host.geometry(GeometryCommand::InspectFace {
            reference: reference.clone(),
        })
        .unwrap();
        let s = idle_after(&host, serial);
        if matches!(s.inspection.as_ref().unwrap().face.geometry.analytic,Some(AnalyticSurfaceDescription::Plane{normal,..}) if normal[2].abs()<1e-12)
        {
            selected = Some(reference);
            break;
        }
    }
    let mut reference = selected.expect("vertical native plane");
    let intent = Arc::new(
        decode_intent(
            &std::fs::read(repo.join("fixtures/manufacturing/solid-fill.intent.json")).unwrap(),
        )
        .unwrap(),
    );
    manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::SetIntent {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            intent: intent.clone(),
        }
    });
    let placed = place_on_bed(&host, &intent);
    reference.scene_revision = placed.scene.as_ref().unwrap().revision;
    let s = project_edit(&host, |session_id, base_revision| {
        ProjectCommand::AddSurfaceFinish {
            session_id,
            base_revision,
            face_ref: reference.clone(),
            scope: SurfaceScopeKind::Definition,
            finish: SurfaceFinish::VerticalRibs {
                depth_mm: 0.1,
                pitch_mm: 2.0,
                edge_margin_mm: 1.0,
                phase_mm: 0.0,
            },
        }
    });
    assert_eq!(
        s.project.as_ref().unwrap().surface_overlays.layers.len(),
        1,
        "{:?}",
        s.problems
    );
    let preflight = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::Preflight {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
        }
    });
    assert_eq!(
        preflight
            .manufacturing
            .preflight
            .as_ref()
            .unwrap()
            .disposition,
        PreflightDisposition::Admitted,
        "{:?}",
        preflight.manufacturing.preflight
    );
    let s = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::Compile {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
        }
    });
    assert!(s.manufacturing.artifact.is_some(), "{:?}", s.problems);
    manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::Inspect {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            output: VerificationOutput::ReportAndMotion,
        }
    });
    let s = wait(&host, |s| s.manufacturing.review.is_some());
    assert!(
        s.manufacturing
            .review
            .as_ref()
            .unwrap()
            .capture
            .report
            .verified
    );
    let image = root.path().join("height.pgm");
    std::fs::write(&image, b"P5\n2 2\n255\n\x00\xff\xff\x00").unwrap();
    let s = project_edit(&host, |session_id, base_revision| {
        ProjectCommand::ImportSurfaceImageFinish {
            session_id,
            base_revision,
            reference: reference.clone(),
            scope: SurfaceScopeKind::Occurrence,
            source: native(&image),
            parameters: ImageReliefParameters {
                depth_mm: 0.1,
                u_repeats: 1,
                u_phase_fraction: 0.0,
                z_period_mm: 10.0,
                z_phase_mm: 0.0,
                edge_margin_mm: 1.0,
                vertical_error_mm: 0.05,
            },
        }
    });
    let p = s.project.as_ref().unwrap();
    assert_eq!(p.surface_overlays.layers.len(), 2, "{:?}", s.problems);
    assert!(s.manufacturing.capture.is_none());
    let record = p.surface_overlays.images[0].clone();
    let id = p.surface_overlays.layers[1].layer_id;
    project_edit(&host, |session_id, base_revision| {
        ProjectCommand::SetSurfaceFinishEnabled {
            session_id,
            base_revision,
            layer_id: id,
            enabled: false,
        }
    });
    let serial = host.snapshot().serial;
    host.command(Command::Undo).unwrap();
    let s = idle_after(&host, serial);
    assert!(s.project.as_ref().unwrap().surface_overlays.layers[1].enabled);
    project_edit(&host, |session_id, base_revision| {
        ProjectCommand::GetSurfaceImage {
            session_id,
            base_revision,
            image_hash: record.image_hash.clone(),
        }
    });
    let s = wait(&host, |s| s.image_preview.is_some());
    assert_eq!(s.image_preview.as_ref().unwrap().pixels.size, [2, 2]);
    let saved = root.path().join("project.spil");
    send(
        &host,
        Action::Save {
            target: Some(native(&saved)),
        },
    );
    send(&host, Action::Close);
    drop(host);
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(image).unwrap();
    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("ops2"),
        },
        || {},
    )
    .unwrap();
    wait(&host, |s| s.phase == Phase::Running);
    send(
        &host,
        Action::Open {
            path: native(&saved),
            read_only: true,
            recover_previous: false,
        },
    );
    project_edit(&host, |session_id, base_revision| {
        ProjectCommand::GetSurfaceImage {
            session_id,
            base_revision,
            image_hash: record.image_hash.clone(),
        }
    });
    let s = wait(&host, |s| s.image_preview.is_some());
    assert_eq!(s.image_preview.as_ref().unwrap().record, record);
    assert_eq!(s.project.as_ref().unwrap().surface_overlays.layers.len(), 2);
    assert_eq!(send(&host, Action::Close).phase, Phase::Closed);
}

fn checked_review(host: &Host) -> Snapshot {
    use spiling_contracts::manufacturing::*;
    let preflight = manufacture_control(host, |session_id, base_revision| {
        ManufacturingCommand::Preflight {
            layout_id: layout_id(host),
            session_id,
            base_revision,
        }
    });
    assert_eq!(
        preflight
            .manufacturing
            .preflight
            .as_ref()
            .unwrap()
            .disposition,
        PreflightDisposition::Admitted,
        "{:?}",
        preflight.manufacturing.preflight
    );
    let s = manufacture_control(host, |session_id, base_revision| {
        ManufacturingCommand::Compile {
            layout_id: layout_id(host),
            session_id,
            base_revision,
        }
    });
    assert!(s.manufacturing.artifact.is_some(), "{:?}", s.problems);
    manufacture_control(host, |session_id, base_revision| {
        ManufacturingCommand::Inspect {
            layout_id: layout_id(host),
            session_id,
            base_revision,
            output: VerificationOutput::ReportAndMotion,
        }
    });
    wait(host, |s| s.manufacturing.review.is_some())
}
fn retain_project(host: &Host, temporary_root: &std::path::Path, name: &str) {
    let root = std::env::var_os("SPILING_NATIVE_EVIDENCE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| temporary_root.join("saved-projects"));
    std::fs::create_dir_all(&root).unwrap();
    let s = send(
        host,
        Action::Save {
            target: Some(native(&root.join(format!("{name}.spil")))),
        },
    );
    assert!(!s.project.as_ref().unwrap().dirty, "{:?}", s.problems);
}
#[test]
#[ignore = "actual supervised engine; requires SPILING_TEST_ENGINE"]
fn native_h1_housing_and_physical_plate_parity() {
    use spiling_contracts::{manufacturing::*, process::*, surface::SurfaceScope};
    let root = tempfile::tempdir().unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (name, profile, plate) in [
        ("h1-housing", "housing-h1-combined", false),
        ("zoned-plate", "housing-ordinary", true),
    ] {
        let host = Host::new(
            Config {
                engine: engine(),
                store: root.path().join(name),
            },
            || {},
        )
        .unwrap();
        wait(&host, |s| s.phase == Phase::Running);
        send(
            &host,
            Action::Import(native(&repo.join(
                "fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step",
            ))),
        );
        let intent = Arc::new(
            decode_intent(
                &std::fs::read(repo.join(format!("fixtures/manufacturing/{profile}.intent.json")))
                    .unwrap(),
            )
            .unwrap(),
        );
        manufacture_control(&host, |session_id, base_revision| {
            ManufacturingCommand::SetIntent {
                layout_id: layout_id(&host),
                session_id,
                base_revision,
                intent: intent.clone(),
            }
        });
        let s = place_on_bed(&host, &intent);
        let scope = SurfaceScope::Definition {
            definition_id: s.display.as_ref().unwrap().occurrences[0]
                .definition_id
                .clone(),
        };
        project_edit(&host, |session_id, base_revision| {
            ProjectCommand::EditProcess {
                session_id,
                base_revision,
                edit: ProcessEdit::SetSparseFraction {
                    scope: scope.clone(),
                    fraction: if plate { 0.6 } else { 0.2 },
                },
            }
        });
        project_edit(&host, |session_id, base_revision| {
            ProjectCommand::EditProcess {
                session_id,
                base_revision,
                edit: ProcessEdit::SetBoxSparseFraction {
                    scope: scope.clone(),
                    min: [-1.0, -1.0, -1.0],
                    max: if plate {
                        [25.0, 19.0, 5.03]
                    } else {
                        [8.0, 19.0, 11.0]
                    },
                    fraction: if plate { 0.2 } else { 0.6 },
                    interface: Some(if plate {
                        ProcessInterfacePermission::HorizontalDiaphragm { thickness_mm: 0.23 }
                    } else {
                        ProcessInterfacePermission::SeparatorWall
                    }),
                },
            }
        });
        let s = checked_review(&host);
        let review = s.manufacturing.review.as_ref().unwrap();
        assert!(review.capture.report.verified);
        let source = displayed(&host).display.unwrap();
        let bounds = source.summary.bounds_mm.unwrap();
        let origin =
            glam::DVec3::from_array(bounds.min) * 0.5 + glam::DVec3::from_array(bounds.max) * 0.5;
        let mut inspected_interfaces = 0;
        for layer in &review.bundle.plan.layers {
            for separator in &layer.region_coverage.process_interfaces {
                let area = crate::inspection::Area::separator(
                    separator,
                    [layer.lower_z_mm, layer.z_mm],
                    origin,
                )
                .unwrap();
                assert!(area.target.valid());
                assert_eq!(area.target.bounds_mm.min[2], layer.lower_z_mm);
                assert_eq!(area.target.bounds_mm.max[2], layer.z_mm);
                assert!(area.vertices.len() <= crate::inspection::MAX_INSPECTION_VERTICES);
                inspected_interfaces += 1;
            }
            for plate in &layer.region_coverage.process_diaphragms {
                let area = crate::inspection::Area::plate(plate, origin).unwrap();
                assert!(area.target.valid());
                assert_eq!(area.target.bounds_mm.min[2], plate.physical_z_mm[0]);
                assert_eq!(area.target.bounds_mm.max[2], plate.physical_z_mm[1]);
                inspected_interfaces += 1;
            }
        }
        assert!(
            inspected_interfaces > 0,
            "fixture must exercise an actual construction interface"
        );
        let bead_display = review
            .display
            .as_ref()
            .unwrap()
            .beads
            .as_ref()
            .expect("real H1/plate nominal-bead admission");
        assert!(bead_display.instance_count > 0);
        if !plate {
            assert!(
                bead_display
                    .batches
                    .iter()
                    .flat_map(|batch| &batch.instances)
                    .any(|bead| f64::from(bead.height) > intent.recipe.layer_height_mm + 0.01),
                "H1 bead display must retain genuinely thick physical intervals"
            );
        }
        if plate {
            assert!(
                review
                    .bundle
                    .plan
                    .layers
                    .iter()
                    .any(|l| !l.region_coverage.process_diaphragms.is_empty())
            );
        } else {
            assert!(
                review
                    .bundle
                    .plan
                    .sparse_bands
                    .iter()
                    .any(|b| b.disposition == SparseBandDisposition::Combined),
                "Combine selection alone is not genuine H1"
            );
        }
        retain_project(&host, root.path(), name);
        assert_eq!(send(&host, Action::Close).phase, Phase::Closed);
    }
}
#[test]
#[ignore = "actual supervised engine; requires SPILING_TEST_ENGINE"]
fn native_cylindrical_ribs_and_fixed_dual_replay_parity() {
    use spiling_contracts::{manufacturing::*, surface::*};
    let root = tempfile::tempdir().unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (name, cylinder) in [
        ("cylinder-ribs", true),
        ("fixed-dual", false),
        ("fixed-dual-shared-material", false),
        ("fixed-dual-profiles", false),
        ("mixed-width-interior", false),
        ("mixed-width-combined", false),
        ("fixed-dual-cycle", false),
    ] {
        let host = Host::new(
            Config {
                engine: engine(),
                store: root.path().join(name),
            },
            || {},
        )
        .unwrap();
        wait(&host, |s| s.phase == Phase::Running);
        send(
            &host,
            Action::Import(native(&repo.join(if cylinder {
                "fixtures/geometry/independent-exporters/occt-cylinder-mm.step"
            } else if name.starts_with("mixed-width-") {
                "fixtures/geometry/through-hole.step"
            } else {
                "fixtures/geometry/box-mm.step"
            }))),
        );
        let intent = Arc::new(
            decode_intent(
                &std::fs::read(repo.join(if cylinder {
                    "fixtures/manufacturing/solid-fill.intent.json"
                } else if name == "mixed-width-combined" {
                    "fixtures/manufacturing/mixed-width-combined.intent.json"
                } else if name == "mixed-width-interior" {
                    "fixtures/manufacturing/mixed-width-interior.intent.json"
                } else if name == "fixed-dual-profiles" {
                    "fixtures/manufacturing/fixed-dual-profiles.intent.json"
                } else if name == "fixed-dual-shared-material" {
                    "fixtures/manufacturing/fixed-dual-shared-material.intent.json"
                } else if name == "fixed-dual-cycle" {
                    "fixtures/manufacturing/fixed-dual-cycle.intent.json"
                } else {
                    "fixtures/manufacturing/fixed-dual.intent.json"
                }))
                .unwrap(),
            )
            .unwrap(),
        );
        manufacture_control(&host, |session_id, base_revision| {
            ManufacturingCommand::SetIntent {
                layout_id: layout_id(&host),
                session_id,
                base_revision,
                intent: intent.clone(),
            }
        });
        if cylinder {
            place_on_bed(&host, &intent);
            let s = wait(&host, |s| {
                s.display
                    .as_ref()
                    .is_some_and(|d| d.same_revision(s.scene.as_ref().unwrap()))
            });
            let scene = s.display.as_ref().unwrap();
            let o = &scene.occurrences[0];
            let d = scene.definition(&o.definition_id).unwrap();
            let mut reference = None;
            for ordinal in 0..d.faces.len() {
                let r = scene.reference(o, ordinal).unwrap();
                let serial = host.snapshot().serial;
                host.geometry(GeometryCommand::InspectFace {
                    reference: r.clone(),
                })
                .unwrap();
                let s = idle_after(&host, serial);
                if matches!(
                    s.inspection.as_ref().unwrap().face.geometry.analytic,
                    Some(spiling_contracts::geometry::AnalyticSurfaceDescription::Cylinder { .. })
                ) {
                    reference = Some(r);
                    break;
                }
            }
            project_edit(&host, |session_id, base_revision| {
                ProjectCommand::AddSurfaceFinish {
                    session_id,
                    base_revision,
                    face_ref: reference.unwrap(),
                    scope: SurfaceScopeKind::Definition,
                    finish: SurfaceFinish::CylindricalRibs {
                        depth_mm: 0.1,
                        rib_count: 8,
                        phase_fraction: 0.0,
                    },
                }
            });
        } else {
            let s = wait(&host, |s| {
                s.display.as_ref().is_some_and(|d| {
                    d.occurrences.len() == 1
                        && s.scene.as_ref().is_some_and(|scene| d.same_revision(scene))
                })
            });
            let scene = s.scene.as_ref().unwrap();
            let o = &s.display.as_ref().unwrap().occurrences[0];
            let serial = s.serial;
            host.geometry(GeometryCommand::SetInstancePose {
                session_id: scene.session_id.clone(),
                base_revision: scene.revision,
                project_revision: s.project.as_ref().unwrap().revision,
                occurrence_id: o.occurrence_id,
                pose: RigidPoseMm {
                    translation_mm: [20.0, 20.0, 0.0],
                    ..RigidPoseMm::IDENTITY
                },
            })
            .unwrap();
            let s = idle_after(&host, serial);
            if !name.starts_with("mixed-width-") {
                let scene = s.scene.as_ref().unwrap();
                let serial = s.serial;
                host.geometry(GeometryCommand::AddInstance {
                    session_id: scene.session_id.clone(),
                    base_revision: scene.revision,
                    definition_id: o.definition_id.clone(),
                    pose: RigidPoseMm {
                        translation_mm: [120.0, 20.0, 0.0],
                        ..RigidPoseMm::IDENTITY
                    },
                })
                .unwrap();
                idle_after(&host, serial);
            }
        }
        if name == "mixed-width-combined" {
            let s = host.snapshot();
            let occurrence = &s.display.as_ref().unwrap().occurrences[0];
            let pin = InteriorToolOverride {
                enabled: true,
                value: InteriorToolPolicy::PrimaryOnly,
            };
            let edit = InteriorToolPolicyEdit {
                occurrence_id: occurrence.occurrence_id,
                expected_definition_id: occurrence.definition_id.clone(),
                expected: None,
                value: Some(pin),
            };
            let applied = manufacture_control(&host, |session_id, base_revision| {
                ManufacturingCommand::EditInteriorPolicy {
                    layout_id: layout_id(&host),
                    session_id,
                    base_revision,
                    edit: edit.clone(),
                }
            });
            assert_eq!(
                applied
                    .manufacturing
                    .applied_settings
                    .as_ref()
                    .unwrap()
                    .interior_edit
                    .as_ref(),
                Some(&edit)
            );
            let pinned = applied.manufacturing.intent.clone();
            let rejected = manufacture_control(&host, |session_id, base_revision| {
                ManufacturingCommand::EditInteriorPolicy {
                    layout_id: layout_id(&host),
                    session_id,
                    base_revision,
                    edit: edit.clone(),
                }
            });
            assert_eq!(rejected.manufacturing.intent, pinned);
            assert_eq!(
                rejected.project.as_ref().unwrap().revision,
                applied.project.as_ref().unwrap().revision
            );
            host.command(Command::DismissProblems).unwrap();
            idle_after(&host, rejected.serial);
            let undo = send(&host, Action::Undo);
            assert_eq!(undo.manufacturing.intent, Some(intent.clone()));
            assert_eq!(send(&host, Action::Redo).manufacturing.intent, pinned);
            let pinned_review = checked_review(&host);
            assert_eq!(
                pinned_review
                    .manufacturing
                    .review
                    .as_ref()
                    .unwrap()
                    .bundle
                    .plan
                    .interior_selection
                    .as_ref()
                    .unwrap()
                    .reason,
                InteriorSelectionReason::ExplicitPrimary
            );
            manufacture_control(&host, |session_id, base_revision| {
                ManufacturingCommand::EditInteriorPolicy {
                    layout_id: layout_id(&host),
                    session_id,
                    base_revision,
                    edit: InteriorToolPolicyEdit {
                        expected: Some(pin),
                        value: None,
                        ..edit.clone()
                    },
                }
            });
        }
        let s = checked_review(&host);
        let review = s.manufacturing.review.as_ref().unwrap();
        assert!(review.capture.report.verified);
        if !cylinder {
            let r = review.display.as_ref().unwrap();
            assert!(r.batches.iter().any(|b| b.tool == 1 && b.kind == 15));
            assert!(r.batches.iter().any(|b| b.kind == 11 && b.points));
            assert!(r.batches.iter().all(|b| b.kind != 14));
            assert!(review.capture.report.routing.is_some());
        }
        retain_project(&host, root.path(), name);
        assert_eq!(send(&host, Action::Close).phase, Phase::Closed);
    }
}
#[test]
#[ignore = "actual supervised engine; requires SPILING_TEST_ENGINE"]
fn diagnostic_operations_remain_separate_cancellable_and_checked() {
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("ops"),
        },
        || {},
    )
    .unwrap();
    let s = wait(&host, |s| s.phase == Phase::Running);
    let project = s.project.as_ref().unwrap().clone();
    let serial = s.serial;
    host.command(Command::Diagnostic).unwrap();
    wait(&host, |s| s.serial > serial && s.diagnostic.is_some());
    host.command(Command::Cancel).unwrap();
    wait(&host, |s| s.diagnostic.as_ref().is_some_and(|d| d.done));
    host.command(Command::Diagnostic).unwrap();
    let s = wait(&host, |s| s.diagnostic_triangle);
    assert_eq!(s.project.as_ref().unwrap().project_id, project.project_id);
    assert_eq!(s.project.as_ref().unwrap().revision, project.revision);
    assert_eq!(s.scene.as_ref().unwrap().occurrence_count, 0);
    assert_eq!(send(&host, Action::Close).phase, Phase::Closed);
}

#[test]
#[ignore = "actual supervised material-profile engine; requires SPILING_TEST_ENGINE"]
fn captured_material_profiles_preserve_full_intent_and_recover() {
    use spiling_contracts::manufacturing::*;
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("ops"),
        },
        || {},
    )
    .unwrap();
    wait(&host, |s| s.phase == Phase::Running);
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/manufacturing");
    let original =
        decode_intent(&std::fs::read(fixtures.join("complete-sparse.intent.json")).unwrap())
            .unwrap();
    manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::SetIntent {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            intent: Arc::new(original),
        }
    });
    let profile_path = root.path().join("Matériau Ω.json");
    std::fs::copy(fixtures.join("material-profile-a.json"), &profile_path).unwrap();
    let before_load = host.snapshot();
    let loaded = send(&host, Action::LoadMaterial(native(&profile_path)));
    assert_eq!(
        loaded.project.as_ref().unwrap().revision,
        before_load.project.as_ref().unwrap().revision
    );
    let selected = loaded.manufacturing.loaded_material.as_ref().unwrap();
    assert_eq!(selected.revision, loaded.project.as_ref().unwrap().revision);
    assert_eq!(selected.label, "Matériau Ω.json");
    let mut selection = selected.selection.clone();
    selection.overrides.nozzle_temperature_c = Some(210.0);
    selection.overrides.print_speed_mm_s = Some(28.0);
    // Invalid input must retain the prior selected file/draft and committed intent.
    let invalid = root.path().join("invalid.json");
    std::fs::write(&invalid, b"{unexpected}").unwrap();
    let failed = send(&host, Action::LoadMaterial(native(&invalid)));
    assert_eq!(
        failed
            .manufacturing
            .loaded_material
            .as_ref()
            .unwrap()
            .selection,
        selected.selection
    );
    assert_eq!(failed.project.as_ref().unwrap().revision, selected.revision);
    host.command(Command::DismissProblems).unwrap();
    idle_after(&host, failed.serial);
    std::fs::remove_file(&profile_path).unwrap();
    let before = host.snapshot();
    let mut oversized = selection.clone();
    oversized.profile.material.product = "oversized".repeat(8192);
    assert!(
        host.manufacturing(ManufacturingCommand::ApplyMaterialProfile {
            tool_id: None,
            layout_id: layout_id(&host),
            session_id: before.hello.as_ref().unwrap().session_id.clone(),
            base_revision: before.project.as_ref().unwrap().revision,
            selection: oversized,
        })
        .is_err()
    );
    assert_eq!(host.snapshot().serial, before.serial);
    let s = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::ApplyMaterialProfile {
            tool_id: None,
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            selection: selection.clone(),
        }
    });
    assert!(s.problems.is_empty(), "{:?}", s.problems);
    assert!(s.manufacturing.loaded_material.is_none());
    let resolved = s.manufacturing.intent.as_ref().unwrap();
    assert_eq!(resolved.recipe.print_speed_mm_s, 28.0);
    let accepted_revision = s.project.as_ref().unwrap().revision;
    let reviewed = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::ReviewMaterials {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            request: MaterialReviewRequest {
                catalog: None,
                query: String::new(),
                offset: 0,
            },
        }
    });
    let report = &reviewed
        .manufacturing
        .material_review
        .as_ref()
        .unwrap()
        .report;
    assert_eq!(report.input_revision, accepted_revision);
    assert_eq!(
        reviewed.project.as_ref().unwrap().revision,
        accepted_revision
    );
    assert_eq!(report.candidates[0].input_changed, Some(false));
    let temperature = report.candidates[0]
        .settings
        .iter()
        .find(|row| row.field == MaterialSettingField::NozzleTemperatureC)
        .unwrap();
    assert_eq!(temperature.proposed, Some(210.0));
    assert_eq!(
        temperature.proposed_origin,
        MaterialValueOrigin::ExplicitOverride
    );
    let undo = send(&host, Action::Undo);
    assert!(undo.manufacturing.material_review.is_none());
    assert!(
        undo.manufacturing
            .intent
            .as_ref()
            .unwrap()
            .material_profile
            .is_none()
    );
    let redo = send(&host, Action::Redo);
    assert_eq!(redo.manufacturing.intent, s.manufacturing.intent);
    let mut incompatible = selection.clone();
    incompatible.profile.printer_revision = "different-printer".into();
    let rejected = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::ApplyMaterialProfile {
            tool_id: None,
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            selection: incompatible,
        }
    });
    assert_eq!(
        rejected.project.as_ref().unwrap().revision,
        redo.project.as_ref().unwrap().revision
    );
    assert_eq!(rejected.manufacturing.intent, redo.manufacturing.intent);
    assert!(!rejected.problems.is_empty());
    let stale_serial = rejected.serial;
    host.manufacturing(ManufacturingCommand::ApplyMaterialProfile {
        tool_id: None,
        layout_id: layout_id(&host),
        session_id: rejected.hello.as_ref().unwrap().session_id.clone(),
        base_revision: accepted_revision,
        selection: selection.clone(),
    })
    .unwrap();
    let stale = idle_after(&host, stale_serial);
    assert_eq!(
        stale.project.as_ref().unwrap().revision,
        rejected.project.as_ref().unwrap().revision
    );
    assert_eq!(stale.manufacturing.intent, rejected.manufacturing.intent);
    host.command(Command::DismissProblems).unwrap();
    idle_after(&host, stale.serial);
    assert_eq!(
        resolved.material_profile.as_ref().unwrap().selection,
        selection
    );
    let mut draft = (**resolved).clone();
    draft.recipe.infill_fraction = 0.2;
    let s = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::SetIntent {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            intent: Arc::new(draft.clone()),
        }
    });
    assert_eq!(s.manufacturing.intent.as_deref(), Some(&draft));
    let mut inconsistent = draft.clone();
    inconsistent.recipe.print_speed_mm_s += 1.0;
    assert!(
        host.manufacturing(ManufacturingCommand::SetIntent {
            layout_id: layout_id(&host),
            session_id: s.hello.as_ref().unwrap().session_id.clone(),
            base_revision: s.project.as_ref().unwrap().revision,
            intent: Arc::new(inconsistent)
        })
        .is_err()
    );
    assert_eq!(
        host.snapshot().project.as_ref().unwrap().revision,
        s.project.as_ref().unwrap().revision
    );
    retain_project(&host, root.path(), "material-profile");
    let saved = send(
        &host,
        Action::Save {
            target: Some(native(&root.path().join("captured"))),
        },
    );
    assert!(
        !saved.project.as_ref().unwrap().dirty,
        "{:?}",
        saved.problems
    );
    let old_session = saved.hello.as_ref().unwrap().session_id.clone();
    let reopened = send(&host, Action::Restart);
    assert_ne!(reopened.hello.as_ref().unwrap().session_id, old_session);
    assert_eq!(reopened.manufacturing.intent.as_deref(), Some(&draft));
    send(&host, Action::Close);
    wait(&host, |s| s.phase == Phase::Closed);
}

#[test]
#[ignore = "actual supervised material catalog engine; requires SPILING_TEST_ENGINE"]
fn catalog_review_is_nonmutating_bound_and_available_read_only() {
    use spiling_contracts::manufacturing::*;
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("ops"),
        },
        || {},
    )
    .unwrap();
    wait(&host, |s| s.phase == Phase::Running);
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/manufacturing");
    let intent =
        decode_intent(&std::fs::read(fixtures.join("complete-sparse.intent.json")).unwrap())
            .unwrap();
    let before = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::SetIntent {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            intent: Arc::new(intent),
        }
    });
    let selection: MaterialProfileSelection =
        serde_json::from_slice(&std::fs::read(fixtures.join("material-profile-a.json")).unwrap())
            .unwrap();
    let catalog = MaterialCatalog {
        setup: selection.setup.clone(),
        profiles: (0..6)
            .map(|i| {
                let mut profile = selection.profile.clone();
                profile.id = format!("catalog-{i}");
                if i == 5 {
                    profile.printer_revision = "incompatible-printer".into();
                }
                profile
            })
            .collect(),
    };
    let path = root.path().join("Matériaux Ω.json");
    std::fs::write(&path, serde_json::to_vec(&catalog).unwrap()).unwrap();
    let loaded = send(&host, Action::LoadMaterialCatalog(native(&path)));
    assert_eq!(
        loaded.project.as_ref().unwrap().revision,
        before.project.as_ref().unwrap().revision
    );
    assert_eq!(loaded.manufacturing.intent, before.manufacturing.intent);
    assert_eq!(
        loaded.manufacturing.loaded_catalog.as_ref().unwrap().label,
        "Matériaux Ω.json"
    );
    let invalid = root.path().join("invalid-catalog.json");
    std::fs::write(&invalid, b"{unexpected}").unwrap();
    let failed = send(&host, Action::LoadMaterialCatalog(native(&invalid)));
    assert_eq!(
        failed
            .manufacturing
            .loaded_catalog
            .as_ref()
            .unwrap()
            .catalog
            .as_ref(),
        &catalog
    );
    assert_eq!(
        failed.project.as_ref().unwrap().revision,
        loaded.project.as_ref().unwrap().revision
    );
    assert!(!failed.problems.is_empty());
    host.command(Command::DismissProblems).unwrap();
    idle_after(&host, failed.serial);
    let reviewed = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::ReviewMaterials {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            request: MaterialReviewRequest {
                catalog: Some(catalog.clone()),
                query: "catalog".into(),
                offset: 0,
            },
        }
    });
    let review = reviewed.manufacturing.material_review.as_ref().unwrap();
    assert!(review.current(
        reviewed.project.as_ref().unwrap(),
        &reviewed.hello.as_ref().unwrap().session_id
    ));
    assert_eq!(review.request.catalog.as_ref(), Some(&catalog));
    assert_eq!(review.report.matching_profiles, 6);
    assert_eq!(review.report.candidates.len(), MATERIAL_REVIEW_PAGE_SIZE);
    assert_eq!(
        reviewed.project.as_ref().unwrap().revision,
        before.project.as_ref().unwrap().revision
    );
    assert_eq!(reviewed.manufacturing.intent, before.manufacturing.intent);
    let candidate = review.report.candidates[0].clone();
    assert!(candidate.issues.is_empty());
    assert_eq!(candidate.settings.len(), MaterialSettingField::ALL.len());
    let last = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::ReviewMaterials {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            request: MaterialReviewRequest {
                catalog: Some(catalog.clone()),
                query: "catalog".into(),
                offset: MATERIAL_REVIEW_PAGE_SIZE as u32,
            },
        }
    });
    let report = &last.manufacturing.material_review.as_ref().unwrap().report;
    assert_eq!(report.candidates.len(), 2);
    assert!(!report.candidates[1].issues.is_empty());
    assert_eq!(report.candidates[1].input_changed, None);
    assert!(report.candidates[1].settings.is_empty());
    let applied = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::ApplyMaterialProfile {
            tool_id: None,
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            selection: candidate.selection,
        }
    });
    assert!(applied.problems.is_empty(), "{:?}", applied.problems);
    assert_eq!(
        applied.project.as_ref().unwrap().revision.0,
        before.project.as_ref().unwrap().revision.0 + 1
    );
    assert!(applied.manufacturing.material_review.is_none());
    assert!(!review.current(
        applied.project.as_ref().unwrap(),
        &applied.hello.as_ref().unwrap().session_id
    ));
    assert!(applied.manufacturing.loaded_catalog.is_some());
    host.manufacturing(ManufacturingCommand::ReviewMaterials {
        layout_id: layout_id(&host),
        session_id: applied.hello.as_ref().unwrap().session_id.clone(),
        base_revision: review.report.input_revision,
        request: (*review.request).clone(),
    })
    .unwrap();
    let stale = idle_after(&host, applied.serial);
    assert!(!stale.problems.is_empty());
    assert!(stale.manufacturing.material_review.is_none());
    assert_eq!(
        stale.project.as_ref().unwrap().revision,
        applied.project.as_ref().unwrap().revision
    );
    host.command(Command::DismissProblems).unwrap();
    idle_after(&host, stale.serial);
    let saved = send(
        &host,
        Action::Save {
            target: Some(native(&root.path().join("captured"))),
        },
    );
    assert!(!saved.project.as_ref().unwrap().dirty);
    let old_session = saved.hello.as_ref().unwrap().session_id.clone();
    let restarted = send(&host, Action::Restart);
    assert_ne!(restarted.hello.as_ref().unwrap().session_id, old_session);
    assert!(restarted.manufacturing.loaded_catalog.is_none());
    assert!(restarted.manufacturing.material_review.is_none());
    assert_eq!(restarted.manufacturing.intent, applied.manufacturing.intent);
    let opened = send(
        &host,
        Action::Open {
            path: native(&root.path().join("captured")),
            read_only: true,
            recover_previous: false,
        },
    );
    assert!(opened.project.as_ref().unwrap().read_only);
    assert!(
        crate::commands::Action::LoadMaterialCatalog
            .disabled(&opened)
            .is_none()
    );
    std::fs::remove_file(&path).unwrap();
    let readonly = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::ReviewMaterials {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            request: MaterialReviewRequest {
                catalog: None,
                query: String::new(),
                offset: 0,
            },
        }
    });
    let current = readonly.manufacturing.material_review.as_ref().unwrap();
    assert!(current.current(
        readonly.project.as_ref().unwrap(),
        &readonly.hello.as_ref().unwrap().session_id
    ));
    assert_eq!(current.report.candidates[0].input_changed, Some(false));
    assert_eq!(readonly.manufacturing.intent, applied.manufacturing.intent);
    assert!(!readonly.project.as_ref().unwrap().dirty);
    assert!(readonly.problems.is_empty(), "{:?}", readonly.problems);
    assert_eq!(send(&host, Action::Close).phase, Phase::Closed);
}

#[test]
#[ignore = "actual library engine services; requires SPILING_TEST_ENGINE"]
fn library_inventory_and_project_copy_use_native_host_services() {
    use spiling_contracts::library::*;
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
    assert_eq!(initial.phase, Phase::Running);
    let serial = initial.serial;
    host.library(LibraryAction::SetLocation(native(
        &root.path().join("My library.spil"),
    )))
    .unwrap();
    wait(&host, |s| {
        s.serial > serial && !s.busy && s.library.status.is_some()
    });
    let selection: spiling_contracts::manufacturing::MaterialProfileSelection =
        serde_json::from_slice(include_bytes!(
            "../../../../fixtures/manufacturing/material-profile-a.json"
        ))
        .unwrap();
    let definition = FilamentDefinition {
        declaration: Default::default(),
        catalog_origin: None,
        name: "Native stock".into(),
        material: Some(selection.profile.material.clone()),
        color_name: None,
        appearance: None,
        profiles: vec![selection.profile.clone()],
        source: DefinitionSource::Operator,
    };
    let id = FilamentId("b6a8aaac-0506-4c0b-89f4-6dbec7d3fd67".into());
    let request = LibraryWrite {
        library_id: None,
        base_revision: 0,
        request_id: "b6a8aaac-0506-4c0b-89f4-6dbec7d3fd68".into(),
        mutation: LibraryMutation::PutFilament {
            id: id.clone(),
            expected: None,
            definition,
        },
    };
    host.library(LibraryAction::Write(request.clone())).unwrap();
    let saved = wait(&host, |s| {
        !s.busy
            && s.library
                .receipt
                .as_ref()
                .is_some_and(|r| r.request_id == request.request_id)
    });
    assert_eq!(
        saved.library.receipt.as_ref().unwrap().publication,
        LibraryPublication::Confirmed
    );
    host.library(LibraryAction::Refresh(LibraryFilamentQuery::default()))
        .unwrap();
    let listed = wait(&host, |s| {
        !s.busy
            && s.library
                .personal
                .as_ref()
                .is_some_and(|p| p.entries.len() == 1)
    });
    let reference = listed.library.personal.as_ref().unwrap().entries[0]
        .entry
        .current
        .clone();
    host.library(LibraryAction::Inspect {
        personal: true,
        reference: reference.clone(),
    })
    .unwrap();
    let inspected = wait(&host, |s| !s.busy && s.library.selected.is_some());
    let revision = inspected.library.selected.as_ref().unwrap().1.clone();
    host.library(LibraryAction::PutProjectFilament {
        project_id: inspected.project.as_ref().unwrap().project_id.clone(),
        revision,
        origin: Some(LibraryOrigin {
            library_id: saved.library.receipt.as_ref().unwrap().identity.id.clone(),
            filament: reference.clone(),
        }),
        expected: None,
        base_revision: inspected.project.unwrap().revision,
    })
    .unwrap();
    let copied = wait(&host, |s| {
        !s.busy && s.project.as_ref().is_some_and(|p| p.materials.is_some())
    });
    assert!(copied.project.unwrap().dirty);
    host.library(LibraryAction::Refresh(LibraryFilamentQuery::default()))
        .unwrap();
    let project = wait(&host, |s| !s.busy && s.library.project.len() == 1);
    assert_eq!(project.library.project[0].entry.current, reference);
    let serial = host.snapshot().serial;
    host.library(LibraryAction::Inspect {
        personal: false,
        reference: reference.clone(),
    })
    .unwrap();
    let inspected = idle_after(&host, serial);
    assert_eq!(
        inspected.library.selected_project,
        inspected
            .project
            .as_ref()
            .map(|p| (p.project_id.clone(), p.revision))
    );
    let serial = inspected.serial;
    host.library(LibraryAction::ReviewProfile(selection.profile.clone()))
        .unwrap();
    let reviewed = idle_after(&host, serial);
    assert_eq!(
        reviewed
            .manufacturing
            .loaded_material
            .as_ref()
            .unwrap()
            .revision,
        inspected.project.as_ref().unwrap().revision
    );
    let serial = reviewed.serial;
    host.command(Command::Undo).unwrap();
    let undone = idle_after(&host, serial);
    let serial = undone.serial;
    host.command(Command::Redo).unwrap();
    let redone = idle_after(&host, serial);
    assert_ne!(
        redone.project.as_ref().unwrap().revision,
        inspected.project.as_ref().unwrap().revision
    );
    let serial = redone.serial;
    host.library(LibraryAction::ReviewProfile(selection.profile))
        .unwrap();
    let stale = idle_after(&host, serial);
    assert!(
        stale
            .problems
            .back()
            .unwrap()
            .contains("Refresh and inspect")
    );
    assert!(
        stale
            .manufacturing
            .loaded_material
            .as_ref()
            .is_none_or(|m| { m.revision == inspected.project.as_ref().unwrap().revision })
    );
    std::fs::write(
        root.path().join("My library.spil"),
        b"damaged personal file",
    )
    .unwrap();
    let serial = host.snapshot().serial;
    host.library(LibraryAction::Refresh(LibraryFilamentQuery::default()))
        .unwrap();
    let independent = idle_after(&host, serial);
    assert_eq!(independent.phase, Phase::Running);
    assert_eq!(independent.library.project.len(), 1);
    assert!(independent.library.personal.is_none());
    host.command(Command::Close).unwrap();
    let closing = wait(&host, |s| s.confirmation.is_some());
    host.confirm(closing.confirmation.unwrap().id, true)
        .unwrap();
    wait(&host, |s| s.phase == Phase::Closed);
}

#[test]
#[ignore = "actual library engine services; requires SPILING_TEST_ENGINE"]
fn project_library_comparison_preserves_copies_and_rejects_stale_promotion() {
    use spiling_contracts::{geometry::SourceHash, library::*};
    fn act(host: &Host, action: LibraryAction) -> Snapshot {
        let serial = host.snapshot().serial;
        host.library(action).unwrap();
        idle_after(host, serial)
    }
    fn inspect(host: &Host, reference: &FilamentRef) -> Snapshot {
        act(
            host,
            LibraryAction::Refresh(LibraryFilamentQuery::default()),
        );
        act(
            host,
            LibraryAction::Inspect {
                personal: false,
                reference: reference.clone(),
            },
        )
    }
    fn compare(host: &Host, reference: &FilamentRef) -> library::DefinitionComparison {
        let s = inspect(host, reference);
        let p = s.project.unwrap();
        act(
            host,
            LibraryAction::CompareOrigin {
                project_id: p.project_id.clone(),
                base_revision: p.revision,
                reference: reference.clone(),
            },
        )
        .library
        .comparison
        .clone()
        .unwrap()
    }
    fn request(
        identity: Option<&LibraryIdentity>,
        id: &FilamentId,
        expected: Option<SourceHash>,
        definition: FilamentDefinition,
    ) -> LibraryWrite {
        LibraryWrite {
            library_id: identity.map(|i| i.id.clone()),
            base_revision: identity.map_or(0, |i| i.revision),
            request_id: uuid::Uuid::new_v4().to_string(),
            mutation: LibraryMutation::PutFilament {
                id: id.clone(),
                expected,
                definition,
            },
        }
    }
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("operations"),
        },
        || {},
    )
    .unwrap();
    assert_eq!(
        wait(&host, |s| s.phase != Phase::Starting).phase,
        Phase::Running
    );
    act(
        &host,
        LibraryAction::SetLocation(native(&root.path().join("My library.spil"))),
    );
    let original = FilamentDefinition {
        declaration: Default::default(),
        catalog_origin: None,
        name: "Original personal filament".into(),
        material: None,
        color_name: None,
        appearance: None,
        profiles: vec![],
        source: DefinitionSource::Operator,
    };
    let id = FilamentId(uuid::Uuid::new_v4().to_string());
    let reference = FilamentRef {
        id: id.clone(),
        revision: original.content_hash().unwrap(),
    };
    let saved = act(
        &host,
        LibraryAction::Write(request(None, &id, None, original.clone())),
    );
    let identity = saved.library.receipt.as_ref().unwrap().identity.clone();
    let origin = LibraryOrigin {
        library_id: identity.id.clone(),
        filament: reference.clone(),
    };
    let p = saved.project.unwrap();
    act(
        &host,
        LibraryAction::PutProjectFilament {
            project_id: p.project_id.clone(),
            base_revision: p.revision,
            revision: FilamentRevision {
                reference: reference.clone(),
                definition: original.clone(),
            },
            origin: Some(origin.clone()),
            expected: None,
        },
    );
    let same = compare(&host, &reference);
    assert_eq!(same.summary(), "Same content as My library");
    let mut edited = original.clone();
    edited.name = "Project-only revision".into();
    let edited_ref = FilamentRef {
        id: id.clone(),
        revision: edited.content_hash().unwrap(),
    };
    act(
        &host,
        LibraryAction::PutProjectFilament {
            project_id: same.project.0,
            base_revision: same.project.1,
            revision: FilamentRevision {
                reference: edited_ref.clone(),
                definition: edited.clone(),
            },
            origin: Some(origin.clone()),
            expected: Some(reference.revision.clone()),
        },
    );
    let project_only = compare(&host, &edited_ref);
    assert_eq!(project_only.personal.definition, original);
    assert_eq!(project_only.summary(), "Edited in Project");
    let mut personal = original.clone();
    personal.name = "Concurrent personal revision".into();
    let updated = act(
        &host,
        LibraryAction::Write(request(
            Some(&identity),
            &id,
            Some(reference.revision.clone()),
            personal.clone(),
        )),
    );
    let personal_identity = updated.library.receipt.as_ref().unwrap().identity.clone();
    assert_eq!(
        updated.project.as_ref().unwrap().revision,
        project_only.project.1
    );
    // A reviewed promotion cannot be stamped with the latest library generation.
    let stale = request(
        Some(&project_only.identity),
        &id,
        Some(project_only.personal.reference.revision.clone()),
        project_only.copy.definition,
    );
    let rejected = act(&host, LibraryAction::Write(stale.clone()));
    assert_eq!(
        rejected.library.rejected_request.as_deref(),
        Some(stale.request_id.as_str())
    );
    assert_eq!(rejected.phase, Phase::Running);
    let both = compare(&host, &edited_ref);
    assert_eq!(both.summary(), "Both copies changed since capture");
    assert_eq!(both.identity, personal_identity);
    assert_eq!(both.personal.definition, personal);
    assert_eq!(both.copy.definition, edited);
    let promoted = act(
        &host,
        LibraryAction::Write(request(
            Some(&both.identity),
            &id,
            Some(both.personal.reference.revision.clone()),
            both.copy.definition,
        )),
    );
    let promoted_identity = promoted.library.receipt.as_ref().unwrap().identity.clone();
    assert_eq!(promoted.project.as_ref().unwrap().revision, both.project.1);
    assert_eq!(
        compare(&host, &edited_ref).summary(),
        "Same content as My library"
    );
    personal.name = "Next reusable revision".into();
    let latest = act(
        &host,
        LibraryAction::Write(request(
            Some(&promoted_identity),
            &id,
            Some(edited_ref.revision.clone()),
            personal.clone(),
        )),
    );
    assert_eq!(latest.project.as_ref().unwrap().revision, both.project.1);
    let update = compare(&host, &edited_ref);
    let replacement = update.personal.clone();
    act(
        &host,
        LibraryAction::PutProjectFilament {
            project_id: update.project.0.clone(),
            base_revision: update.project.1,
            revision: replacement.clone(),
            origin: Some(LibraryOrigin {
                library_id: update.identity.id,
                filament: replacement.reference.clone(),
            }),
            expected: Some(edited_ref.revision.clone()),
        },
    );
    let refreshed = act(
        &host,
        LibraryAction::Refresh(LibraryFilamentQuery::default()),
    );
    assert_eq!(
        refreshed.library.project[0].entry.current,
        replacement.reference
    );
    let undone = send(&host, Action::Undo);
    assert!(undone.project.as_ref().unwrap().revision > update.project.1);
    let refreshed = act(
        &host,
        LibraryAction::Refresh(LibraryFilamentQuery::default()),
    );
    assert_eq!(refreshed.library.project[0].entry.current, edited_ref);
    assert_eq!(
        refreshed.library.project[0].entry.origin.as_ref(),
        Some(&origin)
    );
    send(&host, Action::Redo);
    let path = root.path().join("Portable.spil");
    let saved = send(
        &host,
        Action::Save {
            target: Some(native(&path)),
        },
    );
    assert!(!saved.project.as_ref().unwrap().dirty);
    send(&host, Action::New);
    let opened = send(
        &host,
        Action::Open {
            path: native(&path),
            read_only: true,
            recover_previous: false,
        },
    );
    assert!(opened.project.as_ref().unwrap().read_only);
    let readonly = compare(&host, &replacement.reference);
    assert_eq!(readonly.summary(), "Same content as My library");
    assert_eq!(readonly.copy.definition, personal);
    // A draft from the old project cannot target a new project with the same revision.
    send(&host, Action::New);
    let wrong_project = act(
        &host,
        LibraryAction::PutProjectFilament {
            project_id: readonly.project.0,
            base_revision: readonly.project.1,
            revision: replacement,
            origin: None,
            expected: None,
        },
    );
    assert!(
        wrong_project
            .problems
            .back()
            .unwrap()
            .contains("another project")
    );
    assert!(wrong_project.project.as_ref().unwrap().materials.is_none());
    assert_eq!(send(&host, Action::Close).phase, Phase::Closed);
}

#[test]
#[ignore = "actual recipe engine services; requires SPILING_TEST_ENGINE"]
fn recipe_library_copy_and_fresh_study_use_native_host_services() {
    use super::recipes::RecipeAction;
    use spiling_contracts::{appearance::AppearanceColor, library::*};
    fn act(host: &Host, action: LibraryAction) -> Snapshot {
        let serial = host.snapshot().serial;
        host.library(action).unwrap();
        idle_after(host, serial)
    }
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
    assert_eq!(initial.phase, Phase::Running);
    act(
        &host,
        LibraryAction::SetLocation(native(&root.path().join("My library.spil"))),
    );
    let filaments = (0..2)
        .map(|i| {
            let definition = FilamentDefinition {
                declaration: Default::default(),
                catalog_origin: None,
                name: format!("Host ingredient {i}"),
                material: None,
                color_name: None,
                appearance: Some(DeclaredAppearance {
                    color: AppearanceColor::Srgb {
                        channels: [0.2 + i as f64 * 0.4, 0.4, 0.8],
                    },
                    source: DefinitionSource::Operator,
                }),
                profiles: vec![],
                source: DefinitionSource::Operator,
            };
            FilamentRevision {
                reference: FilamentRef {
                    id: FilamentId(uuid::Uuid::new_v4().to_string()),
                    revision: definition.content_hash().unwrap(),
                },
                definition,
            }
        })
        .collect();
    let transfer = RecipeTransfer::from_declarations(
        RecipeId(uuid::Uuid::new_v4().to_string()),
        "Native reusable recipe".into(),
        filaments,
        AppearanceColor::Srgb { channels: [0.5; 3] },
        vec![0.2; 2],
    )
    .unwrap();
    let saved = act(
        &host,
        LibraryAction::Write(LibraryWrite {
            library_id: None,
            base_revision: 0,
            request_id: uuid::Uuid::new_v4().to_string(),
            mutation: LibraryMutation::PutRecipe {
                transfer: transfer.clone(),
                expected: None,
            },
        }),
    );
    assert_eq!(
        saved.library.receipt.as_ref().unwrap().publication,
        LibraryPublication::Confirmed
    );
    let refreshed = act(
        &host,
        LibraryAction::Recipe(RecipeAction::Refresh {
            query: LibraryRecipeQuery::default(),
        }),
    );
    assert_eq!(
        refreshed
            .library
            .personal_recipes
            .as_ref()
            .unwrap()
            .entries
            .len(),
        1
    );
    let inspected = act(
        &host,
        LibraryAction::Recipe(RecipeAction::Inspect {
            personal: true,
            reference: transfer.recipe.reference(),
        }),
    );
    assert_eq!(
        inspected
            .library
            .recipe_selection
            .as_ref()
            .unwrap()
            .transfer,
        transfer
    );
    let p = inspected.project.as_ref().unwrap();
    let capture = (p.project_id.clone(), p.revision);
    let copied = act(
        &host,
        LibraryAction::Recipe(RecipeAction::CopyToProject {
            project: capture.clone(),
            transfer: transfer.clone(),
        }),
    );
    assert_eq!(
        copied
            .project
            .as_ref()
            .unwrap()
            .materials
            .as_ref()
            .unwrap()
            .recipe_count,
        1
    );
    assert_eq!(
        copied
            .project
            .as_ref()
            .unwrap()
            .materials
            .as_ref()
            .unwrap()
            .filament_count,
        2
    );
    assert!(
        copied
            .project
            .as_ref()
            .unwrap()
            .appearance_study
            .optical
            .is_none()
    );
    let rejected = act(
        &host,
        LibraryAction::Recipe(RecipeAction::CopyToProject {
            project: capture,
            transfer: transfer.clone(),
        }),
    );
    assert!(
        rejected
            .problems
            .iter()
            .any(|p| p.contains("project changed"))
    );
    let refreshed = act(
        &host,
        LibraryAction::Recipe(RecipeAction::Refresh {
            query: LibraryRecipeQuery::default(),
        }),
    );
    assert_eq!(refreshed.library.project_recipes.len(), 1);
    let inspected = act(
        &host,
        LibraryAction::Recipe(RecipeAction::Inspect {
            personal: false,
            reference: transfer.recipe.reference(),
        }),
    );
    assert!(
        inspected
            .library
            .recipe_selection
            .as_ref()
            .unwrap()
            .project
            .is_some()
    );
    let serial = inspected.serial;
    host.project(ProjectCommand::LoadRecipeStudy {
        session_id: inspected.hello.as_ref().unwrap().session_id.clone(),
        base_revision: inspected.project.as_ref().unwrap().revision,
        reference: transfer.recipe.reference(),
        context: RecipeStudyContext {
            patch: transfer.recipe.definition.request.patch,
            cycle_origin_z_mm: 0.03,
        },
    })
    .unwrap();
    let loaded = idle_after(&host, serial);
    let study = loaded
        .project
        .as_ref()
        .unwrap()
        .appearance_study
        .optical
        .as_ref()
        .unwrap();
    assert!(study.report.is_none() && study.selection.is_none());
    let serial = loaded.serial;
    host.project(ProjectCommand::CompareOptical {
        session_id: loaded.hello.as_ref().unwrap().session_id.clone(),
        base_revision: loaded.project.as_ref().unwrap().revision,
    })
    .unwrap();
    let report = wait(&host, |s| {
        s.serial > serial
            && !s.busy
            && !s.active_operation
            && !s.optical_loading
            && s.optical_report.is_some()
    });
    let capture = report.optical_report.as_ref().unwrap();
    assert!(capture.report.is_complete());
    assert!(capture.is_current(report.project.as_ref().unwrap()));
    assert!(
        report
            .project
            .as_ref()
            .unwrap()
            .appearance_study
            .optical
            .as_ref()
            .unwrap()
            .report
            .is_none(),
        "fresh comparison must be available before acceptance"
    );
    let serial = report.serial;
    host.project(ProjectCommand::AcceptOpticalReport {
        session_id: report.hello.as_ref().unwrap().session_id.clone(),
        base_revision: report.project.as_ref().unwrap().revision,
        report_hash: capture.record.hash.clone(),
        selection: Some(spiling_contracts::optical::OpticalSelection::Requested),
    })
    .unwrap();
    let accepted = idle_after(&host, serial);
    assert!(
        accepted
            .optical_report
            .as_ref()
            .unwrap()
            .is_current(accepted.project.as_ref().unwrap())
    );
    assert!(
        accepted
            .project
            .as_ref()
            .unwrap()
            .appearance_study
            .optical
            .as_ref()
            .unwrap()
            .selection
            .is_some()
    );
    let closing = send(&host, Action::Close);
    host.confirm(closing.confirmation.unwrap().id, true)
        .unwrap();
    wait(&host, |s| s.phase == Phase::Closed);
}

#[test]
#[ignore = "actual hardware engine services; requires SPILING_TEST_ENGINE"]
fn hardware_selection_save_as_restart_and_missing_library_keep_portable_authority() {
    use super::hardware::HardwareAction;
    use spiling_contracts::{hardware::*, library::*};
    fn act(host: &Host, action: LibraryAction) -> Snapshot {
        let serial = host.snapshot().serial;
        host.library(action).unwrap();
        idle_after(host, serial)
    }
    let root = tempfile::tempdir().unwrap();
    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("operations"),
        },
        || {},
    )
    .unwrap();
    assert_eq!(
        wait(&host, |s| s.phase != Phase::Starting).phase,
        Phase::Running
    );
    act(
        &host,
        LibraryAction::SetLocation(native(&root.path().join("private-library.spil"))),
    );
    let s = act(&host, LibraryAction::Hardware(HardwareAction::Refresh));
    let catalog = s.library.hardware.catalog.as_ref().unwrap();
    let mut h = HardwareLibrary::default();
    for _ in 0..3 {
        h.machines.push(OwnedMachine {
            id: uuid::Uuid::new_v4().to_string(),
            nickname: "PRIVATE same-name workshop".into(),
            serial: Some("PRIVATE machine serial".into()),
            location: None,
            identity: PrinterIdentity::from_setup(&catalog.setups[0]),
            configuration: MachineConfiguration::Configured {
                setup: Box::new(catalog.setups[0].clone()),
            },
            generation: 1,
        });
    }
    for _ in 0..2 {
        h.plates.push(OwnedPlate {
            id: uuid::Uuid::new_v4().to_string(),
            nickname: "PRIVATE sheet".into(),
            definition: catalog.plates[0].clone(),
            installed_machine: None,
            notes: String::new(),
        });
    }
    let s = act(
        &host,
        LibraryAction::Write(LibraryWrite {
            library_id: None,
            base_revision: 0,
            request_id: uuid::Uuid::new_v4().to_string(),
            mutation: LibraryMutation::ReplaceHardware {
                hardware: h.clone(),
            },
        }),
    );
    assert!(s.problems.is_empty(), "{:?}", s.problems);
    let s = act(&host, LibraryAction::Hardware(HardwareAction::Refresh));
    let project = s.project.as_ref().unwrap();
    let s = act(
        &host,
        LibraryAction::Hardware(HardwareAction::Select {
            layout_id: project.layout_id.clone(),
            project: project.project_id.clone(),
            base: project.revision,
            expected: s
                .library
                .hardware
                .personal
                .as_ref()
                .unwrap()
                .identity
                .clone(),
            machine: h.machines[0].id.clone(),
            plate: h.plates[0].id.clone(),
            face: "textured".into(),
        }),
    );
    assert!(s.library.hardware.local_selection.as_ref().unwrap().matches);
    let project = s.project.as_ref().unwrap();
    let identity = s
        .library
        .hardware
        .personal
        .as_ref()
        .unwrap()
        .identity
        .as_ref()
        .unwrap();
    h.machines[0].generation = 2;
    act(
        &host,
        LibraryAction::Write(LibraryWrite {
            library_id: Some(identity.id.clone()),
            base_revision: identity.revision,
            request_id: uuid::Uuid::new_v4().to_string(),
            mutation: LibraryMutation::ReplaceHardware {
                hardware: h.clone(),
            },
        }),
    );
    let refreshed = act(&host, LibraryAction::Hardware(HardwareAction::Refresh));
    assert!(
        !refreshed
            .library
            .hardware
            .local_selection
            .as_ref()
            .unwrap()
            .matches,
        "an unsaved choice must observe a changed installation"
    );
    let s = act(
        &host,
        LibraryAction::Hardware(HardwareAction::Select {
            layout_id: project.layout_id.clone(),
            project: project.project_id.clone(),
            base: project.revision,
            expected: refreshed
                .library
                .hardware
                .personal
                .as_ref()
                .unwrap()
                .identity
                .clone(),
            machine: h.machines[0].id.clone(),
            plate: h.plates[0].id.clone(),
            face: "textured".into(),
        }),
    );
    assert!(s.library.hardware.local_selection.as_ref().unwrap().matches);
    let capture = s.project.as_ref().unwrap().hardware.clone().unwrap();
    let revision = s.project.as_ref().unwrap().revision;
    let first = root.path().join("first.spil");
    let second = root.path().join("second.spil");
    let s = send(
        &host,
        Action::Save {
            target: Some(native(&first)),
        },
    );
    assert!(!s.project.as_ref().unwrap().dirty, "{:?}", s.problems);
    let s = send(
        &host,
        Action::Save {
            target: Some(native(&second)),
        },
    );
    assert!(s.library.hardware.local_selection.as_ref().unwrap().matches);
    assert_eq!(
        s.library
            .hardware
            .personal
            .as_ref()
            .unwrap()
            .hardware
            .bindings
            .len(),
        2
    );
    let s = act(
        &host,
        LibraryAction::Hardware(HardwareAction::Select {
            layout_id: project.layout_id.clone(),
            project: s.project.as_ref().unwrap().project_id.clone(),
            base: revision,
            expected: s
                .library
                .hardware
                .personal
                .as_ref()
                .unwrap()
                .identity
                .clone(),
            machine: h.machines[1].id.clone(),
            plate: h.plates[0].id.clone(),
            face: "textured".into(),
        }),
    );
    assert_eq!(
        s.project.as_ref().unwrap().revision,
        revision,
        "identical physical replacement must preserve planning"
    );
    let s = send(&host, Action::Restart);
    let selection = s.library.hardware.local_selection.as_ref().unwrap();
    assert_eq!(selection.machine_id, h.machines[1].id);
    assert!(selection.matches);
    act(
        &host,
        LibraryAction::SetLocation(native(&root.path().join("absent.spil"))),
    );
    let s = act(&host, LibraryAction::Hardware(HardwareAction::Refresh));
    assert!(s.library.hardware.local_selection.is_none());
    assert_eq!(
        s.project.as_ref().unwrap().hardware.as_ref(),
        Some(&capture)
    );
    assert_eq!(s.project.as_ref().unwrap().revision, revision);
    let s = send(&host, Action::Restart);
    assert_eq!(
        s.library.status.as_ref().unwrap().location,
        native(&root.path().join("absent.spil"))
    );
    assert!(
        s.library
            .hardware
            .personal
            .as_ref()
            .unwrap()
            .identity
            .is_none()
    );
    act(
        &host,
        LibraryAction::SetLocation(native(&root.path().join("private-library.spil"))),
    );
    std::fs::write(
        root.path().join("private-library.spil"),
        b"corrupt test inventory",
    )
    .unwrap();
    act(
        &host,
        LibraryAction::Refresh(LibraryFilamentQuery::default()),
    );
    let s = send(&host, Action::Restart);
    assert!(
        s.library.hardware.personal.is_none(),
        "a corrupt selected store cannot fall back to another library"
    );
    assert!(s.library.hardware.error.is_some());
    assert_eq!(
        s.project.as_ref().unwrap().hardware.as_ref(),
        Some(&capture)
    );
    assert_eq!(send(&host, Action::Close).phase, Phase::Closed);
    for path in [first, second] {
        let bytes = std::fs::read(path).unwrap();
        for sentinel in [
            "PRIVATE same-name workshop",
            "PRIVATE machine serial",
            "PRIVATE sheet",
            h.machines[0].id.as_str(),
            h.machines[1].id.as_str(),
            h.plates[0].id.as_str(),
        ] {
            assert!(
                !bytes
                    .windows(sentinel.len())
                    .any(|w| w == sentinel.as_bytes()),
                "private inventory leaked"
            );
        }
    }
}

#[test]
#[ignore = "actual supervised engine; requires SPILING_TEST_ENGINE"]
fn named_plate_refresh_empty_scope_and_edit_receipts_are_authoritative() {
    use spiling_contracts::project::PrintLayoutEdit;
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
    assert_eq!(initial.phase, Phase::Running);
    imported(&host);
    let first = wait(&host, |s| {
        !s.display_loading
            && s.display
                .as_ref()
                .is_some_and(|d| s.scene.as_ref().is_some_and(|scene| d.same_revision(scene)))
    });
    let original = first.project.as_ref().unwrap().layout_id.clone();
    let edit = PrintLayoutEdit::Create {
        name: "Empty job".into(),
        copy_setup_from: None,
    };
    let created = project_edit(&host, |session_id, base_revision| {
        ProjectCommand::EditPrintLayout {
            session_id,
            base_revision,
            edit: edit.clone(),
        }
    });
    assert_eq!(created.plate_edit.as_ref().unwrap().edit, edit);
    assert_eq!(
        created.plate_edit.as_ref().unwrap().base_revision,
        first.project.as_ref().unwrap().revision
    );
    let empty = created.project.as_ref().unwrap().layout_id.clone();
    assert_ne!(empty, original);
    let displayed = wait(&host, |s| {
        s.display
            .as_ref()
            .is_some_and(|d| d.summary.layout_id == empty)
            && !s.display_loading
    });
    assert!(displayed.display.as_ref().unwrap().occurrences.is_empty());
    let context = displayed.display.as_ref().unwrap();
    assert_eq!(
        context.context_definitions.len(),
        1,
        "inactive source mesh must remain available to the overview"
    );
    assert!(context.context_error.is_none());
    assert!(
        Arc::ptr_eq(
            &context.context_definitions[0],
            &first.display.as_ref().unwrap().definitions[0]
        ),
        "focus must share existing immutable mesh bytes"
    );
    assert_eq!(displayed.scene.as_ref().unwrap().definition_count, 0);
    let overview = displayed.layout_overview.as_ref().unwrap();
    assert_eq!(overview.layouts.len(), 2);
    assert_eq!(
        overview
            .layouts
            .iter()
            .map(|l| l.parts.len())
            .sum::<usize>(),
        1
    );
    assert_eq!(
        overview.project_revision,
        displayed.project.as_ref().unwrap().revision
    );
    assert!(
        overview
            .layouts
            .iter()
            .find(|l| l.layout_id == empty)
            .unwrap()
            .parts
            .is_empty()
    );
    assert_eq!(displayed.manufacturing.layout_id.as_ref(), Some(&empty));
    assert!(displayed.project.as_ref().unwrap().dirty);
    let selected = project_edit(&host, |session_id, base_revision| {
        ProjectCommand::SelectPrintLayout {
            session_id,
            base_revision,
            layout_id: original.clone(),
        }
    });
    assert_eq!(
        selected.project.as_ref().unwrap().revision,
        displayed.project.as_ref().unwrap().revision
    );
    assert_eq!(selected.scene.as_ref().unwrap().occurrence_count, 1);
    assert!(selected.problems.is_empty(), "{:?}", selected.problems);
    let selected = wait(&host, |s| {
        !s.display_loading
            && s.display
                .as_ref()
                .is_some_and(|d| d.summary.layout_id == original)
    });
    let active = selected.display.as_ref().unwrap().clone();
    let part = active.occurrences[0].clone();
    let changed = project_edit(&host, |session_id, base_revision| {
        ProjectCommand::EditProcess {
            session_id,
            base_revision,
            edit: spiling_contracts::process::ProcessEdit::SetSparseFraction {
                scope: spiling_contracts::surface::SurfaceScope::Occurrence {
                    occurrence_id: part.occurrence_id,
                    expected_definition_id: part.definition_id,
                },
                fraction: 0.6,
            },
        }
    });
    assert!(
        Arc::ptr_eq(changed.display.as_ref().unwrap(), &active),
        "process edits must not reload source previews"
    );
    send(&host, Action::Undo);
    let before = host.snapshot().scene.as_ref().unwrap().revision;
    send(&host, Action::Undo); // Remove the inactive empty layout without changing the active geometry.
    let reduced = wait(&host, |s| {
        !s.display_loading
            && s.display
                .as_ref()
                .is_some_and(|d| d.context.layouts.len() == 1)
    });
    assert_eq!(reduced.scene.as_ref().unwrap().revision, before);
    assert!(Arc::ptr_eq(
        &reduced.display.as_ref().unwrap().context_definitions[0],
        &active.definitions[0]
    ));
    let pid = reduced.hello.as_ref().unwrap().pid;
    let closing = send(&host, Action::Close);
    host.confirm(closing.confirmation.unwrap().id, true)
        .unwrap();
    wait(&host, |s| s.phase == Phase::Closed);
    assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
}

#[test]
#[ignore = "actual supervised engine; requires SPILING_TEST_ENGINE"]
fn scoped_modifiers_preserve_neighbors_and_receipts_across_history_and_saved_return() {
    use spiling_contracts::{process::*, surface::SurfaceScope};
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
    let s = displayed(&host);
    let o = &s.display.as_ref().unwrap().occurrences[0];
    let scope = SurfaceScope::Occurrence {
        occurrence_id: o.occurrence_id,
        expected_definition_id: o.definition_id.clone(),
    };
    let apply = |edit: ProcessEdit| {
        project_edit(&host, |session_id, base_revision| {
            ProjectCommand::EditProcess {
                session_id,
                base_revision,
                edit,
            }
        })
    };
    let add = |selector, fraction| ProcessEdit::AddLayer {
        scope: scope.clone(),
        selector,
        action: ProcessAction::Replace {
            value: ProcessValue::SparseFraction { fraction },
        },
        interface: None,
    };
    let first = apply(add(ProcessSelector::WholeOwnedVolume, 0.25));
    let whole = first.project.as_ref().unwrap().process_document.layers[0].clone();
    let box_selector = ProcessSelector::SourceBoxMm {
        min: [1.0; 3],
        max: [2.0; 3],
    };
    let boxed = apply(add(box_selector, 0.6));
    let neighbor = boxed.project.as_ref().unwrap().process_document.layers[1].clone();
    let edit = ProcessEdit::UpdateOperation {
        operation_id: whole.operations[0].operation_id,
        selector: ProcessSelector::WholeOwnedVolume,
        action: ProcessAction::Replace {
            value: ProcessValue::SparseFraction { fraction: 0.4 },
        },
        interface: None,
    };
    let changed = apply(edit.clone());
    let receipt = changed.process_edit.as_ref().unwrap();
    assert_eq!(receipt.edit, edit);
    assert_eq!(receipt.revision, changed.project.as_ref().unwrap().revision);
    let doc = &changed.project.as_ref().unwrap().process_document;
    assert_eq!(
        doc.layers.len(),
        2,
        "change must not append another modifier"
    );
    assert_eq!(
        doc.layers[1], neighbor,
        "neighbor must retain exact content and identity"
    );
    let stale_revision = boxed.project.as_ref().unwrap().revision;
    let serial = changed.serial;
    host.project(ProjectCommand::EditProcess {
        session_id: changed.hello.as_ref().unwrap().session_id.clone(),
        base_revision: stale_revision,
        edit,
    })
    .unwrap();
    let failed = idle_after(&host, serial);
    assert_eq!(
        failed.process_edit.as_ref().unwrap().nonce,
        receipt.nonce,
        "failed mutation must not publish a new receipt"
    );
    let disabled = apply(ProcessEdit::SetLayerEnabled {
        layer_id: whole.layer_id,
        enabled: false,
    });
    assert!(!disabled.project.as_ref().unwrap().process_document.layers[0].enabled);
    let enabled = apply(ProcessEdit::SetLayerEnabled {
        layer_id: whole.layer_id,
        enabled: true,
    });
    assert!(enabled.project.as_ref().unwrap().process_document.layers[0].enabled);
    let removed = apply(ProcessEdit::RemoveLayer {
        layer_id: whole.layer_id,
    });
    assert_eq!(
        removed.project.as_ref().unwrap().process_document.layers,
        vec![neighbor.clone()]
    );
    let undone = send(&host, Action::Undo);
    assert_eq!(
        undone
            .project
            .as_ref()
            .unwrap()
            .process_document
            .layers
            .len(),
        2
    );
    let redone = send(&host, Action::Redo);
    assert_eq!(
        redone.project.as_ref().unwrap().process_document.layers,
        vec![neighbor.clone()]
    );
    let path = root.path().join("Modifiers.spil");
    send(
        &host,
        Action::Save {
            target: Some(native(&path)),
        },
    );
    let opened = send(
        &host,
        Action::Open {
            path: native(&path),
            read_only: false,
            recover_previous: false,
        },
    );
    assert_eq!(
        opened.project.as_ref().unwrap().process_document.layers,
        vec![neighbor]
    );
    let pid = opened.hello.as_ref().unwrap().pid;
    let closed = send(&host, Action::Close);
    assert_eq!(closed.phase, Phase::Closed);
    #[cfg(target_os = "linux")]
    assert!(
        !PathBuf::from(format!("/proc/{pid}")).exists(),
        "child must be reaped"
    );
}

#[test]
#[ignore = "actual detached policy comparison; requires SPILING_TEST_ENGINE"]
fn interior_policy_suggestion_is_detached_cancelable_and_exactly_adopted() {
    use spiling_contracts::manufacturing::*;
    let root = tempfile::tempdir().unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = root.path().join("source.step");
    std::fs::copy(repo.join("fixtures/geometry/through-hole.step"), &source).unwrap();
    let host = Host::new(
        Config {
            engine: engine(),
            store: root.path().join("ops"),
        },
        || {},
    )
    .unwrap();
    wait(&host, |s| s.phase == Phase::Running);
    send(&host, Action::Import(native(&source)));
    let s = wait(&host, |s| s.display.is_some() && !s.display_loading);
    let occurrence = s.display.as_ref().unwrap().occurrences[0].clone();
    let scene = s.scene.as_ref().unwrap();
    host.geometry(GeometryCommand::SetInstancePose {
        session_id: scene.session_id.clone(),
        base_revision: scene.revision,
        project_revision: s.project.as_ref().unwrap().revision,
        occurrence_id: occurrence.occurrence_id,
        pose: RigidPoseMm {
            translation_mm: [20.0, 20.0, 0.0],
            ..RigidPoseMm::IDENTITY
        },
    })
    .unwrap();
    idle_after(&host, s.serial);
    let intent = Arc::new(
        decode_intent(include_bytes!(
            "../../../../fixtures/manufacturing/mixed-width-combined.intent.json"
        ))
        .unwrap(),
    );
    manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::SetIntent {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            intent: intent.clone(),
        }
    });
    let pin = InteriorToolOverride {
        enabled: true,
        value: InteriorToolPolicy::PrimaryOnly,
    };
    let edit = InteriorToolPolicyEdit {
        occurrence_id: occurrence.occurrence_id,
        expected_definition_id: occurrence.definition_id,
        expected: None,
        value: Some(pin),
    };
    let pinned = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::EditInteriorPolicy {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            edit: edit.clone(),
        }
    });
    assert!(pinned.manufacturing.interior_opportunity.is_none());
    let mut changed = pinned
        .manufacturing
        .intent
        .as_ref()
        .unwrap()
        .as_ref()
        .clone();
    if let JobWorkflow::FixedDualMarlin { job } = &mut changed.printer.workflow {
        job.interior_goal
            .as_mut()
            .unwrap()
            .minimum_modeled_saving_seconds = 2.0;
    }
    manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::SetIntent {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            intent: Arc::new(changed),
        }
    });
    retain_project(&host, root.path(), "interior-pin-review");
    let baseline = host.snapshot();
    let opportunity = baseline.manufacturing.interior_opportunity.clone().unwrap();
    let info = baseline.project.clone();
    host.manufacturing(ManufacturingCommand::EvaluateInteriorPolicy {
        layout_id: layout_id(&host),
        session_id: baseline.hello.as_ref().unwrap().session_id.clone(),
        base_revision: info.as_ref().unwrap().revision,
        opportunity: opportunity.clone(),
    })
    .unwrap();
    wait(&host, |s| s.active_operation);
    host.command(Command::Cancel).unwrap();
    wait(&host, |s| !s.busy && !s.active_operation);
    assert_eq!(host.snapshot().project, info);
    assert_eq!(
        host.snapshot().manufacturing.intent,
        baseline.manufacturing.intent
    );
    assert!(host.snapshot().manufacturing.alternative.is_none());
    let serial = host.snapshot().serial;
    host.command(Command::DismissProblems).unwrap();
    idle_after(&host, serial);
    let evaluated = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::EvaluateInteriorPolicy {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            opportunity: opportunity.clone(),
        }
    });
    assert_eq!(evaluated.project, info);
    assert_eq!(
        evaluated.manufacturing.intent,
        baseline.manufacturing.intent
    );
    let loaded = wait(&host, |s| !s.busy && !s.manufacturing.alternative_loading);
    let assessment = loaded
        .manufacturing
        .alternative
        .as_ref()
        .unwrap()
        .interior_policy
        .as_deref()
        .unwrap();
    assert!(assessment.qualified());
    assert_eq!(assessment.opportunity, opportunity);
    let candidate = loaded
        .manufacturing
        .alternative_review
        .as_ref()
        .unwrap()
        .bundle
        .clone();
    assert_eq!(
        InteriorPolicyAssessment::from_selection(
            opportunity.clone(),
            candidate.plan.interior_selection.as_ref().unwrap()
        )
        .unwrap(),
        *assessment
    );
    let mut stale = opportunity.clone();
    stale.input_hash = spiling_contracts::geometry::SourceHash::from_bytes(b"wrong input");
    let rejected = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::EvaluateInteriorPolicy {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            opportunity: stale,
        }
    });
    assert_eq!(rejected.project, info);
    let serial = rejected.serial;
    host.command(Command::DismissProblems).unwrap();
    idle_after(&host, serial);
    let adopted = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::EditInteriorPolicy {
            layout_id: layout_id(&host),
            session_id,
            base_revision,
            edit: opportunity.edit.clone(),
        }
    });
    assert_eq!(
        adopted.manufacturing.intent.as_deref(),
        Some(&candidate.provenance.intent)
    );
    assert_eq!(
        adopted
            .manufacturing
            .applied_settings
            .as_ref()
            .unwrap()
            .interior_edit
            .as_ref(),
        Some(&opportunity.edit)
    );
    let review = checked_review(&host);
    assert_eq!(
        review.manufacturing.review.as_ref().unwrap().bundle.program,
        candidate.program
    );
    let output = root.path().join("export");
    send(&host, Action::Export(native(&output)));
    assert_eq!(
        std::fs::read_to_string(output.join("program.gcode")).unwrap(),
        candidate.program
    );
    let saved_path = root.path().join("suggested.spil");
    let saved = send(
        &host,
        Action::Save {
            target: Some(native(&saved_path)),
        },
    );
    assert!(!saved.project.as_ref().unwrap().dirty);
    std::fs::remove_file(&source).unwrap();
    let restarted = send(&host, Action::Restart);
    assert_eq!(restarted.manufacturing.intent, adopted.manufacturing.intent);
    let revision = restarted.project.as_ref().unwrap().revision;
    manufacture_control(&host, |session_id, _| ManufacturingCommand::Inspect {
        layout_id: layout_id(&host),
        session_id,
        base_revision: revision,
        output: VerificationOutput::ReportAndMotion,
    });
    let returned = wait(&host, |s| {
        s.manufacturing.review.is_some() && !s.manufacturing.loading
    });
    assert_eq!(
        returned
            .manufacturing
            .review
            .as_ref()
            .unwrap()
            .bundle
            .program,
        candidate.program
    );
    retain_project(&host, root.path(), "interior-suggestion");
    assert_eq!(send(&host, Action::Close).phase, Phase::Closed);
}
