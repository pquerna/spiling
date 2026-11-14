<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native engine operations and artifacts

The owned schemas under `crates/contracts/proto/spiling/` are the native API authority. Google Operations, Status and ByteStream schemas are vendored unmodified with Apache-2.0 provenance. Pinned tonic/prost and vendored protoc generate native services/clients at build time. Checked Rust domain adapters generate the webview view types; `pnpm contracts --check` rejects drift. Persisted project/intent/bundle JSON is a separate domain schema, not an opaque RPC tunnel. Engine and shell ship together: there is no cross-version negotiation, alternate transport or compatibility shim.

## Process ownership and readiness

The shared Rust client launches `spiling-engine --store PATH`. The parent sends a random 64-hex-character launch capability plus newline on inherited stdin and keeps that pipe open for ownership liveness. The child binds ephemeral IPv4 loopback and writes one bounded JSON startup record on stdout containing endpoint, instance UUID and PID. No capability is returned in stdout or persisted. The client validates a loopback endpoint, child PID, instance identity and message limits, then calls authenticated `GetEngineInfo`.

Readiness requires the exact shared `ENGINE_BUILD_ID` from `crates/contracts/src/lib.rs` and declared format/feature capabilities, including `ptac_projects_v1`, `manufacturing_schema11`, `fixed_dual_material_routing_v1`, `whole_occurrence_layer_cycles_v1`, `source_bound_surface_color_construction_v1`, `source_bound_process_v3`, `appearance_palette_study_c1a`, `optical_construction_reference_study_v1`, native trimmed-source/query/bed and existing finish/image capabilities. The retired project-storage capability is rejected; there is no compatibility negotiation. Executable client requirements are authoritative. These identify a coordinated semantic generation, not exact Git attestation, physical printer support or optical calibration.

The `analysis5` semantic generation includes native prepared contact rectangles,
required whole-face/disk footprints, conservative finite-contact transfer and typed assumed-law whole-part
numerical studies, checked SPLA fields and explicit portable Core result
attachment, complete native bore preparation, ideal distributing fasteners and
rational Hex27 SPLA2 fields. `AnalysisResultPrepared` binds a read revision separately from the
original capture; `AnalysisCompleted` retains exact completed-capture equality.
It does not identify a qualified Basic/Enhanced product or physical calibration;
[Analyze contracts](analysis.md) define the admitted model and evidence limits.

All RPCs require the capability in authorization metadata. This is local plaintext HTTP/2 under a trusted-host assumption, not remote access or protection against a privileged local observer. The native Rust client connects directly without HTTP proxy routing. Only the native workbench/CLI connect. The parent never discovers engines by fixed port or adopts an unrelated process.

The shared client retains two authenticated connections to that same endpoint: one for control/operation observation and one for ByteStream reads. Bulk response flow control cannot consume the control connection's HTTP/2 window. Cloned RPC handles reuse this pair; it is not another process, protocol, negotiated transport or artifact authority.

Unary calls have five-second deadlines; watches are explicitly long-lived. Expiring/dropping an RPC or watcher does not kill the engine or cancel accepted work. Shutdown/owner EOF interrupt remaining work and bound server drain to two seconds. The process owner waits for observed exit, can explicitly terminate, and uses kill-on-drop. Logs go to stderr; stdout is not a custom frame transport.

## Compute policy

CubeCL compute preferences are machine-local, not project or manufacturing input.
The default backend is `auto`; an optional FEA/Analyze backend defaults to
`inherit`, using the same resolved default device. This reserved override does
not enable an FEA operation. The initial compiled backends are CPU/LLVM and
Vulkan. Automatic selection prefers an eligible hardware Vulkan adapter,
otherwise CPU; this deterministic preference is not a measured fastest-device
claim. An explicit unavailable backend refuses startup rather than downgrading.

The workbench persists these two preferences under **Edit → Compute settings**.
Changes apply on the next supervised engine session, through the ordinary
save/discard/restart protection. The engine accepts `--compute-backend
auto|cpu|vulkan` and `--fea-compute-backend inherit|auto|cpu|vulkan`; CLI/headless
launches can use `SPILING_COMPUTE_BACKEND` and `SPILING_FEA_COMPUTE_BACKEND`.
Explicit launch arguments take precedence over environment values. Settings
are immutable during an engine session. Checked `EngineInfo.compute` returns
the requested policy and both resolved device identities; missing, unspecified,
unknown or inconsistent responses refuse readiness.

