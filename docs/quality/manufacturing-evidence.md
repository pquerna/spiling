<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Manufacturing: pipeline and support boundary

Manufacturing is **software validation only, not machine readiness or physical printability**.
Import, display, Preflight, Compile and fresh replay are distinct operations; success
at one boundary does not qualify the next. The [protocol](../protocol/manufacturing.md),
executable [contracts](../../crates/contracts/src/manufacturing/mod.rs) and
[native-material contract](../adr/004-native-geometry-parity.md) define authority.

The **SolidCore development cutover is not general manufacturing parity**.
Native box/protected-bore, unchanged ordinary rounded housing, source-zoned
centered physical plate and paired fine/genuine combined H1 journeys have real
CLI evidence through placement, Preflight, Compile, independent saved-byte
verification/export/save and source-deleted recovery. The unchanged tapered
fitting also completes that journey with `complete-sparse.intent.json`, including
fresh recovery of a geometry-only scene and its saved program. This does not
qualify original bracket, affected workbench or broader mechanical-part journeys.
Missing declared PCurves may use a checked inverse of the actual original plane
or admitted cylinder family; generated native UV curves and failed declared
associations cannot substitute. Historical Monstertruck evidence below retains
its original producer provenance, not transferred SolidCore acceptance.

## Entry points and immutable capture

- [`CompilerInput`, `compile`, `inspect_bundle`](../../crates/manufacturing/src/lib.rs)
  borrow captured project identity/revision, native definitions, stored provenance,
  occurrence poses, intent, ordered process/finish documents and canonical images. Compilation performs no
  uncaptured file reads. Semantic fingerprint and producer identity are separate bindings.
  Both compilation and independent inspection bind the complete native producer
  name/version/revision from the same geometry identity used by the engine
  handshake. An obsolete kernel label is not a compatibility alias; a changed
  dependency never relabels saved manufacturing bytes without fresh compilation.
- Engine [`Runtime` manufacturing commands](../../apps/engine/src/geometry/manufacturing/mod.rs)
  capture that snapshot; the [native worker](../../apps/engine/src/geometry/worker.rs)
  resolves retained native definitions and invokes the same crate for CLI and UI.
  Core remains the sole project writer; clients do not slice or emit programs.
- [`preflight::admit`](../../crates/manufacturing/src/planner/preflight/mod.rs) is shared
  by Preflight and Compile: intent, source/face/pose identity, process bounds, finish
  eligibility and bounded native preparation. Preflight returns a revision-bound
  report without artifact/history mutation. `Admitted` is not native layer, wall,
  bridge or support success; required Compile evidence remains not evaluated.
- Workbench [`useManufacturing`](../../apps/desktop/src/manufacturing/useManufacturing.ts)
  owns transient drafts, scoped operation failures and current inspection;
  [`workflow.ts`](../../apps/desktop/src/manufacturing/workflow.ts) derives the next
  explicit action without another authority/state machine. Status must match the
  current project/revision; bundle freshness remains selected-content/session bound,
  so stale Preflight after Compile does not invalidate fresh bytes. Required input
  failures stay separate from optional Compile analysis. Read-only snapshots allow
  Preflight/fresh inspection/export, never Apply or Compile.
- Desktop [`styles.css`](../../apps/desktop/src/styles.css) bounds the accepted
  workspace and independently scrolling control panes around one visible native
  canvas, including narrow native windows. The existing
  [viewport resize owner](../../packages/viewport/src/index.ts) retains device-bounded
  drawing-buffer sizing; layout changes do not request geometry or replay again.

