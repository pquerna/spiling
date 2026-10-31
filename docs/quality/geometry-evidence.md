<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Geometry authority and code map

Geometry has separate admission, inspection, display and material-query boundaries.
A successful import, pick or section is **not manufacturing acceptance**.
[ADR005](../adr/ADR005-native-first-development-geometry.md) owns the implemented
source12 DEVELOPMENT route (current planner identity is owned by the manufacturing contract): the imported SolidCore BREP is its sole
material geometry. Qualification requires the actual coherent producer,
recorded dependency sources and preserved operators. Earlier producer receipts
do not transfer by relabeling. The
[parity RFC](../adr/004-native-geometry-parity.md) is superseded on independent
original-STEP material reconstruction. See [manufacturing](manufacturing-evidence.md)
for compiler/replay obligations and [development](development.md) for build
prerequisites. No geometry result establishes physical printability or machine readiness.

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
  Rational surface weights outside the native canonical constructor's envelope
  may use one exactly reversible power-of-two homogeneous gauge. Every weight
  and weighted control coordinate must retain its exact value under the inverse
  gauge; individual weight clamping, lossy scaling and incompatible weight ratios
  refuse. Original STEP operands and in-envelope native representations remain
  unchanged. This representation admission does not qualify sections, display,
  critical topology or manufacturing for the newly admitted source.
  Both SolidCraft libraries are checked-in third-party source under
  `vendor/solidcraft/`; its `UPSTREAM.toml` identifies the exact snapshot,
  homogeneous-boundary closure correction, native spline-chart construction and retained licensing. They are
  exchange/evaluation libraries, not another material kernel.
- [`import/source.rs`](../../crates/geometry/src/import/source.rs) captures
  literal original face, loop, coedge, vertex and selected-curve identities in
  [`kernel/source.rs`](../../crates/geometry/src/kernel/source.rs). These facts
  are not reconstructed from native arena ordinals, generated UV curves,
  display triangles or section-loop order. Occurrences add poses, not definitions.
- [`kernel/trim.rs`](../../crates/geometry/src/kernel/trim.rs) prepares native
  face/use domains for material queries. `prepare_solid` never selects original
  STEP operands as a material fallback or requires a separately reconstructed
  original solid. Missing native query authority still refuses.
  Retained original operands and
  [`analysis/source_restriction.rs`](../../crates/geometry/src/analysis/source_restriction.rs)
  serve the separate bounded inspection/finish path. Literal PCurve associations,
  selected masters and source frames remain distinct from generated native
  charts; absence or ambiguity can refuse that operand/finish query without
  conferring original declaration or embedding authority on a native query aid.

Spline import can retain a unique associated original PCURVE as a native chart
proposal when the retained source carrier and active parameter gauge match.
Construction may restore rounding-close constant boundary coordinates and shared
original-vertex endpoints; genuine exterior controls and distinct pole/seam UV
branches remain separate. Completed BREP geometry validation and independent
whole-curve native correspondence/containment still decide each query. Literal
original STEP operands are preserved. The evaluator's active knot domain bounds
queries even when native trim metadata has a larger conservative rectangle.

Native periodic-seam qualification uses the literal stored carrier and chart
coefficients, opposed uses of the same native edge, and exact periodic G1
identities. Native charts have no original-source transport error; their
independent spatial/chart trace tolerance remains in force. Original inspection
charts retain their measured transport error. Unadmitted charts cannot borrow
either premise. A smooth seam does not establish height transversality or
authorize a section through a critical, coplanar or tangent stratum.

Height-event discovery restricts continuous native carrier cells to the requested
height band and retains direct scalar coefficient bounds for height and its
first derivatives. Projecting an already broadened XYZ enclosure can lose an
exact height cancellation and cannot replace that tighter certificate. Whole-face
G1 and actual coedge/vertex incidence remain separate. Artificial crease cells
require trim classification only after height/gradient or complete geometric
transversality cannot exclude an event; physical boundary strata are still
inventoried. Piecewise first derivatives grant exclusion, while stationary-point
contraction/existence retains its independent closed-C1 requirement. The torus
equator census and oblique-height/cusp controls exercise this distinction.

