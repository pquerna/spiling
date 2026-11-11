<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# SPLR schema 6: actual commands, tool epochs and nominal associations

SPLR is a bounded immutable little-endian artifact requested by `report_and_motion` inspection. Its coordinates, extrusion, feeds, source lines and active tool come from independent decoding of the actual program, never emitter reconstruction or a renderer's second planner. Expected-plan width, physical interval, construction owner and source/material bindings remain nominal associations, not physical measurements. Reports remain **SOFTWARE VALIDATION ONLY, NOT MACHINE READY**. [Manufacturing](manufacturing.md) owns verification and declared firmware/process assumptions.

## Admission and descriptor

The header remains 96 bytes and each record 80 bytes. Replay capacity derives from the admitted command-category counts: deposition, travel, stationary process, tool selection and four lifecycle events. `MAX_REPLAY_DISPLAY_BYTES = 96 + 80 * MAX_REPLAY_EVENTS` is a checked representation bound, not a resident allocation or a 32 MiB display cutoff. Report-only inspection allocates no motion or index. Report-and-motion writes accepted records sequentially into anonymous disk-backed compressed frames with fixed staging; sealing the header and body produces one immutable SPLR stream. Failed verification/cancellation drops unpublished backing. Whole-program acceptance and publication ACK precede display authority.

`ReplayPage` indexes raw ranges of at most `REPLAY_PAGE_EVENTS = 65_536` records within that stream. Each page carries its global `first_event`, count, SHA-256 of exact record bytes, first/last source lines, first/last body layers (`u32::MAX` for lifecycle-only pages), endpoint bounds and physical-Z span. The first page starts at ordinal zero; consecutive pages cover every event exactly once. A page is not a standalone SPLR document and has no new binary header or reset decoder state. The native protobuf carries the typed index alongside the descriptor; existing ByteStream reads retain fragments of at most 1 MiB.

Rust streaming admission checks the exact stream hash, header, every record and continuous tool/E/debt/layer state across all pages, then independently regenerates and compares the index. TypeScript admission checks the same correspondence. Valid hashes cannot authorize forged visibility bounds, physical spans, layers or ordinal gaps. Index metadata never substitutes for complete semantic admission, source construction or independent program verification. Clients retain a seekable anonymous backing and one bounded page of decoding scratch; no whole-job raw byte vector is required by native Review or CLI replay export.

`ReplayDisplayInfo` contains `schema_version:6`, canonical SHA-256 `bundle_hash,input_hash,program_hash`, `verifier_version`, `frame:"right_handed_millimetres"`, `event_count,deposition_count,travel_count,layer_count,lifecycle_count,process_count,tool_epoch_count,waste_count`, and actual endpoint `bounds_mm`. The program hash covers exact UTF-8 program bytes. `ReplayDisplayDescriptor {pages,info,resource}` uses `ArtifactView`, canonical decimal size, resource name `artifacts/{sha256}` and MIME `application/x-spiling-replay-motion`. Its hash covers every packed byte; length is exactly `96+80*event_count`.

Descriptor validation binds bundle/input identity, layers and model/support/waste accounting to the bundle record. Checked native results additionally bind workflow/material identity, fresh verifier/report and actual process/epoch counts. The two-feeder route finishes with both feeders explicitly retracted: its stationary process count is `2*retraction_count-2`, not an invented pair of final recoveries. Single-tool workflows retain paired recovery accounting. Packed Rust/TypeScript admission checks hash/layout/state/aggregates; neither a descriptor nor standalone packed admission re-proves source geometry or independently verifies machine commands.

Schema6 is an incompatible interpretation change: reserved fields now carry active tool and epoch count, and new kinds distinguish frame state and waste. Schema5 refuses; no compatibility reader or reinterpretation of an old kind exists. Project/manufacturing numerical format values are unchanged.

## Header (96 bytes)

| Offset | Bytes | Value                                         |
| ------ | ----- | --------------------------------------------- |
| 0      | 4     | ASCII `SPLR`                                  |
| 4      | 2     | u16 schema version6                           |
| 6      | 2     | u16 header bytes96                            |
| 8      | 4     | u32 event count                               |
| 12     | 4     | u32 record bytes80                            |
| 16     | 4     | u32 deposition count: model + support + waste |
| 20     | 4     | u32 travel count                              |
| 24     | 4     | u32 scheduled layer count                     |
| 28     | 4     | u32 lifecycle count                           |
| 32     | 24    | f64 minimum XYZ                               |
| 56     | 24    | f64 maximum XYZ                               |
| 80     | 8     | reserved zero                                 |
| 88     | 4     | u32 stationary retract/recover count          |
| 92     | 4     | u32 tool epoch count                          |