Native geometry work has one fixed, whole-job cumulative 512-million-unit CPU
envelope, including complete original three-dimensional reference-cycle proofs.
Deposited-stroke anchor comparisons retain their cumulative 128-million-unit
ceiling; planar construction/overlay work uses 16,384,000,000 cumulative units.
These bound computation, not geometric
error: no per-source override, retry reset or geometric-accuracy relaxation is
implied. The 100,000 emitted-segment limit is separate from the 64 MiB
manufacturing retained-geometry/staging allowance. A coordinate-equivalent
inventory bound derived from that byte allowance is an early arithmetic guard,
not a command count or a substitute for allocated-capacity accounting. Physical
skin queries reserve all outside owners separately from their own caches and
temporary outputs; pinned shared payloads are charged once and only optional,
unpinned cache entries may be reclaimed. Known old/new containers, path/profile
buffers and metadata coexistence are admitted before growth or publication.
Opaque Boolean-kernel transients and host-process memory remain distinct from
these application-owned receipts. Native byte allowances and depth, chord,
plane and shape-error gates are unchanged. Work units are heterogeneous, not a
universal wall-clock estimate. The engine allows five minutes per accepted native
job, with a separate ten-second cancellation acknowledgement grace.

## One schedule, two references

[`planner::plan`, `scheduled_layers`, `LayerElevation`](../../crates/manufacturing/src/planner/mod.rs)
combine native events, images and support constraints into one physical schedule.
Native refinement mutates it; there is no second slicing grid. Real event boundaries,
printable thickness, command precision and resource limits remain constraints.

[`native_span_reference`, `capture_base`, `NativeSpanSizing`](../../crates/manufacturing/src/planner/native.rs)
separate native proof reference from scheduled layer sample. Singletons use their
scheduled sample; multi-layer spans use the complete interval midpoint. Captures,
source leaves, actual merged candidates and ready frontiers must agree on both
endpoints and native reference after refinement. Image vertical-error checks,
finish evaluation, section/deposition elevations, skin horizons and support/contact
scheduling retain physical-layer samples and semantics.

Event partitions and job-local span sizing propose work, never prove geometry.
Multi-layer proposals shorten before a typed singleton shape-error path may split
a layer. Unsupported, indeterminate, conditioning, resource and cancellation
failures propagate rather than masquerading as successful refinement.

Literal source-height event bands also delimit native capture proposals without
creating physical slice planes. A source-F event is not automatically a selected-G
material event. Contacting physical layers use singleton captures; refinements
recompute proposals from the same schedule. Retained proposal storage coexists
with source promotion and later query scratch; all allowance calculations charge it.

## Source material and reuse

[`PreparedGeometrySource` → `PreparedMaterialSource`](../../crates/geometry/src/analysis/material_source.rs)
moves original domains/plans rather than reconstructing selection. Geometry owns
trim/face/use correspondence, placed source uncertainty and opaque constructors;
raw native queries do not require complete manufacturing admission.

[`native::collect_span`](../../crates/manufacturing/src/planner/native.rs) binds source
leaves and actual union polygons through geometry's
[`certify_native_source_leaf`, `NativeUnionBindingLedger`](../../crates/geometry/src/analysis/joint.rs).
Authority binds source, pose, interval/reference, policy and candidate. Per-source
fidelity is not composed connectivity: closed caps, nonzero collars, births/deaths
and attachment need accepted neighboring material. One-sided leaves and regular
radial cores are not complete closed-material certificates.

- **M:** Layer reuse needs exact invariance or a sealed closed radial material
  product with full metric, topology and actual-polygon calibration, not a family
  enum or one good section.
- **C:** Required reachable material has independent identity. Image finishes must
  retain native M and strict non-image additions; advisories cannot erase either.
- **D:** Requested realized material includes applied finishes. M reuse does not
  establish D invariance; [`surface`](../../crates/manufacturing/src/planner/surface/mod.rs)
  must prove the effective finish independently. Downstream consumers use final D,
  not a source-only substitute. Here C denotes required reachable material, not a
  replacement for the geometry service's continuous-source occupancy proof.

