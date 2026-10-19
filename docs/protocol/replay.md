<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# SPLR schema 5: actual commands and nominal associations

SPLR is one bounded immutable, little-endian packed artifact emitted only on explicit `report_and_motion` inspection. It represents the independent verifier's decoded actual commands, not a planner preview or emitter reconstruction. The accompanying report remains SOFTWARE VALIDATION ONLY, NOT MACHINE READY. SPLR provides no execution, hardware, collision, thermal, firmware or physical printability evidence. [Manufacturing](manufacturing.md) owns program verification and provenance.

## Admission and descriptor

`MAX_REPLAY_DISPLAY_BYTES = 32*1024*1024`, `MAX_REPLAY_EVENTS = 300004`, `REPLAY_HEADER_BYTES = 96`, `REPLAY_RECORD_BYTES = 80`. Native admission charges actual allocated Vec capacity and reserves checked exact final length before append. Maximum event count encodes24,000,416 bytes. The larger transfer allowance is intentional; native geometry proof caps are unchanged. There is no event-object array, repeated motion Vec, hot per-stroke JSON descriptor or per-segment RPC. ByteStream chunks remain at most1MiB.

`ReplayDisplayInfo` contains `schema_version:5`, canonical SHA-256 `bundle_hash,input_hash,program_hash`, bounded `verifier_version`, `frame:"right_handed_millimetres"`, event/role/layer counts and exact actual endpoint `bounds_mm`. `program_hash` hashes exact UTF-8 machine-program bytes. `ReplayDisplayDescriptor {info,resource}` uses ordinary `ArtifactView`; size is canonical decimal, name `artifacts/{sha256}`, MIME `application/x-spiling-replay-motion`. Resource SHA-256 covers all bytes; length is exactly `96 + 80*event_count`.

`ReplayDisplayDescriptor::validate_for_record` binds bundle/input hashes, scheduled layer count and combined object-plus-support deposition count to the original bundle record. Verified native result conversion additionally binds verifier version to inspection provenance, travel count and paired process count to the fresh report; metadata outputs must contain both original bundle and replay resource descriptors. `validate_replay_display(bytes,descriptor)` checks exact bytes/hash/layout/sequence/aggregates without event allocations. Descriptor admission alone cannot establish packed integrity or independent program verification.

## Header (96 bytes)

| Offset | Bytes | Value                  |
| ------ | ----- | ---------------------- |
| 0      | 4     | ASCII `SPLR`           |
| 4      | 2     | u16 schema version = 5 |
| 6      | 2     | u16 header bytes = 96  |
| 8      | 4     | u32 event count        |
| 12     | 4     | u32 record bytes = 80  |
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

## Event record (80 bytes)

| Offset | Bytes | Value                                                                                                            |
| ------ | ----- | ---------------------------------------------------------------------------------------------------------------- |
| 0      | 8     | actual decoded X, f64 millimetres                                                                                |
| 8      | 8     | actual decoded Y, f64 millimetres                                                                                |
| 16     | 8     | actual decoded Z, f64 millimetres                                                                                |
| 24     | 8     | current absolute E, f64 millimetres                                                                              |
| 32     | 8     | actual F, f64 millimetres/minute                                                                                 |
| 40     | 4     | source program line, 1-based u32                                                                                 |
| 44     | 2     | layer index u16; lifecycle sentinel 65535                                                                        |
| 46     | 1     | kind: 0 travel, 1 perimeter, 2 solid fill, 3 bottom skin, 4 top skin, 5 sparse fill, 6 lifecycle travel, 7 prime, 8 bridge, 9 support, 10 support interface, 11 retract, 12 unretract, 13 separator wall |
| 47     | 1     | flags: bit0 known start; bit1 nominal deposition association; other bits zero |
| 48     | 8     | associated expected-plan nominal bead width, f64 mm |
| 56     | 8     | associated physical interval lower, f64 mm |
| 64     | 8     | associated physical interval upper, f64 mm |
| 72     | 4     | owner: u32 regional index below256; `0xfffffffe` mandatory skin; `0xffffffff` unknown |
| 76     | 4     | reserved zero |

