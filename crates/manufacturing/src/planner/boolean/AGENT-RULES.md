<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Boolean work admission rules

Admission before a geo/i_overlay overlay is linear and changes no geometry: inventory every input point once, charge it to the caller's cumulative planar work counter (counted for cancellation and accounting, never capped, ADR009), refuse nonfinite coordinates and coordinate frames the pinned integer conversion cannot condition (`overlay_padding`). Do not add a pre-overlay edge sort, sweep, candidate-pair count or work ceiling; i_overlay 9 bounds its own work, and the consumer admits the result by size (`region::check_size` or an explicit byte admission of the returned contours).

Never alter source vertices, polygon/ring ownership or the order of operands passed to the kernel. `Prepared`/`Workspace` hold only an operand's inventory so repeated queries do not rescan it; they retain no scratch and decide no kernel route.

`sort_by` is a cancellable, fully charged heapsort for planner callers that need ordered records (walls); it is not part of Boolean admission.
