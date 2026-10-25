<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Geometry authority and code map

Geometry has separate admission, inspection, display and material-proof boundaries.
A successful import, pick or section is **not manufacturing acceptance**. The
[parity RFC](../adr/004-native-geometry-parity.md) defines the intended G0–G3
contract; those gates are not completely qualified. See
[manufacturing](manufacturing-evidence.md) for the compiler/replay boundary and
[development](development.md) for build and desktop prerequisites. No geometry
result establishes physical printability or machine readiness.

## Entry points and immutable source identity

- Start at [`crates/geometry/src/lib.rs`](../../crates/geometry/src/lib.rs):
  `from_step_bytes`, its bounded/cancellable variants, `Definition`,
  `prepare_solid` and query results are the public facade. `KERNEL_VERSION` /
  `KERNEL_REVISION` identify the actual SolidCore producer; historical
  Monstertruck results do not qualify the SolidCore development cutover.
- [`import/mod.rs`](../../crates/geometry/src/import/mod.rs) owns bounded STEP
  admission and physical unit conversion. SolidCraft decodes the admitted
  single-solid subset; [`brep_data_bridge.rs`](../../crates/geometry/src/import/brep_data_bridge.rs)
  constructs native carriers. Unsupported conversion is not malformed source.
- [`import/source.rs`](../../crates/geometry/src/import/source.rs) captures
  literal original face, loop, coedge, vertex and selected-curve identities in
  [`kernel/source.rs`](../../crates/geometry/src/kernel/source.rs). These facts
  are not reconstructed from native arena ordinals, generated UV curves,
  display triangles or section-loop order. Occurrences add poses, not definitions.
- [`kernel/trim.rs`](../../crates/geometry/src/kernel/trim.rs) prepares original
  domains and complete correspondence receipts with
  [`analysis/source_restriction.rs`](../../crates/geometry/src/analysis/source_restriction.rs).
  Literal PCurve associations and the selected original master remain distinct
  from generated native chart proposals. Missing correspondence refuses the
  material query; a native mesh or fitted inverse cannot fill that gap.

The importer materializes the decoder's optional `Loop.edges` cache once from
bounded, reciprocal, owner-checked Loopuse/Edgeuse cycles and charges its retained
capacity. Consumers still cross-check native incidence; the cache does not supply
original-source correspondence or material authority.

Before native conversion and chart-map construction, exact adjacent null uses
may cancel only for the same edge, opposite directions, identical PCurves and
exactly two uses in one face/wire. Owned normalization preserves surviving
coefficients, radial order and face provenance; only unreferenced derived data
disappears, and affected chart domains only narrow. Genuine periodic seam
branches remain. Original-use census and material qualification still apply.

Import bounds are complete, conservative native enclosures for scene fitting
and coarse exclusion. The importer evaluates retained face-carrier domains,
edge restrictions and vertices with one cancellable coefficient-work allowance;
it neither invokes the native tight-box solver nor substitutes a vertex hull.
Whole carrier rectangles can over-enclose complex trims. These bounds are not
precision measurements, source uncertainty claims or material receipts:
placement and manufacturing retain their independent source-material queries.

[`parametric.rs`](../../crates/geometry/src/parametric.rs) and its coefficient
providers own bounded original-surface/curve evaluation. Unclamped splines use
their regular domains, not exterior knot support. Knot crossings retain both
incident continuous pieces; outward transported parameter bands are not feasible
point witnesses or C1 authority. Whole-range position correspondence does not
prove relative tangent correspondence, trim topology, inverse coverage or
native incidence. Samples and native tolerance fields do not discharge them.

Generic `FaceInfo.geometry` may carry optional exact analytic metadata. That
metadata describes a face; it does not authorize a finish, slab reuse or a
family-wide operation. Current import/display profiles and wire ownership are
specified by the [RFC](../adr/004-native-geometry-parity.md) and
[display protocol](../protocol/display.md).

## Prepared owners and placed inspection