[`process`](../../crates/contracts/src/process.rs) is a separate Core-owned channel,
not a face finish. SparseFraction resolves recipe → ordered definition layers →
ordered occurrence layers, with spatial, layer-local Replace/Erase. Whole-owned
Apply never edits a box winner; its resolved value is the **base**, not a claim of
uniform output. Box Apply moves the unique active scoped box without changing its
operation identity; ambiguous targets diagnose. Reset clears every exact-scope
contribution, including disabled/erased layers. Source/pose/finish/process edits
invalidate the complete artifact and pending jobs.

Native capture retains admitted per-occurrence operands before union, sharing
unchanged singleton geometry. Finish realization produces occurrence final D
before global walls/skins. [`zones`](../../crates/manufacturing/src/planner/zones.rs)
admits one definition-local physical-mm X/Y/Z boundary against the **finished**
enclosure, including relief; the other five box faces must lie outside it.
Local Z remains parallel to build Z. An XY separator additionally requires a fixed
compatible hatch axis. Empty effective selection reports `NoEligibleInterior`. No-op intent
retains the ordinary lattice. Multi-source spatial overlap diagnoses rather than
lending a different source's density, permission or contributors.

The two band cuts share original vertices and cut nodes through a
[source-edge parameter ledger](../../crates/manufacturing/src/planner/zones/partition.rs).
Complete edge coverage, opposite cut pairs, halfspace membership and all cycle
consumption establish disjoint ownership without independently quantized reunion,
area epsilon or positive-sliver discard. Exterior walls, mandatory skins and
source holes retain their original global construction.

`process_regions` records each area's fraction and source/winner contributions.
The explicitly authorized `process_interfaces` band is reserved once, excluded
from both fills and emitted as an **open SeparatorWall**, never a closed perimeter
or support-exempt Bridge. Actual volume is actual stroke/bead volume, not nominal
fraction. [`bonds`](../../crates/manufacturing/src/planner/bonds.rs) freezes actual
shortened endpoints before a layer becomes lower-anchor authority. Terminal
contact uses actual same-source walls, or earlier continuous mandatory skin
itself bonded to a wall; skin-carried genuine Bridge also needs its full actual
span/lower-anchor proof. Current future ribs cannot anchor earlier skin.

Adjacent generators share the definition-origin physical-mm frame transported
with the occurrence. Recovery preserves unanimous immutable mandatory-skin or
regional owner; mixed ownership grants neither authority. Regional genuine
parallel Bridge recovery uses the boundary-aware phase and actual capsule join,
not a rotation/contact waiver. The
[fresh interface verifier](../../crates/manufacturing/src/verifier/interfaces.rs)
checks decoded segments, order, flow, ownership and previous-layer anchors.
Half-bead maximum physical overlap is separate from XYZ6 representation enclosure.

