<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Packed section contract vision

Preserve inspectable native f64 loop data across a bounded transport boundary, with explicit plane, occurrence identity, winding and approximation provenance. Whole-loop chunking and independent admission keep malformed or stale sections away from display allocation.

Native geometry owns section evaluation, including explicitly requested placements; engine composition owns scene/artifact identity and lifetime. This codec transports supplied world or plane-relative loops without performing native sectioning, placement queries or manufacturing verification. SPLS availability and algebraic goldens remain distinct from native-derived numerical evidence and runtime capability acceptance.
