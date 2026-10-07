// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::google::bytestream::ReadRequest;
use spiling_contracts::{
    google::longrunning::*, metadata, operation_view, rpc::*, synthetic_triangle,
};
use spiling_engine_client::{ClientError, EngineClient, EngineRpc};
use std::time::Duration;
use tonic::Code;
use uuid::Uuid;
const ENGINE: &str = env!("CARGO_BIN_EXE_spiling-engine");
fn request(chunks: u32, delay: u32) -> RunDiagnosticRequest {
    RunDiagnosticRequest {
        parent: "diagnostics/test".into(),
        request_id: Uuid::new_v4().to_string(),
        chunk_count: chunks,
        delay_ms: delay,
        chunk_bytes: 64,
        input_revision: "revision-1".into(),
    }
}
async fn completed(rpc: &EngineRpc, name: &str) -> Operation {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut stream = rpc.watch_operation(name).await.unwrap();
        loop {
            let op = stream.message().await.unwrap().expect("terminal snapshot");
            if op.done {
                return op;
            }
        }
    })
    .await
    .unwrap()
}
fn code(error: ClientError) -> Code {
    if let ClientError::Rpc(status) = error {
        status.code()
    } else {
        panic!("expected RPC error: {error}")
    }
}

#[tokio::test]
async fn actual_child_diagnostics_and_clean_shutdown() {
    let mut client = EngineClient::spawn(ENGINE).await.unwrap();
    assert!(client.status().await.unwrap());
    assert_eq!(client.hello().kernel, "monstertruck");
    assert!(client.hello().geometry_capabilities.is_empty());
    client.ping().await.unwrap();
    assert_eq!(client.triangle().await.unwrap(), synthetic_triangle());
    client.shutdown().await.unwrap();
    assert!(!client.status().await.unwrap());
}
#[tokio::test]
async fn acceptance_retry_conflict_and_validation_do_not_kill_engine() {
    let mut client = EngineClient::spawn(ENGINE).await.unwrap();
    let rpc = client.rpc();
    let req = request(4, 100);
    let first = rpc.run_diagnostic(req.clone()).await.unwrap();
    let again = rpc.run_diagnostic(req.clone()).await.unwrap();
    assert_eq!(first.name, again.name);
    let mut conflict = req;
    conflict.chunk_count = 3;
    assert_eq!(
        code(rpc.run_diagnostic(conflict).await.unwrap_err()),
        Code::AlreadyExists
    );
    assert_eq!(
        code(rpc.run_diagnostic(request(0, 0)).await.unwrap_err()),
        Code::InvalidArgument
    );
    assert_eq!(
        code(
            rpc.get_operation("diagnostics/test/operations/missing")
                .await
                .unwrap_err()
        ),
        Code::NotFound
    );
    client.ping().await.unwrap();
    completed(&rpc, &first.name).await;
    client.shutdown().await.unwrap();
}
#[tokio::test]
async fn incremental_outputs_and_resubscription_reconstruct_state() {
    let mut client = EngineClient::spawn(ENGINE).await.unwrap();
    let rpc = client.rpc();
    let op = rpc.run_diagnostic(request(6, 120)).await.unwrap();
    let mut watch = rpc.watch_operation(&op.name).await.unwrap();
    let mut previous = 0;
    let partial = loop {
        let next = watch.message().await.unwrap().unwrap();
        let meta = metadata(&next).unwrap();
        assert!(meta.state_version >= previous);
        previous = meta.state_version;
        if !meta.outputs.is_empty() {
            assert!(!next.done);
            break next;
        }
    };
    let artifact = metadata(&partial).unwrap().outputs[0].clone();
    assert_eq!(
        rpc.read_artifact(&artifact).await.unwrap(),
        synthetic_triangle()
    );
    drop(watch);
    let final_op = completed(&rpc, &op.name).await;
    assert_eq!(operation_view(&final_op).unwrap().state, "succeeded");
    assert_eq!(metadata(&final_op).unwrap().outputs.len(), 6);
    let mut reconnected = rpc.watch_operation(&op.name).await.unwrap();
    assert_eq!(reconnected.message().await.unwrap().unwrap(), final_op);
    assert!(reconnected.message().await.unwrap().is_none());
    client.shutdown().await.unwrap();
}
#[tokio::test]
async fn operation_outlives_request_timeout_and_watch_drop() {
    let mut client = EngineClient::spawn(ENGINE).await.unwrap();
    let rpc = client.rpc();
    let op = rpc.run_diagnostic(request(6, 1000)).await.unwrap();
    let watch = rpc.watch_operation(&op.name).await.unwrap();
    drop(watch);
    let waited = rpc
        .wait_operation(&op.name, Duration::from_millis(50))
        .await
        .unwrap();
    assert!(!waited.done);
    client.ping().await.unwrap();
    let done = completed(&rpc, &op.name).await;
    assert!(done.done);
    assert_eq!(
        metadata(&done).unwrap().state,
        DiagnosticState::Succeeded as i32
    );
    client.shutdown().await.unwrap();
}
#[tokio::test]
async fn cancellation_and_queued_cancellation_are_idempotent() {
    let mut client = EngineClient::spawn(ENGINE).await.unwrap();
    let rpc = client.rpc();
    let first = rpc.run_diagnostic(request(8, 1000)).await.unwrap();
    let second = rpc.run_diagnostic(request(2, 1000)).await.unwrap();
    rpc.cancel_operation(&second.name).await.unwrap();
    rpc.cancel_operation(&second.name).await.unwrap();
    let cancelled = completed(&rpc, &second.name).await;
    assert_eq!(
        operation_view(&cancelled).unwrap().error_code,
        Some(Code::Cancelled as i32)
    );
    assert!(metadata(&cancelled).unwrap().outputs.is_empty());
    let began = tokio::time::Instant::now();
    rpc.cancel_operation(&first.name).await.unwrap();
    let done = completed(&rpc, &first.name).await;
    assert_eq!(operation_view(&done).unwrap().state, "cancelled");
    assert!(began.elapsed() < Duration::from_secs(1));
    rpc.cancel_operation(&done.name).await.unwrap();
    assert_eq!(rpc.get_operation(&done.name).await.unwrap(), done);
    client.ping().await.unwrap();
    client.shutdown().await.unwrap();
}
#[tokio::test]
async fn cancellation_completion_races_have_one_terminal_outcome() {
    let mut client = EngineClient::spawn(ENGINE).await.unwrap();
    let rpc = client.rpc();
    for _ in 0..12 {
        let op = rpc.run_diagnostic(request(1, 0)).await.unwrap();
        rpc.cancel_operation(&op.name).await.unwrap();
        let done = completed(&rpc, &op.name).await;
        assert!(
            matches!(metadata(&done).unwrap().state, x if x==DiagnosticState::Succeeded as i32 || x==DiagnosticState::Cancelled as i32)
        );
        rpc.cancel_operation(&op.name).await.unwrap();
        assert_eq!(done, rpc.get_operation(&op.name).await.unwrap());
    }
    client.shutdown().await.unwrap();
}
#[tokio::test]
async fn interrupted_restart_preserves_deduplication_and_published_artifacts() {
    let store = tempfile::tempdir().unwrap();
    let mut client = EngineClient::spawn_in(ENGINE, store.path()).await.unwrap();
    let rpc = client.rpc();
    let req = request(8, 100);
    let op = rpc.run_diagnostic(req.clone()).await.unwrap();
    let mut watch = rpc.watch_operation(&op.name).await.unwrap();
    let artifact = loop {
        let snapshot = watch.message().await.unwrap().unwrap();
        if let Some(value) = metadata(&snapshot).unwrap().outputs.first() {
            break value.clone();
        }
    };
    let first_pid = client.pid();
    client.terminate().await.unwrap();
    drop(client);
    drop(watch);
    drop(rpc);
    let mut next = EngineClient::spawn_in(ENGINE, store.path()).await.unwrap();
    assert_ne!(next.pid(), first_pid);
    let rpc = next.rpc();
    let recovered = rpc.get_operation(&op.name).await.unwrap();
    assert_eq!(operation_view(&recovered).unwrap().state, "interrupted");
    assert_eq!(
        operation_view(&recovered).unwrap().error_code,
        Some(Code::Aborted as i32)
    );
    assert_eq!(rpc.run_diagnostic(req).await.unwrap().name, op.name);
    assert_eq!(
        rpc.read_artifact(&artifact).await.unwrap(),
        synthetic_triangle()
    );
    next.shutdown().await.unwrap();
}
#[tokio::test]
async fn store_has_exclusive_engine_owner() {
    let store = tempfile::tempdir().unwrap();
    let mut client = EngineClient::spawn_in(ENGINE, store.path()).await.unwrap();
    assert!(EngineClient::spawn_in(ENGINE, store.path()).await.is_err());
    client.ping().await.unwrap();
    client.shutdown().await.unwrap();
}
#[tokio::test]
async fn pagination_is_bounded_and_bound_to_parent() {
    let mut client = EngineClient::spawn(ENGINE).await.unwrap();
    let rpc = client.rpc();
    for _ in 0..5 {
        let op = rpc.run_diagnostic(request(1, 0)).await.unwrap();
        completed(&rpc, &op.name).await;
    }
    let mut req = ListOperationsRequest {
        name: "diagnostics/test".into(),
        page_size: 2,
        ..Default::default()
    };
    let page = rpc.list_operations(req.clone()).await.unwrap();
    assert_eq!(page.operations.len(), 2);
    assert!(!page.next_page_token.is_empty());
    req.page_token = page.next_page_token.clone();
    let next = rpc.list_operations(req.clone()).await.unwrap();
    assert_eq!(next.operations.len(), 2);
    assert!(
        page.operations
            .iter()
            .all(|p| next.operations.iter().all(|n| n.name != p.name))
    );
    req.name = "diagnostics/other".into();
    assert_eq!(
        code(rpc.list_operations(req).await.unwrap_err()),
        Code::InvalidArgument
    );
    client.shutdown().await.unwrap();
}
#[tokio::test]
async fn bulk_ranges_checksums_and_control_under_slow_downloads() {
    let mut client = EngineClient::spawn(ENGINE).await.unwrap();
    let rpc = client.rpc();
    let mut req = request(2, 20);
    req.chunk_bytes = 262144;
    let op = rpc.run_diagnostic(req).await.unwrap();
    let done = completed(&rpc, &op.name).await;
    let artifact = metadata(&done).unwrap().outputs[1].clone();
    let bytes = rpc.read_artifact(&artifact).await.unwrap();
    assert_eq!(bytes.len(), 262144);
    assert_eq!(&bytes[..64], synthetic_triangle());
    assert!(bytes[64..].iter().all(|b| *b == 1));
    let mut range = rpc
        .read_artifact_range(ReadRequest {
            resource_name: artifact.name.clone(),
            read_offset: 64,
            read_limit: 17,
        })
        .await
        .unwrap();
    let fragment = range.message().await.unwrap().unwrap();
    assert_eq!(fragment.data, vec![1; 17]);
    assert!(range.message().await.unwrap().is_none());
    assert_eq!(
        code(
            rpc.read_artifact_range(ReadRequest {
                resource_name: artifact.name.clone(),
                read_offset: 262145,
                read_limit: 0
            })
            .await
            .err()
            .unwrap()
        ),
        Code::OutOfRange
    );
    let mut corrupt = artifact.clone();
    corrupt.sha256 = "bad".into();
    assert!(rpc.read_artifact(&corrupt).await.is_err());
    let mut stalled = Vec::new();
    for _ in 0..4 {
        stalled.push(
            rpc.read_artifact_range(ReadRequest {
                resource_name: artifact.name.clone(),
                read_offset: 0,
                read_limit: 0,
            })
            .await
            .unwrap(),
        );
    }
    let began = tokio::time::Instant::now();
    client.ping().await.unwrap();
    let pending = rpc.run_diagnostic(request(8, 1000)).await.unwrap();
    rpc.cancel_operation(&pending.name).await.unwrap();
    completed(&rpc, &pending.name).await;
    assert!(began.elapsed() < Duration::from_secs(1));
    drop(stalled);
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn standard_services_require_launch_capability_and_owner_eof_stops_child() {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::process::Command;
    let store = tempfile::tempdir().unwrap();
    let mut child = Command::new(ENGINE)
        .arg("--store")
        .arg(store.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut owner = child.stdin.take().unwrap();
    let capability = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    owner
        .write_all(format!("{capability}\n").as_bytes())
        .await
        .unwrap();
    let mut startup = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut startup)
        .await
        .unwrap();
    let startup: spiling_contracts::StartupInfo = serde_json::from_str(&startup).unwrap();
    let channel = tonic::transport::Endpoint::from_shared(startup.endpoint)
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut engine = engine_client::EngineClient::new(channel.clone());
    assert_eq!(
        engine.get_engine_info(()).await.unwrap_err().code(),
        Code::Unauthenticated
    );
    let mut info = tonic::Request::new(());
    info.metadata_mut()
        .insert("authorization", capability.parse().unwrap());
    assert_eq!(
        engine.get_engine_info(info).await.unwrap().into_inner().pid,
        child.id().unwrap()
    );
    let mut operations = operations_client::OperationsClient::new(channel.clone());
    assert_eq!(
        operations
            .get_operation(GetOperationRequest {
                name: "missing".into()
            })
            .await
            .unwrap_err()
            .code(),
        Code::Unauthenticated
    );
    let mut bytes =
        spiling_contracts::google::bytestream::byte_stream_client::ByteStreamClient::new(channel);
    assert_eq!(
        bytes
            .read(ReadRequest {
                resource_name: "missing".into(),
                read_offset: 0,
                read_limit: 0
            })
            .await
            .unwrap_err()
            .code(),
        Code::Unauthenticated
    );
    drop(owner);
    assert!(
        tokio::time::timeout(Duration::from_secs(4), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn bounded_admission_and_observers_preserve_control_capacity() {
    let mut client = EngineClient::spawn(ENGINE).await.unwrap();
    let rpc = client.rpc();
    let first = rpc.run_diagnostic(request(64, 1000)).await.unwrap();
    for _ in 0..7 {
        rpc.run_diagnostic(request(64, 1000)).await.unwrap();
    }
    assert_eq!(
        code(rpc.run_diagnostic(request(64, 1000)).await.unwrap_err()),
        Code::ResourceExhausted
    );
    let mut watchers = Vec::new();
    for _ in 0..16 {
        watchers.push(rpc.watch_operation(&first.name).await.unwrap());
    }
    assert_eq!(
        code(rpc.watch_operation(&first.name).await.err().unwrap()),
        Code::ResourceExhausted
    );
    client.ping().await.unwrap();
    rpc.cancel_operation(&first.name).await.unwrap();
    // Shutdown must not wait indefinitely for subscriptions that nobody consumes.
    client.shutdown().await.unwrap();
    drop(watchers);
}

#[tokio::test]
async fn concurrent_retries_and_optional_request_ids_follow_aip155() {
    let mut client = EngineClient::spawn(ENGINE).await.unwrap();
    let rpc = client.rpc();
    let req = request(4, 40);
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let rpc = rpc.clone();
        let req = req.clone();
        tasks.push(tokio::spawn(async move {
            rpc.run_diagnostic(req).await.unwrap()
        }));
    }
    let mut name = None;
    for task in tasks {
        let op = task.await.unwrap();
        if let Some(name) = &name {
            assert_eq!(name, &op.name);
        } else {
            name = Some(op.name);
        }
    }
    completed(&rpc, name.as_ref().unwrap()).await;
    let mut unspecified = request(1, 0);
    unspecified.request_id.clear();
    let a = rpc.run_diagnostic(unspecified.clone()).await.unwrap();
    let b = rpc.run_diagnostic(unspecified).await.unwrap();
    assert_ne!(a.name, b.name);
    let mut zero = request(1, 0);
    zero.request_id = Uuid::nil().to_string();
    assert_eq!(
        code(rpc.run_diagnostic(zero).await.unwrap_err()),
        Code::InvalidArgument
    );
    client.shutdown().await.unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn dropping_process_owner_reaps_actual_child() {
    let client = EngineClient::spawn(ENGINE).await.unwrap();
    let pid = client.pid().unwrap();
    drop(client);
    tokio::time::timeout(Duration::from_secs(2), async {
        while std::path::Path::new(&format!("/proc/{pid}")).exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