| Owner / operation | Invariant and navigation |
| --- | --- |
| `PreparedSolid` / `prepare_solid` | [`analysis/material_source.rs`](../../crates/geometry/src/analysis/material_source.rs): one definition-borrowed owner for original domains, source-vertex plans and query preparation. No promotion to a second material owner or native Boolean seed phase. |
| `PreparedSectionSource` | Raw native inspection wrapper in the facade; [`section/mod.rs`](../../crates/geometry/src/section/mod.rs) constructs direct native surface/plane intersection graphs. Raw inspection does not require material admission. |
| Placed queries | [`analysis/placed.rs`](../../crates/geometry/src/analysis/placed.rs): actual accepted quaternion-polynomial transforms, not an assumed exactly orthogonal transpose. |

`NativeSection` in the facade carries plane/frame, original-face lineage and
achieved chord/plane errors. A raw native intersection is not closure of
source material. Conservative slab envelopes are explicitly distinguishable
from intersections and cannot be promoted into manufactured layers.

Sections intersect the retained native carriers directly. Complete root/span
inventories, original face lineage, ordered closure, Jordan and achieved
chord/plane checks remain independent requirements; unresolved tangencies or
branch ambiguities refuse rather than synthesize a closed loop. There is no
SolidCore Boolean-clipping or old-kernel fallback.

Raw section success does not establish source-material occupancy. Material slabs
bind their actual reference to original-domain analysis, placed-world proof and
planner conversion. Source chord, placement arithmetic, plane drift and complete
realized/native-shape bounds retain their separate budgets; a differently sampled
hole or topology-conditioned precision retry cannot silently change the contract.

Native definitions contain non-`Sync` kernel caches. The engine confines them to
owning workers rather than adding a reentrant carrier lock or cloning the BREP
for each query.

[`parametric.rs`](../../crates/geometry/src/parametric.rs) separates batched
`SurfaceFunctionalEnclosure` and spatial `SurfaceFirstEnclosure` from full
second-order jets. Continuous one-sided first bounds are not C1 or Hessian
authority; strict smoothness and second-order consumers retain their own queries.
Taylor qualification treats a spline's terminal coefficient extension as one
piece, not an interior C0 join. Genuine internal and reflected uncertain knot
bands still need original C1 proof.
Native work is debited on refusal and cancellation as well as success.
Optional surface-to-curve reductions must decline outside the reduced curve's
regular domain, preserving the full tensor evaluator's extension semantics.

Closed-height contraction in `analysis/slab.rs` computes a necessary parameter
cover, not occupancy or containment. Only proved whole-box height containment
and trim `Inside` survive subset refinement. The radial atlas refines actual XY
metrics in XY, while unresolved radius/determinant obligations retain spatial
refinement. Native scalar facet support preserves coefficient correlation;
[`material_link/fiber.rs`](../../crates/geometry/src/analysis/critical/material_link/fiber.rs)
uses uniform affine-family root proofs after original-span C1 qualification,
not sampled roots or smoothness manufactured by subdivision.

Source-vertex scalar bounds in `material_link/vertex_transport.rs` use the exact
closed-cone law `G(s,t)=V+s(B(t)-V)`: extrema come from the apex and complete
boundary, so only `t` needs adaptive search. Feasible boundary/apex witnesses
tighten achieved endpoints without clipping the complete original comparison
inventory or changing the selected G. They assert no global Original-F/G distance.

[`tessellation/mod.rs`](../../crates/geometry/src/tessellation/mod.rs) obtains
bounded SolidCore mesh candidates. Its `audit` and `trim` modules independently
check whole-triangle carriers, normals, native trim/triangle coverage and packed
output. Candidate deviation estimates or generated UV curves are not original-F
correspondence or material authority. Unqualified collapsed/singular coverage
refuses display rather than dropping triangles or publishing an empty mesh.

Singular-chart publication binds actual native vertex/edge-use ownership to the
exact coefficient collapse law and one-sided coefficient normals. Missing curved
fans must prove complete transport to the same retained mesh spoke; shared IDs
or zero chord area alone do not establish coverage. Fan and ordinary triangle
carrier bounds combine over disjoint coverage, while source-to-chord transport
remains independently additive. No vertex snapping or raw-mesh fallback supplies
these proofs.

