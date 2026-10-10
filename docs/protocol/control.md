<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Version-four local-pipe project, geometry and manufacturing protocol

Rust `spiling-contracts` is executable schema authority. Its generator emits all project/geometry/manufacturing/job DTOs and frozen constants into `packages/protocol/src/generated.ts`. Generate with `cargo run --locked -p spiling-contracts --bin generate`; append `-- --check` for byte drift. Never hand-maintain or independently format generated output.

The engine uses inherited stdin/stdout pipes. Stdout contains only frames; stderr contains bounded JSON-line diagnostics. Core owns project transactions/storage; one worker composes native STEP inspection, software-only planar compilation and independent emitted-program replay. There is no network listener, general STEP assembly importer or printer execution. This is a clean pre-alpha cutover without previous-protocol or old-project-format decoding.

## Framing

The 16-byte `SPLG` header contains little-endian fields:

| Offset | Bytes | Meaning                                                                             |
| ------ | ----- | ----------------------------------------------------------------------------------- |
| 0      | 4     | ASCII `SPLG`                                                                        |
| 4      | 2     | Protocol version 4                                                                  |
| 6      | 2     | Kind: 1 JSON control, 2 synthetic triangle, 3 geometry chunk, 4 manufacturing chunk |
| 8      | 4     | Nonzero strictly increasing request ID                                              |
| 12     | 4     | Payload bytes, excluding header                                                     |

Control is at most 65,536 bytes, diagnostic/manufacturing chunks at most 4,194,304 bytes and geometry chunks at most 1,048,576 bytes. Validate magic/version/kind/correlation/length before payload allocation. Reject truncation, wrong IDs, unknown kinds and excess lengths; EOF between frames is clean, EOF within a frame is malformed. Only control frames are requests. One request/chunk may be outstanding; no unsolicited events or unbounded queues. Manufacturing chunk exchanges have typed metadata followed by exact raw JSON bytes; [manufacturing.md](manufacturing.md) defines assembly/hash/schema validation.

## Negotiation and diagnostics

Control JSON uses strict snake_case `type` tags. The first request is `{"type":"hello","protocol_version":4,"client_build":"0.1.0"}`. Hello responds flatly with protocol/build identity, observed child PID, fresh UUID `session_id`, `max_control_bytes`, `max_binary_bytes`, structured `kernel: {name,version,revision}`, implemented `geometry_capabilities`, `project_capabilities`, `manufacturing_capabilities` and `geometry_limits`. Clients validate PID, version and frozen limits. Capability names describe implemented software paths, never physical printer/platform certification.

Geometry capabilities are `step_planar_cylindrical_v1`, `multi_instance_scene_v1`, `mesh_chunks_v1` and `native_plane_section_v1`. These are bounded capabilities, not universal STEP or physical-platform certification.

Project capabilities are `recoverable_projects_v2`, `project_transactions_v1` and `project_read_only_v1`. [Project format and durability](project.md) governs their storage envelope; declarations do not certify every filesystem/platform or recover arbitrary unsaved edits.

Manufacturing capabilities are `planar_software_compile_v1`, `independent_program_replay_v1` and `immutable_bundle_chunks_v1`; [software manufacturing](manufacturing.md) defines the namespace, explicit profiles/recipes and exclusions. No physical printer support is negotiated.

`ping` → `pong`, `triangle` → one kind-2 synthetic frame, and `shutdown` → `bye` followed by successful exit remain explicit diagnostics/lifecycle operations. Triangle is never an import fallback and the workbench starts with an empty scene.

Hello occurs once. Version mismatch returns `upgrade_required` and exits nonzero; frame-version mismatch is fatal framing with stderr only. Older protocols and the next version are rejected. The CLI override changes the hello payload, not the known framing envelope.

## Project control

Requests are `{"type":"project","command":{...}}`; responses use the same outer tag and generated `ProjectResponse`. Source assets and rigid occurrence state belong to core, not the dispatcher, CLI or UI. ProjectId and ProjectRevision are persistent; SceneRevision is a distinct session-scoped publication epoch used by geometry/display/job references. Every scene replacement advances that epoch, including opening an older checkpoint. Undo/redo advances authoring revision rather than restoring an old revision or allocator. Save changes neither revision and preserves session undo.

