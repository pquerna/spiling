<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Planar planner vision

Provide one deterministic native-section planner for bed-anchored integral-height solids and repeated rigid placements. Robust union retains disconnected islands and holes; controlled wall offsets and nominal solid hatch provide inspectable paths and declared approximation policy. Certified extrusion slabs avoid repeated native/Boolean/path-shape computation without changing layer Z, hatch parity, topology or resource accounting.

The support envelope is deliberately conservative: world vertical planar/cylindrical carriers, layer-aligned horizontal planes, fixed integral top layers, no generated supports or bridging, no erased thin regions, no collapsed requested walls and no hole/island topology change under wall offset. Placement rotation is evaluated through transformed native carrier geometry, not rejected by pose name. Sloped planes, tilted cylinders and non-layer-aligned horizontal features are explicitly unsupported. Output describes nominal bead geometry and material volume only; physical printability and dimensions remain outside the software gate.
