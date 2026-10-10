<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Supervised native client rules

- Share this client between CLI and desktop shell; do not duplicate pipe/protocol state machines in either consumer.
- One mutable client permits one short request at a time. Correlate IDs and expected frame kinds, validate headers before allocating, enforce the 1 MiB geometry and 4 MiB manufacturing chunk bounds, and negotiate protocol v4 and frozen geometry limits. Five-second request/exit deadlines remain transport deadlines, not native-job deadlines.
- Spawn performs mandatory hello; observe OS process status. Framing/correlation/version failures, partial exchanges, timeouts and cancelled in-flight futures kill/reap the child. Explicit shutdown waits for acknowledgement and successful exit; Drop uses Tokio kill-on-drop.
- GeometryResponse::Error, ClientError::Geometry, ClientError::Project, ClientError::Manufacturing and unwritten local InvalidRequest failures preserve the session. Typed project/manufacturing errors map to their nonfatal client errors. Native paths, poses, planes, manufacturing intent and control size are preflighted before writing. Generic geometry/manufacturing reject binary reads; use read_geometry_chunk or fetch_manufacturing_bundle. Consumers use is_fatal rather than treating every client error as a disconnect.
- A manufacturing chunk is typed control metadata followed by FrameKind::ManufacturingChunk raw bytes on the same request ID. Keep KillGuard armed across both frames; interrupted/malformed framing kills the child. Drain bounded raw bodies before returning typed metadata mismatches. Assembly is capped at 16 MiB; verify exact size/hash, strict contracts decoding, semantic input fingerprint and artifact summary without planning or replay dependencies. Complete bounded corrupt bundles are nonfatal; stored verification is not independent replay.
- Stderr is inherited; stdout is reserved for frames. No Tauri, CAD or domain/planner implementation dependencies. Heavy work uses engine jobs, 100 ms polling and protocol CancelJob, never an aborted pipe read.
- Run `cargo test --locked -p spiling-engine-client` for bounded wire behavior. Deep real-child regressions require a built native engine: `SPILING_TEST_ENGINE=PATH cargo test --locked -p spiling-engine-client -- --ignored --test-threads=1`. See [control protocol](../../docs/protocol/control.md).
