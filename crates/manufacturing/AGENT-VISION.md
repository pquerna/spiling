<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Manufacturing boundary vision

Turn explicit printer/recipe data and authoritative placed BREP definitions into deterministic bounded planar paths, a software-only absolute Cartesian program, provenance and independent replay measurements. Opt-in inspection additionally supplies bounded packed actual-command motion for the workbench, preserving accepted rounding deviations rather than echoing normalized plan coordinates. This crate isolates native planning and 2D Boolean/offset dependencies from persistence and protocol consumers.

Current support is one extruder, fixed-height bed-anchored layers, integral-height top policy, explicit perimeter offsets, physical-thickness top/bottom skins and alternating-axis solid or sparse rectilinear interiors. Closed native sections are unioned with holes and islands retained; disjoint material ownership is explicit before paths. Expanding sections requiring support, unresolved thin regions and offset collapse are rejected rather than repaired. Rounded nominal bead accounting and requested density are not physical geometry, strength or material guarantees.

Diagnostic body programs and typed complete single-extruder Marlin jobs are independently replayed, including startup modes, declared homing, thermal waits, priming and shutdown. Firmware/configuration/calibration hashes are provenance declarations, not measurements. Physical execution, machine qualification, sensor truth, collision certification, arbitrary infill, generated supports and Blender remain outside this software gate. Acceptance is durable end-to-end software compilation and independent emitted replay, not physical printer support.