Compute owns bounded numeric staging and synchronized dispatch, not native
geometry, project mutation, manufacturing admission or verification authority.
Backend errors and cancellation are explicit; optional small/oversize numeric
batches may retain their original host algorithm. Compute device selection is
independent of the workbench's rendering adapter and is not platform or physical
machine qualification.

## Services and Google API patterns

- `Engine.GetEngineInfo`, `Shutdown`, `RunDiagnostic`, `WatchOperation`.
- Standard `google.longrunning.Operations.GetOperation`, `ListOperations`, `CancelOperation`, `WaitOperation`.
- `Artifacts.GetArtifact` returns metadata; standard `google.bytestream.ByteStream.Read` streams bytes.
- `ByteStream.Write` accepts only staged uploads (below). Standard optional DeleteOperation and QueryWriteStatus explicitly return UNIMPLEMENTED; there are no resumable writes. List filters, wildcard parents and partial-success listing are unsupported.
- `Geometry.Execute`, `ImportPart`, `StartSection`; `Projects.Execute`, `Open`, `Save`, `ImportSurfaceImageFinish`, `GetSurfaceImage`, `ImportAppearancePalette`, `CompareAppearance`, `GetAppearanceReport`; `Manufacturing.Execute`, `Compile`, `Verify` are typed protobuf native services. Short actions use typed oneofs; long actions return standard Operations.

