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

## Entry points and immutable capture

- [`CompilerInput`, `compile`, `inspect_bundle`](../../crates/manufacturing/src/lib.rs)
  borrow captured project identity/revision, native definitions, stored provenance,
  occurrence poses, intent, overlays and canonical images. Compilation performs no
  uncaptured file reads. Semantic fingerprint and producer identity are separate bindings.
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

Native interval work, deposited-stroke anchor comparisons and planar overlay work
each have an independent cumulative 128-million-unit ceiling. These bound
computation, not geometric error. Prepared sources, live query scratch and retained
outputs still share their existing byte allowances; host-process memory limits
remain separate. The engine allows five minutes per accepted native job, with a
separate ten-second cancellation acknowledgement grace.

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

Required job-wide seam and rectilinear-axis intent uses the existing full-intent
Apply/history transaction. [`HatchAxis::for_policy`](../../crates/manufacturing/src/planner/region.rs)
resolves X/Y or alternating XY/YX from actual scheduled index. Template reuse
keys the resolved axis inside the existing final-D/classification/height identity.
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

Current identities are project6/manufacturing7/surface3, planner11, emitter/verifier4
and SPLR3; executable constants and exact native pin remain authoritative.
Box, independent tapered-fitting and rounded-housing operators complete Compile,
fresh replay/export, save and source-deleted fresh read-only reopen. Both
independent parts also complete the hardware workbench flow with native picking.
Cylinder/through-hole replay retains nominal material and protected voids;
mapped image consumers exercise final-D walls, fill, skin and actual programs.
These are **not** general manufacturing or G0–G3 qualification.

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
