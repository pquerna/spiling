<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Support centerline realization rules

Own the private physical-support centerline and process-role realization consumed by the parent support planner. Inputs are immutable bed-certified clearance-clipped support union S, interface ownership I, recipe and actual scheduled layer. Never access native M, image parameters, schedule construction or object ownership here.

Erode S once by declared half-bead width. Every nominal support bead remains confined to physical S. Interface is a centerline ownership/density selector, not an independently cleared physical solid: hatch ownership is E(S) intersect I versus E(S) minus I. Interface uses full density; ordinary support retains declared density. Beads may overlap a role seam; each emitted path belongs to exactly one role and is accounted once. A sub-bead I strip must not erase the physical support wall or create artificial clearance.

Retain each physical exterior/hole wall edge and original vertex in order. Split only at genuine role changes from analytic line intersections and midpoint ownership, including interface boundaries. Same-role crossings and tangencies insert no extra point; never add internal seam outlines, flatten contours, discard walls or silently remove unresolved micro-edges. Numerical degeneracy refuses.

Return staged paths without changing the parent's segment count. The parent serially attempts preferred and orthogonal axes, drops failed candidates before another attempt and commits only paths whose every stroke is proved against actual preceding support/interface deposition. This module's proof preserves the exact capsule, endpoint, overhang and bounded bridge rules; ownership regions themselves are never anchors.

Charge physical/partition contours, staged path points, wall-crossing and hatch-hit scratch, and anchor interval records against the remaining geometry allowance. Reserve combined object/support segments before allocation. Share cumulative 20M Boolean/projection/raster/anchor work across all attempts; check cancellation during each stage, row/edge and anchor query. Borrow previous strokes; do not copy the motion plan or retain alternate candidates.

Parent-run evidence: private support raster tests for changing sub-bead interface ownership, unchanged exterior/hole walls, missing actual anchors and bounded staging/cancellation; unchanged native box/hole image support success plus lower-corridor blockage through public compilation and independent replay. Software admission does not establish physical strength or printer readiness.
