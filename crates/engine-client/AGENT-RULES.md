<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Supervised native gRPC client rules

- Share EngineRpc between CLI and shell. Cloneable RPC access does not own the process; EngineClient exclusively owns child lifetime, temporary store and the stdin owner-liveness pipe. There is no framed command reader/writer or transport negotiation.
- Authenticate with a per-launch capability on inherited stdin, never arguments/logs/files. Validate bounded startup metadata, ephemeral IPv4 loopback endpoint, OS child PID, instance identity and checked native session/kernel/capability/frozen-limit readiness. Missing native fields fail startup; diagnostics retain their separate limits.
- Unary exchanges have five-second deadlines; watchers are long-lived. Dropping/expiring a call never cancels accepted work or kills the child. CancelOperation explicitly requests cancellation by the standard Google operation name. NativeOperationView is checked against actual protobuf Any metadata/result and terminal invariants, never a numeric native job handle.
- Geometry/project/manufacturing commands are transport-independent adapters mapped to generated typed RPCs. Preflight native paths, poses, planes, explicit intent and encoded control size. Rich canonical RPC status maps checked domain error details to recoverable client errors; project information retains save uncertainty. Explicit request-ID admission variants support caller-controlled dedupe; never automatically retry uncertain mutations.
- Retrieve immutable descriptors exclusively with ByteStream. Diagnostic artifacts retain their diagnostic allocation cap. Geometry reads bind descriptor, chunk metadata and artifact summary, then check exact size/hash and packed mesh/section layout before returning bytes. Manufacturing reads enforce the 16 MiB bundle cap, exact size/hash, cumulative bounded strict JSON decoding, schema, input fingerprint and artifact summary. Stored verification is not independent replay or machine readiness.
- Structured domain failures, fully received corrupt manufacturing artifacts, unwritten local input rejections and individual RPC deadlines leave the session usable. Consumers use is_fatal rather than treating every client error as a disconnect. A failed observation has no authority to cancel accepted work.
- Shutdown waits for clean observed exit; terminate and owner Drop kill/reap the child. spawn_command preserves caller-scoped environment while retaining the same supervision/auth path; wait_exit observes fault-injected exits. Temporary stores last for the owner; spawn_in uses explicit retained storage.
- No kernel, core storage, planner, verifier, Tauri or alternative scheduler dependencies. Exercise diagnostics through `cargo test -p spiling-engine`; client validation through `cargo test -p spiling-engine-client`. Deep actual-child regressions require `SPILING_TEST_ENGINE=PATH cargo test -p spiling-engine-client -- --ignored --test-threads=1`. Native verification must follow repository resource limits and run sequentially. See [control protocol](../../docs/protocol/control.md).
