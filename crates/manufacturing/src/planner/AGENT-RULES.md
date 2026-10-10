<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Planar planner rules

Consume immutable native definitions and explicit rigid occurrences; inverse-transform world horizontal planes, then transform native mm loop points into world XY. Native outer/hole classification is authoritative. Associate holes with their smallest containing outer before union; canonicalize ring starts/orientation and polygon order. Never replace sections with tessellation or source-name-specific geometry.

Admit only world vertical planar/cylindrical carriers and layer-grid-aligned horizontal planar carriers. Diagnose sloped planes and nonvertical cylinder axes as unsupported. This makes each interior section exhaustive within its fixed layer: sub-layer horizontal shelves cannot be silently missed.

Fixed layer height is bed-anchored: output Z is bed+(index+1)*height, conceptual sample Z is the interior midpoint. Global layer quantization is at most 1e-6 mm; horizontal-face alignment also requires height-relative tolerance `min(1e-6 mm,height*1e-6)`. Reject nonintegral global top heights rather than truncating or silently adding partial layers. Empty per-occurrence sections are valid. Empty aggregate layers or new material outside the previous section buffered by twice native sampling tolerance are unsupported supports.

Reuse a native XY section/union and both parity path shapes only in certified open extrusion slabs between horizontal faces when transformed carriers have exactly zero nonvertical residuals. Otherwise sample every layer natively; do not accumulate undeclared error from approximate verticality. Check support at every slab transition and charge every actual layer's retained segments, not only template construction. Conceptual midpoint metadata is not a native-query log.

Round offsets use a sagitta-derived maximum chord/radius parameter at 0.005 mm tolerance. Erode disconnected islands independently: erosion cannot join them, and distant placements must not reduce overlay precision for an individual contour. Do not discard numerical fragments to satisfy topology checks. Reject component/hole topology loss in requested wall centers and any region unreachable from first-wall centers after conservative miter dilation (0.01 mm tolerance); do not silently erase thin regions. Remaining interior uses alternating X/Y parity-clipped solid hatch; spacing equals rounded bead area divided by layer height, with centered rows covering remainder strips. This is nominal 100% fill, not physical coverage certification.

Check cancellation around native/Boolean/buffer calls and during every layer/path/hatch. Bound input/output point counts, Boolean pair work, hatch rows, layers and retained deposition segments before growth. Native upstream calls are nonpreemptible and do not imply allocator-level RSS guarantees.
