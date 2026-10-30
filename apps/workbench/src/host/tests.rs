// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use std::time::Instant;

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
        optical_task: None,
        diagnostic_task: None,
        diagnostic_paused: false,
        image_task: None,
        review_task: None,
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
    let intent = loaded.manufacturing.loaded.as_ref().unwrap().intent.clone();
    let h = s.hello.as_ref().unwrap();
    let serial = host.snapshot().serial;
    host.manufacturing(ManufacturingCommand::SetIntent {
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
    let replay = r.display.as_ref().unwrap();
    assert!(!replay.batches.is_empty());
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
            session_id,
            base_revision,
        }
    });
    assert!(s.manufacturing.artifact.is_some(), "{:?}", s.problems);
    manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::Inspect {
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
    let saved = root.path().join("project");
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
            session_id,
            base_revision,
        }
    });
    assert!(s.manufacturing.artifact.is_some(), "{:?}", s.problems);
    manufacture_control(host, |session_id, base_revision| {
        ManufacturingCommand::Inspect {
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
            target: Some(native(&root.join(name))),
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
            } else {
                "fixtures/geometry/box-mm.step"
            }))),
        );
        let intent = Arc::new(
            decode_intent(
                &std::fs::read(repo.join(if cylinder {
                    "fixtures/manufacturing/solid-fill.intent.json"
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
            let s = wait(&host, |s| s.display.is_some());
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
            session_id: before.hello.as_ref().unwrap().session_id.clone(),
            base_revision: before.project.as_ref().unwrap().revision,
            selection: oversized,
        })
        .is_err()
    );
    assert_eq!(host.snapshot().serial, before.serial);
    let s = manufacture_control(&host, |session_id, base_revision| {
        ManufacturingCommand::ApplyMaterialProfile {
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
    let undo = send(&host, Action::Undo);
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