| Operation       | Required fields after `op`                                                               | Result                                |
| --------------- | ---------------------------------------------------------------------------------------- | ------------------------------------- |
| `get`           | session_id                                                                               | status with ProjectInfo               |
| `new`           | session_id, base_revision, discard_changes                                               | scene_changed with project info/scene |
| `open`          | session_id, base_revision, NativePath path, read_only, recover_previous, discard_changes | job_accepted                          |
| `save`          | session_id, base_revision, optional NativePath target                                    | job_accepted                          |
| `undo` / `redo` | session_id, base_revision                                                                | scene_changed with project info/scene |

An unattached project edits in memory; first save creates a new owned directory. Normal save uses the attached writer storage. Dirty replacement requires explicit discard intent. Read-only open captures one committed snapshot without the writer lock and refuses edits/save/undo/redo. Explicit recovery loads the validated previous checkpoint and reports recovered/dirty; normal open never silently falls back. Original source paths, BREP, meshes, session handles and undo history are not manifest authority.

Open/save run as bounded asynchronous worker jobs. Failed/cancelled open preserves the entire old project/native scene and writer ownership. Save writes immutable source assets before atomic manifest publication. Cancellation can win before the durable commit gate, never roll back a committed file. Authoring is serialized against project open/save while status, Ping and cancellation remain available.

`ProjectInfo.dirty` compares authoring content with the last confirmed checkpoint, not revision/allocator counters. `saved_revision` records the confirmed checkpoint revision; save preserves session undo history. A directory-sync failure after manifest publication attaches the actual destination but fails the job with typed IO, leaves dirty state and sets `save_uncertain`. Clients query status before offering further action; they do not claim rollback, a clean save, cancellation or automatically retry an uncertain committed write.

`ProjectSaved` job results prove manifest publication, not successful durability by themselves. A postcommit synchronization failure reports `status: failed`, typed IO and a `ProjectSaved` result whose info is dirty/uncertain; precommit failures have no result. Native clients use that job-specific receipt to settle the pending checkpoint route. A sticky uncertainty flag from an earlier write cannot prove that a later failed Save As committed.

ProjectError codes distinguish malformed/incompatible project, missing/corrupt source, writer contention, read-only/dirty/stale state, missing saved path, exhausted undo/redo/resources, cancellation and IO. Messages are bounded to 2,048 UTF-8 bytes. Geometry mutations failing core validation return `GeometryResponse::ProjectError`; asynchronous failures use tagged `JobError` (`domain: geometry|project|manufacturing`). All domain errors remain recoverable in the shared client.

## Manufacturing control

Requests are `{"type":"manufacturing","command":{...}}`; responses are `{"type":"manufacturing","response":{...}}`. Get/intent edits/compile/inspect/chunk pulls use the existing session and bounded job service, not a second scheduler. Intent/compile bases are persistent ProjectRevision, not SceneRevision. Full command/result tables, chunk framing and mandatory independent replay are in [manufacturing.md](manufacturing.md).

## Geometry control

Requests are `{"type":"geometry","command":{...}}`; commands use strict snake_case `op`. Control responses are `{"type":"geometry","response":{...}}`, where the inner `type` is a generated `GeometryResponse` variant. `ReadArtifactChunk` instead returns one kind-3 frame, or a typed recoverable control error. Shell generic control rejects source import and binary reads: source admission is token-only and chunks use the raw binary invoke.

