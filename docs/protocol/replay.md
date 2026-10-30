<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# SPLR schema 6: actual commands, tool epochs and nominal associations

SPLR is a bounded immutable little-endian artifact requested by `report_and_motion` inspection. Its coordinates, extrusion, feeds, source lines and active tool come from independent decoding of the actual program, never emitter reconstruction or a renderer's second planner. Expected-plan width, physical interval, construction owner and source/material bindings remain nominal associations, not physical measurements. Reports remain **SOFTWARE VALIDATION ONLY, NOT MACHINE READY**. [Manufacturing](manufacturing.md) owns verification and declared firmware/process assumptions.

## Admission and descriptor

The header remains96 bytes and each record80 bytes. `MAX_REPLAY_EVENTS=300004` and `MAX_REPLAY_DISPLAY_BYTES=32*1024*1024`; maximum count encodes24,000,416 bytes. Model, support and transition waste share the100000-deposition ceiling; travels and stationary process events each have a100000 ceiling. Tool epochs have a50000 ceiling and consume the same aggregate event/byte budget, not an additional allocation allowance. ByteStream chunks remain at most1MiB. Native admission reserves checked capacity once; report-only inspection allocates no replay or event-object array.

`ReplayDisplayInfo` contains `schema_version:6`, canonical SHA-256 `bundle_hash,input_hash,program_hash`, `verifier_version`, `frame:"right_handed_millimetres"`, `event_count,deposition_count,travel_count,layer_count,lifecycle_count,process_count,tool_epoch_count,waste_count`, and actual endpoint `bounds_mm`. The program hash covers exact UTF-8 program bytes. `ReplayDisplayDescriptor {info,resource}` uses `ArtifactView`, canonical decimal size, resource name `artifacts/{sha256}` and MIME `application/x-spiling-replay-motion`. Its hash covers every packed byte; length is exactly `96+80*event_count`.

Descriptor validation binds bundle/input identity, layers and model/support/waste accounting to the bundle record. Checked native results additionally bind workflow/material identity, fresh verifier/report and actual process/epoch counts. The two-feeder route finishes with both feeders explicitly retracted: its stationary process count is `2*retraction_count-2`, not an invented pair of final recoveries. Single-tool workflows retain paired recovery accounting. Packed Rust/TypeScript admission checks hash/layout/state/aggregates; neither a descriptor nor standalone packed admission re-proves source geometry or independently verifies machine commands.

Schema6 is an incompatible interpretation change: reserved fields now carry active tool and epoch count, and new kinds distinguish frame state and waste. Schema5 refuses; no compatibility reader or reinterpretation of an old kind exists. Project/manufacturing numerical format values are unchanged.

## Header (96 bytes)

| Offset | Bytes | Value |
| --- | --- | --- |
| 0 | 4 | ASCII `SPLR` |
| 4 | 2 | u16 schema version6 |
| 6 | 2 | u16 header bytes96 |
| 8 | 4 | u32 event count |
| 12 | 4 | u32 record bytes80 |
| 16 | 4 | u32 deposition count: model + support + waste |
| 20 | 4 | u32 travel count |
| 24 | 4 | u32 scheduled layer count |
| 28 | 4 | u32 lifecycle count |
| 32 | 24 | f64 minimum XYZ |
| 56 | 24 | f64 maximum XYZ |
| 80 | 8 | reserved zero |
| 88 | 4 | u32 stationary retract/recover count |
| 92 | 4 | u32 tool epoch count |

Counts and bounds match metadata exactly. `event_count=deposition_count+travel_count+lifecycle_count+process_count+tool_epoch_count`; waste is already included in deposition count. Lifecycle count is0 for diagnostic,4 for single-tool complete, or2 for fixed-dual final lift/park. Scheduled layers are1..4096. Fixed-dual epochs alternate tool0/tool1 on every actual layer, with one explicit waste stroke followed by zero or more model strokes in each epoch. A waste-only epoch retains the same transition, exact feeder-debt and supported-waste obligations. No trailing bytes or unknown reserved bits are admitted.

## Event record (80 bytes)

