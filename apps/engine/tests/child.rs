// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::{
    Frame, FrameKind, PROTOCOL_VERSION, Request, Response, synthetic_triangle,
};
use spiling_engine_client::{ClientError, EngineClient};
use std::{io::Cursor, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::timeout,
};

const ENGINE: &str = env!("CARGO_BIN_EXE_spiling-engine");

fn encode(id: u32, request: Request) -> Vec<u8> {
    let mut bytes = vec![];
    Frame::control(id, &request)
        .unwrap()
        .write(&mut bytes)
        .unwrap();
    bytes
}

async fn raw_child(bytes: &[u8]) -> std::process::Output {
    let mut child = Command::new(ENGINE)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(bytes).await.unwrap();
    drop(input);
    let output = timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    for line in std::str::from_utf8(&output.stderr).unwrap().lines() {
        let diagnostic: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(diagnostic["event"].is_string());
        assert!(diagnostic["level"].is_string());
    }
    output
}

fn responses(bytes: &[u8]) -> Vec<Frame> {
    let mut input = Cursor::new(bytes);
    let mut result = vec![];
    while let Some(frame) = Frame::read(&mut input).unwrap() {
        result.push(frame);
    }
    result
}

#[tokio::test]
async fn actual_client_handshake_ping_triangle_shutdown() {
    let mut client = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    assert_eq!(Some(client.hello().pid), client.pid());
    assert!(client.status().await.unwrap());
    for _ in 0..3 {
        client.ping().await.unwrap();
        assert_eq!(client.triangle().await.unwrap(), synthetic_triangle());
    }
    client.shutdown().await.unwrap();
    assert!(!client.status().await.unwrap());
    assert!(matches!(
        client.ping().await,
        Err(ClientError::UnexpectedExit(_))
    ));
}

#[tokio::test]
async fn mismatch_is_upgrade_required_and_never_runs_commands() {
    assert!(matches!(
        EngineClient::spawn(ENGINE, PROTOCOL_VERSION + 1).await,
        Err(ClientError::UpgradeRequired(_))
    ));
    let mut input = encode(
        1,
        Request::Hello {
            protocol_version: PROTOCOL_VERSION + 1,
            client_build: "test".into(),
        },
    );
    input.extend(encode(2, Request::Triangle {}));
    let output = raw_child(&input).await;
    assert!(!output.status.success());
    let frames = responses(&output.stdout);
    assert_eq!(frames.len(), 1);
    assert!(
        matches!(serde_json::from_slice::<Response>(&frames[0].payload).unwrap(), Response::Error { code, .. } if code == "upgrade_required")
    );
}

#[tokio::test]
async fn obsolete_control_and_frame_versions_are_rejected() {
    for version in [1, 2] {
        assert!(matches!(
            EngineClient::spawn(ENGINE, version).await,
            Err(ClientError::UpgradeRequired(_))
        ));
        let mut frame = encode(
            1,
            Request::Hello {
                protocol_version: PROTOCOL_VERSION,
                client_build: "test".into(),
            },
        );
        frame[4..6].copy_from_slice(&version.to_le_bytes());
        let output = raw_child(&frame).await;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

#[tokio::test]
async fn mandatory_handshake_and_request_order() {
    let output = raw_child(&encode(1, Request::Ping {})).await;
    assert!(!output.status.success());
    assert!(
        matches!(serde_json::from_slice::<Response>(&responses(&output.stdout)[0].payload).unwrap(), Response::Error { code, .. } if code == "handshake_required")
    );
    let mut input = encode(
        5,
        Request::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_build: "test".into(),
        },
    );
    input.extend(encode(5, Request::Ping {}));
    let output = raw_child(&input).await;
    assert!(!output.status.success());
    let frames = responses(&output.stdout);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1].header.kind, FrameKind::Control);
    assert!(
        matches!(serde_json::from_slice::<Response>(&frames[1].payload).unwrap(), Response::Error { code, .. } if code == "invalid_request_id")
    );
}

