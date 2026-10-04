// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use tokio::io::duplex;

async fn reply_with(header: [u8; FRAME_HEADER_BYTES], body: &[u8]) -> Result<Frame, ClientError> {
    let (mut client, mut peer) = duplex(1024);
    let (mut output, mut input) = tokio::io::split(&mut client);
    let responder = async move {
        let mut request_header = [0; FRAME_HEADER_BYTES];
        peer.read_exact(&mut request_header).await.unwrap();
        let request_header = FrameHeader::decode(&request_header).unwrap();
        let mut payload = vec![0; request_header.payload_len as usize];
        peer.read_exact(&mut payload).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Request>(&payload).unwrap(),
            Request::Ping {}
        );
        peer.write_all(&header).await.unwrap();
        peer.write_all(body).await.unwrap();
    };
    let operation = exchange(&mut input, &mut output, 9, &Request::Ping {});
    let (_, response) = timeout(Duration::from_secs(1), async {
        tokio::join!(responder, operation)
    })
    .await
    .unwrap();
    response
}

#[tokio::test]
async fn rejects_wrong_response_id_before_reading_payload() {
    let header = FrameHeader::new(FrameKind::Control, 8, 100)
        .unwrap()
        .encode()
        .unwrap();
    assert!(
        matches!(reply_with(header, &[]).await, Err(ClientError::Protocol(message)) if message.contains("does not match"))
    );
}

#[tokio::test]
async fn rejects_oversized_response_before_reading_payload() {
    let mut header = FrameHeader::new(FrameKind::Control, 9, 0)
        .unwrap()
        .encode()
        .unwrap();
    header[12..16].copy_from_slice(&(MAX_CONTROL_BYTES + 1).to_le_bytes());
    assert!(matches!(
        reply_with(header, &[]).await,
        Err(ClientError::Wire(WireError::Length { .. }))
    ));
}

#[tokio::test]
async fn validates_control_shape_and_classifies_upgrade_required() {
    let frame = Frame::control(9, &Response::Pong {}).unwrap();
    let received = reply_with(frame.header.encode().unwrap(), &frame.payload)
        .await
        .unwrap();
    assert_eq!(response(&received).unwrap(), Response::Pong {});
    let error = Frame::control(
        9,
        &Response::Error {
            code: "upgrade_required".into(),
            message: "different protocol".into(),
        },
    )
    .unwrap();
    assert!(matches!(
        response(&error),
        Err(ClientError::UpgradeRequired(_))
    ));
    let malformed =
        Frame::new(FrameKind::Control, 9, b"{\"type\":\"unexpected\"}".to_vec()).unwrap();
    assert!(matches!(response(&malformed), Err(ClientError::Json(_))));
    let binary = Frame::new(FrameKind::Triangle, 9, vec![]).unwrap();
    assert!(matches!(response(&binary), Err(ClientError::Protocol(_))));
}

#[tokio::test]
async fn truncated_payload_and_stalled_pipe_fail() {
    let header = FrameHeader::new(FrameKind::Control, 9, 30)
        .unwrap()
        .encode()
        .unwrap();
    assert!(
        matches!(reply_with(header, b"{}").await, Err(ClientError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof)
    );
    let (mut client, _peer) = duplex(1024);
    let (mut output, mut input) = tokio::io::split(&mut client);
    assert!(
        timeout(
            Duration::from_millis(10),
            exchange(&mut input, &mut output, 9, &Request::Ping {})
        )
        .await
        .is_err()
    );
}