| Operation                | Required fields after `op`                                       | Result                                              |
| ------------------------ | ---------------------------------------------------------------- | --------------------------------------------------- |
| `import_part`            | session_id, base_revision, NativePath source, initial_pose       | job_accepted                                        |
| `add_instance`           | session_id, base_revision, definition_id, pose                   | scene_changed                                       |
| `set_instance_pose`      | session_id, base_revision, occurrence_id, pose                   | scene_changed                                       |
| `remove_instance`        | session_id, base_revision, occurrence_id                         | scene_changed                                       |
| `get_scene`              | session_id                                                       | scene                                               |
| `get_scene_page`         | session_id, revision, kind (`definitions`/`occurrences`), offset | scene_page, ≤64 rows, next_offset                   |
| `get_face_index_page`    | session_id, definition_id, offset                                | face_index_page, ≤256 rows, next_offset             |
| `inspect_face`           | reference: FaceRef                                               | face_inspection with native carrier/pose/provenance |
| `start_section`          | session_id, base_revision, scene-mm plane                        | job_accepted                                        |
| `get_job` / `cancel_job` | session_id, job_id                                               | job                                                 |
| `get_artifact_page`      | session_id, artifact_id, kind (`chunks`/`loops`), offset         | artifact_page, ≤64 rows, next_offset                |
| `read_artifact_chunk`    | session_id, artifact_id, chunk_index                             | one geometry chunk or typed error                   |
| `release_artifact`       | session_id, artifact_id                                          | released                                            |

Definitions are immutable source/profile hashes. Canonical source-face IDs are decimal `step:<entity>` strings, including IDs beyond JavaScript integer precision. Nonzero occurrence IDs and their monotonic next-free allocator persist with the project; job/artifact handles and scene epochs are session-scoped and never wrap. Occurrences are proper finite mm/f64 rigid poses; no scaling, shear or reflection. `FaceRef` includes session, scene revision, occurrence, definition and source face; stale references are rejected rather than remapped.

Import captures a bounded regular file through one native-path handle, hashes imported bytes and detects size/mtime changes. Captured original bytes become an immutable core source asset; exactly equal retained source bytes reuse native definitions and mesh artifacts. Successful import atomically commits an authoring transaction; a failed/cancelled import leaves core/history/scene unchanged. Multi-file admissions are sequential: earlier successful parts remain if a later part fails. Final occurrence removal hides its definition from current scene pages; bounded undo/redo history may still pin its immutable source/native mesh. Encoded consumer leases remain separate.

## Jobs, publication and ownership

One bounded worker owns kernel objects and asynchronous project IO; core is the sole project-state writer. The dispatcher owns a derived view epoch, face/mesh caches, jobs and immutable encoded artifacts. At most one engine job is active; other job admission returns `busy`. Status/cancel controls do not execute heavy native work inside a five-second client exchange.

EngineJob status is queued/running/cancelling/completed/failed/cancelled, with bounded stage and optional Scene, Section, ProjectSaved, ManufacturingCompiled or ManufacturingVerified result. JobError is tagged geometry/project/manufacturing. Poll every 100 ms. Staged native imports/opens/compiles promote/discard through a decision and worker acknowledgement; core/view/artifact publication follows successful ACK. Stale inputs/cancellation cannot publish manufacturing artifacts. Durable saves gate cancellation before manifest replacement and must report committed state afterward, never false rollback.

The independent watchdog requests cancellation 60 seconds after acceptance. Explicit cancel starts the same grace earlier. If no terminal/promotion ACK arrives ten seconds after the first cancellation request, the engine logs `job_cancel_deadline` and exits 70. Pipe EOF/interruption requires a new session; it is not successful cancellation. Upstream calls remain nonpreemptible until cooperative boundaries.

Current occurrences and bounded undo/redo history pin retained definitions; one consumer lease per published artifact pins encoded bytes, never native objects. Page reads do not create leases. Release is idempotent for bounded known released handles. The latest 64 terminal jobs do not retain encoded bytes. Empty artifacts still consume bounded handle metadata; at most 96 leased artifact handles are retained. Safe-boundary native reclamation uses the latest replaceable retained-definition snapshot, not an unbounded cleanup queue. Shutdown cancels work and releases memory/native ownership and writer locks; clients/shell kill and reap on interrupted exit. Reopen rebuilds native caches from committed source assets, never implicitly replays unsaved authoring or machine operations.

## Frozen support and failure