Polynomial surface jets also intersect the tensor-basis enclosure with convex
hulls of differentiated literal control coefficients on each original knot cell.
A bounded order-three stencil retains coefficient cancellation; exact component
invariance removes unnecessary basis sums along an unchanged axis. Directed
rounding, cumulative work/cancellation and scratch coexistence remain charged.
Closed queries retain both incident one-sided spans. These bounds do not confer
closed-C1 stationary authority or change rational quotient/denominator obligations.

Batched scalar functionals intersect their quotient positions with Cartesian
control hulls only when all active weights have the same nonzero sign. Mixed
signs retain the actual query-denominator proof; zero weights or nonfinite
control quotients decline this optional enclosure. Literal projection,
translation, directed rounding, derivative quotient, C0/C1 and denominator
obligations remain independent. Hull scratch and work coexist with the original
basis/quotient evaluation; no control dehomogenization becomes global admission.

Native vertical-product reuse consumes continuous, closed one-sided first jets,
including parameter-speed joins. Exact zero vertical normal and a nonzero whole
normal hull must hold on every retained closed cell. Once one non-outside regular
side cell is witnessed, a complete carrier bound needs no further trim-membership
work. Unresolved carrier cells retain trim exclusion before refusal or refinement;
resource/cancellation remain terminal. Two exact native cap levels and complete
paired boundary flow remain independent obligations. This path
consumes no Hessian and grants no C1 authority to other queries; positional jumps,
uncertain verticality and unresolved normals still refuse.

Required native analytic-torus height/critical queries remain open. Their native
STEP chart uses degrees, independently of original STEP radians; periodic phase
ownership and selected boundary feasibility must respect that chart. Fitted
native UV curves can also genuinely leave the active carrier sheet. Strict
containment refusal remains binding: a periodic alias, wider numerical interval
or nearby physical trace cannot repair the stored chart. Qualification requires
a construction-owned chart correction and complete consumer acceptance; these
required positives must not be relabeled as unsupported to clear the gate.

Endpoint ownership consumes native **material** support, with one prepared owner
shared by its world-Z and XY queries. Authoring-only chart support cannot supply
that material receipt. Ordinary native lines retain their complete endpoint hull;
the kernel's distinct short-domain branch does not borrow that proof. Native trim
covers in directional queries can intersect evaluated positions with independent
whole-coedge control hulls, reused only within their owning immutable query.
They do not redefine the prepared face chart used for continuation/incidence.
These exclusion bounds
never supply feasible points or attachment incidence. Exact caps remain distinct
from real one-ULP protrusions, including pose transport. Reference design dimensions
retain their declared physical accuracy; converted native endpoints need not put
an ideal dimension inside an arbitrarily tight native support bracket.

The frozen spline housing has physical section regressions below and above its
blind-pocket floor at Z3/5/7, including reversed plane normals, pocket dimensions
and source lineage. This does not qualify complete-height sections, directional
support or display: the Z9 separation, pole-junction and display obligations
remain independent. Reproduce with `cargo test --locked -p spiling-geometry
unchanged_spline_housing` and the unchanged corpus inspector.

The importer materializes the decoder's optional `Loop.edges` cache once from
bounded, reciprocal, owner-checked Loopuse/Edgeuse cycles and charges its retained
capacity. Consumers still cross-check native incidence; the cache does not supply
original-source correspondence or material authority.

[`import/closed_spline_decode.rs`](../../crates/geometry/src/import/closed_spline_decode.rs)
admits the narrow closed-spline decoder case with exact complete endpoint-row
and weight identity, multiplicity equal to degree and one inactive exterior
knot per end. A shared whole-active-domain basis identity permits temporary
decoder normalization; exchange/source coefficients are restored before capture.
Native construction retains exact operand copies, not analytic recognition, and
restores only checked inactive slots before topology publication. Declared
PCurve/seam genealogy remains immutable; generated charts are not original
declarations. Import, save and source-deleted reopen do not qualify display,
sections or manufacturing; their active-chart and coverage gates remain separate.