RunDiagnostic is a typed long-running action following [AIP-151](https://google.aip.dev/151). Its result is a standard Operation with typed DiagnosticMetadata in `metadata` and exactly one terminal `response` or `error`. The schema declares operation_info. Operations have names `diagnostics/{id}/operations/{uuid}`; this namespace does not create an authoring project.

Native operations use `sessions/{session_uuid}/operations/{uuid}` and typed NativeOperationMetadata/NativeOperationResult Any values. Session UUID and captured scene/project revisions are explicit and distinct. Public observation/cancellation never uses an internal numeric worker task ID. Retained completed operations are historical after restart; they do not revive native handles or replace the current scene.

An optional nonzero UUID `request_id` follows [AIP-155](https://google.aip.dev/155). A supplied ID deduplicates equivalent normalized requests within its parent while the operation is retained. Different parameters with the same ID fail ALREADY_EXISTS. Omission creates independent work on every call. Admission/deduplication is committed before acknowledgement and scheduling belongs to the engine even when its caller drops the acceptance RPC. This is deduplicated admission, not a claim of exactly-once physical execution.

List uses bounded keyset pagination with parent-bound opaque tokens and default 20/max 50 items, following [AIP-158](https://google.aip.dev/158). A token is not a fixed historical snapshot: newly inserted operations can alter subsequent pages. Negative page sizes or wrong-parent tokens fail. Request parameters other than the token/page size must remain consistent.

## Bed placement evidence

`PrepareBedPose` is a read-only source/pose/revision-bound proposal; Core alone
applies its pose. Required `BedPosePreparation.basis` distinguishes native
bounded support from the ADR012 tolerant model. Native bounds enclose the native
material and retain the requested no-penetration/contact uncertainty. Tolerant
values are placed model-vertex extrema, carry the exact join/profile policy,
and do not certify native extrema or physical contact. Their reported gap applies
only to that model. The workbench labels this as approximate placement rather
than presenting a native-contact receipt.

Native support uses complete native exclusion and independently feasible
boundary vertex and curve-point witnesses. Ordinary placement uses this bounded
source traversal, not adaptive whole-trim extremum refinement. When import boxes
are loose, a complete prepared chart cover (including native joint covers),
carrier coefficient bounds, spatial edge bounds and native vertices tighten
exclusion together; spatial edges alone never bound trimmed carriers. If the
complete enclosure and feasible witnesses do not close the requested uncertainty,
it selects the explicitly identified tolerant model; cancellation,
stale/bad requests and resource failure propagate. Missing faces never produce a
partial successful bed model. Saved poses remain ordinary project content and
do not confer manufacturing, physical grip or machine-readiness qualification.

## Derived color names

`Engine.NameColors` is an authenticated read-only unary query independent of project/session revision. It takes 1..64 explicitly encoded opaque sRGB8 samples and a required Exact, Nearest or Descriptive mode; unspecified/unknown modes and channels above 255 refuse. Ordered replies retain samples, uppercase hex, optional exact-match name, reference sample/name, optional single modifier, generated-label provenance and normalized OKLab distance. Exact misses return no name. The reply binds the naming algorithm version and pinned dataset digest; clients validate these plus requested mode/order/count. Distance is a numerical description, not a confidence probability or physical color qualification.

The pure implementation is `spiling-appearance::naming`, shared with native presentation components. The complete unmodified xkcd CC0 survey supplies exact/nearest names; descriptive mode uses a deliberately smaller common vocabulary, compatible survey modifiers and deterministic lightness/chroma heuristics. Near-neutrals have no inferred hue. Names never replace declared material/product identity, modify color channels, alter optical ranking or invalidate manufacturing inputs. Survey and naming versions identify presentation semantics separately from planner/emitter/format identities. See the retained dataset provenance and [license guidance](../license.md).

## Lifecycle, progress and cancellation

```text
queued -> running -> succeeded | failed
queued | running -> cancelling -> cancelled
cancelling -> succeeded | failed if completion won the race
unfinished -> interrupted on shutdown or recovery after engine failure
```

Terminal outcomes never change. Cancellation acknowledgement records intent; it does not certify that computation has stopped. Repeated cancellation is idempotent. Ordinary cancellation is a terminal Operation error with canonical CANCELLED status. Recovery/intentional shutdown interruption uses ABORTED. Worker/publication transitions serialize against cancellation; completion may legitimately win.

`CompareAppearance` produces evidence, not a Core mutation: cancellation (including while queued) stops enumeration and returns a bounded best-found report whose search status is `cancelled` and descriptor `complete=false`. The standard Operation succeeds at **report delivery**, not at completed search or matching a target; its typed report remains explicitly incomplete. The same dispatcher/worker/ACK and atomic artifact ledger publish the report. Cancellation never accepts a report/selection or overwrites durable study state; an incomplete or stale result cannot authorize acceptance. See [appearance studies](project.md#appearance-palette-studies).

Metadata contains state/version, phase, completed/total units, input revision/digest and available complete output descriptors. The diagnostic total is known; it reports chunk counts rather than invented phase percentages. WatchOperation immediately returns a current snapshot, then coalesces newer complete snapshots through bounded storage. Intermediate versions may be skipped. Registration/publication cannot lose an update; reconnect by obtaining current state rather than replaying percentage events. WaitOperation returns current state after completion or its bounded timeout, not a guaranteed terminal result.

Input revision is retained provenance only in the synthetic diagnostic, which never mutates a project. Native actions validate the active session and exact captured scene/project revision before admission/publication. Import, placement, source-backed open/save and manufacturing use core transaction authority; native objects/caches remain private. Manufacturing intent/geometry edits invalidate the active artifact, while coherent undo can restore it.

`Geometry.PrepareLayout` captures the scene and persistent project revision, current
printer/recipe, source-bound finishes/process data and explicit quantity/pins/spacing.
It is a cancellable read-only operation, including on read-only projects. The engine
decides every bed, centre and reservation-edge position for rectangular reservations
using native/finish bounds (an exact sweep per candidate row, counted for
cancellation, never a candidate budget), then requires
complete candidate compilation, independent emitted-program replay and whole-bead
containment in disjoint construction reservations. The checked terminal proposal
contains every move/copy, requested/placed totals and software verification. A best
found fill or partial fit is not a maximum-count or physical-clearance claim.
The result distinguishes build-area, plate, occurrence-width and manufacturing-work
limits; copies that are not placed are reported against the requested count.
When the complete plate is refused by this machine's manufacturing working set,
the search decides the exact boundary: the first k placed copies compile and
k+1 are refused (every existing occurrence remains included). The proposal
reports `ManufacturingBudget` with `manufacturing_refusal` naming exactly which
copies were dropped and the refusal that dropped them; attempts are counted,
not capped.
`Geometry.Execute.ApplyLayout` accepts only the exact retained proposal identity and
both captured revisions. Core atomically moves occurrences and copies their scoped
finish/process layers with fresh IDs and shared source/image assets. Apply is one
Undo transaction; preview does not mutate Current or publish its manufacturing
artifact. Ordinary Compile/Inspect/Export remain required after Apply. Fixed-dual
arrangement refuses until its explicit resource routing is qualified.

## Durable publication and limits

An exclusive file lock permits one engine per store. SQLite WAL with FULL synchronization atomically commits operation admission, complete artifact blobs and output/state references. On reopen, unfinished operations become interrupted; they are never automatically replayed. Complete outputs remain retrievable after failure/cancellation/restart. Tests establish process-kill recovery, not a physical power-loss certification.

Current common bounds are 128 retained operations, eight unfinished admitted operations and one shared execution permit. Diagnostics reserve at most 32 MiB total, with at most 64 chunks per operation and 256 KiB per diagnostic artifact. Native operations use a separate disk-backed artifact store whose capacity is the store filesystem's actual free space less a slice kept for the machine (a twentieth of the disk, at most 2 GiB); there is no fixed store total. Request-specific reservations in the engine admission path cover selected immutable outputs and are admitted against that free space less the other unfinished reservations; completion charges newly stored unique bytes. These disk reservations are not a live-RAM budget. Native metadata/output counts and session/runtime/core retention have independent bounds. No deletion/GC API is implemented. Capacity exhaustion is explicit RESOURCE_EXHAUSTED, never silent eviction or truncated success. The UI store is application-local persistent storage; CLI defaults to temporary storage unless `--store` is supplied.

Appearance import reserves no ledger output; Compare/GetAppearanceReport reserve at most1MiB for one immutable typed JSON report artifact. Original/canonical/parsed palette and history retention remain separately charged in Core, and ProjectInfo carries only bounded report metadata. No appearance fields or output change replay/manufacturing format versions.

Watch/Wait share a global budget of 16 observers; binary downloads have eight slots and two queued 16 KiB fragments per stream. Individual gRPC messages are capped at 64 KiB; HTTP/2 permits 32 streams per connection. Engine-client isolates ByteStream on its shared bulk connection so unread downloads cannot exhaust control-connection flow credit. Ledger writes and descriptor lookups use blocking tasks with short serialized transactions; download cursors use separate read-only connections, never holding the Jobs mutex across transfer waits.

Diagnostics generate synthetic triangle chunks with optional explicitly synthetic padding. Native geometry runs on a separate single worker thread, never on Tokio threads; a session-owned watchdog serializes cancellation against commit/promotion. Ordinary accepted native work has no automatic elapsed-time deadline. An explicitly selected numerical-study limit may request cancellation. User cancellation, owner loss and shutdown retain the cancellation/promotion acknowledgement grace; missing acknowledgement terminates the child. Nonpreemptible native calls cannot be reported as stopped merely because an RPC deadline expires.

Source/proof admission is engine-owned and adaptive to available system, process-container and finite address-space headroom. `SPILING_SOURCE_MEMORY_BYTES` optionally selects a positive total source/proof byte allowance; invalid values refuse startup. GeometryLimits.source_bytes describes the immutable source-object representation, not a live RAM quota. Saved return verifies actual stored bytes before sharing retained source/native/display ownership. Manufacturing jobs likewise receive a fresh working-set admission from the same headroom; `SPILING_MANUFACTURING_MEMORY_BYTES` optionally selects a positive explicit allowance and invalid values refuse startup. [ADR009](../adr/009-resource-envelope-policy.md) owns policy and remaining capacity boundaries.

Native results are staged privately. Cancellation/staleness checks and source-pin reservations precede the worker promotion decision; only its ACK permits core/scene publication. Immutable operation bytes and terminal output references then commit together in SQLite. A cancellation request arriving after ACK or after a save manifest replacement cannot turn committed success into cancelled rollback. A ledger failure after native/disk commit interrupts the engine and requires explicit reconciliation, never automatic retry. Saved sources and manufacturing bundles are reopened/replayed in a fresh native session; unfinished native operations are not resumed.

A geometry-only Scene result has no display artifacts, but Open may republish its
immutable saved manufacturing bundle. Validate those bundle descriptors and
their independent budgets even when `unique_mesh_bytes` is zero; lack of a
display mesh does not remove saved-program authority. Fresh verification still
decodes the stored bytes independently and does not trust a saved verified flag.

Rejected-source errors may carry an `ImportDiagnostic` in the typed `DomainErrorDetail`: failure kind, original source hash/name, owning source object, optional affected definition-local bounds and an optional session-local preview lease. The import remains Failed and leaves Core/history/manufacturing untouched. Only such a failure may publish mesh-only diagnostic outputs; unrelated errors cannot acquire output blobs. Preview chunks use the existing immutable artifact/ByteStream store and its retention policy, while the live scene lease expires on dismissal, replacement, mutation or restart. These outputs are audit/inspection data, never successful import provenance or project material. Malformed syntax and failures without meaningful geometry retain textual/object-level diagnostics instead of invented defect locations.

## Incremental artifacts and binary reads

Each available descriptor names complete immutable bytes as `artifacts/{sha256}`, with size, checksum and media type. Availability is published in the same SQLite transaction as its bytes. Diagnostic descriptors are append-only and bounded to 64; repeated identical outputs may share content. Native outputs are published only after their promoted result, not at worker stage time. Overall diagnostic success is distinct from partial availability. Manufacturing exports require a complete independently verified bundle; an incomplete program is never a verified build. Each published manufacturing bundle is accompanied by its program as `artifacts/{program.hash}` with media type `application/x-spiling-manufacturing-program`; clients read it by the recorded descriptor and check its exact length and SHA-256. The engine streams program frames into the stored blob rather than materializing them.

## Staged transfer

One control message is at most 64 KiB. A `Projects.Execute` or `Manufacturing.Execute` request or reply that would not fit instead carries its complete protobuf encoding as `StagedBlob {sha256,size_bytes}` in the message's `staged` oneof arm, with every other outer field empty; small messages stay inline. An `EvaluateAlternative` start stages only its intent (`intent_blob`). Senders upload with one `ByteStream.Write` to `uploads/{uuid}/blobs/{sha256}/{size_bytes}` (canonical lowercase hex, contiguous offsets, fragments within one frame, `finish_write` on the last); the engine byte-admits each upload against fresh memory headroom, checks exact size and SHA-256, and the request naming it consumes it. Staged replies are read from `blobs/{sha256}/{size_bytes}` and released after one complete read; unread staged replies may be released to admit a new one, and reading one then fails NOT_FOUND so the client repeats the request. A staged message never names another. A terminal native result larger than one frame is published in the same ledger transaction as an immutable `application/x-spiling-native-result` artifact named by `NativeOperationResult.staged`; clients read it through ByteStream and check it before interpreting the operation. Sizes are `u32` widths; memory and disk admission, not a document ceiling, bound them.

ByteStream Read uses its standard resource_name, read_offset and read_limit: zero limit reads to end; negative offsets/offsets beyond end fail OUT_OF_RANGE; negative limits fail INVALID_ARGUMENT. Stream messages carry byte fragments, not JSON/base64. Full client reads enforce the allocation bound, exact length and SHA-256. Partial-range callers must validate their range separately; a range does not redefine the artifact checksum.

Artifact retrieval uses one read-only incremental SQLite BLOB cursor per active download, off the async executor and independent of the Jobs writer mutex. The existing eight-download limit and two queued fragments plus one pending 16 KiB fragment bound reader concurrency and application read-ahead; SQLite caches and transport buffers are additional. Completion, failure or receiver closure releases the cursor, connection and download permit. An active cursor retains a WAL read snapshot; a slow download can delay checkpoint progress, so these bounds are not a transfer-duration or WAL-size ceiling. This is the operation artifact ledger, not project PTAC storage or a shared-memory transport.

The native workbench retrieves and validates bounded complete packed artifacts before GPU admission, with nonoverlapping observation and revision/session guards. Diagnostic SPLT, native SPLM meshes and SPLS sections have separate decoders. Cancellation can run independently of display transfer. Paged native metadata binds returned resource descriptors to expected packed chunk hashes, sizes and source mappings. Native artifact release frees session-derived memory; it is not operation cancellation or deletion of retained immutable output. Progressive geometric refinement/atomic replacement sets are not implemented.

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

CLI progress goes to stderr; final JSON follows child cleanup. The standard Operation terminal status is separate from RPC transport errors. Shell/CLI/native tests exercise the same generated API, not an alternate in-process engine. Native-window presentation and physical GPU support require separate evidence.
