<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Private kernel ownership rules

Own immutable opaque native definitions, canonical source-face metadata, conservative native bounds and source/native curve/chart witnesses. Exact kernel maps retain bounded intervals, orientation and mm conversion; coedges borrow shared native geometry and chart curves without copied carrier surfaces. Analytic line/circle witnesses stay narrow fast paths; generic endpoint witnesses are not chords. Do not export kernel types, pointers, locks or handles. Preserve positional source face/edge/vertex identity at every conversion boundary. Downstream modules borrow definitions; topology, conversion and conservative bounds are release-active admission invariants, while query regularity/trim coverage is operation-specific.

Wall certification uses the admitted face-table index to borrow the unchanged native face. Borrow carriers/curves without cloning the BREP; require four distinct native corners/edges and the oriented outer rectangular wire, finite coefficients and bounded spans. Prove full localZ height and outward support against exact line/circle source witnesses; reject additional coplanar face uses rather than guessing a merged carrier interval. Preserve the 1e-7 mm admission residual and 1e-10 angular boundary; cancellation occurs at every face/edge/projection witness. No certificate may promote carrier proximity into source identity or imply all other source faces form an invariant extrusion.

Cylinder certification borrows the exact positional native face and generic retained coedge-to-analytic-witness references. Keep a single shared analytic geometry store, not copied BREP/circle vectors or cylinder-only import flags. Retain native face/edge identities and orientations, cross-check endpoints, and preserve periodic subedge and opposite seam identity. Cap retained coedges and wires at `2 * MAX_STEP_ENTITIES`; temporary incidence maps are bounded by those coedges and check cancellation at every insertion/traversal. Prove transformed line-profile revolution coefficients, radial-outward orientation, two simple continuous one-turn physical rim cycles at whole-source Z extrema and unique cylinder carrier ownership. Chart wire count/seam angle and rim subdivision count are not certificate criteria. Source +X/right-handed +Z defines phase independently of native UV.

Native two-vertex edge representation may subdivide an exact periodic source edge without subdividing its source faces. Closed-solid validation remains connected, closed, oriented and manifold; boundary vertex repetitions require opposite paired seam incidence, not a bypass of face validation. Keep the source hash and face-use mapping independent of native subedge cardinality.

Native section AND must preserve the correct material halfspace, shared section-edge incidences and inward hole-wall orientation when periodic source faces carry paired chart coedges. A closed full-turn intersection must not disappear as a zero-area open covering-chart path. Physical dimensions come from independent analytic source witnesses; certified bounds must enclose those extrema and may legitimately exceed them by their proved control-hull budget.

Carrier certification must retain original Processor transforms and inversion for ordinary and intersection curves. Closed-chart attachment derives an oriented physical tangent from both certified carrier normals and the original intersection orientation; a stalled guide endpoint is not proof of tangent surfaces. Zero/nonfinite physical transversality, unresolved orientation and invalid wedges still refuse.

Exercise `cargo test --locked -p spiling-geometry` and the real native corpus smoke; see [geometry evidence](../../../../docs/quality/geometry-evidence.md).
