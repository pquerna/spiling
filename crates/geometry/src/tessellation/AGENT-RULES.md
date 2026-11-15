<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# SolidCore display conversion rules

Ordinary source display selects `MeshTolerantMm005V1`, or the next coarser tolerant level (`MeshTolerantMm02V1`, `MeshTolerantMm1V1`) through the same path with only the recorded allowance and facet angle changed: complete source-face ownership, measured carrier and trim deviation, closed oriented incidence and checked packing. It is disposable approximation, not coefficient certification or material authority. The proof obligations below apply to the separately requested `MeshTrimmedMm005V2` route; never label a tolerant preview as satisfying them. Both preserve cancellation and fail rather than publish omitted faces or malformed buffers.

**Candidates come from `solidcore-tess`, planned off the definition's own record.** `candidate::plan` runs `solidcore_tess::plan_with` on `Definition::record()` under a `Control` that charges the caller's `GeometryWorkBudget`, honours cancellation and admits the plan's retained bytes against `DISPLAY_PROOF_BYTES`. A stop is terminal (typed `Cancelled`/`ResourceLimit`), never a partial plan. Record faces and edges map to kernel ids through `Definition::record_order()`; publish in `Definition::face_ids()` order.

**Plan statistics are reported, never dropped.** An unmeshed face or unsampled edge refuses the display, named. A face the planner filled leniently refuses the trimmed route (its boundary sides are no triangle sides). Lenient, over-tolerance, averaged-pole and capped-edge entries are appended to every display failure (`candidate::plan_notes`). Only the independent audits decide publication.

**Shared identity is exact.** Edge nodes are known by their position bits; a face's fixed nodes must name an edge node. A pole's (u, v) is a chart choice: each fan triangle gets a local pole copy on its collapsed row (`chart_pole_fans`), proved by the singular inventory. Parameters within four representable steps of the natural domain are projected onto it before evaluation; larger excursions refuse.

Candidates use native finite point/normal evaluation, not interval jets or C1 proof as a prerequisite. Ordinary preview measures physical sample-to-carrier distance: a barycentric UV seed crossing a periodic seam is not correspondence. Alternative seeds and native minimization propose points only; finite re-evaluation and unchanged distance/trim/incidence/packing checks decide publication. The trimmed diagnostic retains its independent coefficient audits.

**Refinement is whole-body.** Only an independent shape-error audit requests it: the next attempt plans the body four times finer, at most eight times, sharing the cumulative allowance. Resource failures and cancellation are terminal. Only a fresh complete audited and packed pass publishes.

Audit every published triangle against the native coefficients in its natural UV chart with `parametric::DisplayTriangleProof` (triangles crossing a non-C1 knot line are bounded piece by piece across it); prove positive UV triangles, cancelled interior incidence, simple boundary circuits and a complete original-loop/edge-interval bijection; bound every whole native 3D trim interval and its lifted pcurve against the chord. A pcurve image past its chart by rounding only is evaluated on the chart with its speed-widened displacement; any larger excursion refuses. Keep carrier plus trim at most 0.025 mm and packed reconstruction at most 0.025 mm.