Before native conversion and chart-map construction, exact adjacent null uses
may cancel only for the same edge, opposite directions, identical PCurves and
exactly two uses in one face/wire. Owned normalization preserves surviving
coefficients, radial order and face provenance; only unreferenced derived data
disappears, and affected chart domains only narrow. Genuine periodic seam
branches remain. Complete native-use and operation-specific material admission
still apply; retained original-use operands remain available to finish checks.

Import bounds are complete, conservative native enclosures for scene fitting
and coarse exclusion. The importer evaluates retained face-carrier domains,
edge restrictions and vertices with one cancellable coefficient-work allowance;
it neither invokes the native tight-box solver nor substitutes a vertex hull.
Whole carrier rectangles include the decoder's UV padding and can over-enclose
even planar trimmed faces. These bounds are not precision measurements, source
uncertainty claims or material receipts. Physical-extent controls use certified
`projected_bounds` on literal native trims at their declared endpoint uncertainty;
placement and manufacturing retain their independent source-material queries.

[`parametric.rs`](../../crates/geometry/src/parametric.rs) and its providers own
bounded native surface/curve evaluation and separate original-operand queries.
Unclamped splines use their regular domains, not exterior knot support. Closed
knot queries retain both incident pieces; outward parameter bands are not
feasible point witnesses or C1 authority. Whole-range positional correspondence
does not prove relative tangents, trim topology, inverse coverage or native
incidence. Samples and native tolerance fields do not discharge those obligations.

Generic `FaceInfo.geometry` may carry optional exact analytic metadata. That
metadata describes a face; it does not authorize a finish, slab reuse or a
family-wide operation. Current import/display profiles and wire ownership are
specified by the [RFC](../adr/004-native-geometry-parity.md) and
[display protocol](../protocol/display.md).

## Prepared owners and placed inspection

| Owner / operation                 | Invariant and navigation                                                                                                                                                                                                                                                                                                                                       |
| --------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `PreparedSolid` / `prepare_solid` | [`analysis/material_source.rs`](../../crates/geometry/src/analysis/material_source.rs): one definition-borrowed owner for immutable native material domains, bounds, section operands and positional query maps. No duplicate BREP or original-material reconstruction.                                                                                        |
| `PreparedSectionSource`           | Definition-borrowed native intersection owner in [`section/mod.rs`](../../crates/geometry/src/section/mod.rs), retained by `PreparedSolid`; raw inspection does not require original STEP correspondence.                                                                                                                                                      |
| Native face sections              | [`section/atlas.rs`](../../crates/geometry/src/section/atlas.rs) exhausts local graph coverage; [`cuts.rs`](../../crates/geometry/src/section/cuts.rs) owns whole-line roots; [`continuation.rs`](../../crates/geometry/src/section/continuation.rs) assembles native trim/material incidence and checked spans. The single-graph case remains in `direct.rs`. |
| Placed queries                    | [`analysis/placed.rs`](../../crates/geometry/src/analysis/placed.rs): actual accepted quaternion-polynomial transforms, not an assumed exactly orthogonal transpose.                                                                                                                                                                                           |

`NativeSection` carries plane/frame, face lineage, exact single/piecewise native
surface-plane expressions and achieved chord/plane errors. Internal chart limits
retain exact cut-root expressions, not invented BREP edges. Owned chart capacities
are charged and moved through assembly. A raw intersection alone is not closed-slab
occupancy; conservative envelopes cannot be promoted to manufactured layers.

Sections intersect the retained native carriers directly. Complete root/span
inventories, original face lineage, ordered closure, Jordan and achieved
chord/plane checks remain independent requirements; unresolved tangencies or
branch ambiguities refuse rather than synthesize a closed loop. There is no
SolidCore Boolean-clipping or old-kernel fallback.

Native scalar roots may retain one continuous parent across an uncertain
subdivision-point value. Strict opposite outer endpoint signs prove existence;
a complete same-sign derivative cover proves uniqueness before either child is
published. Refinement intersects fresh derivative bounds with that whole-parent
cover. A partial, zero or mixed-sign cover supplies no authority; existing
isolation/refusal, depth, cumulative work and cancellation remain in force.
No split-point snap or plane shift is used.

