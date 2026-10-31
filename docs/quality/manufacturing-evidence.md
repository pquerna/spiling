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
[native-first decision](../adr/ADR005-native-first-development-geometry.md) separate
adopted authority from implementation and qualification.

**This implementation uses source8 native-first DEVELOPMENT material authority.**
Current planner/emitter/verifier identities are defined by the executable contracts.
The imported SolidCore BREP is the single M, with original STEP
identity/provenance and original finish inputs preserved. Independent actual
program/process decoding remains mandatory and explicitly reports native-kernel
reliance, not independent certification of original STEP topology/embedding.
There is no custom original-source material reconstruction or fallback.

Qualification is specific to the actual producer and inputs. Historical
source5/planner23–24 and Monstertruck evidence does not transfer to this route.
Fitting, ordinary housing, source-zoned centered plate, genuine H1 and narrow
finish workflows require their complete current-producer operators and fresh
replay/recovery. Native source/void/closed-height/resource controls remain
independent of successful manufacture. Original bracket/W1, deferred noncolumn
torus critical-band slab capability and general mechanical support are not
qualified by the cutover.

The bounded [fixed-dual software route](../protocol/manufacturing.md#fixed-dual-software-route)
adds a recoverable two-material headless job: two explicitly assigned upright native
occurrences, static bindings or independent uniform physical cycles, supported waste,
preselection staging and fresh decoded tool/frame/E/debt/thermal state. Native fine
refinement does not advance physical phase. Full bead reservations and conservative
inactive-tip/carriage-prefix sweeps remain separate from native source qualification.
The source-reviewed Marlin configuration and pressure/material records are declarations,
not deployable firmware, physical nozzle measurements, clean color or printability.
Enabled finishes/process constructions, supports, coarse bands and fixtures refuse this
first route. Original single-tool operator evidence retains its producer/input identity;
the routing increment does not requalify every historical geometry journey.

The [engine G-code comparison](gcode-parity.md) independently audits a bounded
analytic cube matrix against a versioned mature slicer. It retains explicit
refusals and unresolved process/material differences; reference agreement is
separate from the native verifier and physical qualification.

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
- Native [manufacturing forms](../../apps/workbench/src/app/manufacturing.rs) own transient drafts; [host](../../apps/workbench/src/host/) admits typed engine operations. Project/revision ownership and immutable artifact-input provenance remain distinct. Read-only permits Preflight/fresh inspection/export, never Apply or Compile.
- [Manufacture](../../apps/workbench/src/manufacture.rs) retrieves checked bundles and fresh independent motion/report capture. Export rechecks session, observed revision, active record/resource and capture before the shared exclusive output transaction commits. History never restores fresh verification authority.
- Native panels scroll independently around one [wgpu viewport](../../apps/workbench/src/viewport/). Layout/camera/filter changes do not request or regenerate source geometry/replay.

Native geometry work has one fixed, whole-job cumulative 512-million-unit CPU
envelope, including native coefficient, correspondence and closed-capture work.
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

Native height event bands also delimit capture proposals without creating
physical slice planes. Contacting physical layers use singleton captures;
refinements recompute proposals from the same schedule. Retained proposal storage
coexists with source promotion and later query scratch; all allowance calculations
charge it.

## Source material and reuse

[`PreparedGeometrySource` → `PreparedMaterialSource`](../../crates/geometry/src/analysis/material_source.rs)
promotes the same borrowed native BREP trim domains rather than constructing
another material realization. Geometry owns native trim/face/use correspondence,
placed source uncertainty and opaque constructors; raw native queries do not
require complete manufacturing admission.

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
[source-edge parameter ledger](../../crates/manufacturing/src/planner/partition.rs).
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
support remain separate gates. The
[band contract](../protocol/manufacturing.md#same-nozzle-combined-sparse-bands)
owns the current construction details; ADR005 alone does not independently qualify
this operation. Require its actual current-producer operator and replay evidence.

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

Ordinary unpinned regional fill selects at most one default-first quarter-pitch
phase per scheduled layer, conjoined across every free regional piece. It retains
each polygon's nonempty row count/cadence, source owner and holes; it never phases
fragments independently. A geometrically unusable speculative hatch rejects that
phase, not later proposals or ordinary recovery; resource/cancellation failures
remain terminal and charged work is not refunded. Authored definition-origin
grids, mandatory skins and coarse H1 retain their own construction. The selected
phase stays fixed through support reconstruction and participates in template
identity. A lower-layer guide proposes phase only; final actual-stroke anchors,
Bridge and replay obligations remain fresh.
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

Source-bound planar surface color is exercised through the real CLI on the committed box and protected-hole source: one full-height planar face or one/two finite horizontal windows, physical colored skins, shared solid white backing and sparse white core. Two regions retain opposite physical phases at common Z on both native inputs; overlapping backed requests refuse. Compatible adjoining orthogonal faces also construct their shared corner once on both sources, preserve the protected hole, consume equal physical-depth edits and refuse conflicting depths. Shell physical cycles retain white backing/core and produce waste-only tool groups on all-white model layers; both lanes remain supported every layer. A completely white model also retains zero model accounting for the other feeder. Depth/window/phase edits change actual emitted operations; undo/redo and source-absent fresh read-only verification/export retain the captured program and motion. `tools/smoke-surface-color.mjs` resolves returned native face identities, checks region attribution and protected voids, and exercises source/depth/core refusals. The [construction contract](../protocol/manufacturing.md#source-bound-planar-surface-color) owns the finite envelope. This is software construction evidence, not physical color, adhesion or printer qualification.

Retained optical studies apply through typed Core/engine/client/CLI transactions with complete accepted report/selection and explicit two-material evidence/declaration bindings. Queued cancellation and stale revisions preserve the committed project; ACK commits one revision without changing SceneRevision or publishing scene artifacts. Both native source workflows exercise application, physical depth/history edits, changed-pose reassessment and source/palette/study-deleted fresh verification/export. Inspection distinguishes the small requested patch from complete realized side exposure and reuses the existing model with captured evidence; it does not reuse the requested prediction as a printed outcome. Unknown evidence/conditions remain unassessed and backing optics remain unmodelled. The native Review panel supports source-hidden actual replay and catalog metadata tint by decoded tool, plus requested/realized inspection and fresh four-file export. The native window can consume a CLI-produced recovered project; local hardware evidence does not qualify other GPUs/platforms or physical color.

Current identities are project10/manufacturing11/process3/surface3, source8/planner36,
emitter11/verifier17 and SPLR6; executable constants and exact native pins remain authoritative.
Native geometry admission, actual wall/skin/support construction, independent
emitted-byte verification and source-backed recovery are separate obligations.
Raw inspection, an older producer's successful job or historical hardware
evidence does not qualify the current producer. Current software admission is
not general manufacturing or physical qualification; executable refusal boundaries
and freshly verified outputs remain authoritative.

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

The source-zoned XY-separator journey also admits fixed-X ordinary hatch with
independently alternating top skin through native bed placement and fresh
source-independent replay/export. Hole-split orthogonal rows select genuine
Bridge intervals independently of supported siblings, replacing only their
actual bead corridors. The supported siblings retain their authored hatch;
missing actual anchors and over-limit spans still refuse. Reproduce the shared
engine journey with `cargo test --locked -p spiling-engine --test manufacturing
rounded_housing_two_zone_separator_survives_native_bed_placement_and_fresh_export`.
The planner's `demanded_hole_split_bridge_preserves_supported_sibling_and_anchor_limits`
regression exercises both axes and the nearby anchor/span refusals.

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

The canonical `box-mm.step` with `solid-fill.intent.json` and definition-owned
vertical ribs (depth0.1mm, pitch2mm, edge margin1mm, phase0) also completes
Compile, independent Verify/export, Undo/Redo, save and fresh source-deleted
recovery with changed actual extrusion and the baseline physical Z schedule.
Existing sharp-corner fields retain priority; a source-owned nominal-width
inner bevel is a separately qualified finite construction, not extra overlap
credit. Source/void/mandatory coverage, local and aggregate dose, finite-command
resolution and gradient gates remain unchanged. This bounded operator does not
qualify arbitrary planar finishes, image carriers or a new micro-texture family.
The unchanged split-cylinder baseline compiles; split-face finishing still
requires separate eligibility and does not inherit full-cylinder authority.

The [planar-image recovery journey](../../apps/cli/tests/project.rs)
`planar_image_relief_survives_source_deletion_and_fresh_readonly_retrieval`
currently refuses at its first thin image-event layer: local wall dose exceeds
the source-local junction/count allowance. The source5/planner24 baseline has the
same defect; it is not a native-first regression or a qualified image workflow.
Keep the executable journey and its dose gate unchanged. Fixing that wall
construction is separate from this landing.

The additional8×8/depth0.35 cylindrical-image workbench Compile also remains
unqualified. Its captured image/intent/pose refuses on source5/planner24 at
finished-material/bed bounds and on the native-first route during surface
construction; neither proves a shared cause. Actual native carrier selection,
canonical PNG preview and source-independent image authoring/history are
exercised, not successful manufactured output for that parameter set. The
separate cylindrical-image CLI journey retains its own narrower input evidence.

The fixed-Y native consumer
`authored_housing_bridge_recovery_keeps_supported_fill_on_its_axis` verifies
ordinary-axis preservation, genuine orthogonal bridges and unique aligned
actual deposition. The separately captured fixed-Y workbench parameter set
remains unqualified until exercised through its own recovery/export flow;
a shared axis label does not equate its inputs to that native consumer.
The accepted ordinary/physical-plate reference retains alternating XY.

The native operator acceptance uses the actual local Vulkan workbench:
physical native-face picking, Compile, fresh actual-program report/preview,
export/save and source-deleted fresh read-only reopen. The accepted alternating-XY
housing/physical-plate operator also exercises consumed maximum-width
Apply/Undo/Redo/invalidation/recompile, density history, source-hidden physical
slab/pocket inspection and source-deleted fresh replay/export. Historical
artifacts do not regain fresh authority through Undo.
The fitting/housing scene's native picking, world section and lifecycle have their
own [geometry qualification](geometry-evidence.md#minimal-user-flows). Neither
bounded workbench result qualifies arbitrary GPUs/platforms, packaging or physical
deposition.

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
