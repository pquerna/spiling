<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# SPLR schema 3: actual verified program display

SPLR is one bounded immutable, little-endian packed artifact emitted only on explicit `report_and_motion` inspection. It represents the independent verifier's decoded actual commands, not a planner preview or emitter reconstruction. The accompanying report remains SOFTWARE VALIDATION ONLY, NOT MACHINE READY. SPLR provides no execution, hardware, collision, thermal, firmware or physical printability evidence. [Manufacturing](manufacturing.md) owns program verification and provenance.

## Admission and descriptor

`MAX_REPLAY_DISPLAY_BYTES = 16*1024*1024`, `MAX_REPLAY_EVENTS = 300004`, `REPLAY_HEADER_BYTES = 96`, `REPLAY_RECORD_BYTES = 48`. Native admission charges actual allocated Vec capacity and reserves checked exact final length before any record append. At maximum event count, the exact byte length is 14,400,288. There is no event-object array, repeated motion Vec, separate parser or per-segment RPC. Artifacts are streamed through ByteStream in chunks no larger than 1 MiB.

`ReplayDisplayInfo` contains `schema_version:3`, canonical SHA-256 `bundle_hash,input_hash,program_hash`, bounded `verifier_version`, `frame:"right_handed_millimetres"`, `event_count,deposition_count,travel_count,lifecycle_count,process_count,layer_count`, and exact actual endpoint `bounds_mm:{min:[f64;3],max:[f64;3]}`. `program_hash` hashes exact UTF-8 machine-program bytes. `ReplayDisplayDescriptor {info,resource}` uses the ordinary `ArtifactView {name,size_bytes,sha256,media_type}`; size is a canonical decimal string, name is `artifacts/{sha256}`, and MIME is `application/x-spiling-replay-motion`. The resource hash covers all SPLR bytes; length is exactly `96 + 48*event_count`.

`ReplayDisplayDescriptor::validate_for_record` binds bundle/input hashes, scheduled layer count and combined object-plus-support deposition count to the original bundle record. Verified native result conversion additionally binds verifier version to inspection provenance, travel count and paired process count to the fresh report; metadata outputs must contain both original bundle and replay resource descriptors. `validate_replay_display(bytes,descriptor)` checks exact bytes/hash/layout/sequence/aggregates without event allocations. Descriptor admission alone cannot establish packed integrity or independent program verification.

## Header (96 bytes)

| Offset | Bytes | Value                  |
| ------ | ----- | ---------------------- |
| 0      | 4     | ASCII `SPLR`           |
| 4      | 2     | u16 schema version = 3 |
| 6      | 2     | u16 header bytes = 96  |
| 8      | 4     | u32 event count        |
| 12     | 4     | u32 record bytes = 48  |
| 16     | 4     | u32 deposition count   |
| 20     | 4     | u32 travel count       |
| 24     | 4     | u32 layer count        |
| 28     | 4     | u32 lifecycle count    |
| 32     | 24    | f64 minimum XYZ        |
| 56     | 24    | f64 maximum XYZ        |
| 80     | 8     | reserved zero          |
| 88     | 4     | u32 process count      |
| 92     | 4     | reserved zero          |

Header counts and bounds exactly match metadata. `event_count = deposition_count + travel_count + lifecycle_count + process_count`; deposition, travel and process counts are each bounded by 100000, lifecycle count is 0 or 4, and scheduled layer count is 1..4096. Packed deposition includes object and support XYZ extrusion, excluding prime and recovered extrusion. No trailing bytes, old schema or unknown reserved bits are accepted.

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
| 46     | 1     | kind: 0 travel, 1 perimeter, 2 solid fill, 3 bottom skin, 4 top skin, 5 sparse fill, 6 lifecycle travel, 7 prime, 8 bridge, 9 support, 10 support interface, 11 retract, 12 unretract |
| 47     | 1     | flags: bit 0 known start; every other bit zero                                                                   |

XYZ/E/F are finite; F is positive. Absolute E may become negative only while retracted. Source lines are strictly increasing, positive and bounded by the program-byte limit. Body layers are bounded and monotone; the first and last printed layers are schedule endpoints. Layer IDs may skip genuine support-contact-gap layers, whose strict zero-area/volume and interface/object adjacency semantics are proved by the native plan verifier, not standalone packed admission. A new printed layer begins with travel or stationary retraction. Lifecycle uses sentinel 65535, ordered positioning/prime/body/vertical lift/constant-Z park. The first endpoint has unknown start; all later starts are known. Travel preserves E. Deposition/prime requires nonzero XYZ movement and increasing E with no pending recovery. Retract/unretract requires stationary known XYZ, actual decreasing/increasing E, real layer/source/feed attribution and paired exact recovery to the prior absolute E. Neither is motion, deposition, lifecycle or prime. Fan actions are independently decoded modal process state bound to the next queued motion; they produce no fictitious packed trajectory or measured cooling claim.

The complete-job body may start at layer zero with stationary retraction after the known lifecycle prime endpoint, before its first object positioning travel and paired recovery. This exception never admits an unknown-start feeder event: diagnostic replay and the overall first packed record still start with unknown-start travel. A startup recovery without preceding body travel is not a valid first body event.

The first event is an **endpoint only**: there is no invented origin, start segment, distance or duration. Homing trajectories are not encoded or fabricated from the declared home coordinate. Later coincident travel records remain observable actual commands; they are not invalid deposition and cannot be silently removed. Layer/kind labels come from ordinal association with validated expected layer/path/lifecycle semantics. Encoded XYZ/E/F come solely from the actual decoder, preserving f64 values exactly, including accepted serialization-rounding deviations from normalized plan coordinates.

## Consumer boundary

The decoder borrows DataView bytes and strided accessors rather than event objects. Browser SHA-256 admission is asynchronous. Viewport batches all deposition/travel roles into lines and stationary retraction/recovery into endpoint points, origin-rebased to f32 within 0.001 mm. Layer/event/travel controls change only draw visibility; selecting an empty contact-gap layer honestly shows no events. Layer-free lifecycle appears only in all-layers view. Travel visibility hides travel, not prime or stationary extrusion. Mesh, sections and replay share combined active/staged CPU/GPU admission, atomic replacement and explicit GPU failure. Freshness remains the workbench's captured-revision/resource-binding responsibility. Display controls never execute or replan.

`fixtures/protocol/replay.json` is generated by the real Rust regression `actual_motion_inspection_rounding_coincident_travel_and_fixture` only when `SPILING_UPDATE_REPLAY_FIXTURE` is explicitly set. It contains packed hex bytes, the descriptor, actual positions and the actual emitted program. Ordinary tests never write fixtures; a fabricated synthetic golden must not be presented as native verification evidence.