Display trim intervals retain immutable validated coefficient metadata through
`PreparedValidatedCurve`, not sampled jets. Each query still evaluates every
active closed original knot span, including both incident sides at exact knots,
with its actual controls, denominator, derivatives and charged work. Interval
owners and construction scratch share the existing bounded audit workspace.

Triangle proof stopping criteria consume the remaining carrier allowance after
certified trim and auxiliary transport, rather than the full publication cap.
Existing proof-only subdivisions retain the original mesh triangle's affine
image; they do not insert published vertices or substitute a finer candidate.
The final carrier-plus-trim gate remains independent.

[`apps/engine/src/geometry/worker.rs`](../../apps/engine/src/geometry/worker.rs)
keeps native admission independent of display failure: `DefinitionDisplay::Unavailable`
means no mesh artifact or GPU face picking, not an empty successful mesh.
Native source records and supported queries remain available; cancellation
must never be converted into display unavailability.

## Source material and proof boundaries

- **Source-only G:** `OriginalMaterialRealization` / `MaterialRealizationIdentity`
  in `kernel/mod.rs` bind the implemented source-relative transport recipe and
  exact source-allowance bits. Query radial witnesses cannot change that recipe.
  [`critical/material_link.rs`](../../crates/geometry/src/analysis/critical/material_link.rs)
  and its transport/vertex owners prove original-use material embedding; a
  retained chart or enclosure-only recipe cannot supply embedding authority.
  Closed-unit recipe coordinates are validated before convex endpoint hulls
  tighten their affine enclosures. Exact zero/sign identities preserve attained
  boundaries, including signed underflow, rather than inventing outward material
  or widening source domains. Unproved coordinates are never clipped; coefficient
  domain checks and original-material embedding remain independent obligations.
  Finite F/G homotopies use explicit original coefficient-sheet queries, not
  trim-box membership as an evaluation shortcut. These borrow the captured
  inventory and retain literal rectangular/spline restrictions, denominators
  and charged work. Position-only images infer no continuity; piecewise first
  queries require closed C0, retain derivative jumps and avoid Hessians, while
  smooth second queries require closed C1. Source scalar support projects these
  same original coefficients in their actual gauge, not a native degree chart.
  None of these queries mutates selected trim domains or establishes native
  correspondence, branch ownership or realized-material embedding.
- **Uncertainty:** [`analysis/bounds.rs`](../../crates/geometry/src/analysis/bounds.rs)
  and `analysis/placed.rs` distinguish geometric from realized-material bounds.
  Source realization error must survive actual affine conversion, placement
  arithmetic and plane drift. A complete bound is not made cheaper by omitting
  uncertainty or charging it in a different coordinate system.
  Original C0 bounds prioritize refinement by actual projected cross-section
  variation, not unrelated UV lengths. This changes search order only: complete
  cell enclosures, feasible witnesses and the work limit remain authoritative.
  Native source support carries independently feasible extrema across faces.
  A complete coefficient-sheet enclosure already covered by those extrema may
  skip trim contact search under the same outward stopping rule; it supplies no
  witness and never promotes a carrier bound to a material-contact receipt.
- **Regular slabs:** [`analysis/slab.rs`](../../crates/geometry/src/analysis/slab.rs)
  binds the actual source, interval, reference and policy. Height invariance is
  local to regular partitions; it does not cross critical-event brackets.
  [`placed/material_coordinates.rs`](../../crates/geometry/src/analysis/placed/material_coordinates.rs)
  owns actual-coordinate capture, not a nominal orthogonal-cylinder surrogate.
- **Radial products:** [`critical/material_link/radial_product.rs`](../../crates/geometry/src/analysis/critical/material_link/radial_product.rs)
  and [`joint/radial_metric.rs`](../../crates/geometry/src/analysis/joint/radial_metric.rs)
  pair closed topology with a complete filled-material metric and calibrate the
  actual used polygon in matching physical coordinates. Boundary nearness,
  directed/core-only metrics and separately isotopic annuli are insufficient.
