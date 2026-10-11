<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native engine operations and artifacts

The owned schemas under `crates/contracts/proto/spiling/` are the native API authority. Google Operations, Status and ByteStream schemas are vendored unmodified with Apache-2.0 provenance. Pinned tonic/prost and vendored protoc generate native services/clients at build time. Checked Rust domain adapters generate the webview view types; `pnpm contracts --check` rejects drift. Persisted project/intent/bundle JSON is a separate domain schema, not an opaque RPC tunnel. Engine and shell ship together: there is no cross-version negotiation, alternate transport or compatibility shim.

## Process ownership and readiness

The shared Rust client launches `spiling-engine --store PATH`. The parent sends a random 64-hex-character launch capability plus newline on inherited stdin and keeps that pipe open for ownership liveness. The child binds ephemeral IPv4 loopback and writes one bounded JSON startup record on stdout containing endpoint, instance UUID and PID. No capability is returned in stdout or persisted. The client validates a loopback endpoint, child PID, instance identity and message limits, then calls authenticated `GetEngineInfo`.

All RPCs require the capability in authorization metadata. This is local plaintext HTTP/2 under a trusted-host assumption, not remote access or protection against a privileged local observer. The native Rust client connects directly without HTTP proxy routing. Only the native shell/CLI connect; the webview uses narrow Tauri invokes. The parent never discovers engines by fixed port or adopts an unrelated process.

Unary calls have five-second deadlines; watches are explicitly long-lived. Expiring/dropping an RPC or watcher does not kill the engine or cancel accepted work. Shutdown/owner EOF interrupt remaining work and bound server drain to two seconds. The process owner waits for observed exit, can explicitly terminate, and uses kill-on-drop. Logs go to stderr; stdout is not a custom frame transport.

## Services and Google API patterns

- `Engine.GetEngineInfo`, `Shutdown`, `RunDiagnostic`, `WatchOperation`.
- Standard `google.longrunning.Operations.GetOperation`, `ListOperations`, `CancelOperation`, `WaitOperation`.
- `Artifacts.GetArtifact` returns metadata; standard `google.bytestream.ByteStream.Read` streams bytes.
- Standard optional DeleteOperation, ByteStream Write and QueryWriteStatus explicitly return UNIMPLEMENTED. List filters, wildcard parents and partial-success listing are unsupported.
- `Geometry.Execute`, `ImportPart`, `StartSection`; `Projects.Execute`, `Open`, `Save`; `Manufacturing.Execute`, `Compile`, `Verify` are typed protobuf native services. Short actions use typed oneofs; long actions return standard Operations.

