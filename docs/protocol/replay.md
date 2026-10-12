<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# SPLR schema 2: actual verified program display

SPLR is one bounded immutable, little-endian packed artifact emitted only on explicit `report_and_motion` inspection. It represents the independent verifier's decoded actual commands, not a planner preview or emitter reconstruction. The accompanying report remains SOFTWARE VALIDATION ONLY, NOT MACHINE READY. SPLR provides no execution, hardware, collision, thermal, firmware or physical printability evidence. [Manufacturing](manufacturing.md) owns program verification and provenance.

## Admission and descriptor

`MAX_REPLAY_DISPLAY_BYTES = 16*1024*1024`, `MAX_REPLAY_EVENTS = 200004`, `REPLAY_HEADER_BYTES = 96`, `REPLAY_RECORD_BYTES = 48`. Native admission charges actual allocated Vec capacity and reserves checked exact final length before any record append. At maximum event count, the exact byte length is 9,600,288. There is no event-object array, repeated motion Vec, separate parser or per-segment RPC. Artifacts are streamed through ByteStream in chunks no larger than 1 MiB.

`ReplayDisplayInfo` contains `schema_version:2`, canonical SHA-256 `bundle_hash,input_hash,program_hash`, bounded `verifier_version`, `frame:"right_handed_millimetres"`, `event_count,deposition_count,travel_count,lifecycle_count,layer_count`, and exact actual endpoint `bounds_mm:{min:[f64;3],max:[f64;3]}`. `program_hash` hashes exact UTF-8 machine-program bytes. `ReplayDisplayDescriptor {info,resource}` uses the ordinary `ArtifactView {name,size_bytes,sha256,media_type}`; size is a canonical decimal string, name is `artifacts/{sha256}`, and MIME is `application/x-spiling-replay-motion`. The resource hash covers all SPLR bytes; length is exactly `96 + 48*event_count`.

`ReplayDisplayDescriptor::validate_for_record` binds `bundle_hash,input_hash,layer_count,deposition_count,travel_count` to the original bundle record's hash/input/summary. Verified native result conversion additionally binds verifier version to inspection provenance and all counts, including lifecycle, to the fresh report; metadata outputs must contain both original bundle and replay resource descriptors. `validate_replay_display(bytes,descriptor)` checks structural descriptor admission, exact bytes/hash/layout/sequence/aggregates without allocating an event index. Descriptor validation alone is not packed-byte or independent program verification.

## Header (96 bytes)

| Offset | Bytes | Value                  |
| ------ | ----- | ---------------------- |
| 0      | 4     | ASCII `SPLR`           |
| 4      | 2     | u16 schema version = 2 |
| 6      | 2     | u16 header bytes = 96  |
| 8      | 4     | u32 event count        |
| 12     | 4     | u32 record bytes = 48  |
| 16     | 4     | u32 deposition count   |
| 20     | 4     | u32 travel count       |
| 24     | 4     | u32 layer count        |
| 28     | 4     | u32 lifecycle count    |
| 32     | 24    | f64 minimum XYZ        |
| 56     | 24    | f64 maximum XYZ        |
| 80     | 16    | reserved zero          |

Header counts and bounds exactly match descriptor metadata. Body counts are positive, `event_count = deposition_count + travel_count + lifecycle_count`, deposition and travel are each at most 100000, lifecycle count is 0 or 4, and `1 <= layer_count <= 4096`. Every layer must contain deposition; therefore layer count cannot exceed deposition count. Priming is not body deposition. No trailing bytes or unknown schema/reserved bits are accepted.

## Event record (48 bytes)

| Offset | Bytes | Value                                                                                                            |
| ------ | ----- | ---------------------------------------------------------------------------------------------------------------- |
| 0      | 8     | actual decoded X, f64 millimetres                                                                                |
| 8      | 8     | actual decoded Y, f64 millimetres                                                                                |
| 16     | 8     | actual decoded Z, f64 millimetres                                                                                |
| 24     | 8     | current absolute E, f64 millimetres                                                                              |
| 32     | 8     | actual F, f64 millimetres/minute                                                                                 |
| 40     | 4     | source program line, 1-based u32                                                                                 |
| 44     | 2     | layer index u16; lifecycle sentinel 65535                                                                        |
| 46     | 1     | kind: 0 travel, 1 perimeter, 2 solid fill, 3 bottom skin, 4 top skin, 5 sparse fill, 6 lifecycle travel, 7 prime |
| 47     | 1     | flags: bit 0 known start; every other bit zero                                                                   |

XYZ/E/F are finite; F is positive and E nonnegative. Source lines are positive, strictly increasing and at most the 16 MiB program-byte limit. Body layer indices are monotone, consecutive and below layer count; lifecycle records use sentinel 65535 and do not belong to a layer. A new body layer starts with travel, and every completed layer contains deposition. Diagnostic display begins with body travel at layer zero; complete display begins with lifecycle positioning then prime, followed by all body layers and exactly a vertical clearance lift then constant-Z XY park. First event has `known_start=false`; every later event has `known_start=true`. Travel leaves E unchanged, including zero-distance coincident travel. Object deposition and prime strictly increase E and change position. Exact aggregate counts and endpoint min/max bounds are recomputed during admission.

The first event is an **endpoint only**: there is no invented origin, start segment, distance or duration. Homing trajectories are not encoded or fabricated from the declared home coordinate. Later coincident travel records remain observable actual commands; they are not invalid deposition and cannot be silently removed. Layer/kind labels come from ordinal association with validated expected layer/path/lifecycle semantics. Encoded XYZ/E/F come solely from the actual decoder, preserving f64 values exactly, including accepted serialization-rounding deviations from normalized plan coordinates.

## Consumer boundary

The protocol decoder uses a borrowed DataView and strided accessors rather than materializing per-event objects. Browser SHA-256 admission is asynchronous. The viewport batches segments by kind/chunk, explicitly represents the first marker, origin-rebases to f32 and rejects any projection with error over 0.001 mm. Layer/event/travel controls change draw visibility, never replan or regenerate packed geometry. Layer-free lifecycle is visible only in the all-layers view; travel visibility hides lifecycle travel but not priming deposition. Mesh, sections and replay share the same combined active/staged CPU/GPU admission; no uncharged independent viewer or silent renderer/GPU fallback exists. Scene staging replacement is atomic, and freshness remains the workbench/service responsibility, not the viewport's.

`fixtures/protocol/replay.json` is generated by the real Rust regression `actual_motion_inspection_rounding_coincident_travel_and_fixture` only when `SPILING_UPDATE_REPLAY_FIXTURE` is explicitly set. It contains packed hex bytes, the descriptor, actual positions and the actual emitted program. Ordinary tests never write fixtures; a fabricated synthetic golden must not be presented as native verification evidence.