Counts and bounds match metadata exactly. `event_count=deposition_count+travel_count+lifecycle_count+process_count+tool_epoch_count`; waste is already included in deposition count. Lifecycle count is0 for diagnostic,4 for single-tool complete, or2 for fixed-dual final lift/park. Scheduled layers are1..4096. Fixed-dual tool epochs record actual selections and E resets. Every actual layer retains exactly one supported waste stroke from each tool before its model group, including waste-only groups. The scheduled tool order may alternate across layers: a continuing active tool begins its next layer's waste group in the same epoch, without a fictitious selection/reset or retraction debt. Waste count remains twice the layer count; stationary process count is twice the selection count minus the two final outstanding debts. Record admission checks complete per-layer service, actual tool changes, earlier waste before model work and exact feeder recovery independently of an epoch-parity assumption. The SPLR6 field meanings and byte layout remain unchanged. No trailing bytes or unknown reserved bits are admitted.

## Event record (80 bytes)

| Offset | Bytes | Value                                                                                         |
| ------ | ----- | --------------------------------------------------------------------------------------------- |
| 0      | 8     | decoded X, f64 mm                                                                             |
| 8      | 8     | decoded Y, f64 mm                                                                             |
| 16     | 8     | decoded Z, f64 mm                                                                             |
| 24     | 8     | current epoch's absolute E, f64 filament mm                                                   |
| 32     | 8     | F, f64 mm/min; zero sentinel only for non-motion tool epoch                                   |
| 40     | 4     | source program line, 1-based u32                                                              |
| 44     | 2     | u16 layer index; lifecycle sentinel65535                                                      |
| 46     | 1     | event kind below                                                                              |
| 47     | 1     | bit0 known start; bit1 nominal deposition association; remaining bits zero                    |
| 48     | 8     | associated nominal bead width, f64 mm                                                         |
| 56     | 8     | associated physical interval lower, f64 mm                                                    |
| 64     | 8     | associated physical interval upper, f64 mm                                                    |
| 72     | 4     | regional index below256; `0xfffffffd` Wall; `0xfffffffe` mandatory skin; `0xffffffff` unknown |
| 76     | 4     | u32 active tool0 or1                                                                          |

Kinds0–13 retain their meanings:0 travel,1 perimeter,2 solid fill,3 bottom skin,4 top skin,5 sparse fill,6 lifecycle travel,7 single-tool startup prime,8 bridge,9 support,10 support interface,11 retract,12 unretract,13 separator wall. **Kind14 is a tool epoch, not motion; kind15 is transition-waste deposition, not model/support.** The reference fixed-dual route refuses support/interface/separator roles; their existing single-tool meanings are unchanged.

Kind14 records the actual known-position `G92 E0` following admitted selection and native homing/transition state. E and F are zero; association bytes are zero/unknown and bit0 is clear. `Tn S1` changes logical tip position by the offset difference while physical carriage position stays fixed. No connecting line represents that frame jump. The next motion starts from the epoch endpoint. The record's source line is the actual reset line, not a fabricated tool-selection motion. Offset/temperature/barrier commands themselves are checked by native program replay rather than encoded as fictional motions.

XYZ/E/F are finite; F is positive outside epochs. Source lines never decrease and are bounded by program bytes; they repeat only between consecutive deposition records, where one arc command (G2/G3) replays as its planned edges. Travel preserves E; deposition needs known nonzero movement and positive E after recovery. Retract/recover remains stationary known XYZ. Negative absolute E may persist during retracted travel. Feeder debt survives a new numeric E origin: fixed-dual packed admission compares exact E8 ticks within its representable tick domain, not a whole-quantum recovery tolerance. Each final feeder remains explicitly retracted; recovered length is not deposited material.

Single-tool replay begins with an unknown-start endpoint, not an invented origin segment or homing trajectory. Complete single-tool positioning/prime/body/final lift/park retains its existing order. Its body can begin with stationary retraction after the known prime endpoint. Body layer indices are bounded/monotone; native verification, not packed admission, owns any genuine support-contact-gap skip. Coincident travel records remain observable commands and are not silently removed.

