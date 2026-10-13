<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native planar section queries vision

Provide deterministic owned native sections for the admitted planar/cylindrical STEP subset while preserving immutable BREP authority. Queries return definition-local f64 boundaries with explicit sampling/boolean tolerances; downstream engine composition places and packs them without source-face identity claims. Exact boundary witnesses make empty and degenerate queries distinguishable before native intersection, while certified curved bounds keep clipping safe.

Expose allocation-free native analytic directional extrema so posed consumers can certify bed/top contact without treating transformed AABB corners as actual cylindrical surface extrema. Original trimmed line/circle witnesses remain the only authority, with finite-direction normalization and cancellation. This does not broaden STEP admission or replace native intersections; mixed-axis cylinder projection is independently checked against analytic radius/height, not a planner or display echo.

Repeated layer queries reuse bounded native search preparation across immutable shared definitions without changing intersection geometry or tolerances. Borrowed lifetime, explicit source/query/intermediate budgets and prompt release keep acceleration separate from persistent authority and manufacturing ownership.


Box, cylinder and through-hole midplanes, oblique planes and placed first-layer planes must pass original and frozen independent OCCT corpus numerical and topology evidence before native section support is claimed. The periodic-edge gate invokes the public native boolean on unchanged imported solids, checks every result shell's manifold closure, and retains original source face slots. A genuine upstream capability failure blocks that gate with an original reproducer rather than a substitute kernel, sampled-extrema repair, display-derived section or omitted curved fixture. General freeform admission, arbitrary mesh slicing, occurrence union and manufacturing planning are out of scope.
