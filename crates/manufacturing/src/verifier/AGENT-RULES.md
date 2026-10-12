<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Independent replay rules

Decode actual program bytes without importing backend formatting, parsers or extrusion helpers. Validate intent and normalized plan first. Consume expected path points through an iterator; do not reconstruct an expected program or allocate a second motion vector. Stored `verified` flags have no authority.

An optional SPLR sink is fed only after each actual command passes all replay checks. Append decoded XYZ, current absolute E, F and source line; obtain layer/kind only through validated expected ordinal association. Reserve checked final bytes once and reject actual Vec capacity above 16 MiB; support 200000 aggregate events with no per-motion objects. No display work/allocation/hash for report-only verification. First travel is endpoint-only with unknown start; later zero-distance travel is known and valid. Drop the sink on any decode failure/cancellation; finish only a complete replay. [SPLR](../../../../docs/protocol/replay.md) owns layout.

The supported state sequence is G21, G90, M82, G92 E0, then only G0 travel to each path start and G1 deposition to each subsequent point. Every motion carries XYZ and F; only G1 carries absolute E. Reject unknown/duplicate/malformed fields, unsupported commands/modes, missing/extra moves, zero-length deposition and nonpositive extrusion. Bound program/line parsing and poll cancellation during decoding.

XYZ uses at most six decimal places, E eight, F six. Compare each coordinate within 0.0000005 mm, absolute E within 0.000000005 mm and feed within 0.0000005 mm/min, plus scale-dependent floating arithmetic slack. Consecutive E deltas receive twice the E rounding bound. Verify envelope, expected feeds and nominal rounded-bead volume `(width-height)*height + pi*(height/2)^2`; actual decoded flow is checked with explicitly derived serialization-rounding slack. Plan approximation tolerances do not enlarge replay thresholds.

Report actual replay counts, extrusion totals and maximum measured deviations with explicit coverage and exclusions. No physics, collision, firmware, calibration, startup/homing, thermal, physical support or printability claims. Regression tests use real backend emission plus independent small numeric goldens and adversarial command mutations; main integration runs the crate tests after all owners finish.
