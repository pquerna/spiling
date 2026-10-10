<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Manufacturing boundary vision

Turn explicit printer/recipe data and authoritative placed BREP definitions into deterministic bounded planar paths, a software-only absolute Cartesian program, provenance and independent replay measurements. This crate isolates native planning and 2D Boolean/offset dependencies from persistence and protocol consumers.

Current support is one extruder, fixed-height bed-anchored layers, integral-height top policy, explicit perimeter offsets and alternating-axis 100% solid hatch. Closed native sections are unioned with holes and islands retained. Expanding sections requiring support, unresolved thin regions and offset collapse are rejected rather than repaired. Rounded nominal bead accounting is not a physical geometry or material guarantee.

Physical execution, printer-ready startup, thermal behavior, collision certification, arbitrary infill, generated supports and Blender are outside this software gate. Acceptance is durable end-to-end software compilation and independent emitted replay, not physical printer support.