XYZ/E/F are finite; F is positive. Absolute E may become negative only while retracted. Source lines are strictly increasing, positive and bounded by the program-byte limit. Body layers are bounded and monotone; the first and last printed layers are schedule endpoints. Layer IDs may skip genuine support-contact-gap layers, whose strict zero-area/volume and interface/object adjacency semantics are proved by the native plan verifier, not standalone packed admission. A new printed layer begins with travel or stationary retraction. Lifecycle uses sentinel 65535, ordered positioning/prime/body/vertical lift/constant-Z park. The first endpoint has unknown start; all later starts are known. Travel preserves E. Deposition/prime requires nonzero XYZ movement and increasing E with no pending recovery. Retract/unretract requires stationary known XYZ, actual decreasing/increasing E, real layer/source/feed attribution and paired exact recovery to the prior absolute E. Neither is motion, deposition, lifecycle or prime. Fan actions are independently decoded modal process state bound to the next queued motion; they produce no fictitious packed trajectory or measured cooling claim.

The complete-job body may start at layer zero with stationary retraction after the known lifecycle prime endpoint, before its first object positioning travel and paired recovery. This exception never admits an unknown-start feeder event: diagnostic replay and the overall first packed record still start with unknown-start travel. A startup recovery without preceding body travel is not a valid first body event.

The first event is an **endpoint only**: there is no invented origin, start segment, distance or duration. Homing trajectories are not encoded or fabricated from the declared home coordinate. Later coincident travel records remain observable actual commands; they are not invalid deposition and cannot be silently removed. Layer/kind labels come from ordinal association with validated expected layer/path/lifecycle semantics. Encoded XYZ/E/F come solely from the actual decoder, preserving f64 values exactly, including accepted serialization-rounding deviations from normalized plan coordinates.

XYZ/E/F and source lines are byte-derived. Width, physical Z interval and owner are **validated expected-plan associations, not measured physical bead dimensions**. Every object/support deposition kind1–5,8–10,13 requires bit1, finite positive width and ordered finite interval with upper consistent with actual XYZ6 Z. Nominal H is upper minus lower, not movement delta or owning fine-layer thickness. A coarse SparseFill or genuine regional Bridge is still emitted at its terminal layer's Z. Nondeposition, including prime, has bit1 clear, zero bytes48–71 and unknown-owner sentinel; no bead geometry is inferred from E. Packed structural admission does not recreate native source/finish proofs or independently establish expected-path ownership.

Kind13 is a real fine open separator with its explicit nominal width/physical interval/flow, never Perimeter or a Bridge exemption. Native purpose is distinct from process role. Fresh verification binds decoded path/order/flow, separator uniqueness, actual contacts and lower anchors. A validated Combined terminal SparseFill or genuine regional Bridge may supply eventual nominal full-Z core contact to adjoining fine separators, never earlier printing-time support. Genuine Bridge also needs actual band-base gap/end-anchor/span proof. Outer clearance and strict positive nominal inner-core bonds are separate; neither packed admission nor the nominal stadium model claims adhesion strength.

## Consumer boundary

The decoder borrows DataView bytes and strided accessors, not event objects. Browser SHA-256 admission is asynchronous. Viewport batches actual deposition/travel into lines and stationary retract/recover into endpoint points, origin-rebased to f32 within0.001mm. Sparse colors expose associated nominal height/width; per-event review exposes the complete physical interval and regional owner, separately from fine-layer thickness. The checked layer view carries only bounded terminal-band combined owners and deduplicated retained-fine reason counts, not per-stroke JSON. Source-hidden replay reveals interior paths without opaque native meshes. Layer/event/travel/source-visibility controls commit only after successful bounded GPU update; a refusal preserves the previous committed view. Empty contact-gap layers honestly show no events; lifecycle appears only in all-layers view. Mesh/section/replay share active/staged CPU/GPU admission and explicit failure. Freshness remains captured-content/session bound. Display never executes or replans.

`fixtures/protocol/replay.json` and `replay-sparse.json` come from real Rust producer regressions with explicit `SPILING_UPDATE_REPLAY_FIXTURE`; ordinary tests never write them. The first covers rounded actual commands/coincident travel; the second includes genuine combined dimensions and actual lower-anchor/fine-stability verification. TypeScript consumes those exact packed bytes/descriptors/programs. A fabricated synthetic golden is not native verification evidence.