[`diaphragm construction`](../../crates/manufacturing/src/planner/zones/diaphragm.rs)
owns one explicitly permitted centered physical-thickness horizontal plate.
The density boundary and both slab edges enter global balancing alongside actual
native/field stops; downstream process spans split while native M stays borrowed.
Each actual plate interval carries typed source/permission/slab/extent ownership
of its exact solid-construction regional index. Authored lower/upper densities
remain independent, and separate plate area is charged once. Whole-slab strong
`NativeInnerMaterialPrism` authority includes the full bead/XYZ6 radius and native
holes; invariant metadata alone is insufficient. A missing capture refuses.
Checked inward XY construction keeps the original row/axis/phase and raw Z;
lost/split/sub-printable rows cannot be silently discarded.
[`fresh plate verification`](../../crates/manufacturing/src/verifier/diaphragms.rs)
checks actual whole-line raw-extent coverage, protected voids, owners and order,
direct actual lower anchors for the plate and upper landing, and separate actual
wall/hole-wall attachment. Plate-owned residual demands refuse before ordinary
temporary-support generation. H1 retains fine at the plate/intent neighborhood.
The workbench exposes physical slab and path-derived nominal material cost,
with source-hidden actual motion and visible native refusal diagnostics.
See the [owning contract](../protocol/manufacturing.md#centered-permanent-horizontal-plate)
for exact coverage and bounded schedule-only thickness advice.

Playback's **Show source bodies** control hides opaque native meshes and highlights
so an interior layer can be inspected directly in actual emitted-program motion.
Layer/event/travel controls filter that decoded motion, not normalized planned paths.
Hidden bodies are not pickable; clearing or replacing replay restores source visibility.
Replay view controls use the existing bounded working/single-flight admission:
layer, cursor and source-visibility values commit only after the GPU update
succeeds. A refused update cannot advertise a source-hidden draw that never happened.

## Fine exterior, combined eligible sparse interior

[`planner::combined`](../../crates/manufacturing/src/planner/combined.rs) consumes
the actual admitted fine alternative after ordinary analysis, without replacing
the global fine schedule. It stages bounded same-nozzle/same-material regional
SparseFill or genuine regional Bridge paths; walls, skins, holes, relief and the
authorized separator remain fine. Effective source/profile identity crosses
per-layer regional indices; an index alone is not band ownership.

[`NativeInnerMaterialPrism`](../../crates/geometry/src/analysis/joint/inner_prism.rs)
owns full-height inner material for a hashed filled superset. Only direct
per-role radial displacement plus complete filled-map authority qualifies;
a general radial metric seal or outer envelope does not. All native outer/hole
boundaries, including a boundary wholly inside the candidate, participate.
Actual outer XYZ6 Z scope and strict bead/projection/error margin are mandatory.
Final-D/field invariance, skins/events, process ownership and actual deposited
support remain separate gates. The [native decision](../adr/004-native-geometry-parity.md#constant-inner-material-for-eligible-deposition)
and [band contract](../protocol/manufacturing.md#same-nozzle-combined-sparse-bands)
own the theorem and construction details.

Nominal width and physical `[lower,upper]` are path data throughout hatch pitch,
stadium area/volume, capsules, clearance, backend E/flow and independent replay.
H is the difference of true contiguous schedule boundaries, not clamped layer
height or nominal interval count. Actual band-base strokes prove lower anchors;
future coarse never stabilizes earlier fine walls. Each fine separator interval
needs strict nominal inner-core contact plus independent outer overlap consent.
Printing-time fine walls/rows and landing are reproved; a genuine fine landing
Bridge is counted at its actual process feed/flow, not hidden as unchanged timing.
Selecting CombineEligible need not produce coarse output: the admitted fixed-X
plate job records real anchor/neighborhood vetoes, while the separate admitted H1
reference retains genuine coarse acceptance. Plate-boundary exclusion is a checked
contract, not a Fine-policy-only observation.

Only known eligibility/support/core-bond failures retain fine with a reason.
Numerical/topological hatch refusal, indeterminate work, exhaustion, stale work
and cancellation propagate. Replay exposes actual commands separately from
nominal dimensions and bounded band reasons; **Show source bodies** must be off
to inspect the interior. These are associations/model assumptions, not measured
beads, physical adhesion, strength, fit qualification or machine readiness.

For comparisons use the actual admitted fine alternative with identical source,
pose, profile, zones and complete fine Z schedule. Independent native jobs can
adaptively refine differently within declared bounds; same nominal height is
not a controlled schedule. Account the whole command stream, including travel,
prime and retract/recover overhead. Commanded-motion lower bounds exclude
acceleration/heating/homing and are not measured print-time speedups.

## Walls, skins, bridges and supports

[`WallLayout`](../../crates/manufacturing/src/planner/classification.rs) admits wall
centers/fill from D and checks required coverage; neither a nominal bead nor a
surface-resolution advisory repairs unresolved mandatory geometry.
[`SkinNeighborhoods`](../../crates/manufacturing/src/planner/neighborhood/mod.rs)
fixes the innermost-wall-center inset once per Compile and lazily derives masks
from immutable final-D leaves. Erosion distributes over intersection: the same
bounded range cache intersects these masks over physical-distance horizons,
rather than repeatedly offsetting complex horizon aggregates. Bit-exact returning
D leaves share one canonical cache entry without erasing their physical ranges.
Derived leaves, pinned operands, eviction and recomputation share retention/work
accounting; native M/C identity cannot authorize D reuse.
Classification clips to current fill. Skin reaches beneath upcoming pocket walls
with half-bead wall overlap; unchanged vertical sections gain no extra skin.

Required seam, ordinary rectilinear axis and independent top-skin axis use the
existing full-intent Apply/history transaction.
[`HatchAxis::for_policy`](../../crates/manufacturing/src/planner/region.rs)
resolves X/Y or alternating XY/YX from the actual scheduled index. TopSkin uses
its explicit role policy; regional sparse/Separator construction keeps its
source-bound ordinary axis and phase. Template reuse keys the resolved axis
within final-D/classification/physical-height dependencies.
Actual-stroke recovery selects only complete all-genuine Bridge row runs from
the same borrowed role owner/density, rooted in actual original XY demand.
Core ownership is exactly that original polygon intersected with the selected
row bands, including curved/hole-edge cells—not each stroke's endpoint rectangle.
Existing nearest-direct-anchor construction reserves only its actual merged
stroke extent × bead-width corridor across affected roles.
Clip original authored strokes to the exact complement, preserving lattice, role
and holes elsewhere. Merge coincident Bridge rows once, reprove every final Bridge
and the full mixed candidate, and reconstruct after lower/contact strokes change.
No unrelated fully supported row inherits a Bridge label. Actual extra anchor
extrusion enters nominal volume, not a physical porosity claim. Support raster
keeps its separately admitted policy and all geometry/work/flow limits stay fixed.

[`support::realize`](../../crates/manufacturing/src/planner/support/mod.rs) and
[`AnchorWorkspace`](../../crates/manufacturing/src/planner/support/anchors.rs) derive
bridge/contact anchors from actual deposited capsules, never filled domains or an
invented anchor map. Demand-rooted regional Bridge construction may span a genuine
nontrivial uncovered interval between two certified endpoint anchors, including
the original eligible support-contact layer. Contact provenance stays distinct;
fully supported, tiny and one-sided-admissible strokes do not acquire Bridge process.

Support routes exclude continuous-target occupancy, protected native voids and
sampled final-D owners extruded over prior object beads' full actual XYZ6 height.
These D prisms are conservative outer exclusions, not anchor maps. Higher column
portions shrink monotonically around them; the original nearest gap-eligible
contact top never moves down to manufacture contact authority. Half-bead beveled
placement search and outward protected-void query scopes propose candidates only.
Final support/interface capsules must clear continuous target and actual
object/bridge capsules under the shared
[XYZ6 coordinate contract](../../crates/contracts/src/manufacturing/program_coordinates.rs),
including full bead height and unchanged minimum Z/XY gaps.

Mapped-image occupancy uses the shared bilinear interval evaluator over the whole
outward transported requested/native source-Z intersection, with actual period,
phase and row orientation. Local maxima rebuild the original planar/cylindrical
construction envelope, never scale its base. Closed capture pieces meet at the
next representable value above actual XYZ6 bead-height upper enclosures, preserving
native C and independent non-image additions. If conservative outer exclusions
leave no certified corridor, refusal is indeterminate; if no admitted bead is
printable, geometry is unsupported. Neither result establishes exact collision.
Editing a lower finish can invalidate an upper route; regenerated hatches retain
the same D/skin owners.
Auxiliary bridge rows clip positive fill at actual required hatch coordinates.
Required groups use strict physical XYZ6 bead-footprint overlap, not rounded
endpoint equality; tangency and genuine sparse gaps stay separate. Each group
may extend only to a direct previous-stroke capsule anchor inside its positive
row and neighboring required bounds. Internal extensions must retain a positive
physical footprint gap to both required and already accepted neighboring groups.
This adds accounted Bridge deposition, not material-derived anchor evidence or
a dense-fill fallback. Protected holes stay excluded; curved unsupported strokes
do not inherit straight-bridge authority.

[`classification::paths_with_bridge_recovery`](../../crates/manufacturing/src/planner/classification.rs)
also constructs a finite necessary endpoint joint when an authored sparse row
terminates on changing mandatory skin without a real lower anchor. The original
regional core joins adjacent already-required skin only to the nearest actual
previous capsule, as one three-point Bridge with explicit one-transition ownership.
The required skin lattice remains intact; only exact coincident commanded
subsegments are removed. Finite joint overlap and its entire commanded volume
remain explicit, never hidden sparse densification or a permanent interface.
[`verifier::skin_bridges`](../../crates/manufacturing/src/verifier/skin_bridges.rs)
checks actual XYZ6 portions against captured classified domains/holes and the
complete continuous stroke against actual lower capsules after byte association.
Ambiguous/multiple crossings, another regional owner or a separator still refuse.


## Program, fresh replay and publication

[`realization::canonicalize`](../../crates/manufacturing/src/realization/mod.rs) fixes
the normalized plan before [`backend::emit`](../../crates/manufacturing/src/backend/mod.rs).
Perimeter starts select existing vertices at XYZ6 positions from the explicit
nearest/preferred build-XY policy, preserving winding and closure. Exact projected
ties use actual previous-position distance, lexicographic projected/raw XY and
original cyclic index; complete-job prime supplies the initial previous position.
No source pose or camera rotation changes the declared build direction.
The independent [`verifier`](../../crates/manufacturing/src/verifier/mod.rs) parses
actual command bytes and modal state against expected paths, lifecycle, coordinates,
extrusion, feeds and declared process limits; it does not trust emitter comments.
Final coordinate validation rejects ordinary paths off the authored layer axis;
the independent verifier checks preferred seams before consuming expected paths.
`inspect_bundle` repeats this verification and optionally streams bounded actual
motion into [SPLR](../protocol/replay.md), not a visualization of intended moves.

The engine rechecks session/revision/cancellation before native promotion ACK and
Core artifact commit. [Durable publication](../protocol/control.md) commits complete
immutable outputs and terminal status together after ACK; stale/cancelled staging
must not publish, and late cancellation cannot undo a winning commit. Input edits
invalidate the artifact atomically; undo restores coherent inputs and artifact.
[Project storage](../protocol/project.md) embeds source/image/bundle content for
source-independent reopen. A persisted `verified` flag is descriptive, not authority:
open/inspection requires current producer admission and fresh independent replay.
Read-only inspection/export is allowed; compilation publication is not.

## Current support and refusal boundary

Current identities are project10/manufacturing11/process3/surface3, planner22,
emitter7/verifier9 and SPLR5; executable constants and exact native pin remain authoritative.
Native geometry admission, actual wall/skin/support construction, independent
emitted-byte verification and source-backed recovery are separate obligations.
Raw inspection, an older producer's successful job or historical hardware
evidence does not qualify the current producer. Current software admission is
not general manufacturing or G0–G3 qualification; the executable refusal
boundaries and freshly verified outputs remain authoritative.

The SolidCore software controls include native box/protected bore with
`solid-fill.intent.json` and unchanged rounded housing with
`housing-ordinary.intent.json`. Placement/Preflight/Compile, independent saved-byte
Verify/export/save and source-deleted fresh-engine recovery are exercised.
Recovered program/replay bytes remain identical within the qualified producer.
Housing uses the existing variable-radius source sweep, complete local native
section charts, conservative boundary-contact
classification and complete coefficient exclusion of potential carrier roots
without increasing source/error/work limits. A necessary zero-set overcover
does not establish that a root exists. These constructions do not waive source
metrics, void topology or the union gate.

The source-zoned ordinary job retains lower0.20/upper0.60 and the centered0.23mm
plate at sourceZ5.03. Independent replay checks its actual slab intervals,
protected pocket, lower anchors, upper landing and separate wall attachment.
Paired H1 exercises genuine coarse SparseFill and Bridge, fine walls/skins and
separator, native inner-material and actual-anchor/core-bond obligations.
Both jobs retain byte-identical fresh source-deleted recovery; CombineEligible
selection or an all-fine result alone is not H1 qualification.

The unchanged full OCCT cylinder with `solid-fill.intent.json` and source-owned
cylindrical ribs exercises Compile, independent Verify/export, Undo/Redo, save
and fresh source-deleted recovery. Actual extrusion changes while the full-height
XY finish preserves the baseline's placed physical Z schedule. Finish bounds
expand XY only under the exact admitted zero-world-Z displacement witness;
native conservative Z, genuine penetration and XY obstacle refusals remain.
Placed closed-endpoint translation uses the same directed arithmetic as bed
placement; exact contact is not a downward-rounding artifact.

Planar finish certificates do not establish complete planar-rib manufacturing:
the canonical box still refuses its unchanged source-local wall-overlap budget.
Partial finite-turn contraction preserves endpoint recovery constraints, but does
not establish recovery across arbitrary source-cell boundaries. The split-cylinder
baseline's material-bound precision gate also remains separate from full-cylinder
finish qualification.

The default native box also exercises the actual local AMD/CEF production panel:
physical native-face picking, consumed maximum-width Apply/Undo/Redo/recompile,
fresh actual-program report/preview, export/save and source-deleted fresh read-only
reopen. Historical artifacts do not regain fresh authority through Undo.
This bounded workbench result is not qualification of the broader multi-part
geometry section, arbitrary GPU/platforms, packaging or physical deposition.

The tapered-fitting CLI uses the same source/query authority and independent
replay, not a fitting-specific planner. A geometry-only restored scene may
publish its saved manufacturing bundle without a source display mesh; native
operation validation still rejects display artifacts in that case.

Original spline-carrier gauge/correspondence, closed-spline seams, original
bracket, remaining native-query controls and affected workbench remain obligations.

Broader STEP conversion still has explicit closed-edge refusals. Mixed-source
contact, tilted-placement/critical collars, inward-ramp conditioning and broader
support remain outside the exercised operator envelope; the complete generated
parity corpus is not available. Raw inspection is not Compile success.
Specialized finishes remain narrow, and thin-feature/incidence/conditioning/work
failures retain typed refusals. Do not repin tests, relax numerical limits or
reinterpret failures as acceptance.

Consumer-property entry points: [native planner tests](../../crates/manufacturing/src/planner/native_tests.rs)
(material/void preservation, image-dependent D and actual anchors),
[verifier tests](../../crates/manufacturing/src/verifier/tests.rs) (actual-byte tampering),
and [engine tests](../../apps/engine/tests/manufacturing.rs) (fresh source-independent
reopen; stale/cancelled/resource nonpublication). [Development](development.md) owns
build, unchanged real-operator and hardware-runner commands. Preserve owned temporary
inputs, exclusive outputs and source-deleted fresh reopen in those user flows.

For separate-copy process authoring, use `--process-set 1 definition 0.20
--process-set 2 occurrence 0.60` on admitted separated occurrences. CLI resolution
labels `base_whole_owned_resolution` separately from `finite_box_intent`.

The bounded within-part reference is the unchanged
[`occt-rounded-housing-mm.step`](../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step):
a housing, **not a bracket**. `housing-ordinary.intent.ts` is the ordinary Fine
0.10 mm job with both fill and top skin AlternatingXY. Compile it first without
process edits, then author the lighter lower0.20/upper0.60 interior and centered
plate without changing the ordinary axes:

```sh
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 RAYON_NUM_THREADS=1 target/debug/spiling-cli project create NEW_PROJECT \
  --import OWNED_SOURCE.step --intent fixtures/manufacturing/housing-ordinary.intent.json --center-on-bed 1 \
  --process-set 1 definition 0.60 \
  --process-set-box 1 definition -1 -1 -1 25 19 5.03 0.20 horizontal-diaphragm:0.23 \
  --preflight --compile --verify --manufacturing-out NEW_EXPORT --replay-out NEW_REPLAY.splr --save \
  --engine target/debug/spiling-engine
```

The hardware `--process-diaphragm` journey uses that ordinary profile, revises
only lower density0.20→0.25 with Apply/Undo/Redo/recompile, inspects the actual
required-skin joints and plate, then exercises pose/history/save/source-delete/
fresh read-only replay/export. Required joint overlap is explicit and all volume
is charged; these are not densification settings or physical-printability proof.

The separate paired `housing-h1-fine.intent.ts` and `housing-h1-combined.intent.ts`
controls keep the explicitly fixed-X axis required by their XY separator. They
share physical process/schedule/phase and differ only in sparse combination intent.
Do not use that axis-constrained control as the ordinary default or infer coarse
success from selecting CombineEligible. Generate inert data with `pnpm manufacturing:intent
fixtures/manufacturing/housing-h1-combined.intent.ts NEW_INTENT.json`. Use an owned
byte-identical source copy and exclusive output paths for that control:

```sh
RAYON_NUM_THREADS=1 target/debug/spiling-cli project create NEW_PROJECT \
  --import OWNED_SOURCE.step --intent NEW_INTENT.json --center-on-bed 1 \
  --process-set 1 definition 0.20 \
  --process-set-box 1 definition -1 -1 -1 8 19 11 0.60 separator-wall \
  --process-resolve 1 --preflight --compile --verify \
  --manufacturing-out NEW_EXPORT --replay-out NEW_REPLAY.splr --save \
  --engine target/debug/spiling-engine
```

Native center-and-bed placement supplies the actual pose; do not author a guessed
Z reserve. Edit bounds/fractions with visible Apply, inspect explicit permanent
separator approval, then Undo/Redo. Save, remove only the owned original source
copy, and use fresh `project inspect --verify --manufacturing-out NEW_DIRECTORY
--replay-out NEW_SPLR` to inspect/export read-only from stored program authority.
The hardware runner's `--process-zones --combine-sparse --manufacturing-source
fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step
--evidence-directory NEW_DIRECTORY` exercises this flow. Launch engine/CLI with
`RAYON_NUM_THREADS=1`; this bounds execution, not geometric tolerances. Turn
**Show source bodies** off for actual coarse-height/owner and retained-fine
inspection; opaque source PNGs are not evidence.

Measure ALL adjacent actual regional rows in source-relative interior windows:
pitch is `((width-height)*height + pi*(height/2)^2)/(height*fraction)`. Separately
check one kind13 rib, actual terminal/side bonds, unique extrusion, E8/XYZ6
enclosures and protected voids. New fill/rib capsules must not enter the analytic
pocket. Existing ordinary native contour conformance uses the plan's declared
sampling/offset budgets, not requested vertical shape error or physical contact
slack; this is bounded hole preservation, not an exact-circle claim.

## Physical qualification remains separate

The [reference printer](../../fixtures/manufacturing/software-marlin.printer.ts) is unqualified software data.
Freeze actual firmware/configuration, machine/nozzle/material, measured calibration, exact intent/source/program/fresh report and evaluation criteria.
The [process contract](../protocol/manufacturing.md#printer-and-recipe-data) requires
compatible homing, extrusion, cooling-fan/retraction and thermal behavior; declaration
hashes are not hardware truth. Supervise first-layer/extrusion calibration, dimensional
and protected-hole coupons, roofs/bridges and support removal against fixed tolerances;
retain failures and raw measurements. Check prime/travel/hop/park clearance and shutdown
on actual firmware. Replay proves no sensor truth, airflow, swept-tool collision freedom
or bead physics. No execution connector or software report grants permission to print.