Spatial edges and chart lifts keep independent parameter gauges. Native
correspondence proves a complete directed position map with strict interior
monotonicity; stationary outer endpoints need coefficient proof, not a forced
secant slope. This produces no source-relative tangent receipt. Chart/spatial
roots still require independent complete inventories, ordered bijection and
actual endpoint-gap checks. Original-source calibration retains its stronger
relative-tangent contract and cannot consume the weaker native position proof.

Native rational first/higher jets retain axis-constant homogeneous components
from the actual active control net, independently on each original knot piece.
Differentiating a constant component annihilates it; recentering still retains
the weight derivative, so constant raw coordinates with varying weights do not
become constant Cartesian coordinates. Perturbed controls and both incident
closed-knot sides remain enclosed. This tightens arithmetic bounds without a
geometry-family flag, C1 promotion, tolerance change or persistent cache.

The prepared section owner retains achieved whole-coedge positional bounds by
immutable native definition/edge-use. `PreparedSolid` warms complete native maps
before shared-owner byte snapshots and seeds those existing receipts only after
exact surface/UV owner/curve, C3D and full-range validation. Equal source IDs on
different definition owners are insufficient. Only an equal or looser error
request reuses the achieved bound; absent, mismatched or insufficient evidence
retains fresh calibration. No map derivatives or original-relative tangent
authority are exported. Actual retained capacities remain charged; chart
containment, plane roots, crossings, endpoint gaps, sampling and cancellation
stay fresh. Standalone inspection uses the same algorithm without material map
warming; there is no process-global or persistent cache.

Graph isolation contracts each transverse search leaf to a necessary zero-set
enclosure while retaining conservative endpoint slack. The midpoint is not an
existence witness. Closed knot coverage, final whole-hull derivative direction
and whole-independent-interval root existence remain mandatory.

Raw section success does not establish whole-material occupancy. Slabs bind the
actual reference to native face/coedge/vertex analysis, placed-world proof and
planner conversion. Chord error, placement arithmetic, plane drift and complete
native-shape bounds retain separate budgets; resampling a hole or retrying at a
different precision cannot silently change the contract.

Native definitions contain non-`Sync` kernel caches. The engine confines them to
owning workers rather than adding a reentrant carrier lock or cloning the BREP
for each query.

[`parametric.rs`](../../crates/geometry/src/parametric.rs) separates batched
`SurfaceFunctionalEnclosure` and spatial `SurfaceFirstEnclosure` from full
second-order jets. Continuous one-sided first bounds are not C1 or Hessian
authority; strict smoothness and second-order consumers retain their own queries.
Taylor qualification treats a spline's terminal coefficient extension as one
piece, not an interior C0 join. Genuine internal and reflected uncertain knot
bands retain their own closed-span continuity/smoothness obligations.
Height-event curve census likewise distinguishes strictly internal parameter
knots from the selected coedge's closed endpoints. The complete knot visitor
retains endpoints, but only internal joins have two nonempty neighborhoods;
physical endpoint vertices keep their separate source-incidence inventory.
This does not promote a C0 parameter join to a physical critical event or erase
an internal extremum.
Constant-height admission follows the complete selected curve, not an unselected
spatial line or its entire surface carrier. Whole-operator C0 and an identically
zero projected first hull can prove constancy; only then may a tight anchor
bound its height. Small drift and disconnected constant pieces are not constant.
Native work is debited on refusal and cancellation as well as success.
Optional surface-to-curve reductions must decline outside the reduced curve's
regular domain, preserving the full tensor evaluator's extension semantics.

Actual active-net axis/component identities may annihilate homogeneous
derivatives separately on each original knot rectangle. This is coefficient
equality, not a cylinder label or near-zero test. Recentered components retain
the weight-derivative term when weights vary; rational division, mixed jets,
closed-knot sides and denominator/weight checks remain independent.

