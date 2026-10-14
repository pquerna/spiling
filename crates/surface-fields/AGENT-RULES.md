<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Immutable image field rules

Own bounded PNG/PGM normalization and borrowed periodic pixel-node evaluation. Depend only on contracts and optional pinned PNG; never depend on Core, BREP, planner, renderer or persistence. Contracts validate canonical P5 content and identity. `HeightImage` retains one Arc canonical allocation; callers share it across occurrences and layers.

Follow [the accepted mapping and normalization decision](../../docs/adr/003-spatial-image-relief.md) exactly: high-byte16-bit conversion, integer luma/black alpha compositing, no gamma/profile conversion, no APNG/resampling, reverse source-Z rows and complete last-to-first cells. `normalize(encoded,source_name,canonical_budget,cancel)` admits the exact P5 header+raster size against caller-owned remaining retained capacity before any canonical Vec allocation/copy; encoded capture and bounded PNG working memory are separate allowances. Bound dimensions, event growth and finite arithmetic before allocation. Cancellation surrounds nonpreemptible bounded codec work and is checked during pixel/event work.

Native normalization golden samples and asymmetric phase/row/wrap/bounds tests are the acceptance surface. Carrier correspondence, pose, chord/retention proof and nominal-bead diagnostics belong to manufacturing, not this crate.