- **Original corner cuts:** [`vertex_transport/uv_ownership/source_cut.rs`](../../crates/geometry/src/analysis/critical/material_link/vertex_transport/uv_ownership/source_cut.rs)
  proves fixed-exit relative surgery against the actual original germs,
  retained central continuations and foreign sources. Original classification
  masks complete owned joint cells as Boundary, never Inside. A sealed affine
  endpoint family need not have a provably nonzero derivative; incident germs
  may overlap only inside the same incidence-owned cell. Bounded interval
  mean-value contraction can establish that containment without welding their
  uncertain endpoints. Foreign boundaries must stay outside the whole cell.
  These chart receipts do not replace material corner surgery: an uncertainty
  connector need not be Jordan, and a completed reference disk cannot simply
  be subtracted from native UV. The closed atlas covers original F and actual G,
  including literal NativeV apices. Genuine poles need independent quotient authority.
- **Critical sets and collars:** [`analysis/critical.rs`](../../crates/geometry/src/analysis/critical.rs)
  covers original face/coedge complexes; [`source_incidence.rs`](../../crates/geometry/src/analysis/source_incidence.rs)
  owns actual edge/vertex use incidence. [`analysis/collar.rs`](../../crates/geometry/src/analysis/collar.rs)
  binds nonzero collars and closed endpoints to actual neighboring material.
  A regular radial product does not discharge births/deaths, pocket floors,
  disappearing voids or cross-occurrence attachment; loop counts do not prove incidence.
  Rectangle and port covers use closed UV contraction and honest first bounds;
  only a smooth affine fallback requests Hessians. One actual segment may cover
  a complete affine patch only when it covers every corner plus the full
  remainder; corner-wise different edges cannot certify holes or nonconvex gaps.
  Isolated definite vertices additionally require whole-cell Hessian/orientation
  bounds, an all-roots cover and a physical NativeV connection on the proved
  original relative-cut side. The native joint is not padded. Strict outward
  world normals can bound one material-facing crossing shadow, not regular
  boundary speed; target selection and retained cells use that same shadow.
- **Final composition:** [`analysis/joint.rs`](../../crates/geometry/src/analysis/joint.rs)
  owns source leaves, union captures and `NativeUnionBindingLedger`. Receipts
  bind source/realization, occurrence, pose, interval/reference, policy and actual
  candidate geometry. Changed polygons or stale captures cannot inherit proof.
  `certify_regular_interface_pair` registers complete actual material/void loops
  and consumes the whole non-star collar theorem in
  [`joint/planar.rs`](../../crates/geometry/src/analysis/joint/planar.rs).
  Receipt-bound topology does not replace native boundary confinement or
  closed-cut incidence; critical pairs retain the original proof route.
  Endpoint-local radial products can prove regular-side confinement beside a
  critical cap. Their own source/world domains and actual polygon calibration
  do not upgrade the whole leaf or fabricate a boundary-motion tube.
  Critical support-exclusion capture combines a complete raw-F slab overcover of
  retained F with direct world-height/XY C0 covers of all closed G cones and
  complete central ribbons. Its rectangle conservatively excludes support from
  the complement; it is not a no-void receipt, Original-F correspondence,
  source-material anchor or precise Hole origin.
  Native material reuse does not establish realized finish or deposited support;
  those remain independent [manufacturing obligations](../protocol/manufacturing.md).

## Original source correspondence

Original source operands retain literal coefficients, trim restrictions,
preferred-master selection and PCurve association order. Source restriction
preparation must prove a complete correspondence to the actual retained native
use; endpoint proximity and native labels cannot replace whole-range evidence.

Untrimmed nonperiodic selected PCurve LINEs reuse the finite source-coefficient
window provider with the edge's captured original vertices. A search cover stays
in that declared master's gauge; it does not borrow the independently gauged 3D
reference or generated native UV. Endpoint uniqueness, whole correspondence,
relative derivatives and chart/joint qualification remain separate obligations.
Finite authored trims and periodic selectors retain their own branch inventory.

Independent PCurve endpoint-root pairs and midpoint tangent checks propose
branches; they do not establish ambiguity or authorize the first match. Each
candidate must pass complete world-position and relative-derivative calibration
in its own gauge before it counts as qualified. Distinct qualified branches
refuse; cumulative work and peak root/proof/workspace bytes stay bounded.