Closed-height contraction in `analysis/slab.rs` computes a necessary parameter
cover, not occupancy or containment. Only proved whole-box height containment
and trim `Inside` survive subset refinement. The radial atlas refines actual XY
metrics in XY, while unresolved radius/determinant obligations retain spatial
refinement. Native scalar facet support preserves coefficient correlation;
[`material_link/fiber.rs`](../../crates/geometry/src/analysis/critical/material_link/fiber.rs)
uses uniform affine-family root proofs after native closed-span C1 qualification,
not sampled roots or smoothness manufactured by subdivision.

Feasible support witnesses belong to actual native vertices and qualified
spatial restrictions on the captured face/use inventory. A carrier rectangle,
UV overcover or positional error tube supplies no feasible material point.
Inherited trim `Inside` applies only to verified subset cells of the same
immutable face. Complete coefficient/trim coverage, outward bounds, cumulative
work and cancellation remain mandatory.

[`tessellation/mod.rs`](../../crates/geometry/src/tessellation/mod.rs) obtains
bounded SolidCore mesh candidates. Its `audit` and `trim` modules independently
check whole-triangle carriers, normals, native trim/triangle coverage and packed
output. Candidate deviation estimates and generated UV curves are not original
STEP declarations or manufacturing authority. Unqualified collapsed/singular
coverage refuses display rather than dropping triangles or publishing an empty mesh.

Singular-chart publication binds actual native vertex/edge-use ownership to the
exact coefficient collapse law and one-sided coefficient normals. Missing curved
fans must prove complete transport to the same retained mesh spoke; shared IDs
or zero chord area alone do not establish coverage. Fan and ordinary triangle
carrier bounds combine over disjoint coverage, while native-to-chord error
remains independently additive. No vertex snapping or raw-mesh fallback supplies
these proofs.

Display trim intervals retain immutable validated coefficient metadata through
`PreparedValidatedCurve`, not sampled jets. Each query still evaluates every
active closed original knot span, including both incident sides at exact knots,
with its actual controls, denominator, derivatives and charged work. Interval
owners and construction scratch share the existing bounded audit workspace.

Trim deviation bounds geometric traces, not equal-parameter interpolation.
Project complete coefficient images onto the finite chord, retain endcap
overshoot and actual curve/chord endpoint gaps, and require complete C0 for
reverse coverage. Closed C1 leaves may use composed first/second derivatives;
C0 knot leaves use position enclosures only. Lifted PCurves use physical 3D
positions and jets. A nonuniform parameter speed must not force mesh refinement
when the complete geometric trace already satisfies the unchanged error cap.

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

One `PreparedSolid` owns native query preparation for a captured definition.
`material_bounds`, placed bounds/height analysis and slab queries borrow that
same BREP; repeated XYZ queries do not rebuild it or reselect implicit
branches. Unposed directional bounds retain the frozen combined 32 MiB
owner/query cap. The standalone material-bounds function composes the same
prepared path for one-off consumers.

## Native material and operation proof boundaries

The implemented candidate uses the imported SolidCore BREP as the single
DEVELOPMENT material authority. Results bind source identity, exact kernel
producer, canonical millimetres, tolerance policy, occurrence pose, closed query
scope and achieved approximation bounds. `MaterialRealizationIdentity` in
[`kernel/mod.rs`](../../crates/geometry/src/kernel/mod.rs) identifies this native
material contract, not a selectable original-source transport recipe.

Original STEP bytes, canonical face/use provenance and finish-authoring operands
remain immutable. They do not constitute an independently reconstructed material
or original-STEP topology/embedding certificate. Query caches, derived charts
and positional maps cannot change the BREP or silently heal a failed query.
Native trace/connectors are uncertainty covers, not added material. Effective
native tolerance includes the pinned kernel policy and actual entity zones;
a tolerance label does not replace a complete measured query bound.

- **Support and uncertainty:** [`analysis/bounds.rs`](../../crates/geometry/src/analysis/bounds.rs)
  and [`analysis/placed.rs`](../../crates/geometry/src/analysis/placed.rs) separate
  inspection enclosures from native-material contact receipts. Native vertices
  and spatial restrictions provide feasible witnesses; complete carrier/trim
  bounds exclude competing extrema. Independently feasible extrema may prune
  another complete enclosure without creating a witness on that face.
  Actual affine coefficients, pose arithmetic, closed height limits and plane
  drift retain directed error accounting; process-scale padding is not a
  substitute for the placed query's arithmetic bound.
