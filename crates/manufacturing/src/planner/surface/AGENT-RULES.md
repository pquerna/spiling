<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Source-backed surface construction rules

Resolve captured ordered definition layers before occurrence layers; last enabled whole-face fill wins, and Smooth has no geometric construction. Exact native face identity and `VerticalWallFace` certification are mandatory. Only upright bed-anchored invariant native extrusions enter this lane; source BREP is never edited.

Construct constant-source-Z outward periodic ribs with nominal zero displacement in protected end bands and a total 0.001 mm construction bound: chord error ≤0.00099 mm plus native-frame agreement/directed rounding ≤0.00001 mm, independent of native/offset budgets. Use the conservative taper second-derivative bound only on each half-pitch ramp and the constant-amplitude bound in the interior; include both ramp breakpoints and merge native sample parameters. Admit sample/proof/retained/segment work before growth.

Match one unique contiguous monotone full-span native exterior sample chain using the immutable face certificate, both trimmed endpoints, supporting side and material-left winding; support the closed-ring seam and never associate by plane proximity alone. Prove positive inward overlap and no native boundary/hole entering its collar. Anchor merged profile intervals to actual native segments; robust outward orientation, progression, material inclusion and nonintersection prove M union A. Original active-face samples may become interior material rather than D boundary vertices; every original material point remains included, while untouched/protected vertices, endpoints and hole rings remain exact. Never use a floating Boolean to rewrite unrelated source rings.

Directed rounding applies only to new samples, fits the fixed construction reserve and never tolerates missing M. Refuse unsafe widths, lost additions, topology changes, excessive coordinate precision/work or expanded-envelope/occurrence/component/prime exclusions.

All wall/fill/skin caches, actual-stroke bridge/support regeneration and clearance use the same realized material D. Finished occurrences require strictly separated XY bounds in overlapping Z interiors; append their native-preserving rings without re-overlaying them. Ordinary untextured material retains its existing union path. Source area remains separate from D and added area; geometric added volume is not deposited extrusion or tactile efficacy. Acceptance requires native compile, exact native retention/phase/chord-bound regressions and independent actual-byte replay through the [CLI/workbench workflow](../../../../../docs/quality/development.md#source-face-vertical-ribs); never claim edited BREP, physical grip or printer qualification.