Endpoint proposals include finite coefficient fences, midpoints and reference
zero. Recheck representable candidates after Newton contraction: an initial
search midpoint need not hit the contracted root. Every singleton still requires
exact outward stationarity and its full physical residual bound; proximity or a
small interval is not parameter identity.

An exactly isolated stationary parameter is not exact vertex coincidence.
Its nonzero physical source residual remains charged to the source allowance.
A joint collapses only with coefficient identity or equal singleton outward UV
enclosures; overlapping or merely small uncertainty boxes are not identity and
do not erase displacement or correspondence obligations.

Numerical knot/cut leaves are not new source endpoints. A leaf can use its
complete boundary's exact shared C0 anchor only after opposite monotone germs
are certified over both entire leaf-to-anchor parameter hulls. This preserves
separation across uncertain internal cuts without endpoint welding or distance
tolerances.
Complete C0 coefficient pieces with a strict derivative coordinate qualify as
injective spans before pairwise separation. Unproved pieces subdivide within the
same depth/work policy, preferring an actual cover fence near the midpoint so
narrow original spans do not depend on dyadic resolution. Numerical calibration
cuts and original knots remain in the evaluator; they are not mandatory
pairwise topology.

Source edge/vertex census checks complete original-use ownership against native
incidence, including opposed uses, rather than matching stars by coordinates.
`FACE_OUTER_BOUND` is an explicit outer marker; generic `FACE_BOUND` is not a
declaration of a hole. Whole-domain winding and nesting retain the actual
outer/hole qualification.

When no declared PCurve exists on an original plane, preparation may derive UV
from that plane's coefficient-proved nondegenerate global Gram inverse applied
to the same selected original spatial expression. Derived evidence stays distinct
from declaration and never overrides a failed declared branch. Whole lift,
relative-derivative, native association, domain, joint and incidence checks remain;
only exact whole-affine compose-back identity grants a zero transport receipt.

`PreparedSolid` readiness means its actual selected realization operands are
prepared. It is not a global Original-F embedding, filled-material, attachment
or manufacturing certificate. Slab, critical-collar, pair and final binding-ledger
queries retain those independent obligations. Unsupported original associations
remain explicit refusals without disabling unrelated raw native inspection.

## Refusals and support boundary

`GeometryWorkBudget` in [`analysis/mod.rs`](../../crates/geometry/src/analysis/mod.rs)
is cumulative across retries/subdivision. Prepared sources, pinned proofs,
coexisting query scratch and output have separate retained-capacity accounting;
a packed-output allowance cannot fund native preparation. These are not RSS
limits: native intermediate objects also require host-process memory bounds.
Resource exhaustion, cancellation, unavailable parameter correspondence,
conditioning, unsupported operations and invalid geometry remain distinct failures.
No partial proof, stale job or failed display grants successful material authority.

This is a development cutover, not transferred kernel qualification. Historical
Monstertruck fitting/housing CLI and hardware-workbench journeys do not establish
SolidCore parity. The unchanged original fixtures and operator workflows remain
the reproduction inputs; report the SolidCore paths actually exercised and their
explicit refusals before expanding support claims.

General STEP documents/assemblies, unproved charts, arbitrary freeform finishes,
singular/nonmanifold material and unqualified critical interfaces remain outside
support. Native descriptors and successful raw inspection do not broaden it.

## Minimal user flows

Build the coordinated engine and client together, then run from the repository
root. Mixing binaries from different semantic generations is a handshake refusal:

```sh
cargo build --locked -p spiling-engine -p spiling-cli
target/debug/spiling-cli geometry --source fixtures/geometry/box-mm.step \
  --section 0,0,4:0,0,1 --engine "$PWD/target/debug/spiling-engine"
```

Inspect `DefinitionDisplay` as well as the command status: an explicit unavailable
display is not proof of a working mesh. The CLI validates transferred mesh and
section payloads; raw inspection does not qualify manufacturing.

The broader fitting/housing scene and hardware picking control remain separate:

```sh
node tools/smoke-desktop.mjs --executable "$PWD/target/debug/spiling-desktop" \
  --scenario geometry --scene fixtures/geometry/scenes/realistic-native.scene.json \
  --port 9222
```

A box CLI result does not qualify that scene, the desktop surface, physical
printability or packaged platforms.
