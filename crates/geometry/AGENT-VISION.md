<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native geometry boundary vision

Provide one authoritative native BREP boundary for downstream engine inspection. Import returns an immutable mm/f64 definition with source provenance, canonical face-use identity and certified bounds; tessellation returns owned packed display chunks, and sections return native f64 loops. Engine composition, not this crate, owns scene/session identity, worker supervision and artifact lifetimes.

The implemented lane targets `step-planar-cylindrical-v1`: one closed solid, planar/cylindrical carriers and analytic line/circle boundaries. Original box/mm/inch/cylinder/through-hole proof, bounded perforated-plate transfer and independent OCCT exporter outcomes are recorded in [geometry evidence](../../docs/quality/geometry-evidence.md). The v2 engine composes this facade with remaining resource budgets and placed occurrences. Numerical support is limited to the exercised corpus: the independent OCCT box passes, but OCCT cylinder/through-hole converted topology is currently rejected. General STEP and the overall physical geometry milestone remain unaccepted.

No general STEP hierarchy, project persistence, alternative kernel, mesh-derived section or manufacturing. Future consumers reuse this facade rather than introducing a second import or geometry service.