Limits: source/file 16 MiB; live/staged source 64 MiB; 100,000 entities/file; 50,000 native faces; 32 definitions; 256 occurrences; one million unique display vertices and triangles each; unique scene mesh 64 MiB; section 16 MiB; live/staged engine encoded artifacts 128 MiB; geometry chunk 1 MiB; combined active/staged display buffers 128 MiB; complete closed loop 40,000 points. Occurrences do not multiply unique geometry counters. Admission/packing budgets include staging and unreclaimed native ownership; encoded staging may be reserved conservatively. Kernel-private allocation is not claimed allocator-hard-capped; measured native RSS is separate evidence.

Recoverable domain codes are invalid_geometry, unsupported_geometry, unsupported_units, source_io, source_changed, invalid_pose, stale_revision, unknown_handle, busy, resource_limit, cancelled, degenerate_section and kernel_failure. Messages are at most 1,024 UTF-8 bytes. Invalid pose/plane domain input remains recoverable despite checked DTO construction. Typed errors preserve healthy session/PID and prior scene.

Corrupt/malformed/unknown control or framing, correlation/version failure, worker panic/disconnect, timeout and aborted in-flight exchanges remain fatal and are killed/reaped. EngineClient geometry returns typed domain responses; binary reads expose a nonfatal typed Geometry error. Local preflight rejects oversized/invalid commands before writing any partial exchange. Each complete transport exchange and successful shutdown/observed exit has a five-second deadline; heavy jobs are polled, never hidden in an exchange. Never abort a partial pipe read to simulate cancellation.

## Synthetic triangle payload

This is a diagnostic display triangle, **not** a BREP or manufacturing artifact. The 64-byte payload has a 16-byte header:

| Offset | Bytes | Meaning                                        |
| ------ | ----- | ---------------------------------------------- |
| 0      | 4     | ASCII `SPLT`                                   |
| 4      | 2     | Schema version 1                               |
| 6      | 2     | Reserved, exactly 0                            |
| 8      | 4     | Vertex count 3                                 |
| 12     | 4     | Index count 3                                  |
| 16     | 36    | Nine little-endian float32 position components |
| 52     | 12    | Three little-endian u32 indices                |

Positions are `[-0.75, -0.6, 0, 0.75, -0.6, 0, 0, 0.75, 0]`; indices are `[0, 1, 2]`. Validate magic, schema, reserved bits, configured count limits, exact size using checked arithmetic, finite coordinates, triangle index count, and every index against vertex count before constructing views or GPU buffers. The canonical hex fixture is [`fixtures/protocol/triangle.json`](../../fixtures/protocol/triangle.json); its adjacent license sidecar records original authorship.

## Real operator surface

```sh
spiling-cli diagnose --engine /path/to/spiling-engine
spiling-cli triangle --engine /path/to/spiling-engine --output triangle.bin
spiling-cli diagnose --engine /path/to/spiling-engine --protocol-version 5
spiling-cli geometry --scene fixtures/geometry/scenes/two-parts.scene.json --section 0,0,4:0,0,1 --out NEW_DIRECTORY --engine /path/to/spiling-engine
spiling-cli geometry --source PART.step --source OTHER.step --engine /path/to/spiling-engine
```

Scene schema 1 contains parts with unique key/path/pose and instances referring to admitted part keys. Paths resolve relative to the recipe; it is evaluation input, not a saved project. Native --source paths preserve OsString bytes/code units. Recipe/numeric/pose errors fail before process launch. Both output and metadata-only runs fetch and independently validate every unique chunk. Output exclusively creates a new directory/files, writes completion metadata last and removes only command-owned output on failure without following replacements.

Final stdout follows successful engine shutdown; geometry reports source/definition/occurrence mapping, bounds/faces/unique bytes/chunks and optional native per-occurrence section measurements. Diagnostics remain labelled synthetic. Domain/import/output/transport failures are stderr errors, never successful partial completion. Canonical numerical, real-child and desktop evidence is recorded in [geometry evidence](../quality/geometry-evidence.md); wire tests and compilation do not establish physical support.