#[tokio::test]
async fn malformed_and_truncated_frames_exit_without_protocol_noise() {
    let mut malformed = encode(1, Request::Ping {});
    malformed[0] = 0;
    for bytes in [&malformed[..], &malformed[1..7]] {
        let output = raw_child(bytes).await;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
    let output = raw_child(&[]).await;
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}

#[tokio::test]
async fn intentional_termination_is_observed_and_restart_is_fresh() {
    let mut client = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    client.terminate().await.unwrap();
    assert!(!client.status().await.unwrap());
    client.terminate().await.unwrap();
    let mut restarted = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    restarted.ping().await.unwrap();
    restarted.shutdown().await.unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn dropped_client_kills_actual_process() {
    let client = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    let process = std::path::PathBuf::from(format!("/proc/{}", client.pid().unwrap()));
    drop(client);
    timeout(Duration::from_secs(5), async {
        while process.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn timed_out_actual_child_is_killed_and_reaped() {
    let mut client = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    let pid = client.pid().unwrap();
    assert!(
        std::process::Command::new("kill")
            .args(["-STOP", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert!(matches!(client.ping().await, Err(ClientError::Timeout)));
    assert!(!client.status().await.unwrap());
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancelled_actual_child_request_initiates_cleanup() {
    let mut client = EngineClient::spawn(ENGINE, PROTOCOL_VERSION).await.unwrap();
    let pid = client.pid().unwrap();
    assert!(
        std::process::Command::new("kill")
            .args(["-STOP", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        timeout(Duration::from_millis(10), client.ping())
            .await
            .is_err()
    );
    timeout(Duration::from_secs(5), async {
        while client.status().await.unwrap() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        client.triangle().await,
        Err(ClientError::UnexpectedExit(_))
    ));
}

async fn raw_response(output: &mut tokio::process::ChildStdout) -> Response {
    let mut header = [0; spiling_contracts::FRAME_HEADER_BYTES];
    timeout(Duration::from_secs(5), output.read_exact(&mut header))
        .await
        .unwrap()
        .unwrap();
    let header = spiling_contracts::FrameHeader::decode(&header).unwrap();
    assert_eq!(header.kind, FrameKind::Control);
    let mut bytes = vec![0; header.payload_len as usize];
    output.read_exact(&mut bytes).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn unchecked_pose_and_plane_values_are_recoverable_in_actual_engine() {
    use spiling_contracts::geometry::*;
    let mut child = Command::new(ENGINE)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = child.stdout.take().unwrap();
    input
        .write_all(&encode(
            1,
            Request::Hello {
                protocol_version: PROTOCOL_VERSION,
                client_build: "raw-domain-test".into(),
            },
        ))
        .await
        .unwrap();
    let Response::Hello(hello) = raw_response(&mut output).await else {
        panic!("expected hello");
    };
    assert_eq!(hello.pid, pid);
    let bad_pose = RigidPoseMm {
        translation_mm: [0.0; 3],
        rotation_xyzw: [0.0; 4],
    };
    input
        .write_all(&encode(
            2,
            Request::Geometry {
                command: GeometryCommand::AddInstance {
                    session_id: hello.session_id.clone(),
                    base_revision: SceneRevision::ZERO,
                    definition_id: DefinitionId::parse("a".repeat(64)).unwrap(),
                    pose: bad_pose,
                },
            },
        ))
        .await
        .unwrap();
    assert!(
        matches!(raw_response(&mut output).await,Response::Geometry { response: GeometryResponse::Error { error } } if error.code==GeometryErrorCode::InvalidPose)
    );
    input
        .write_all(&encode(
            3,
            Request::Geometry {
                command: GeometryCommand::StartSection {
                    session_id: hello.session_id.clone(),
                    base_revision: SceneRevision::ZERO,
                    plane: PlaneMm {
                        origin_mm: [0.0; 3],
                        normal: [0.0; 3],
                    },
                },
            },
        ))
        .await
        .unwrap();
    assert!(
        matches!(raw_response(&mut output).await,Response::Geometry { response: GeometryResponse::Error { error } } if error.code==GeometryErrorCode::InvalidGeometry)
    );
    input.write_all(&encode(4, Request::Ping {})).await.unwrap();
    assert!(matches!(raw_response(&mut output).await, Response::Pong {}));
    input
        .write_all(&encode(
            5,
            Request::Geometry {
                command: GeometryCommand::GetScene {
                    session_id: hello.session_id.clone(),
                },
            },
        ))
        .await
        .unwrap();
    assert!(
        matches!(raw_response(&mut output).await,Response::Geometry { response: GeometryResponse::Scene { summary } } if summary.revision==SceneRevision::ZERO && summary.occurrence_count==0 && summary.session_id==hello.session_id)
    );
    input
        .write_all(&encode(6, Request::Shutdown {}))
        .await
        .unwrap();
    assert!(matches!(raw_response(&mut output).await, Response::Bye {}));
    drop(input);
    assert!(
        timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn unknown_fields_remain_fatal_even_with_a_domain_invalid_pose() {
    use spiling_contracts::geometry::*;
    let mut bytes = encode(
        1,
        Request::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_build: "strict-test".into(),
        },
    );
    let value = serde_json::json!({
        "type":"geometry","command":{
            "op":"add_instance","session_id":SessionId::new(),"base_revision":0,
            "definition_id":"a".repeat(64),"pose":{"translation_mm":[0,0,0],"rotation_xyzw":[0,0,0,0]},
            "unexpected":true
        }
    });
    Frame::control(2, &value)
        .unwrap()
        .write(&mut bytes)
        .unwrap();
    bytes.extend(encode(3, Request::Ping {}));
    let output = raw_child(&bytes).await;
    assert!(!output.status.success());
    let frames = responses(&output.stdout);
    assert_eq!(frames.len(), 2);
    assert!(
        matches!(serde_json::from_slice::<Response>(&frames[1].payload).unwrap(),Response::Error { code,.. } if code=="invalid_request")
    );
}

#[tokio::test]
async fn unknown_project_fault_is_rejected_before_handshake() {
    let child = Command::new(ENGINE)
        .env("SPILING_PROJECT_FAULT", "unknown")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let output = timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}