| Offset | Bytes | Value |
| --- | --- | --- |
| 0 | 8 | decoded X, f64 mm |
| 8 | 8 | decoded Y, f64 mm |
| 16 | 8 | decoded Z, f64 mm |
| 24 | 8 | current epoch's absolute E, f64 filament mm |
| 32 | 8 | F, f64 mm/min; zero sentinel only for non-motion tool epoch |
| 40 | 4 | source program line, 1-based u32 |
| 44 | 2 | u16 layer index; lifecycle sentinel65535 |
| 46 | 1 | event kind below |
| 47 | 1 | bit0 known start; bit1 nominal deposition association; remaining bits zero |
| 48 | 8 | associated nominal bead width, f64 mm |
| 56 | 8 | associated physical interval lower, f64 mm |
| 64 | 8 | associated physical interval upper, f64 mm |
| 72 | 4 | regional index below256; `0xfffffffd` Wall; `0xfffffffe` mandatory skin; `0xffffffff` unknown |
| 76 | 4 | u32 active tool0 or1 |

Kinds0–13 retain their meanings:0 travel,1 perimeter,2 solid fill,3 bottom skin,4 top skin,5 sparse fill,6 lifecycle travel,7 single-tool startup prime,8 bridge,9 support,10 support interface,11 retract,12 unretract,13 separator wall. **Kind14 is a tool epoch, not motion; kind15 is transition-waste deposition, not model/support.** The reference fixed-dual route refuses support/interface/separator roles; their existing single-tool meanings are unchanged.

Kind14 records the actual known-position `G92 E0` following admitted selection and native homing/transition state. E and F are zero; association bytes are zero/unknown and bit0 is clear. `Tn S1` changes logical tip position by the offset difference while physical carriage position stays fixed. No connecting line represents that frame jump. The next motion starts from the epoch endpoint. The record's source line is the actual reset line, not a fabricated tool-selection motion. Offset/temperature/barrier commands themselves are checked by native program replay rather than encoded as fictional motions.

XYZ/E/F are finite; F is positive outside epochs. Source lines strictly increase and are bounded by program bytes. Travel preserves E; deposition needs known nonzero movement and positive E after recovery. Retract/recover remains stationary known XYZ. Negative absolute E may persist during retracted travel. Feeder debt survives a new numeric E origin: fixed-dual packed admission compares exact E8 ticks within its representable tick domain, not a whole-quantum recovery tolerance. Each final feeder remains explicitly retracted; recovered length is not deposited material.

Single-tool replay begins with an unknown-start endpoint, not an invented origin segment or homing trajectory. Complete single-tool positioning/prime/body/final lift/park retains its existing order. Its body can begin with stationary retraction after the known prime endpoint. Body layer indices are bounded/monotone; native verification, not packed admission, owns any genuine support-contact-gap skip. Coincident travel records remain observable commands and are not silently removed.

Every model/support/waste deposition kind1–5,8–10,13,15 requires bit1, finite positive nominal width and ordered physical interval whose upper agrees with actual XYZ6 Z. Height is upper minus lower, never movement delta or reporting layer thickness. Kind15 requires unknown model owner. Other records have zero bytes48–71 and unknown owner; no bead is inferred from E alone. Wall ownership survives genuine Bridge and open junction strokes. Per-edge width is the expected consumed edge's width, not a path average. A mixed regional/mandatory skin bridge associates each emitted portion independently; whole-path construction remains in the verified plan.

## Consumer boundary and reproduction

The decoder borrows DataView bytes and strided accessors. Viewport batches real deposition/travel into lines and stationary pressure operations into points, rebased to f32 within0.001mm. Epochs are frame breaks, never printed segments; explicit waste remains visible separately from model/support when travel is hidden. Source-hidden inspection and layer/event controls consume these same bytes. These are nominal command views, not measured bead shape, calibrated optical color or a physical swept-tool certificate.

Cross-language fixtures are produced by real Rust emission and independent byte replay, not a TypeScript invented verified model:

- `fixtures/protocol/replay.json`: command rounding and coincident travel.
- `replay-sparse.json`: genuinely combined nominal dimensions and the fine/lower-anchor obligations.
- `replay-fixed-dual.json`: actual tool epochs, per-feeder debt, source-associated model deposition and distinct supported waste.
- `replay-waste-only.json`: the same supported tool epochs/debt with no model work from tool zero.

Regenerate only explicitly with `SPILING_UPDATE_REPLAY_FIXTURE=1 cargo test --locked -p spiling-manufacturing --lib FILTER`, using filters `actual_motion_inspection_rounding_coincident_travel_and_fixture`, `coarse_sparse_dimensions_are_verified_and_packed`, `fixed_dual_actual_epochs_repay_feeders_and_separate_waste_from_model`, and `fixed_dual_waste_only_groups_keep_exact_debt_and_zero_model_accounting`. Ordinary tests check drift without writing. These fixed logical fixture identities are interoperability evidence, not a native geometry, GPU or physical printer qualification.