RunDiagnostic is a typed long-running action following [AIP-151](https://google.aip.dev/151). Its result is a standard Operation with typed DiagnosticMetadata in `metadata` and exactly one terminal `response` or `error`. The schema declares operation_info. Operations have names `diagnostics/{id}/operations/{uuid}`; this namespace does not create an authoring project.

Native operations use `sessions/{session_uuid}/operations/{uuid}` and typed NativeOperationMetadata/NativeOperationResult Any values. Session UUID and captured scene/project revisions are explicit and distinct. Public observation/cancellation never uses an internal numeric worker task ID. Retained completed operations are historical after restart; they do not revive native handles or replace the current scene.

An optional nonzero UUID `request_id` follows [AIP-155](https://google.aip.dev/155). A supplied ID deduplicates equivalent normalized requests within its parent while the operation is retained. Different parameters with the same ID fail ALREADY_EXISTS. Omission creates independent work on every call. Admission/deduplication is committed before acknowledgement and scheduling belongs to the engine even when its caller drops the acceptance RPC. This is deduplicated admission, not a claim of exactly-once physical execution.

List uses bounded keyset pagination with parent-bound opaque tokens and default 20/max 50 items, following [AIP-158](https://google.aip.dev/158). A token is not a fixed historical snapshot: newly inserted operations can alter subsequent pages. Negative page sizes or wrong-parent tokens fail. Request parameters other than the token/page size must remain consistent.

## Lifecycle, progress and cancellation

```text
queued -> running -> succeeded | failed
queued | running -> cancelling -> cancelled
cancelling -> succeeded | failed if completion won the race
unfinished -> interrupted on shutdown or recovery after engine failure
```

Terminal outcomes never change. Cancellation acknowledgement records intent; it does not certify that computation has stopped. Repeated cancellation is idempotent. Successful cancellation is a terminal Operation error with canonical CANCELLED status. Recovery/intentional shutdown interruption uses ABORTED. Worker/publication transitions serialize against cancellation; completion may legitimately win.

Metadata contains state/version, phase, completed/total units, input revision/digest and available complete output descriptors. The diagnostic total is known; it reports chunk counts rather than invented phase percentages. WatchOperation immediately returns a current snapshot, then coalesces newer complete snapshots through bounded storage. Intermediate versions may be skipped. Registration/publication cannot lose an update; reconnect by obtaining current state rather than replaying percentage events. WaitOperation returns current state after completion or its bounded timeout, not a guaranteed terminal result.

Input revision is retained provenance only in the synthetic diagnostic, which never mutates a project. Native actions validate the active session and exact captured scene/project revision before admission/publication. Import, placement, source-backed open/save and manufacturing use core transaction authority; native objects/caches remain private. Manufacturing intent/geometry edits invalidate the active artifact, while coherent undo can restore it.

## Durable publication and limits

An exclusive file lock permits one engine per store. SQLite WAL with FULL synchronization atomically commits operation admission, complete artifact blobs and output/state references. On reopen, unfinished operations become interrupted; they are never automatically replayed. Complete outputs remain retrievable after failure/cancellation/restart. Tests establish process-kill recovery, not a physical power-loss certification.

Current common bounds are 128 retained operations, eight unfinished admitted operations and one shared execution permit. Diagnostics reserve at most 32 MiB total, with at most 64 chunks per operation and 256 KiB per diagnostic artifact. Native operations have a separate 256 MiB conservative reservation/storage budget, reserving mesh 64 MiB, section 16 MiB, open 80 MiB, manufacturing bundle 16 MiB or save 0 bytes as appropriate; completion charges actual complete outputs. Native metadata/output counts and session/runtime/core retention have independent bounds. No deletion/GC API is implemented. Capacity exhaustion is explicit RESOURCE_EXHAUSTED, never silent eviction or truncated success. The UI store is application-local persistent storage; CLI defaults to temporary storage unless `--store` is supplied.

Watch/Wait share a global budget of 16 observers; binary downloads have eight slots and two queued 16 KiB fragments per stream. Individual gRPC messages are capped at 64 KiB; HTTP/2 permits 32 streams per connection, leaving room for control when subscription/download limits are reached. SQLite accesses use blocking tasks with short serialized transactions, not an async runtime lock held across transfer waits.

Diagnostics generate synthetic triangle chunks with optional explicitly synthetic padding. Native geometry runs on a separate single worker thread, never on Tokio threads; a session-owned watchdog serializes cancellation against commit/promotion. Nonpreemptible native calls cannot be reported as stopped merely because an RPC deadline expires.

Native results are staged privately. Cancellation/staleness checks and source-pin reservations precede the worker promotion decision; only its ACK permits core/scene publication. Immutable operation bytes and terminal output references then commit together in SQLite. A cancellation request arriving after ACK or after a save manifest replacement cannot turn committed success into cancelled rollback. A ledger failure after native/disk commit interrupts the engine and requires explicit reconciliation, never automatic retry. Saved sources and manufacturing bundles are reopened/replayed in a fresh native session; unfinished native operations are not resumed.

## Incremental artifacts and binary reads

Each available descriptor names complete immutable bytes as `artifacts/{sha256}`, with size, checksum and media type. Availability is published in the same SQLite transaction as its bytes. Diagnostic descriptors are append-only and bounded to 64; repeated identical outputs may share content. Native outputs are published only after their promoted result, not at worker stage time. Overall diagnostic success is distinct from partial availability. Manufacturing exports require a complete independently verified bundle; an incomplete program is never a verified build.

ByteStream Read uses its standard resource_name, read_offset and read_limit: zero limit reads to end; negative offsets/offsets beyond end fail OUT_OF_RANGE; negative limits fail INVALID_ARGUMENT. Stream messages carry byte fragments, not JSON/base64. Full client reads enforce the allocation bound, exact length and SHA-256. Partial-range callers must validate their range separately; a range does not redefine the artifact checksum.

The shell returns bounded bytes as raw Tauri responses. React pulls and validates complete packed artifacts before GPU admission, with nonoverlapping observation and revision/session generation guards. Diagnostic SPLT, native SPLM meshes and SPLS sections have separate decoders. Cancellation can run independently of display transfer. Paged native metadata binds returned resource descriptors to expected packed chunk hashes, sizes and source mappings. Native artifact release frees session-derived memory; it is not operation cancellation or deletion of retained immutable output. Progressive geometric refinement/atomic replacement sets are not implemented.

## SPLT synthetic triangle

The original 64-byte packed display format remains independent of gRPC. All fields are little-endian:

| Offset | Bytes | Value                       |
| ------ | ----- | --------------------------- |
| 0      | 4     | ASCII SPLT                  |
| 4      | 2     | Schema 1                    |
| 6      | 2     | Reserved zero               |
| 8      | 4     | Vertex count 3              |
| 12     | 4     | Index count 3               |
| 16     | 36    | Nine float32 XYZ components |
| 52     | 12    | Three uint32 indices        |

Positions are `[-0.75,-0.6,0, 0.75,-0.6,0, 0,0.75,0]`; indices are `[0,1,2]`. TypeScript validates magic, schema, counts, exact size, alignment, finite coordinates and index bounds before typed views or GPU upload. The native-owned [golden fixture](../../fixtures/protocol/triangle.json) is retained. The optional padded diagnostic media type contains this payload followed by synthetic bytes and is not accepted directly by the triangle decoder.

## Exercising the actual services

```sh
cargo build --locked -p spiling-engine -p spiling-cli
cargo test --locked -p spiling-engine
node --import tsx tools/smoke-cli.mjs
spiling-cli job --engine /absolute/path/spiling-engine --chunks 4 --delay-ms 250
spiling-cli job --engine /absolute/path/spiling-engine --store /absolute/path/store --request-id UUID
```

CLI progress goes to stderr; final JSON follows child cleanup. The standard Operation terminal status is separate from RPC transport errors. Shell/CLI/native tests exercise the same generated API, not an alternate in-process engine. CEF presentation and physical GPU support require separate evidence.