- **Regular slabs:** [`analysis/slab.rs`](../../crates/geometry/src/analysis/slab.rs)
  binds the actual material, closed interval, sample and shape policy. Complete
  native face, physical coedge and vertex coverage is required. Whole-control
  scalar support may exclude an inactive coedge only with the actual complete
  native law; endpoint samples and near-zero derivatives are insufficient.
  Height invariance and regular continuation do not cross unqualified critical
  brackets. [`placed/material_coordinates.rs`](../../crates/geometry/src/analysis/placed/material_coordinates.rs)
  captures actual physical coordinates, not a nominal orthogonal surrogate.
- **Critical incidence and collars:** [`analysis/critical.rs`](../../crates/geometry/src/analysis/critical.rs),
  [`source_incidence.rs`](../../crates/geometry/src/analysis/source_incidence.rs)
  and [`analysis/collar.rs`](../../crates/geometry/src/analysis/collar.rs) consume
  actual native face/coedge/vertex ownership. They retain complete closed-cut,
  orientation, shape and neighboring-material obligations. A scalar event cover
  or loop count does not discharge births/deaths, a pocket floor, disappearing
  voids or attachment. Missing/ambiguous incidence refuses; this is not an
  independent original-STEP material-embedding claim.
  Exact regular vertical-tangent face cells have no transverse vertical-ray
  crossing: their boundary-point cover retains their own enclosed height image.
  Uncertain or degenerate normals retain the complete shadow. All other faces,
  closed event/port ownership and the filled-sample obligation remain mandatory.
  A single exact-height, single-wire native coface may join hole-free disjoint
  components to one, or split one into them, only with complete paired ports,
  no other interface event/critical band and independently proved whole actual
  child-in-parent nesting. General material merge/split arrangements, holes,
  cycles and partial overlap retain explicit refusals. Frozen original bridge
  coupons in `fixtures/manufacturing/bridges/manifest.json` exercise the native
  source and the real compiler/fresh-replay boundary; they are not calibrated
  physical bridge limits.
- **Filled-material metrics:** [`critical/material_link/radial_product.rs`](../../crates/geometry/src/analysis/critical/material_link/radial_product.rs)
  and [`joint/radial_metric.rs`](../../crates/geometry/src/analysis/joint/radial_metric.rs)
  qualify the actual used polygon over its complete native scope. Directed,
  boundary-only or core metrics cannot stand in for a two-sided filled-material
  result. A regular product does not upgrade the critical endpoints or supply
  unsupported void/floor authority.
- **Protected voids:** [`analysis/voids.rs`](../../crates/geometry/src/analysis/voids.rs)
  keeps capture, projected extent and physical height/attachment scope distinct.
  Complete C0 piecewise first bounds suffice for the implemented column normal
  and crease-tangent consumers; rank, cap orientation, exact joins or vertical
  creases, native paired ownership and floor checks remain independent.
  Deferred projection is not permission to consume an unproved void scope.
  Native noncolumn torus critical-material operations remain unqualified.
- **Final composition:** [`analysis/joint.rs`](../../crates/geometry/src/analysis/joint.rs)
  owns native leaves, union captures and `NativeUnionBindingLedger`. Receipts
  bind source/material, occurrence, pose, interval/sample, policy and actual
  candidate geometry. Changed polygons and stale captures inherit no authority.
  Registered actual material/void loops and the complete collar predicates in
  [`joint/planar.rs`](../../crates/geometry/src/analysis/joint/planar.rs) remain
  necessary; a registered regular pair is not whole closed-cut incidence.
  Endpoint-local products or conservative support-exclusion rectangles are not
  whole-leaf, no-void, anchor or deposited-support certificates.

Native material reuse does not establish realized finish, process coverage or
deposited support. Those retain independent
[manufacturing obligations](../protocol/manufacturing.md). Unsupported,
indeterminate, ill-conditioned and invalid operations refuse explicitly under
the same work, byte and cancellation gates.