Every model/support/waste deposition kind1–5,8–10,13,15 requires bit1, finite positive nominal width and ordered physical interval whose upper agrees with actual XYZ6 Z. Height is upper minus lower, never movement delta or reporting layer thickness. Kind15 requires unknown model owner. Other records have zero bytes48–71 and unknown owner; no bead is inferred from E alone. Wall ownership survives genuine Bridge and open junction strokes. Per-edge width is the expected consumed edge's width, not a path average. A mixed regional/mandatory skin bridge associates each emitted portion independently; whole-path construction remains in the verified plan.

## Native closed-wall start inspection

The native workbench derives transient start markers only from fresh current
checked replay. An explicit path-entry travel supplies the actual start endpoint;
an optional stationary unretraction at that same layer/tool/position may precede
the contiguous Wall-associated deposition run, which must close exactly at that
XYZ in command coordinates. Travel, stationary process events within a run, tool epochs and
incompatible layer/tool/physical-interval associations break runs. Perimeter role
alone is insufficient; open Wall strokes have no marker, while closed Wall-owned
Bridge strokes do. Per-edge width changes do not split a path.

Markers retain entry, first-deposition and closing ordinals, source command lines,
actual XYZ, tool/role and nominal physical interval. They obey selected layer/range,
role/tool, playback and section clipping; dim surrounding-layer context adds no
markers or selected counts. Markers share the compact geometry representation used by nominal beads and
centerlines; global ordinals and predecessors remain intact across page boundaries. Visibility and marker navigation are inspection state, not project edits.
SPLR carries no face/occurrence identity, wall depth or adjacency here; these markers
do not establish cosmetic seam assignment, separation guarantees or physical sealing.
No new packed layout or persistent seam flag is introduced.

## Consumer boundary and reproduction

The TypeScript decoder borrows DataView bytes and strided accessors. Native Review uses checked seekable replay backing and compact 36-byte stroke instances: relative start/end XYZ, nominal width/height and packed role/facet flags. Centerline, bead, context and marker passes share the same immutable CPU tables and GPU buffers. Event/layer selections share one ordinal table. Shader-derived normals are numerically checked before bead admission; excess directional error retains checked centerlines. Epochs are frame breaks, never printed segments; explicit waste remains separate from model/support when travel is hidden.

CPU retention uses fresh machine/container/address-space headroom after source allocations; larger compact tables spill to one shared anonymous backing. `SPILING_REPLAY_RESIDENT_BYTES` can constrain retention for diagnostics, including zero, without refusing the job. A backpressured worker decodes pages off the UI thread. GPU allocations and uploads are staged per frame; incomplete loading is visible, and allocation failure preserves verification/export with a display diagnostic. Current rendering retains all uploaded compact GPU pages; GPU eviction and device-loss recovery are not qualified. Buffer representation checks are not claims about available VRAM. Layer/role/tool/playback/section changes reuse immutable geometry without planner or verification calls. These remain nominal command views, not measured bead shape, calibrated optical color or a physical swept-tool certificate.

Cross-language fixtures are produced by real Rust emission and independent byte replay, not a TypeScript invented verified model:

- `fixtures/protocol/replay.json`: command rounding and coincident travel.
- `replay-sparse.json`: genuinely combined nominal dimensions and the fine/lower-anchor obligations.
- `replay-fixed-dual.json`: actual tool epochs, per-feeder debt, source-associated model deposition and distinct supported waste.
- `replay-waste-only.json`: the same supported tool epochs/debt with no model work from tool zero.

Regenerate only explicitly with `SPILING_UPDATE_REPLAY_FIXTURE=1 cargo test --locked -p spiling-manufacturing --lib FILTER`, using filters `actual_motion_inspection_rounding_coincident_travel_and_fixture`, `coarse_sparse_dimensions_are_verified_and_packed`, `fixed_dual_actual_epochs_repay_feeders_and_separate_waste_from_model`, and `fixed_dual_waste_only_groups_keep_exact_debt_and_zero_model_accounting`. Ordinary tests check drift without writing. These fixed logical fixture identities are interoperability evidence, not a native geometry, GPU or physical printer qualification.
