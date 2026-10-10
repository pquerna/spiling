<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native planar section queries vision

Provide deterministic owned native sections for the admitted planar/cylindrical STEP subset while preserving immutable BREP authority. Queries return definition-local f64 boundaries with explicit sampling/boolean tolerances; downstream engine composition places and packs them without source-face identity claims. Exact boundary witnesses make empty and degenerate queries distinguishable before native intersection, while certified curved bounds keep clipping safe.

Box, cylinder and through-hole midplanes/oblique planes must pass original-corpus numerical and topology evidence before native section support is claimed. A genuine upstream capability failure blocks that gate with an original reproducer rather than a substitute kernel, sampled-extrema repair, display-derived section or omitted curved fixture. General freeform admission, arbitrary mesh slicing, occurrence union and manufacturing planning are out of scope.