## Original STEP operands and bounded finish correspondence

Retained original operands preserve literal coefficients, face/use/vertex
identity, finite restrictions, selected masters and PCurve association order.
They support bounded source inspection and the existing planar/full-cylinder
finish lanes, not a second manufacturing material. Native query maps, generated
UV and proximity cannot fabricate original declarations or stronger original
relative-tangent correspondence.

[`analysis/source_restriction/`](../../crates/geometry/src/analysis/source_restriction/)
and [`kernel/source_operand.rs`](../../crates/geometry/src/kernel/source_operand.rs)
retain the original providers. Actual finite domains, knot bands, denominators,
units and selector gauges apply even for literal-zero residuals. Profile
extrusion magnitude remains a parameter scale, not another physical-size scale.
The importer shares the original-carrier reader with source capture and retains
canonical face order. A proved circular-extrusion/native gauge does not turn
generated native angular/axial UV into the original radians/scaled-v declaration.
Generic extrusion import/display/section availability is not generic-extrusion
Compile qualification.

For an original-operand query, missing declared PCurves require the implemented
bounded inverse, not a nearest branch. A plane inverse uses the selected spatial
expression and the coefficient-proved nondegenerate Gram map; the narrow
cylinder inverse requires actual same-frame axial-line/coaxial-circle evidence,
owned-rim angular lifts and complete forward correspondence. Derived evidence
never overrides a failed declared branch. These restrictions do not make
original correspondence a prerequisite for querying native material.

Endpoint-root pairs and tangent samples only propose PCurve branches. Each
qualifying branch retains its own selected-domain, complete world-position and
relative-derivative proof; distinct qualified branches refuse, never select the
first match. Native positional calibration cannot construct the stronger
`SourceCurveCalibration` receipt. Original source and native spatial parameters
remain independent gauges, including periodic selectors and finite trims.

Coefficient fences and exact stationary parameters are not exact vertex
coincidence. Full physical residuals remain charged; interval overlap or a small
box cannot collapse a joint. Recheck representable candidates after contraction,
and retain actual branch/endpoint coverage. A common whole-turn UV
Circle/Ellipse translation requires independently proved endpoint roots and
strict closed-hull uniqueness; world periodicity does not make a UV LINE periodic.
Numerical carrier-derived windows acquire no selector-proved domain token.

Original chart errors are achieved upper bounds, not new admission tolerances.
Whole-image/domain, joint, orientation and actual native-association checks
remain independent. An original boundary point in a query box proves only
conservative `Boundary`, not winding, an interior or source-hole nesting.
Numerical cuts are not new source vertices; complete C0/monotone germs and
whole-span separation cannot be replaced by welding uncertain endpoints.
`FACE_OUTER_BOUND` is an explicit marker; generic `FACE_BOUND` is not a declared
hole. Source-use census retains actual incidence rather than coordinate matches.
Only an exact coefficient compose-back identity can claim zero residual;
positional native maps and small measured error do not.

Native material realization identity 6 and candidate planner25 distinguish the
new authority from published source5/planner24. The source hash, pinned kernel,
query policy and captured original uncertainty remain identity/provenance
inputs. A preferred-master or declared-association change still invalidates
affected finish/source-operand receipts; it never selects an alternate material
for native queries. Older artifacts gain no current inspection/export authority
by relabeling.

`PreparedSolid` readiness means native operands are prepared, not that a slab,
whole material/void result, attachment, finish or manufacturing operation has
passed. Missing original associations can refuse the separate operand/finish
path without disabling unrelated native import, inspection or admitted queries.

## Refusals and support boundary

`GeometryWorkBudget` in [`analysis/mod.rs`](../../crates/geometry/src/analysis/mod.rs)
is cumulative across retries/subdivision. Prepared sources, pinned proofs,
coexisting query scratch and output have separate retained-capacity accounting;
a packed-output allowance cannot fund native preparation. These are not RSS
limits: native intermediate objects also require host-process memory bounds.
Resource exhaustion, cancellation, unavailable parameter correspondence,
conditioning, unsupported operations and invalid geometry remain distinct failures.
No partial proof, stale job or failed display grants successful material authority.

This is DEVELOPMENT geometry, not transferred kernel qualification.
Historical Monstertruck or earlier SolidCore receipts do not establish the
source6/planner25 operator gate. The unchanged original fixtures and workflows
remain reproduction inputs; match the actual workspace binaries and producer
identities before interpreting results.

Prepared native housing sections preserve material before/after the corner event
and the pocket through the rounded cap under plane reversal. A necessary carrier
root overcover is not existence authority. A complete strict height derivative
and whole native boundary coefficients on one strict side exclude the trimmed
face even when its inflated carrier box crosses the plane. Otherwise, a face
without trim incidences still requires complete carrier-root exclusion; missing
crossings alone grant nothing. Packed section points are relative to the captured
plane origin: compare physical cuts only after restoring that world origin.

Preserved operation requirements include the ordinary housing, source-zoned
centered physical plate, fine/genuine combined H1 and tapered fitting through
placement, Preflight, Compile, independent saved-byte Verify, export/save and
byte-identical source-deleted fresh-engine recovery. Fitting recovery may retain
a geometry-only scene with its immutable program; that does not create a source
display mesh. These operator gates remain separate from physical qualification
and the explicitly unqualified journeys below.

Housing corner/cap, primitive/void and native section controls retain complete
root existence, area, orientation and lineage obligations. Missing trim
crossings alone do not prove a face empty. General STEP and unqualified critical
material interfaces remain outside support. The original bracket remains
manufacturing-unqualified; its separate W1 completion is not claimed here.

Standalone fitting/housing section queries and original bracket cut controls
retain their frozen work/source/output limits. Literal native-chart bounds use
the complete coefficient-backed trim hull rather than an arbitrary large natural
plane domain; that exclusion cover never supplies an `Inside` witness.
Nonclosed or ambiguous native coefficient circuits can still refuse. There is
no original-material reassembly, implicitly healed raw chart or mesh-success
fallback. A prepared positive waives no unrelated native-query, material-scope
or resource obligation.

The workspace has no Monstertruck runtime, development, test or tooling dependency.
Retained historical STEP provenance may name that producer; the data is not a
linked backend or fallback. Immutable hashes/licenses are checked independently of
current geometry acceptance.

Bounded original-source rectangular-wall and full-cylinder finish certificates
retain finite edge intervals, selected-master authority, source frames and
orientation/refusal checks. Source-only support exclusion does not require a
native LINE enum or raw UV-chart qualification; it also does not supply a raw
directional-bounds receipt or waive separate exact-arithmetic/domain refusals.
The full-cylinder and canonical box planar-rib operators have independent
emitted-program, history and source-deleted recovery evidence within the
[bounded manufacturing envelope](manufacturing-evidence.md#current-support-and-refusal-boundary).

General STEP documents/assemblies, unproved charts, arbitrary freeform finishes,
singular/nonmanifold material and unqualified critical interfaces remain outside
support. Native descriptors and successful raw inspection do not broaden it.

The vendored STEP reader retains an upstream placed-revolution PCurve failure:
`topology::tests::surfaces_of_revolution_read_as_placed_revolutions` fails in the
standalone default test profile on both the patched snapshot and unpatched base.
Do not suppress this regression or infer general revolution admission from the
passing conformance corpus and narrower native workflows.

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

Use the fitting/housing scene to exercise shared definitions, actual rendered
native-face picking, world section overlays, cancellation/resume and engine/window
lifecycle on the local native GPU surface. Raw section jobs prepare the existing
`PreparedSectionSource` once per definition, not unused material-map owners.
Native correspondence, fresh roots/loops and unchanged work/source/output caps
remain independent gates. Launch `pnpm dev`, import the unchanged fitting/housing
sources, add the declared shared occurrence poses and use the native face list,
viewport picks and world-Z section controls. The [native-window procedure](development.md#native-egui-workbench)
distinguishes host tests from actual display/dialog evidence.

A box CLI result does not qualify that scene, the desktop surface, physical
printability or packaged platforms.
