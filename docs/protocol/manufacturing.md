<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Software-only manufacturing protocol and schema

`spiling-contracts::manufacturing` is executable schema authority; generated TypeScript is the authoring data language. The Rust manufacturing crate owns planar compilation, a narrow emitter and independent replay. Core owns transactions/storage. The engine composes them, and the shared client/CLI consume their results. These are software-validation artifacts, **not machine-ready programs, physical printer support or printability certification**.

Native control uses typed gRPC services; project format10 and manufacturing schema11 are a strict cutover without old-format decoders, aliases or implicit intent defaults. SPLT/SPLM/SPLS remain schema1; [SPLR actual replay](replay.md) is schema6.

Format evolution, producer invalidation and the initial release compatibility boundary follow the [canonical pre-release format policy](../../AGENTS.md#pre-release-format-evolution).

An explicit optional [hardware capture](hardware.md) binds the existing printer
specification to a selected plate face and installed accessory definitions.
Preflight checks conservative material placement, and independent replay checks
actual deposition bead bounds. The personal inventory and network route never
supply manufacturing authority.

## Print-job scope

Every manufacturing control carries the required `PrintLayoutId` in addition to session
and revision. The engine refuses an address different from selected focus and blocks
focus changes during active work. Captures contain only that job's occurrences,
referenced sources and applicable definition/occurrence surface/process/image closure.
Each plate owns intent, options and current output; project-level libraries remain
shared. A retained result can have an older input revision when its exact semantic
inputs remain unchanged. Fresh Inspect independently decodes the stored program and
binds its descriptor before export; input equality never substitutes for replay.

## Authoring and trust boundary

An operator may author a TypeScript data module against generated Rust-defined types. Build tooling may execute that explicitly trusted local authoring module to produce JSON; executing authoring code has the operator's permissions and is not a sandbox. The engine never executes TypeScript: it receives bounded inert JSON, rejects unknown/duplicate fields and trailing JSON, and validates it in Rust. A schema capability declaration does not enable unsupported behavior. Ordinary printer data does not require a new backend; genuinely custom behavior requires separately reviewed Rust implementation.

All names, IDs and revision labels below are nonempty strings of at most 256 UTF-8 bytes, without control characters. Numeric process parameters must be finite and positive. Unknown enum discriminants fail decoding. Profile/intent JSON is at most 32,768 bytes. All fields below are required, including explicit `null` manifest option values.

## Evaluating alternatives

`EvaluateAlternative` captures the current project revision and requires an exact
`expected_context_hash` plus complete alternate intent. The checked Manufacturing
status exposes the current context hash. An option supplies its original context,
not a newly substituted hash. A stale revision, foreign context or incompatible
hardware refuses before construction.

The existing sole Jobs worker compiles a detached Core snapshot and independently
inspects the actual program, producing a checked bundle and SPLR replay. No active
Project mutation, artifact publication, dirty transition or undo transaction is
performed. Read-only projects can evaluate. Failure/cancellation leaves Current
and its last valid output untouched.

`ManufacturingAlternativeEvaluated` returns exact project/revision/context/intent,
record, fresh report/provenance and bundle/replay descriptors. Display-only input
differences are bounded and carry an explicit completeness flag; they cannot
authorize mutation or substitute for the complete intent. The terminal receipt is
checked against the control envelope before publication. All existing source,
construction, actual-program, cancellation and resource checks still apply.

Alternative artifacts are not Current's stored artifact. Viewing or retrieving
them cannot authorize ordinary Current export. Adoption is a separate
revision-bound intent transaction, followed by ordinary fresh output qualification.
Estimated time, nominal material and geometric starts remain software observations,
not physical sealing, strength or printer qualification. See [ADR010](../adr/010-manufacturing-alternatives.md).

With matching built binaries and an existing saved project, the CLI uses the same
services (use a complete candidate intent JSON, not a partial settings patch):

```sh
target/debug/spiling-cli project edit part.spil --engine "$PWD/target/debug/spiling-engine" \
  --evaluate candidate.intent.json --keep-option "Spread starts"
target/debug/spiling-cli project inspect part.spil --engine "$PWD/target/debug/spiling-engine" \
  --options
target/debug/spiling-cli project inspect part.spil --engine "$PWD/target/debug/spiling-engine" \
  --compare-option OPTION_ID
target/debug/spiling-cli project edit part.spil --engine "$PWD/target/debug/spiling-engine" \
  --apply-option OPTION_ID --compile --verify
```

`inspect` is read-only. `edit` saves explicit project edits; inspection/evaluation alone does not apply the
candidate. Use the content-derived ID from the option list rather than its display
name. `--get-option` retrieves complete settings; `--remove-option` deliberately
removes one. `--manufacturing-out` always exports freshly inspected Current,
including when a candidate has just been evaluated. The final JSON retains exact
capture/provenance and comparison completeness. Unavailable duration is `null`,
not an invented print-time estimate.

## Construction goals and protected layer heights

`PlanarPrintRecipe.layer_height_policy` is required: `preferred` or
`protected_maximum`. Missing/unknown JSON or protobuf values refuse; there is no
legacy policy default. `layer_height_mm` always retains the authored request.
The effective height range is the intersection of captured machine/material
limits, additionally bounded above by bead width. Preferred targets select the
nearest value in that range; protected maxima require an admitted requested
height and preserve it as an upper bound. Empty ranges and invalid/nonfinite
parameters remain errors. Fixed-dual physical slots require `protected_maximum`.

Both policies preserve native event boundaries and actual shape, void, anchor,
flow and command obligations. A preferred schedule may use slightly thicker
layers than its selected nominal to keep whole intervals above the captured
process minimum; it never raises the process maximum or changes source geometry.
A valid preferred result may carry a nonrequired Preflight advisory. Eligibility
and draft selection still do not establish completed construction.

`VerificationReport.layer_height` and `ManufacturingSummary.layer_height` carry
one required checked `LayerHeightAssessment`: policy, requested/selected heights,
`requested`/`process_minimum`/`process_maximum` reason, command-projected
`realized_min_mm`/`realized_max_mm`, layer count, differing-layer count and nullable
first differing zero-based index. Counts/index/ranges and reason consistency are
checked. Difference reporting ignores one XYZ6 quantum plus subtraction roundoff;
this descriptive allowance never relaxes geometric or deposition validation.
The range describes scheduled physical intervals after coordinate projection,
not measured physical deposition or strength.

Bundle admission binds the assessment to complete captured intent and plan.
Fresh independent replay validates actual command correspondence and recomputes
it. The ordinary four export files retain request, schedule and fresh assessment;
there is no separately trusted warning file. Goal edits use ordinary intent
transactions, invalidate manufacturing and preserve history/scene ownership.
[ADR008](../adr/008-construction-goals.md) owns the product distinction.

## Printer and recipe data

`PrinterSpecification` fields:

- `schema_version:11`, `id`, `revision`, `name`.
- `coordinate_frame: "right_handed_millimetres"`: fixed Cartesian XYZ, right-handed millimetres. Layer planning uses the bed plane at `build_envelope.min[2]`, not an assumed global Z=0.
- `build_envelope: AabbMm {min:[f64;3],max:[f64;3]}`: finite ordered strictly positive dimensions.
- `components: MachineComponent[]`, at most 64, unique component IDs.
- `capabilities: PrinterCapabilities {motion_space:"cartesian_xyz",extruders,output_dialect,generated_supports}`. Diagnostic `cartesian_absolute_gcode_v1` and complete `marlin2_single_extruder_gcode_v1` require one extruder; `marlin2_fixed_dual_gcode_v1` requires exactly two. Generated breakaway supports remain an explicit single-tool capability. Counts alone never authorize a route or qualify hardware.
- `nozzle_diameter_mm`, required `deposition_limits {min_bead_width_mm,max_bead_width_mm,min_bead_height_mm,max_bead_height_mm}`, `max_feed_mm_s`, `max_volumetric_flow_mm3_s`. Limits are consumed software declarations, not nozzle/material or hardware qualification. Every path's actual nominal width/height must fit them.
- Required `motion_resolution {xy_mm,z_mm}`: the smallest position change the machine realizes, each within [0.000001, 0.05] mm. It sets ADR012's height tolerance: feature heights and schedule boundaries closer than `z_mm` are one boundary, so no scheduled layer is thinner than one Z step. `min_bead_height_mm` of the printer and of every fixed-dual tool must be at least `z_mm`. A declaration, not measured accuracy or repeatability; the reference printers declare 0.0125/0.0025 mm.
- Required `capabilities.arc_moves`: the firmware interpolates XY circular moves given as G2/G3 with I/J centers. Planned arc runs are emitted as arcs only for such a printer, never in the fixed-dual dialect (which refuses the declaration); every other printer receives the same runs as their chords. The Marlin software reference printer declares it, as Marlin 2 builds `ARC_SUPPORT` by default.
- Required `workflow` is `{kind:"diagnostic_body_only"}`, `{kind:"complete_marlin",job:SingleExtruderJob}`, or `{kind:"fixed_dual_marlin",job:FixedDualJob}`; count/workflow/dialect mismatches fail.

`SingleExtruderJob` requires bounded `machine_id`, explicitly named Marlin 2 `firmware_version`, `configuration_hash`, `calibration_record_hash`, `homing_contract:"marlin_native_cartesian"`, `home_position_mm`, `nozzle_temperature_c`, `bed_temperature_c`, `minimum_extrusion_temperature_c`, `maximum_nozzle_temperature_c`, `maximum_bed_temperature_c`, `prime_start_mm`, `prime_end_mm`, `prime_filament_length_mm`, `prime_speed_mm_s` and `park_position_mm`. Hashes identify external declarations, not host approval or physical qualification. Temperatures are positive integral Celsius; requested nozzle temperature is within declared minimum/maximum, bed within maximum, maxima are bounded to 500/200°C and priming to 100 mm filament. All lifecycle endpoints and feeds obey the printer envelope/limits; priming also obeys nominal volumetric flow.

The complete-job firmware contract requires native Cartesian coordinates without workspace/home offsets, skew, leveling or restoration, one active extruder, M149/TEMPERATURE_UNITS_SUPPORT, M200 and M420, and no firmware extrusion modifiers or fixed G0 feed override. Configuration/calibration hashes do not prove these assumptions. Prime segments must avoid conservatively expanded all-layer object XY bounds, regardless of Z, holes or gaps; admission includes coordinate-rounding padding. Final park Z must have representable clearance above every object and prime point.

`MachineComponent` has `id`, `role: "bed"|"toolhead"|"fixture"`, a checked `pose: RigidPoseMm {translation_mm:[f64;3],rotation_xyzw:[f64;4]}`, and `shape`:

- `{kind:"box",bounds_mm:AabbMm}` with positive dimensions in component-local millimetres.
- `{kind:"cylinder",radius_mm:f64,height_mm:f64}` with positive dimensions; cylinder is local +Z with base at local origin.

Pose is proper finite rigid motion with a unit quaternion: no scale, shear or reflection. Fixed component conservative bounds additionally exclude finished surface occurrences; this is not a rendered/executable machine model or a swept-tool collision certificate.

`PlanarPrintRecipe` requires `id`, `revision`, `material_id`, `material_revision`, `filament_diameter_mm`, `layer_height_mm`, `layer_height_policy`, `bead_width_mm`, `perimeter_count`, `infill_fraction`, positive physical `top_skin_thickness_mm`, `bottom_skin_thickness_mm`, `print_speed_mm_s`, `travel_speed_mm_s`, `flow_multiplier`. Perimeters are 1..8; selected and actual layer heights are at most bead width (a preferred target can exceed it); finite global density is in `[0,1]`, with 0 meaning intentional hollow interior, 1 meaning solid interior and positive smaller values sparse rectilinear interior. Zero retains interior/void coverage but generates no centered, alternate or transported interior rows; walls and required skins remain mandatory and roof admission still requires actual deposited anchors. Authored `ProcessValue` density overrides retain their positive-only `(0,1]` contract. Skin thickness uses conservative whole-layer horizons. Print/travel speed must not exceed printer feed limits. Nominal bead area is `(width-height)*height + pi*(height/2)^2`; flow is area × multiplier × print speed and must not exceed the explicit volumetric limit. Filament area is `pi*(diameter/2)^2`. Numeric overflow/underflow that destroys finite positive areas is rejected. These are declared geometric accounting models, not material calibration, strength or measured deposition physics.

`bead_width_mm` is the preferred wall allocation width; `perimeter_count` is a maximum nominal contour-depth target, not a promise of that many closed loops. Automatic local allocation may vary count and width under the captured process envelope. No algorithm selector or variable-width opt-in exists.

Required `process:ProcessPolicy` declares first-layer speed/flow factors, first-layer and later fan PWM, bridge speed/flow/fan, sole authoritative positive `max_bridge_span_mm`, and tagged retraction disabled or enabled with positive distance/speed/minimum travel and nonnegative Z hop. Speed factors are in `(0,1]`; actual per-layer height, flow and feed remain bounded by the printer. Fan PWM is commanded process state, never a cooling measurement.

`supports:SupportPolicy` is explicitly disabled or breakaway with density `(0,1]`, positive XY clearance/interface thickness and nonnegative Z gap. Every saved/wire intent carries the complete explicit policy; omitted fields have no compatibility defaults. New single-tool jobs created from an explicitly selected complete Marlin setup and material profile capture Automatic breakaway supports. The shared resolver uses the selected physical layer height for the minimum Z gap, two such heights for interface thickness, two-thirds of the bead width for XY clearance and 30% body density. Existing intent, including explicit Off, is preserved by profile changes. Disabled support permits no generated scaffold; there is no duplicate support bridge-span authority.

Before wall construction, sampled original native M supplies a typed convex-corner command-height bound from the unchanged nozzle/4 inward extent, offset/fit/XYZ6 reserves and physical width floor. A violated bound may subdivide only its existing physical interval; every original/native/image/material/support endpoint remains fixed, process minima and cumulative work/retention limits remain mandatory, and the narrowed source interval is captured again. This proposal grants no source, wall-coverage or final-command authority and does not change admission of caller-specified thick wall intervals. A bounded startup probe can separately propose finer intervals when the original first physical pair lacks actual lower-stroke anchors; its original upper endpoint stays fixed across retries. Enabled support retains the separate physical unreachable-contact-gap probe. A blocked fixed-XY support corridor may also propose a finer current/preceding physical pair when the actual row is near previous strokes and at least one endpoint has nominal footprint overlap using its actual edge width. Retry the complete construction after splitting only an eligible interval; retain every source/event boundary and process minimum. This proximity test authorizes a proposal, never an anchor, clearance waiver or branching/object-rooted support. All final actual-stroke, source/void and emitted-program checks remain mandatory. These constructions require complete native, actual-anchor and emitted-program admission; later unresolved demand still refuses.

Required `seam_policy` is `{kind:"nearest"}`, `{kind:"preferred_build_direction",xy:[x,y]}` with a finite normalized nonzero build/machine-XY direction, or `{kind:"distributed"}`. It selects existing unique closed Wall vertices, excluding the repeated closing point, without changing winding or source geometry. Compare XYZ6-projected coordinates and normalize signed zero. Preferred maximizes computed dot product; exact ties minimize distance from the previous actual position, then projected XY, raw XY and original cyclic index. Nearest minimizes that distance with the same ties; without a previous position choose lexicographic minimum. The complete-job prime endpoint is the initial previous position. Immutable Wall ownership controls directed wall geometry and closed seam obligations even when the process role becomes Bridge; Bridge still uses its actual speed, flow, cooling and lower-anchor obligations. Open Wall strokes retain their admitted directed geometry and explicit travel without inventing a closed seam. The shared scheduler owns entry choice and wall seams; independent replay reconstructs the policy in actual emitted order, not an assumed wall-first order.

Distributed (native UI: **Spread starts**) maximizes the minimum XY Euclidean distance from the most recent eight closed Wall starts for the same tool, retaining a fixed-size history across actual scheduled layers. An empty history gives all candidates equal score; exact ties use the same previous-position and coordinate rules as above. Non-Wall and open paths do not enter this history; tools have separate histories. This is job-wide bounded start placement, not selected-face authoring, contour tracking, guaranteed separation or sealing. It never changes wall precedence, inserts vertices, reverses a closed wall or changes per-edge width/dose. Independent replay checks authored policy separately from plan/program correspondence.

Required `rectilinear_axis` and independent `top_skin_axis` each use `"x"`, `"y"`, `"alternating_xy"` or `"alternating_yx"` in the build frame. Alternation is keyed to physical courses, not output indices. The base schedule (balanced intervals between events, plus finish-driven refinement) makes each layer its own required `hatch_course`; a later native-resolution or actual-anchor refinement split divides one course into thinner layers that keep it. A layer outside refinement keys alternation by its course. Within a maximal run of layers whose courses were split, the key counts down from the run's top layer, which keeps its course axis; alternation starts with its named first axis at key zero. Every layer inside a run therefore crosses its neighbors, the course above still crosses the run's top, a parallel pair can occur only at the bottom of a run that gained an odd number of layers, and a split never changes any layer above its run. Plan validation requires courses to start at zero, advance by at most one per layer and span at most one admissible layer height; validation and replay derive the key from the declared courses. Ordinary TopSkin uses `top_skin_axis`; bottom/solid and rectilinear sparse fill use `rectilinear_axis`. Grid sparse fill has its own two build-X/Y families. An explicit top-skin direction does not rotate source-bound regional sparse lattices or SeparatorWall. Orthogonal recovery selects only independently certified genuine Bridge strokes from the same borrowed classified role owner/density, rooted in actual original XY anchor demand. At a hole-split coordinate, a genuine demand-bound Bridge interval is independent of supported siblings. It replaces only its actual bead corridor, preserving the siblings' authored hatch; a full row-cell replacement still requires an entirely Bridge-certified row. Existing nearest-direct-anchor construction reserves the merged bead-width corridor inside that owner and clips original authored strokes to its complement rather than recentering them. A supported one-ended alternative may propose a minimum extension to a real second lower anchor, but only fresh genuine Bridge proof can authorize its selection. Reprove all final Bridge rows and the complete mixed candidate, including after lower/contact changes; a fully supported row never inherits a Bridge exception. Support retains its separately admitted policy. Camera/source rotation does not rotate job-wide intent. Policy edits use Core history, invalidate artifacts and enter fingerprints.

`ManufacturingIntent {printer:PrinterSpecification,recipe:PlanarPrintRecipe,material_profile:ResolvedMaterialProfile|null}` validates both declarations and their cross-constraints. `decode_intent(bytes)` bounds input before decoding; direct DTO consumers must call `validate()`.

### Captured single-tool material profiles

`MaterialProfileSelection` is bounded inert data: an exact `MaterialProcessProfile`,
installed `MaterialToolSetup` declarations and explicit `MaterialProfileOverrides`.
Material definitions identify manufacturer/product/revision, polymer family,
optional CF/GF filler/loading, filament diameter and an explicit abrasion-resistant
nozzle requirement. Family names never select algorithms. A source kind identifies
synthetic test data, an imported recommendation or an operator declaration, with
source identity/revision/hash; none is a measured-process qualification.

`Manufacturing.Execute` accepts typed `ApplyMaterialProfile` with session and
persistent base revision. Rust manufacturing resolution validates the exact
printer ID/revision/nozzle diameter, hotend/nozzle-material/cooling configuration and declared nozzle
compatibility; only the complete single-extruder Marlin workflow is supported.
Without an existing intent, an unaddressed Apply requires an explicitly selected complete single-tool Marlin hardware capture. It creates a complete standard planar recipe: nozzle-scaled width and preferred 0.2 mm layers within the common declared envelope, three perimeters, 15% rectilinear interior, 0.8 mm minimum skins and Automatic supports. No printer or material is inferred. An addressed tool still requires its existing routed construction. Creation is one ordinary Core intent transaction; saved return never reruns defaults.

With an existing intent, the operation resolves supported explicit overrides over profile values, updates
recipe material/diameter/speed/flow/process and job thermal targets, then commits
one existing Core intent transaction. It captures the complete selection and
resolver identity in required-nullable `intent.material_profile`. No catalog path,
network lookup or executable profile enters compilation. Values remain declared;
actual installed hardware, cooling effectiveness and dry filament are not sensed.

Resolved snapshots are checked for agreement with the recipe and thermal job on
both native wire and persistence admission. Direct edits to bound values must
update the explicit profile overrides, or explicitly detach the binding with
`material_profile:null`; they cannot retain stale profile provenance. Current
unreleased intent files missing this required nullable field need recreation.
Library updates never reinterpret saved snapshots. Profile changes use ordinary
Core Undo/Redo, invalidate active output, enter the complete input fingerprint and
retain captured-revision stale-publication checks without changing SceneRevision.

The effective volumetric ceiling is the minimum of the unchanged machine ceiling
and selected material/hotend profile ceiling. Recipe, first-layer/bridge, prime,
combined-path, backend and fresh decoded-byte flow checks consume it. Actual path
widths/physical heights must satisfy both machine and selected profile envelopes.
No silent speed/temperature clamp or inferred support permission is supplied.
`ProcessPolicy.max_bridge_span_mm` remains the sole geometric span authority;
profiles do not create anchors, physical grip, measured bridge limits or permission
for new support/interface construction. A whole-process override is explicit and
captured; it does not preserve an empirical claim (no measured-profile source kind
is currently admitted).

The CLI accepts ordered `--intent BASE.json --material-profile SELECTION.json`;
ordinary manufacturing status and exported provenance expose the captured
selection, profile values and overrides. `pnpm test:materials` builds a matching
engine/CLI and exercises synthetic paired profiles, thermal/cooling/feed/flow
commands, Undo/Redo, rejected edits and fresh source/catalog-absent replay/export.
It holds geometry/process fixed while changing synthetic family metadata and
requires unchanged decoded commands. Nearby declared bridge limits exercise
positive native admission versus an explicit unsupported-gap refusal with no
published artifact; these are software test limits, not measured material spans.
The runner uses the independent parity byte decoder. Test declarations are not
PLA/PCTG/filled-material printing recommendations. The separate fixed-dual capture below consumes addressed tool profiles; mixed diameters and physical measured envelopes remain outside this cut.

### Material discovery and process review

`Manufacturing.Execute` accepts read-only `ReviewMaterials` with the current
session and persistent base revision. A bounded inert `MaterialCatalog` carries
one declared installed setup and 1–16 exact process profiles, with unique
profile ID/revision pairs. The complete request retains the 32 KiB profile JSON
budget; search is at most 128 UTF-8 bytes without control characters. There is no
filesystem access, catalog service, script execution or network lookup in review.

Search matches all whitespace-separated terms, case-insensitively, against
profile/material identity, manufacturer, product, polymer family and CF/GF
attributes. Results sort by exact profile ID/revision. Pages contain at most four
candidates; explicit offset, catalog count and matching count distinguish an empty
search, end of results and a partial page. The checked report is capped at 48 KiB
and uses the existing bounded native control channel. A missing catalog inspects
the currently captured selection, retaining its explicit overrides; it requires
an existing material binding.

Each candidate exposes its complete selection and structured incompatibility
reasons for printer/nozzle/setup/abrasion requirements or resolved process
constraints. Incompatible candidates have no proposed settings or input-change
claim. Successful declaration resolution uses the **same** material resolver as
ApplyMaterialProfile, without mutating intent, history, artifacts or scene state.
It does not run native geometry, prove bridge anchors or establish physical
qualification. A library's shared setup is explicit; changing a candidate never
silently infers installed hardware.

Compatible candidates expose current/proposed values for 22 typed process fields,
including temperatures, filament diameter, speed, flow, first-layer/bridge
controls, cooling, retraction and the intersected bead/flow envelopes. Origins
distinguish current intent, selected profile, explicit override, machine limit,
material limit and equal shared limits. Null retraction values mean disabled.
Whole-process overrides retain their override attribution. `input_changed`
compares the complete resolved intent, so a capture/metadata change can invalidate
output while every effective numeric value remains equal; neither status alone
predicts different commands. Catalog candidates use profile values without new
overrides, while current-selection inspection preserves captured overrides.

Review reports bind the persistent input revision, query and offset; the shared
client validates that reply binding. Apply still uses the existing undoable
ApplyMaterialProfile transaction. Applying a reviewed selection with an older
base revision rejects rather than refreshing that approval against newer input.
Library review cannot make an incompatible profile applicable.

CLI operations remain ordered: `--material-catalog CATALOG.json` reviews the first
page; `--material-search QUERY` searches the retained catalog (or current captured
selection) and resets the offset; `--material-page OFFSET` reviews another page;
`--material-inspect` reviews the captured selection; `--material-select ID REVISION`
applies an issue-free candidate from the latest reviewed page using that report's
revision. Selection requires writable mode. Other review operations allow
read-only inspection and retain revision/preview freshness in the JSON report.
A saved selection can be inspected after source/catalog deletion. The native
workbench loads bounded catalog JSON through a parented picker, consumes engine
search and compatibility reports, and stages a selected profile for explicit
Apply. Review remains available read-only and without the original catalog.
Foreign slicer preset translation and bundled production libraries are not
supplied by this typed catalog format.

`pnpm test:materials` exercises declaration discovery, search/pagination,
nonmutation, effective-value origins, rejected incompatible/stale selections,
selection/history and catalog-independent inspection alongside independent
actual-program checks. It uses explicit synthetic declarations, not material
printing recommendations.

### Shared deposition scheduling

`crates/deposition-routing` is the sole production owner of admitted path order,
permitted entries and authored wall seams. Manufacturing supplies borrowed path
terminals, actual physical intervals, tools and explicit construction prerequisites.
Actual fine layers remain barriers; a qualified coarse path retains its own lower
height and belongs to the layer ending at its top. No global equal-height or
universal role-rank assumption is introduced. Captured separator endpoint receipts name the exact wall and mandatory-skin
strokes that must precede each separator. Earlier physical anchors remain behind
the layer barriers; independent actual replay checks the complete construction.

The adapter applies a checked permutation in place, preserving each path's
geometry, per-edge widths and ownership, then releases transient search state.
Closed walls preserve directed winding under the authored projected-coordinate
seam policy; admitted open walls retain their direction. The old Cathedral row
router and realization-side seam scheduler are removed. Optional search work may
end with a complete checked schedule; cancellation and invalid required work
refuse rather than silently returning the retired ordering.

Per-machine declared scheduling estimates include travel/retraction, service,
swap and optional prewarm lead. Synthetic multi-tool changers exercise the shared
API and independent lab clock; they do not enable printer commands or physical
thermal/collision qualification. Product time comparisons consume the independently
decoded complete program and its captured timing evidence. Cross-layer routing,
new deposition connectors, scarf/helical construction and new machine backends
remain separate capabilities.

### Fixed-dual software route

[`FixedDualJob`](../../crates/contracts/src/manufacturing/routing.rs) selects the reviewed `marlin2125_native_fixed_hotends` contract at exact Marlin2.1.2.5 source revision `e5a167aaa0a671ea6d9b9d98dc0518c88d5caed7`. This is an explicit **software reference**, not a supported physical printer, firmware build/flash attestation or measured configuration. The adjacent `software-fixed-dual.configuration.json` and calibration/material declarations identify exact inert reference bytes and source-branch assumptions; no deployable firmware or physical calibration is supplied.

The job owns two distinct tools and feeders with captured loaded material identities/revisions/declaration hashes; coplanar XY nozzle offsets; per-tool filament diameter, process/feed/flow/thermal limits, external retraction and minimum reprime volume; native home; addressed standby/print targets; conservative carriage travel/body bounds; inactive-nozzle radius; transition lift and final park. Shared wall planning retains the fine recipe width; every model and waste bead independently satisfies its selected machine/tool/profile limits. Different declared polymer families are permitted for uniformly assigned, strictly XY-separated occurrences without optical or functional construction, including a single occurrence with an unused foreign loaded tool. Separation retains complete native-source reservation and projected whole-bead checks; it grants no contact or adhesion authority. Mixed-family cycles and optical constructions still refuse. The explicit functional mechanical-capture exception below is separate. No arbitrary command/macro strings, hidden toolchange motion, MMU/IDEX, shared melt mixing, firmware retract, extrusion compensation, saved offsets or automatic execution is admitted.

One to 32 `OccurrenceMaterialAssignment {occurrence_id,tool_id,interior,material,work_envelope_mm}` records bind complete native occurrences to explicit work reservations. Every pair must be strictly separated in XY; identity is unique across the whole table. The larger envelope admits uniform parts without material construction only; cycles and optical/functional construction retain their existing smaller limits. `MAX_FIXED_DUAL_OCCURRENCES` owns domain, bounded JSON and protobuf admission. Material is a required tagged uniform binding or physical layer cycle, never an implicit tool/palette index. Required-nullable `tool_id` pins a uniform occurrence to a captured loaded tool. Identical formulation IDs may be loaded in both tools only when their complete material declarations agree; such uniform assignments require an explicit tool binding. A null binding resolves only an unambiguous loaded material. Material-domain and cycle constructions own their routing and reject a uniform tool pin. Normalized complete beads inherit the exact captured reservation binding; replay independently checks actual selected tools. Preflight checks the exact source-ID bijection, upright frame and entire conservative native bound within its reservation. Reservation minimum Z may include conservative native enclosure below the bed; emitted endpoints and occupied bead intervals still obey the command/bed envelope. Every full projected nominal bead capsule must fit exactly one reservation, including per-edge width and coordinate error. This association consumes the already qualified global native union, never a path center, polygon order or inferred regional identity.

Every contiguous actual fine layer must have model deposition from at least one tool. Both tools retain one supported waste stroke and a complete service group on every layer; a group may contain no model paths, even for the entire job. Zero model segments require exactly zero model filament and volume. Enabled finish/process layers, support/interface/separator constructions, unbound combined sparse bands and extra/missing/unassigned sources refuse this route. The existing normalized paths and native geometry authority remain unchanged. Source/refinement/width/permission gates are not bypassed to obtain routing.

The shared deposition scheduler stores each layer's model paths in its selected tool-group order and captures both mandatory groups in `NormalizedPrintPlan.routing_tool_orders`, including empty model groups. Every entry is a permutation of the two installed tools; single-tool plans retain an empty table. Exact separator wall/skin attachment receipts constrain within-layer ordering; earlier physical anchors remain behind layer barriers. No universal role rank is imposed. Fixed order remains an exercised scheduler policy; production may alternate groups to avoid selecting the already active tool at the next fine interval. Each group first deposits its tool's one repeated, bed-rooted waste line at the true physical interval; complete waste beads fit the bed, exclude both source reservations/each other, and satisfy the declared minimum reprime volume. Actual E comes from the same stadium cross-section and selected feeder's diameter, with explicit first-layer factors. Waste is neither model/support nor recovered filament; no stationary blob or purge-into-infill shortcut exists.

Setup resets both complete XYZ hotend offsets, selects `T0 S1` before native homing, establishes Celsius/nonvolumetric/100% addressed flow/feed/shared fan state, and starts a known-home `G92 E0` epoch. With offset `o[t]`, logical active-tip position is `p` and carriage is `c=p-o[t]`. No-move selection changes `p` by `o[new]-o[old]`, not `c`; the host never subtracts offsets twice. Every actual later switch barriers, retracts, lifts, then moves in the outgoing tip frame to the carriage position of the incoming waste lane. A second barrier completes staging before standby/`Tn S1`/`G92 E0`/addressed `M109 Tn R…`; exact incoming E8 debt recovery precedes descent/waste. This avoids selecting an out-of-envelope tip at the model endpoint without weakening either command or carriage bounds. Resetting E does not recover physical feeder length. Adjacent same-tool groups retain their addressed wait, safe-plane travel and mandatory supported waste, without a fictitious selection, standby/reset or outgoing retraction. The captured tool order binds complete group inventory; selection accounting independently follows actual changes, including setup. Physical debt and terminal shutdown remain mandatory.

All routed travel uses the explicit all-layer clearance plane `last actual layer Z + transition_lift_mm`, with vertical lift, XY movement then descent; final park is at or above that plane. Conservative whole-segment inactive-tip/carriage-body sweeps are checked against deposited-height prefixes of source/waste reservations and declared carriage travel bounds. Fixtures refuse. These declared-box checks do not certify physical nozzle geometry, homing trajectories, ooze, adhesion or real swept-bead safety. Body retraction is explicitly disabled; transition/final pressure operations are separate typed policy.

Fresh independent replay checks actual selection/offset/reset/wait/debt/waste/shutdown bytes without calling producer realization. Required-nullable `VerificationReport.routing` and `ManufacturingSummary.routing` retain per-tool model/waste segments, filament, volume, epoch count and final unpaid retraction. Single-tool reports use `null`; missing fields are not compatibility defaults. Both feeders finish retracted, both heaters explicitly shut down, and restart never submits the program. [SPLR6](replay.md) separately exposes actual tool epochs and waste without fictional frame-jump lines.

The existing full `--intent`/typed SetIntent transaction captures material bindings, cycle/phase/origin and route in the functional fingerprint, invalidates manufacturing on change, and retains normal undo/redo/checkpoint/source-absent read-only replay/export. `fixtures/manufacturing/fixed-dual.intent.ts` is the static two-box reference; `fixed-dual-cycle.intent.ts` derives opposite physical phases through the same route. `fixed-dual-shared-material.intent.ts` binds the same formulation to both equal-diameter tools without inventing another material product. [Development](../quality/development.md#fixed-dual-software-job) owns commands. Arbitrary paint, optical study acceptance and measured color are not routing authority.

Decoded fixed-dual model and waste doses accumulate separately per tool as checked integer E8 filament units. Replay converts each total to filament length and volume once, so identical actual doses have identical accounting regardless of path partition, selection order or E reset epochs. A zero-extra-waste objective remains exact; it does not gain a floating-point allowance.

#### Captured fixed-dual tool profiles

Each `FixedDualTool.material_profile` is required nullable captured content. A nonnull `ResolvedMaterialProfile` binds the existing loaded formulation/revision/family, filament diameter, printer revision, installed nozzle and declared setup. `ApplyMaterialProfile.tool_id` explicitly addresses that tool; an absent target retains the single-tool operation. Resolution preserves the other tool and commits one ordinary Core intent edit. A profile cannot silently replace the loaded material or installed nozzle. CLI `--tool-profile TOOL_ID SELECTION.json` and native Material → Tool profiles use this same operation and history.

Actual model and repeated waste consume selected feed, flow, first-layer/bridge factors, queued fan and addressed nozzle targets. Every bead satisfies its selected tool/profile dimensions; emission and independent decoding check the minimum machine/tool/profile flow ceiling. Normalized object-volume accounting selects process by complete captured bead reservation. Independent replay reconstructs tool/frame/E/debt/thermal state and rejects swapped feeds or exceeded flow even when both tools carry the same formulation.

The shared bed target must agree exactly; both profiles must name the same shared cooling configuration. Sequential queued fan targets may differ by active tool. Body retraction remains disabled: captured per-tool external retract/reprime declarations own selection recovery. Different geometric bridge spans refuse until a regional anchor consumer exists. No setting is silently collected and ignored. Equal-diameter, fine-layer route/construction eligibility remains unchanged; declarations are software inputs, not physical process calibration.

#### Mixed-width interior

Required-nullable `OccurrenceMaterialAssignment.interior` captures an `InteriorToolBinding {tool_id,bead_width_mm,policy_override}` separately from the uniform primary binding. The current envelope is up to 32 strictly separated native uniform sources without optical/functional construction, finish/process operations or support. The explicitly bound route may combine eligible physical interior intervals under the combined-band contract. The secondary tool must carry the same exact formulation and a larger admitted width at the shared physical height. It does not create another material domain or join. A null binding keeps all model work on the primary tool while retaining both installed tools, their frames, heating, supported waste and clearance obligations.

Walls and mandatory skins keep their existing fine construction. The same classifier retains original interior coverage, then contracts its hatch-center proposal by the added bead radius before regenerating the selected width's pitch and endpoints. A lost classified component refuses. Shared `HatchLayer.width_mm` is an explicit generator input independent of wall/skin recipe width; ordinary and recovery phases consume it. Regional construction ownership, exact physical interval, protected native holes, actual previous-stroke anchors and bounded geometry/work admission remain mandatory. Expected regional paths must carry the captured constant width; relabelled fine paths refuse. The grouped construction uses the existing qualified physical-band mechanism; automatic selection is separately bound below.

Each tool's required `waste_bead_width_mm` drives actual supported strip volume, bed/carriage exclusions, flow and replay associations. Fine waste cadence is retained; a nozzle/profile whose minimum height excludes any actual interval refuses rather than skipping waste. The original through-hole reference compares fine-only and mixed construction on the same source and retains fine wall/bore paths. Fresh actual-byte replay rejects missing, doubled, incorrectly selected or dimensionally invalid work. Native Material uses the existing part binding/settings draft; Apply, history, save/reopen, actual Review and export keep their ordinary authority.

#### Automatic interior selection and scoped policies

A required-nullable `InteriorToolBinding.policy_override` captures enabled/disabled Automatic or PrimaryOnly. Null or disabled inherits Automatic; an enabled equal-default pin remains explicit authoring. PrimaryOnly changes model work, not installed hardware: both tools retain frames, clearance, heating and fine supported waste. The required-nullable `FixedDualJob.interior_goal` bounds minimum modeled saving, maximum extra model fraction and extra waste. No objective or timing model retains a qualified primary reference with an explicit reason.

`selection` compares at most three complete independently replayed constructions: primary, synchronized mixed widths and eligible grouped interior. One cumulative native work guard and cancellation/deadline cover all attempts. Only the last candidate bundle is retained; a different winner is reconstructed, checked against the assessment and freshly replayed. Resource/conditioning/cancellation failures propagate; explicitly unsupported alternate constructions retain typed Refused assessments. Qualification precedes selection. Common unknown startup context and captured timing model must match; unknown duration never becomes zero. The original authoring intent and semantic fingerprint remain the publication input. `NormalizedPrintPlan.interior_selection` carries the bounded typed assessments, decision, actual interior tool counts and selected program/report identity; emission, bundle validation and fresh decoding consume the same checked execution projection.

`Manufacturing.EditInteriorPolicy` carries a typed compare-and-set `InteriorToolPolicyEdit`, addressed by layout/session/base revision and exact occurrence/definition/expected override. Core changes only that binding, validates the complete intent and applies ordinary artifact invalidation/history/save semantics. Missing/false protobuf expected-state/action sentinels refuse. Use defaults removes the exact pin; disabled/equal-default pins remain persisted. CLI `--interior-policy-edit JSON` and native Infill use this operation. Native Add/Change/Remove opens one owning draft; explicit Close retains it, Discard abandons it, and only the matching scoped receipt confirms application. Unrelated Settings/Material drafts are never committed by this action. Review shows actual interior tool/nozzle ownership and discloses choices beneath the result, without a scheduler page.

#### Whole-plate mixed-tool ownership and placement

The classifier preserves occurrence ownership for mixed-tool interiors before generating widths. Equal density does not coalesce different routed source owners. `assignment_for_region` resolves the owning source contribution; width generation, fine tool envelopes and coarse-band/landing validation consume that same explicit binding. Plan admission and fresh replay independently bind each regional path's complete bead reservation to its source provenance, rejecting a foreign region even if a different part has a valid bead. Walls, mandatory skins, native holes, local dose and actual lower/landing obligations retain their existing independent gates.

One bounded candidate set compares complete plate programs. Existing tool0/tool1 layer batching applies across all parts; model policy does not multiply waste strokes or selection epochs by clone count. Shared definition assets, two tool/profile captures and one current replay remain shared; no per-part candidate collection or redraw compilation is introduced. Existing segment, bundle, CPU/GPU/transfer and native cancellation bounds stay independent of the raised occurrence admission. Ordinary operations have no automatic elapsed-time ceiling.

Core ApplyPlacement transports existing explicit uniform tool bindings and work reservations through same-orientation, same-bed-height XY translations. Copies inherit their exact scoped policy; deliberate copied pins receive the completed context. Translation uses outward coordinate bounds and does not infer BREP geometry from a box. Preview and commit use the same Core transaction, normal invalidation/history and shared immutable assets. The layout worker compiles the trial intent, includes captured reservations, waste and conservative relative inactive-tip/carriage reach in its packing proposals, then independently qualifies the complete emitted result. The search remains bounded best-found placement, not a maximal-packing or physical-collision certificate. This route stays on the active plate; additional-plate copies, constructed material copies and independently recaptured orientation/bed-height changes remain outside this placement increment.

Native Material shows the selected part's binding beneath the two shared loaded-tool/profile rows. Infill retains one owning modifier editor, and Review retains plate-wide actual tool/nozzle ownership. A source-diverse scale cohort uses one unchanged protected-bore part and shared box instances; increasing counts cannot duplicate profiles, source meshes, switches or suggestion rows per clone.

#### Reassessing a deliberate interior policy

Core stamps required-nullable `InteriorToolBinding.policy_context` when a scoped pin is created, changed or re-enabled. Removing the pin clears its context; disabling preserves it. Full-intent imports with unknown/null origin do not trigger reassessment. The semantic context binds layout/source/pose/authoring, hardware, captured profiles and objective while excluding policy overrides and their origin fields. A new deliberate pin remains quiet. Changed semantics may expose one `ManufacturingStatus.interior_opportunity`, with exact occurrence/definition/expected pin and current semantic input/context identities; metadata eligibility never claims benefit. The initial rule requires Current's model policies to match the primary-only reference.

`EvaluateAlternative` has a typed construction oneof for full intent or the exact `InteriorPolicyOpportunity`. The latter uses Core's detached scoped-edit trial and the existing bounded Operations/worker/artifact/replay path. It never mutates/Undo Current. The automatic result compares complete qualified primary and selected work from the same immutable capture. A typed `InteriorPolicyAssessment` binds the release edit, exact input context, two actual program/time/material ledgers and checked strategy/reason. It offers release only when the captured objective is met with comparable timing; missing/incomparable evidence retains Current. Bundle retrieval independently binds that assessment to the normalized selection. Relevant revision/context changes refuse evaluation/adoption; cancellation releases unpublished resources.

Native Infill offers Check default only for an eligible retained pin, on explicit request. One qualified `Primary only → Automatic` row opens the existing policy editor; Use defaults sends only the scoped edit and awaits its exact receipt. Comparison details own modeled work/material and common unknown startup. Dismiss persists a bounded semantic-context key in local preferences, suppressing the same proposal without altering project intent. A new context can be checked again. There is no redraw-driven evaluation or proposal per clone. CLI `--check-interior-default` uses the same detached service and retains its immutable assessment; it does not adopt it or redirect Current export.

### Functional material region and captive keys

`FixedDualJob.functional_construction` is required nullable captured intent. A
non-null `FunctionalMaterialConstruction` binds one occurrence, exact definition,
source hash and native face, two loaded material IDs, `minimum_pure_depth_mm`,
positive `minimum_solid_base_root_depth_mm` and
one `PlanarTKeyJoin`. It cannot coexist with optical construction. The base is
the occurrence's uniform material; the region uses the other loaded declaration.
The required `interface_policy` is explicit: `declared_same_family` retains the
common-polymer-family restriction; `mechanical_capture` permits different declared
families only for this complete headed-key/end-collar construction. Missing or
unknown policies refuse. Selecting materials never selects the exception. Native
construction and independent actual replay must still pass; the policy alone is
not construction evidence or compatibility between arbitrary grades. It grants
no adhesion credit. Shared flow/bead envelopes must fit both loaded tools, with
each tool's captured filament diameter and addressed temperatures. The explicit
shared bed/fan recipe does not establish physical suitability for both materials.

Required-nullable `face_patch` selects either the whole certified face (`null`)
or explicit finite source-face `horizontal_mm` and source `source_z_mm`
intervals. This supports localized pads, full-width height bands, and partial-width
full-height strips through the same construction. The entire requested pure band
and every key must fit inside the selected patch and stock; neither clipping nor
silently reducing dimensions is allowed. Base material retains the stock outside
the active patch. Removing a region restores its uniform base assignment while
retaining both loaded declarations; an unused tool still has required waste work.

Pure depth is a nominal solid-material ownership requirement, not a measured
polymer thickness. In inward face depth `d`, pure region material occupies
`0…p`; the nominal interface is `p + reach_into_region_mm`, and the separately
authorized transition ends at
`p + reach_into_region_mm + reach_into_base_mm`. Foreign teeth never count toward
`p`. `pitch_mm`, `stem_width_mm`, `head_width_mm`, `head_depth_mm`,
`edge_margin_mm`, `end_collar_height_mm` and `collar_head_width_mm` describe a
bounded centered key inventory, not a strength recipe. The head is wider than
the stem; both end collars have narrower heads than the middle, providing
nominal opposing end retainers. Chains, movable hooks and automatic pair recipes
are not exposed as supported strategies.

Material ownership and process density are separate. Region material stays solid.
Base-owned material from `p` through the transition end plus the requested root
depth forms a solid belt across the **whole native face U**, not just the selected
patch. Remaining eligible Base interior inherits the recipe fraction. Root depth
is additional to both reaches and never consumes protected pure depth. Ordinary
walls and mandatory physical skins retain precedence; changing density does not
silently change the authored recipe or introduce another material wall.

Localized constructions also require a Base lower landing and upper cover from
depth zero through the root's inward end, again across whole face U. Their
physical horizon is the maximum of bottom skin thickness, top skin thickness and
nominal layer height, clipped only at stock endpoints. Required bounded
`solid_base_zones` captures at most one `root`, `lower_landing` and `upper_cover`,
with exact request-derived depth/Z scopes and no positive-Z overlap. Those
boundaries participate in scheduling and classification/template reuse, not the
material-wall identity. Initial generation and actual-anchor recovery consume the
same partition and original material-owner lineage.

The first producer requires one translated, axis-aligned native rectangular
stock with six planar boundary faces, four completely certified full-height
walls and the two endpoint caps. Complete native closed-slab admission remains
mandatory even when an endpoint capture does not carry a regular-invariance
flag. A plausible bounding box or a planar-face label is insufficient. Holes,
insufficient remaining parent material, incompatible overlays and unresolved
native or actual-stroke proofs refuse. Sparse Base construction requires one
perimeter and independently qualified source-following walls; unresolved wall
junctions, strip overlaps outside source-local turn allowances, nominal-core
invasion or recovery replacements refuse. A closed source-following Wall may
retain a genuine Bridge role only with unchanged nominal contour dose and the
separate actual lower-anchor proof. Original source
material is partitioned, never expanded or filled. One exact shared internal
boundary supplies both material owners; literal admitted exterior coordinates
are retained. Rounded cuts that cannot establish the requested minimum pure
depth refuse rather than silently thin it.

Patch endpoints, collar events and captured ownership use the same certified
face frame and actual translation arithmetic. Native refinement preserves all
four physical stops. `NormalizedPrintPlan.functional_material_frame` is required nullable
inspection data binding source identity, source/world origins, exact Cartesian
frame, literal `stock_bounds_mm`, whole source height, selected `selected_u_mm`,
active world `region_z_mm`, world `key_middle_z_mm` and at
most sixteen centers. `FunctionalRegion` and `FunctionalBase` use material-domain
index zero, independent of geometric deposition roles or optical shell/backing.
Every layer has exactly one complete base domain; only active layers also have
a region domain. No layer or domain may straddle a patch or collar endpoint.
The contiguous schedule must cover the complete captured stock Z interval;
truncating before a patch or omitting either collar cannot avoid its obligations.
Existing bounded domain inventories, complete physical intervals and full-bead
foreign-material clearance apply unchanged.

After actual-byte/path correspondence, independent replay checks both the
requested region and complete base complement over the finite orthogonal cut
arrangement. Sparse replay independently reconstructs complete nonconvex nominal
cores, qualifies actual wall coverage/extent/dose and excludes original raw-wall
outer occupancy. Complete combined-fill/material ownership operands precede the
solid/inherited process split. Projected commands retain their separate clearance
proof. Exact wall reuse includes closure, kind, every raw XY edge and actual width.
Replay recognizes a complete common legal quarter-pitch phase, not arbitrary
surviving rows, and retains minimum-command fit witnesses in either direction.
Deleting rows from both plan and program cannot create a smaller demand.

Required-nullable `ProcessRegionCoverage.functional_solid_zone` names a solid
Base zone only with fraction one and whole-layer scope. Unreferenced eligible Base
keeps recipe density; Region is solid with no Base-zone reference. Labels or
equal-area summaries cannot establish the required spatial construction.

Replay checks same-material positive nominal-core paths from key and
retainer deposition to their own parents, actual shoulder witnesses and positive
projected head/retainer overlap at each physical phase transition. Base attachment
uses both finite source-side wall runs within the root reserve, never a detached
deep endpoint or remote continuation of a wall loop. Finished connectivity can
cross immediately adjacent closed physical layer interfaces through positive
same-material projected core contact. That symmetric finished bond supplies no
earlier-deposition support. Existing
printing-time lower-anchor, bridge, source, void, command and routing obligations
remain separate. These are nominal program/construction checks, not measured
fusion, flexible-material extraction resistance or physical strength.

The native Material form stages the face assignment, full-face or explicit U/Z
patch selection, pure depth and per-side
join reaches and independent solid root depth in the existing full-intent draft;
one Apply is one Core transaction. Fresh Joins inspection distinguishes solid
reinforcement from inherited Base infill and describes its captured scope.
The centered-pad shortcut supplies estimated starting values only. Full-width
height bands require explicit actual face-U endpoints: conservative display
bounds cannot supply exact face dimensions or manufacturing authority.
History and persistence capture the complete request. Review's **Joins** group
uses fresh verified functional domains for boundary highlight and Focus/Reveal;
inactive physical layers explicitly report that the selected patch is absent.
Tinted actual XYZ6 centerline scopes distinguish Region, protected Base and
inherited Base. Protected scopes clip to the full-U solid belt, including landing
and cover intervals outside the patch; inherited scopes retain its complement.
These display cuts are neither new commands nor material or bead surfaces.
An authored request is not realized geometry. General contact discovery, multiple
competing regions and optical-recipe neighbors remain
separate extensions. One-perimeter solid and sparse references exercise the positive;
other wall policies retain their ordinary geometric refusals without substitution.

#### Source-bound planar surface color

`FixedDualJob.surface_constructions` is a required bounded array, independent of face finishes, sparse-density intent and display tint. The current constructive envelope is one upright, bed-rooted native part with one or two independent regional constructions, each on one or two full-height rectangular exterior planar faces. `SurfaceColorConstruction` captures immutable occurrence/definition/source hash and native face identity, positive physical skin/backing depths, a required-nullable finite horizontal mask, an explicit shell material sequence and the loaded material requested as white. The white assignment must match the occurrence's uniform base material, so uncolored material inherits that same white base. It is operator-declared, not an optical measurement or calibration. Surface and functional constructions resolve one material domain per run of consecutive section spans whose material agrees within the ADR012 profile tolerance (the run's first span, over the run's physical height), so a prism with curved walls does not spend one bounded domain entry per layer.

`SurfaceColorFace.mask` is null for the whole face or `{min_u_mm,max_u_mm}` for a hard horizontal window over the complete finite native face height. U is measured in physical source-local millimetres from the certified face origin along its horizontal direction, not screen X or a guessed STEP UV parameter. `NormalizedPrintPlan.material_frames` exposes that captured origin/direction, outward normal, native width/height and selected U extent for inspection. Outside the window, material inherits the occurrence's white base; source geometry and protected holes are never erased. Out-of-face, empty or unprintable windows refuse rather than clamp or disappear. Mask/pose changes invalidate manufacturing normally.

For each admitted native slab, intersect its actual D with each source-local inward face prism: regional S is depth `[0,s]` and B is `[0,s+l]`. Regional shells retain separate construction indices. Shared white backing is union B minus union S; shared core is D minus union B. Their canonical construction index0 names the complete arrangement, not a second claim on another region’s material. Independent regions require disjoint positive-volume backed domains; unresolved overlaps refuse rather than choose a face/array priority. Preserve all source holes and a positive surviving core. Shell and backing receive solid construction; core retains recipe density. White backing and white core have distinct construction ownership and do not double-own their shared boundary. Unsupported face/frame/depth, missing source, collapsed bead domain and exhausted resources refuse before publication.

Compatible adjoining faces within one construction must have equal skin and backing depths wherever their backed native volumes intersect. Their shell/backed unions own the shared corner once. The exercised corner envelope is orthogonal planar bands; reentrant orthogonal domains use the shared source-edge ledger to partition consecutive X cells, with at most32 source boundaries and no discarded positive slivers. Every cell retains the same material owner and original geometry/void obligations. The existing automatic wall planner consumes each finite material domain or cell. It fits actual per-edge widths against projected material boundaries and independently requalifies the original requested domain under the unchanged source/offset/command bounds. A closed inner count-death proposal below minimum command diameter may be omitted before path admission, with at most8 vertices and no exterior ordinal or open stroke removed. The complete candidate must still pass unchanged original-component coverage, underfill extent, source/void, overlap/dose and junction-connection qualification; omission never grants a coverage exemption. Global D still owns skin horizons; merged wall/fill accounting charges the original source once. `NormalizedPrintPlan.material_domains` records finite polygon/void domains and physical intervals: at most24 domains, two polygons each, four holes per polygon and256 points per closed ring. Their Cartesian product bounds decoding before allocation; programmatic input also retains the cumulative65,536-point gate. Required-nullable `DepositionPath.material_domain` and `ProcessRegionCoverage.material_domain` preserve shell/backing/core associations separately from Wall, skin, Bridge and density ownership.

Realization resolves each complete bead's material to the existing loaded-tool route. Fresh emitted-byte verification checks actual tool/XYZ/E state, full-bead domain ownership, positive-volume domain disjointness and independent interval capsule clearance from neighboring material volumes. Native boundary approximation never authorizes intrusion into another material. The normal intent transaction, history, immutable export and source-absent fresh read-only replay remain the consumer. Each regional shell may use its own physical LayerCycle while backing/core stay white; actual partial intervals retain physical phase. Two finite windows can resolve opposite phases at common Z without changing the whole occurrence. Waste-only groups keep both bed-rooted lanes continuous and include all switches/waste in accounting. Partial-height masks and general curved/nonorthogonal corner construction remain unqualified. A full-height finite window is the current patch envelope. Intersecting unequal depths refuse explicitly. No physical bond, color or printer qualification follows.

`SurfaceColorConstruction.optical_origin` is required nullable historical provenance. `Projects.ApplyOpticalConstruction` applies only an accepted complete retained selection to an existing native source/face construction, with two explicit evidence-to-loaded-declaration bindings. Core captures the exact recipe, evidence rows/hashes, report/selection/model/target/observation/requested patch and applied pose. The current loaded declarations and shell recipe must continue to match; an independent recipe edit must explicitly clear origin. Geometry/depth/pose changes retain the historical origin but require normal recompilation and fresh inspection. The optical record supplies no geometry, attachment or process admission.

After complete independent actual-byte replay, `inspect_bundle` derives one `SurfaceColorInspection` per constructed face. It transforms the captured native frame/mask by the captured occurrence pose to an upright finite build-frame patch, and counts each complete shell physical interval once per actual layer. Missing/overlapping intervals or inconsistent material/tool associations refuse. Per-tool exposed heights and fractions describe replay-qualified nominal side exposure, never deposited-volume ratios, measured bead shape or purge/backing area. Report-only and motion inspection use the same computation; no reference estimate is persisted as verification authority.

The engine composes the existing appearance evaluator after successful replay. For a captured optical origin it recomputes the requested reference and reassesses the realized patch using interval-derived exposure mapped to the captured material order. Both assessments retain model producer, target error, display bytes or explicit unassessed reasons; patch/pose match flags expose changed context. `backing_model: unmodelled` is mandatory. A physical white backing does not confer a light-transport model. The bounded typed inspection table is independent of SPLR6 geometry and is reproduced without external source/palette/study paths. Four-file `provenance.json` retains original captured provenance plus a required `inspection` member supplied by that fresh engine result; shared CLI/shell export validates its captured-input binding before writing.

#### Whole-occurrence physical layer cycles

`OccurrenceMaterial` is `{"kind":"uniform","material_id":…}` or
`{"kind":"layer_cycle","recipe":LayerCycle,"material_ids":[id0,id1]}`.
The recipe is the existing optical `LayerCycle`, not a second sequence schema.
Its request-material indices bind the explicit ordered immutable loaded-material
IDs; they are not feeder indices, palette names or RGB values.

Routing initially admits 1–64 equal positive physical slot heights, valid explicit
phase/origin and material indices0/1. Optical studies retain their independent
unequal-slot capability. Origin starts `phase_slot`; periodic continuation before
origin is explicit. A signed physical ordinal selects the slot, never the mutable
fine-layer index. World boundaries use fused multiply-add, bounded exact ordinals
and unique interval ownership. Only the existing four-machine-epsilon endpoint
identity recognizes alternative arithmetic encodings; real crossings, ambiguity,
collapsed/unrepresentable arithmetic and missing loaded materials refuse.

Cycle boundaries join native/field/process events **before** the existing global
schedule is balanced/refined. They do not replace native events, create another
scheduler, recolor completed coarse layers or move source geometry. Each complete
fine physical path interval must fit one slot. Native partial/refined intervals
inside a slot retain its material; they do not advance phase merely because a new
layer ID appeared. Both occurrences resolve independently at common Z, before
precedence-preserving grouping by tool. Existing per-layer model/waste ownership,
process-height, waste/reprime, native/void/contact and command gates still apply.

The normalized plan, captured route and actual SPLR6 tool/XYZ/E events expose the
realized construction for inspection/reassessment. Routing does not promote a
stored optical estimate. A separately applied captured origin permits explicit reassessment of the realized finite patch from actual intervals/material mapping and original conditions/evidence. Backing optics, arbitrary regional paint, Local-Z/sublayer parity, measured bond and physical color accuracy remain outside support. The coordinated producer binds mandatory physical material events and waste-only groups; emitter14/verifier32
bind preselection waste-lane staging. SPLR6 retains its command/binary interpretation.

## Normalized plan

`NormalizedPrintPlan` uses schema11/planner98. It contains layers, bounded surface/process resolution, native geometry coverage, explicit `sparse_bands` decisions and positive source-sampling, offset, surface and layer-quantization bounds.

Native sampling and offset construction bounds are each 0.005 mm; finish XY construction has a separate 0.001 mm bound. The explicit recipe `native_shape_error_mm` bounds source-target approximation independently of those construction tolerances. Each mapped image additionally declares its own vertical displacement-error bound.

Surface documents remain schema3. Emitter16/verifier42 consume constant/per-edge widths, immutable Wall ownership, permanent plate ownership, mixed regional/mandatory-skin Bridge portions and fresh interval-aware anchors/interface proofs. SPLR6 keeps those nominal associations separate from actual XYZ/E/feed, and retains distinct tool epochs/transition waste. Preselection staging changes the command lifecycle, not the packed layout; document counters and existing kind meanings do not change.

Required `native_geometry.source_feature_coverage` is a bounded count summary, not a global invariance enum: `disjoint_source_layer_count + invariant_source_union_layer_count + native_interface_complex_layer_count == certified_layer_count`, using checked arithmetic. It partitions actual accepted scheduled layers into disjoint source, closed-slab invariant-union and native interface-complex evidence. The interface-complex category requires the actual continuing-core/critical-contribution proof and complete leaf ledger; its name never establishes that capability. Mixed jobs cannot claim global source-union invariance. `slab_count` counts accepted original-occurrence/slab evidence captures, including inactive sources and owned critical contributions. An invariant accepted slab may cover several scheduled layers without creating additional captures, so `height_analysis_count <= slab_count <= height_analysis_count * certified_layer_count`; there is no per-layer lower bound on captures. `owned_critical_slab_count` counts unique original-occurrence/slab captures with any owned endpoint stratum or nonzero transition collar, not faces, and cannot exceed `slab_count`. Missing source contributions cannot be disguised as empty height domains. Reused native evidence remains bound to each accepted capture; failed/refined candidates affect only cumulative `geometry_work`. All count fields are explicitly required on checked JSON/native admission and bounded by existing layer/occurrence caps. Summary admission neither reconstructs private native receipts nor certifies the emitted physical union boundary.

A `PrintLayer` has consecutive schedule `index`, required true `lower_z_mm` from the bed or preceding layer's `z_mm`, increasing upper/deposition `z_mm`, reporting `height_mm`, interior `section_z_mm`, `region_coverage` and paths. Reporting height may normalize an upward arithmetic roundoff excess to nominal height; it is never physical interval authority. Mandatory native/image/material/support events and exact top determine boundaries; subdivision and native refinement preserve this one schedule. Ordinary paths use `[lower_z_mm,z_mm]`. Internal empty support-contact-gap layers require enabled breakaway supports, zero object/support areas and nominal volumes, preceding printed support interface and actual object deposition closing the gap, with independently proved full-bead separation at least the declared Z gap. No leading/trailing empty layer or fictitious bead fills a contact gap. Packed replay honestly skips empty layers.

`native_geometry.height_merge_mm` records the Z step used to merge heights (positive, at most 0.05 mm). `native_geometry.evidence` is `tolerant`: every job is sliced within [ADR012](../adr/012-tolerant-brep-curves-and-feature-heights.md) tolerances, and Compile no longer produces `certified` evidence. The capture/count partition above applies only to certified evidence. Tolerant plans require zero `height_analysis_count`, `slab_count` and every `source_feature_coverage` count: sampled sections cannot manufacture native certificates. `geometry_work` records only actual native-query work, including zero when the tolerant job performed none. The schema's retained `certified_layer_count` field counts admitted scheduled layers in either mode; only certified evidence grants that label. Inspection labels tolerant layers as sampled, not certified. With supports disabled, `region_coverage.unsupported_overhang_mm` records the unanchored span of the first stroke found printing over nothing on that layer; it is zero otherwise and always zero when support is enabled. Fresh verification limitations retain the number of marked layers and the largest recorded first-offending span, not a worst-span inventory. Native Review exposes the marked physical layers and a per-layer notice. Successful command verification does not clear these warnings or establish physical printability.

A `DepositionPath` has process `kind` (Perimeter, SolidFill, BottomSkin, TopSkin, SparseFill, Bridge, Support, SupportInterface or SeparatorWall), required nullable `owner`, `width: WidthProfile`, ordered `physical_z_mm:[lower,upper]`, and at least two finite points at exact owning layer Z. Width is `{"kind":"constant","width_mm":w}` or `{"kind":"per_edge","widths_mm":[...]}`. Per-edge storage has exactly `points_mm.len()-1` admitted widths, including the explicit closing edge; constant paths allocate no width array. There is no parallel scalar path-width field. `checked_edges()` is the allocation-free consumed view; reverse/closed rotation carry widths with their edges. The physical interval upper equals layer Z, and its difference is actual H, never reporting height or a layer-count multiple. Every consumed width is finite, positive, at least H and inside captured process limits.

Optional `arcs` lists runs `{first, last, center_mm, clockwise}`: points `first..=last` of the path lie on one horizontal circle about `center_mm`, turning one way. An omitted list is empty. Runs are ordered and disjoint (they may share an end point), span at least two edges, and keep every point within `MAX_ARC_POINT_DEVIATION_MM` (0.001) of the circle through the run's first point, every chord within `MAX_ARC_SAGITTA_MM` (0.005) of the arc, each edge's turn below half a circle, the whole sweep within `MAX_ARC_SWEEP_RADIANS` (1.5π) and the radius within [0.5, 1000] mm. The points stay the path's geometry for every planar check; a run says only that they share an exact circle. The planner attaches runs to wall paths, with centers taken from the placed analytic surfaces of revolution of the source BREP rather than fitted; reversal and closed-seam rotation carry runs with their points, splitting a run the seam falls inside.

Immutable construction owners include `{"kind":"wall"}`, `{"kind":"mandatory_skin","skin":"bottom_skin"|"top_skin"}`, and `{"kind":"regional","region_index":u16}` into the layer's process regions. Perimeter requires Wall ownership, which survives genuine Bridge role changes. Wall Bridges can have finite open or closed multi-edge geometry; ordinary open hatch Bridge rows still require two points. The explicitly mixed `regional_skin_bridge` owner names regional/mandatory portions and `skin_first`; it alone permits the three-point mixed-owner transition, and reversal toggles `skin_first`. Removing mixed ownership cannot authorize its unchanged open three-point geometry.

Each actual XYZ6 chord deposits constant nominal area `(w-H)*H+pi*(H/2)^2`, with the admitted process factors applied once. E8 is integrated using that actual chord length; independent replay decodes the length and absolute/reset/recovery E state separately from source endpoint fidelity. The algebraic inverse reports a full positive width-equivalent interval from actual parsing/subtraction uncertainty. A rounded circular nominal target may have an equivalent below H; admitted nominal geometry and independent forward E8 fidelity own that model domain. Never clamp the inverse at H, add intended producer quantization to actual uncertainty, or discard either endpoint for profile-width/lateral/continuing-gradient checks. These are nominal command models, not measured bead shape. Plans remain capped at4096 layers/100000 actual deposition segments, with allocation bytes separately bounded before growth.

Native BREP sections and rigid occurrences remain geometry authority. Same-material object sections preserve holes/islands, inward walls and disjoint skin/interior ownership. Physical-distance skin horizons classify floors and roofs. Unsupported spans become bounded bridges or clearance-clipped breakaway support/interface according to explicit policy; support never mutates source BREP or object material ownership. Sloped native planar/cylindrical sections require numerical admission gates. No mesh repair, silently removed feature or triangle fallback exists; numerical residuals are not physical accuracy certification.

CubeCL supplies optional exact numeric screening, not manufacturing authority.
Hatch and required Bridge-row clipping batch half-open boundary crossings;
host f64 interpolation, sorting, parity, row ownership, exclusions and command
construction remain unchanged. Fine-interface replay may batch conservative
actual-path box rejection, after the actual-byte/path bijection; surviving
paths still use their original per-candidate bounds and complete capsule proof.
Skipped path runs charge their complete original edge inventory, including the
same cumulative-work refusal counter. No support, clearance or material proof
comes from a mask. Small or oversized batches retain the original host path;
initialization, execution and cancellation errors never request that fallback.

Numeric staging has an additional bounded envelope separate from retained
native geometry and commanded segments: at most 65,536 source edges/paths,
256 current queries, 3 MiB hatch sidecars or less than 3 MiB anchor box/prefix
sidecars per active consumer. The shared runtime separately bounds encoded
inputs/queries and a 1 MiB packed result, serializes dispatch and drains bounded
tiles before release; see [compute resource limits](../../crates/compute/README.md).
These are application-buffer bounds, not total compiler/driver memory claims.
Backend choice is machine-local and cannot change manufacturing provenance or
replace fresh independent actual-program replay.

### Automatic wall ownership and qualification

The ephemeral [wall owner](../../crates/manufacturing/src/planner/walls.rs) borrows final D and a separate mandatory native/C operand. Its segment-Voronoi graph is a bounded proposal, not native material or protected-void authority. Original boundary identities and f64 feet survive integer construction; exact-source crossings, overlaps, quantization collapse and unconditioned graph/curve arithmetic refuse without healing.

Constructor admission takes the caller's **total** already-owned bytes separately from semantic point inventory; the retained skeleton inventory counts dense finite vertices, never `bytes / 16`. Actual XYZ3 coordinates, per-edge width/association payloads, shared caches and coexistent source/command buffers remain byte-accounted. Geometric growth admits old and replacement buffers before reserve rather than reserving the job-wide command ceiling for every small graph. The external Voronoi constructor uses a conservative per-site staging estimate and cancellation immediately before/after its nonpreemptible call; this is not a hard allocator-RSS guarantee or internal-cancellation claim.

Capsule tessellation entries are transient Boolean staging, not commanded segments. Bounded primitive batches keep the existing Boolean staging ceiling while normalizing every unchanged primitive once, then combine owned results through the same balanced union. Actual command admission, normalized-region limits and retained-byte admission remain separate. Heavily oversized fitted coordinate/profile/correspondence buffers are compacted within the already admitted old/new assembly peak before caching; ordinary growth slack is retained. Resource/cancellation failures propagate rather than becoming a different geometric count candidate.

Inner and nominal command capsules retain their straight tangent rails in addition to bounded inscribed end-cap vertices. Fixed-axis disk tessellation must not charge a non-retracing diagonal stroke as local overlap excess. Outer circumscribed enclosures and predeclared source-permission polygons remain independent and unchanged; rail construction admits its complete vertex/work/scratch bound before allocation.

Compile-local wall reuse binds the immutable final-D span, required-C/image-exemption attribution, exact admitted width envelope and its exact pre-guard stadium floor `max(profile_min_width, actual_height)`. Keeping that floor separately avoids merging height-dominated values through floating-point guard addition. It does not key on bit-distinct Z subtraction results when the effective policy is identical. Each layer materializes actual physical Z independently; source-finish checks, skin horizons, anchors, Bridge decisions, support and H1 qualification are not reused merely because wall XY is shared.

Define the independent required band `R = D \\ erode(D, perimeter_count * preferred_width)` before proposing beads. Finite source-graph count events reserve their corridors before extrusion; continuing widths change over actual bead distance, newborns begin with legal positive width, and true junctions connect required arm ports locally. Command length is at least0.02 mm, continuing width gradient at most0.20 mm/mm and actual XYZ6/E8 lateral enclosure at most0.001 mm. Fitting cannot erase source-boundary obligations or manufacture foreign-lane overlap.
Count candidates require whole-arm width enclosures; a contained uniform-width alternative remains a construction proposal under the same independent C/D qualification. Mandatory corner-underfill seeds require the original support vertex in C, not merely optional image-owned material.
Existing sharp-corner count/completion fields retain priority. If none qualifies, a bounded alternative joins an inner contour's nominal-radius tangent ports with one straight bevel. Eligibility requires a literal convex source corner, affine segment feet, the already bracketed birth and finite ports inside its retained cell. Nominal width, lane depth, count and birth stay fixed; fine exterior contours are unchanged. Only the replaced source intervals disappear. Completion reduces the original birth-cap witness by the bevel's displacement and admitted endpoint/command error; it never borrows the former larger cap. The shared target, cumulative work allowance and independent coverage/dose/gradient/command gates do not reset between constructions.

Ordinary adjacent source lanes, including same-side offsets, must pack their actual XYZ6 proposal chords. Packing reserves each immutable continuing endpoint radius against the opposite chord; uneven source subdivisions cannot independently spend the same clearance. A fitted ordinary capsule is contained in the original connected sweep: its radius plus a conservative bidirectional chain-to-chord distance cannot exceed the original minimum radius. Convex distance on every original chord and continuous projection of the connected chain establish that distance; a centerline error tube at unchanged minimum width is not containment. The same total0.0024 mm fit enclosure, process floor, endpoints and support/ordinal ownership remain. A centered fit, even at constant width, requires the complete candidate capsule inside a source permission declared before beads; independent D/R qualification still applies.
Short-tail fitting may split original source edges at finite interior fractions while retaining a complete, ordered partition of their geometry and width fields. A curved corner may propose three commands with shared joins off the source polyline; its end tangents propose coordinates, not material authority. Every original interval and both command-endpoint displacements enter the same bidirectional fit enclosure. Ordinary inward containment or an independently admitted centered-fit corridor, unchanged minimum command length, width gradients and support/ordinal obligations still qualify each command. Lookahead is bounded by the command minimum and the existing source walk; it neither moves open path endpoints nor drops source intervals.

Qualification independently compares commanded inner/outer occupancy with original D/R and protected holes, preserving native/C and certified image-only exceptions. Inward rounded-corner/endcap/local-joint underfill is bounded by nozzle/4 in extent and2% per required component, with component/arm protection. Source-derived junction/count permissions bound local excess by `.5*(valence-1)*max_width²`, where each source-declared width ceiling is positive, finite and no greater than the admitted process maximum, with5% aggregate required-band excess and no duplicate credit where permissions overlap. Local dose compares summed clipped finite-command rectangles with the clipped union of nominal command capsules, never separately summed endcaps or an inferred smooth-tube area. A nonfolding turn or same-path label grants no exemption. Positive XYZ6 collinear duplicate, partial, reversed and cyclic retraces fail independently of area accounting. Existing0.005 mm geometric policies are not enlarged.

`R` is the minimum wall-needs target and the aggregate-excess denominator, not a maximum wall-ownership domain. Only a bounded permission fixed from the original source junction/count event can authorize overlap in nominal core (`D \\ R`); permissions remain clipped to D, and original-D/exterior/hole qualification stays independent. Exact XYZ6 positive collinear retraces and unauthorized proper nonlocal centerline crossings refuse regardless of whether assembly represents the same deposited geometry as one continuing path or several. Consecutive joins and cyclic seams are not proper crossings; harmless curved-path subdivision is not a per-segment cap-overlap test.

A polygon-conversion overlap can be discharged only by a sufficient certificate over **every** relevant earlier/current actual-XYZ6 nominal capsule pair. Eligible canonical decimal dimensions use checked exact squared-distance comparison; other widths retain their full value in outward arithmetic. Overflow or an inconclusive pair never establishes separation and leaves the ordinary location gate unchanged. A permission alternative contains a whole capsule or the entire convex intersection enclosure, not a sampled witness. After independent uniqueness, the same complete certificate may discharge cross-path location before constructing its polygon unions; a single continuing path has no cross-path pairs. Early multi-path certification bounds candidate inventory and admits a checked work allowance before dispatch; exhaustion publishes no certificate. Declines retain performed cumulative work and use the ordinary polygon gate; cancellation, allocation and execution errors remain terminal. This changes neither dose, uniqueness, material/coverage nor independently decoded process checks.

Source-corner permissions are declared in decreasing sharpness of their literal convex source incidence, before commands are proposed. Shallow neighboring events cannot claim a sharper turn's small fresh fragment first merely because the source graph enumerates them earlier. Arm/count declarations keep their preceding order. A contour birth's footprint derives from the complete original piecewise-affine bead-width field and the distance to its nearest full-width recovery, with birth-location/arithmetic reserves and the existing process-maximum radius cap. Unused machine bead capacity cannot make a constant-width turn's region reach unrelated continuing walls. The same declared footprint gates completion, centered fitting and independent qualification. The turn's dose-width ceiling is the maximum of that same original source field, bounded by the process maximum; a smaller footprint cannot borrow unused machine width as additional overlap credit. Genuine source junction/arm-count declarations keep their existing process ceiling. No new valence/permission is added; fractional freshness, the local budget formula, the aggregate 5% cap and all material/command gates remain mandatory.

Local permission budgets retain original declaration order: `fresh_i = P_i \\ union(P_j, j<i)`, and the prefix incorporates original `P_i`, never only its fresh fragment. Spatial exclusion and owned local unions may normalize the prefix but cannot reorder source credit or count overlapping permissions twice. All simultaneous original/prefix/result buffers and old/new component metadata remain admitted before growth.

The actual outer wall ownership is removed from the nominal core before downstream fill/skins. Physical skin horizons intersect final-D/native material first, then qualify the current fill bead and current residual ownership: neighboring walls are material, not fictitious void. Actual widths and physical intervals also feed anchors, support clearance, source finishes, permanent interfaces and H1; no global retain-fine fallback substitutes for those gates.

`PlanarRegionCoverage.section_area_mm2` is realized object material D, with separate `source_section_area_mm2` (native M) and `surface_added_area_mm2` (D minus M). Additive-only construction checks `section = source + added`; support-only/contact-gap layers have both zero. Global walls and physical bottom/top skins retain their existing precedence and ownership. Required `process_regions` partitions only eligible interior, with positive `area_mm2`, `fraction` in `[0,1]`, and source/winner `contributions`. Solid interior sums regions at fraction1, sparse interior sums regions below1, and intentional nominal void is `sum(area * (1-fraction))`, never a global or weighted-average fraction. At most256 regions and256 cumulative contributions per coverage are admitted before allocation growth. Bundle validation checks each occurrence/definition and winning layer/operation against captured process intent. An empty contributor list denotes unchanged global recipe intent, not omitted overrides. Set partition checks use retained final-D operands; scalar accounting tolerance does not authorize discarded slivers or overlapping spatial ownership. Actual object/support volume still derives from actual stroke lengths, bead area, true height and process flow factors, not nominal occupancy. Protected source holes remain separate and excluded.

Required `skin_bridge_domains` is empty unless actual mixed construction exists. Its bounded regional/mandatory polygon operands and retained full source-owner operand preserve original classified holes and bind the regional index's source/winner contributions. A genuinely unanchored authored regional endpoint may extend only into adjacent already-required mandatory skin to its nearest actual previous-layer capsule anchor. The three-point continuous Bridge independently needs a nontrivial bounded two-sided actual gap; native occupancy is never an anchor. Preserve the original regional core/cadence and required skin strokes. Only exactly coincident commanded subsegments are clipped as duplicates; necessary finite endpoint-joint/crossing overlap remains inspectable and all additional commanded volume is charged. This is neither a density override, permanent interface nor temporary support. Missing, multiple or ambiguous crossings refuse.

Separately, a unanimously mandatory-skin core staged for demand-rooted orthogonal recovery may extend to the nearest original eligible lower Support/SupportInterface capsule within its unchanged whole-fill, hole-split row. Fresh union-capsule proof must certify a nontrivial bounded two-ended gap before that proposal becomes Bridge authority. Preserve the complete required core, dimensions, contact provenance and sparse-group separation; same-axis and regional/mixed-owner extensions stay direct-previous-only. Rechecking the resulting consumed Bridge under the authored merge axis must apply the same fresh complete Contact-gap proof as the independent stroke analyzer, not infer or erase construction authority from that axis flag. This changes neither contact selection nor the final physical Z gap, complete actual-anchor/clearance and independent replay obligations.

After the emitted-byte/path bijection, independent verification reconstructs regional/mandatory intervals at the original authored row (holes included), preserving the defined raw hatch arithmetic before projecting interval boundaries by the sole XYZ6 contract. The regional core remains the complete original interval, not a shortened projected substitute. A separate check confines the whole actual emitted-row stroke to the captured source-owner domain. Verification freshly proves its maximal emitted stroke from actual lower capsules and rejects positive collinear duplicate commanded segments. SPLR6 projects each of its two deposition events to the existing Regional or MandatorySkin association; the plan retains the whole mixed path/transition. Packed ownership is per event, not a lossy whole-path owner encoding.

Required `PrintLayer.skin_command_fits` is empty unless minimum-length mandatory-skin construction is used. Each bounded record retains the complete original authored skin interval, its fitted two-point command, regional index/skin, effective source/winner contributions, and unchanged mandatory/source-owner polygon operands. Positive mandatory hatch intervals remain proposals even when their original XYZ6 endpoints coincide; they must acquire a proved command-length fit or refuse, never disappear as optional scaffolding. A fit preserves axis, row, height, direction and the complete original requirement; any extension stays in the same effective positive material and does not redefine classified skin area. Actual extrusion includes the entire fitted command. Overlapping ordinary regional extrusion is reserved rather than deposited twice.
Decoding shares the existing bundle-sized fit-metadata byte allowance across all layers and charges nested vector capacities and captured identifiers before allocation. Per-domain geometry points and plan-wide witness counts retain their separate limits.

Independent emitted-program verification reconstructs the original mandatory row and phase, checks its complete projected interval, the actual command minimum and original unextended endpoints, and confines both raw construction and actual emitted geometry to the captured source owner. Permanent plate/separator authority remains separate. Ordinary support and genuine Bridge requirements still apply to the final fitted stroke; a fit record grants no lower anchor, source/void waiver or machine-readiness claim.

Required `process_interfaces` accounts each explicitly authorized separator band once, separately in `separator_area_mm2`, never as dense/sparse fill. Each record binds source/provenance, the two stable effective `region_indices`, finite actual centerlines, admitted bead width, half-bead maximum bonding overlap and actual endpoint witnesses. Separator path segments biject to those lines; actual extrusion is counted once from final strokes, including authorized physical overlap rather than an averaged density.

Scheduled-M reuse is a held tolerant section: a span keeps its first layer's section over each following layer whose own section agrees with it within the profile tolerance (same rings, every vertex within it of the other's ring), compared against the held section so error never accumulates. Mapped image finishes keep one-layer spans. Realized D and each applied field retain independent identity/invariance obligations. Horizontal/image events and full-height occupancy remain separate. The full-height inner-material query below proves only its eligible subset, never whole-part invariance.

## Declared job timing

Optional, required-nullable `FixedDualJob.timing_model` captures `DeclaredJobTimeModel`: identity/revision, positive linear and filament acceleration, two nonnegative activation delays and per-command overhead. It configures estimation only, never firmware, physical calibration or a measured duration. Absence means no timing estimate; it does not mean zero overhead.

The independent program decoder derives required-nullable `VerificationReport.job_time` from its accepted actual commands and live XYZ/E/tool state. It retains separate model deposition, auxiliary deposition, travel (including lifecycle), feeder, activation and command-overhead terms. Coordinated deposition obeys both declared XYZ and filament acceleration under a rest-at-each-command model. The captured per-selection activation delay models switching/reactivation waits; no thermal simulation or measured wait duration is implied. It includes both waste strips and all selections, even waste-only tool groups. Counts must bind the complete independently verified command inventory; model identity must exactly match captured provenance in both bundles and native inspection replies. No second parser or per-move timing allocation is introduced.

`modeled_seconds()` sums only those modeled terms. Startup homing/heating is unknown and excluded, never a zero-valued total estimate. The native Job estimate disclosure labels that exclusion and exposes the model assumptions. The ordered `startup_context_hash` binds captured setup/profile inputs and unknown startup commands before the first tool selection; later control commands carry modeled per-command overhead and activation waits use the declared selection model. The hash permits only scoped comparisons within one shared capture; it is not a general physical timing-equivalence certificate. Predictions remain distinct from supervised whole-job measurements and do not confer machine readiness.

## Grid sparse infill

`{kind:"grid"}` generates centered build-X and build-Y line families in every eligible fine interval, each using half the requested sparse fraction. Pitch is nominal stadium area divided by physical height and that half fraction; minimum-row quantization and crossing overlap mean requested fraction is not a measured solid-volume fraction. Commanded dose remains stroke-derived. Solid regions, mandatory skins and walls keep their existing construction and authored axes; 100% fill remains ordinary solid fill. Switching patterns is an ordinary captured intent edit with history and artifact invalidation.

The Grid lane is single-tool with no enabled process/finish layers. Automatic external supports are allowed, with the same complete source/void/contact obligations as rectilinear. Sparse roads retain their complete lattice through bridge/support recovery; genuine regional Bridge roads count toward both-family completeness and still require actual earlier anchors. Missing families refuse instead of substituting rectilinear fill. It does not combine intervals or confer fixed-dual/functional/optical eligibility. Mandatory roof construction retains ordinary genuine-Bridge rules independently of the sparse pattern. Grid crossings are nominal deposition, not physical nozzle-clearance or crossing-quality certification.

A sparse clip whose endpoints coincide on the actual XYZ6 lattice is omitted before support demand: it cannot deposit or anchor material. This does not discard required skin proposals, which retain their fitting/joining path, or authorize a tolerance expansion of a remaining road.

Normalized admission and fresh actual-byte replay require positive straight build-X/Y rows with their classified Regional owner and exact fine physical interval. Replay checks both families for every nonempty sparse region and independently rechecks direct earlier-layer support from actual projected strokes; a same-layer crossing is never lower support. Geometry, hole, width, flow, resource/cancellation and full byte/path correspondence gates remain mandatory. Native controls select Rectilinear or Grid; combined bands remain a Rectilinear option.

## Web and roof infill

**Cathedral infill** is the product name for the retained web-and-roof construction. The typed `WebRoof` / `web_roof` identifiers remain the implementation and wire vocabulary.

`{kind:"web_roof",roof_spacing_mm:null,architecture:null}` is an opt-in retained object-material construction. Two perpendicular families of continuous one-road webs form chambers. Geometry-derived joint flanges reinforce eligible intersections automatically. Additional retained roof corbels grow from the webs into the chambers. They remain inside the finished part; this is not removable support or a load-optimized result. Ordinary Rectilinear remains the default. The required nullable spacing selects automatic policy or an explicit tighter value; protobuf carries it as an optional double, with no JSON RPC payload.

The required nullable `architecture` selects automatic Cathedral construction when null. Reinforcement has no separate user toggle or density setting. Native pattern selection preserves an already valid authored density; switching from hollow/solid proposes an 8% base-web starting point instead of silently keeping an inadmissible fraction. That is an editable software default, not calibrated porosity, strength or printability. It does not disable authored support or other incompatible features automatically. An explicit `WebArchitecture` can request a bounded flat-bottom window (`bottom_z_mm`, `height_mm`, `width_mm`, `cap_height_mm`, `pier_width_mm`) and/or `pier_rib_depth_mm`. These are experimental construction controls for offline studies, not separate product patterns or a mechanical recommendation. Protobuf carries checked typed messages; absent nested nullable authoring fields refuse.

Automatic joints use the captured infill bead width `w`, not a material-strength estimate or a fixed fixture dimension. A joint is eligible only when each intersecting web has at least `8w` to both adjacent parallel roots (or the component bounds at the ends). The centered `6w` flange span must fit completely inside the actual center-web chord and a same-component offset chord. A single parallel one-road flange lies `w` to one side of each eligible web, with half-length `3w` about the crossing. Preferred handedness alternates by parallel-root index; if its complete span would cross a boundary or hole, try the other side, then omit the optional flange if neither fits. Dense intersections, short/clipped chambers and missing material intersections keep the original continuous webs. The rule never invents an attachment across a void.

Automatic seed and joint columns are selected once from the complete retained classified-domain stack, with top-down priority, rather than reconsidered independently on each layer. Seed positions nearer than one bead to an existing column use that column. The same joint footprints are clipped into each eligible sparse layer, including partial spans before a complete flange fits; a change in per-layer full-flange eligibility does not invent a floating full-length road. Holes, source bounds, walls/skins and both roof directions remain intact; no column may fill missing source material, and ordinary actual-anchor analysis still reports genuine unsupported deposition. Collinear joint/web/corbel intervals merge before deposition. Staging, cumulative clipping work and complete actual material accounting remain bounded without reserving the job's entire command ceiling as live RAM. This is a conservative construction heuristic, not a load-aware optimizer, mechanical Pareto claim or physical strength qualification. Explicit study architectures retain their separate reconstruction and eligibility rules.

Windows address absolute build Z, including nonzero/negative beds. Their linear closing cap advances no more than0.5mm per millimetre of rise on each edge; the upper physical interval defines each staircase footprint. Only complete bays between two perpendicular roots with `width + 2 × pier_width` clearance receive a window; clipped end bays and narrower bays retain the original web. Two-bead intersection piers and the source-derived bottom landing remain intact. The upper opening must precede every possible roof-root catchment by the2:1 growth horizon, one bead and one nominal interval; this protection derives from the actual maximum root/boundary gap, not a fixed-height top belt. Requests intersecting a landing or roof-root corridor refuse instead of moving the window. Independent previous-deposition proofs still govern cap closure and every real stroke.

Pier ribs add two parallel one-road flanges to each root family at one bead-width offset, extending the requested bounded depth around each orthogonal intersection. They are vertical, retained, clipped to classified boundaries/holes and rooted by actual earlier deposition. Combined windows retain a pier wider than rib depth plus one bead; there is no rib-spanning-window credit. Collinear flange/web/corbel intervals merge before deposition and complete actual dose includes every retained road. This first rib primitive does not provide arbitrary horizontal/diagonal/hollow sweeps, folds, service-load optimisation or stability qualification. Fresh replay independently reconstructs explicit study windows/piers/ribs or automatic joint eligibility, handedness and complete merged inventory; deleting or duplicating a road cannot be repaired by changing accounting.

Cathedral builds on any sliced part, single or several per plate; there is no constant-section requirement. One part-level XY frame (the material bounds inside the walls) anchors both web families on every layer, so webs stack vertically through changing sections. A **ceiling** is material that ends above a layer — a step, pocket floor or hole underside — found top-down as each layer's section minus the next. Rings thinner than one bead are wall recession and are carried by the wall. Each ceiling carries its own roof height (its top minus top-skin thickness and one nominal interval) down through the 2:1 growth horizon, and corbels close under it by that height; elsewhere they aim at the part roof. A corbel may start on a floor skin: a short chamber is simply well supported. Each `WebRoofDomain` records `frame_mm` and the layer's `ceilings` so replay rebuilds the same roads. Single-tool execution and no enabled process/finish layers are still required, and base web fraction must be strictly between zero and one. Explicit study architectures (windows, pier ribs) still require one upright constant-section part. A ceiling span no longer than the roof spacing is an ordinary bridge, not a defect; when the web pitch is below the roof spacing no corbels are needed and skins bridge web to web.

The shared scheduler keeps protected source/process endpoints and also checks true and emitted XYZ6 interval heights against captured limits. At an exact process ceiling, it retries the existing neighboring balanced allocation rather than accepting subtraction roundoff above a physical maximum.

Construction uses physical millimetres on the existing schedule:

1. Freeze each component's centered web lattice at `2 × nominal_stadium_area / nominal_height / base_fraction`. Clip every road to the component's classified boundary and holes. Refining an interval changes actual bead area, not web XY positions. Minimum-row quantization, local roots and crossings make this a nominal base density, not measured porosity.
2. Divide gaps between parallel webs and component bounds into roof-contact intervals no greater than the resolved roof spacing. Automatic spacing is `0.75 × process.max_bridge_span_mm`; explicit spacing must be at least the resolved bead width and no greater than automatic. The actual bridge policy independently admits every stroke; contact spacing alone proves no support.
3. Before height-dependent growth, visit candidate roof rows in fixed-axis order. If a clipped chord longer than the roof spacing contains no perpendicular web root, retain a full-height perpendicular seed web through its midpoint. A seed within one bead width of an existing parallel web is skipped: that web is the support. This deterministic local seeding prevents roots across holes or outside concave rooms; seeds are constructed from the bottom, never invented wall anchors.
4. Root each corbel only at perpendicular continuous webs inside that same clipped chord. Interior chambers grow from adjacent webs toward their midpoint; end chambers grow from a web toward the clipped boundary. At physical upper Z, each extends `max(0, required_reach − 0.5 × max(0, roof_target_Z − Z))`. The roof target is the native top minus required top-skin thickness and one nominal interval, clamped at the bed. Short optional proposals await the existing minimum command length. Meeting branches merge into one road; rows coinciding with full-height seed webs do not extrude twice.
5. Preserve corbel endpoints during ordinary whole-row recovery. Required roads retain real earlier-layer capsule anchors and commanded dimensions. A required web crossing a genuine earlier hatch gap may acquire a Regional Bridge role only through ordinary bridge admission. Same-layer crossings cannot support a new branch; unsupported growth refuses.

Generation bounds clipped-edge queries, root/contact proposals, actual segments and retained domain allocation, with cancellation throughout. Each sparse process region retains a required `WebRoofDomain` containing its classified polygons and holes, captured before road generation. Empty/nonfinite/unclosed, excessive or misowned domains fail validation; the cumulative retained-domain allowance shares the32MiB bundle envelope. Native/classified geometry, physical-layer, command, flow and whole-program limits remain unchanged. No second scheduler, persisted kernel object, structural solver or new SPLR role is introduced. Joint scans and all collinear-merge sorts charge the same cumulative construction budget before mutation.

After actual-byte/path correspondence, fresh replay independently scans the complete retained domain stack, binds each layer's area to classified ownership, derives the shared seed/joint columns without trusting planner paths or a saved column decision, reconstructs physical-height corbels, and consumes every required projected road exactly once. Missing, extra or duplicate roads fail even after reconciled re-emission; road ends match within the 0.01 mm profile tolerance, and roads shorter than one minimum command are omitted on both sides. Both SparseFill and Regional Bridge roads enter this inventory. Captured minimum-command skin fits reserve their exact collinear regional intervals again after shared-column regeneration; independent replay subtracts only those separately validated fitted-skin intervals and still requires every remaining road fragment. Ordinary skin rows may still join or extend to real lower anchors; retained Grid/Cathedral sparse roads are fixed barriers, not rewritten as ownerless extended bridges. Every road's unsupported gaps must be short — a bridge within the bridge limit or an end overhang of at most three bead widths — unless its layer reports `unsupported_overhang_mm`. A fully supported road relabeled Bridge fails. Ordinary sparse roads retain fine physical intervals and earlier-layer support checks. Roof material is fully included in actual stroke-derived dose and work estimates. Increasing base density can reduce total material by reducing corbel growth; total cost is not monotone in the base fraction.

After canonical role sorting, bounded realization row routing compares the existing order with an axis/row traversal for constant-width sparse line paths, then optionally compares a deterministic nearest-entry tour. It alternates hole-fragment traversal, then the ordinary nearest-entry rule stores actual path direction. It preserves every road, owner, width and physical interval; gaps remain travel and never acquire an extrusion connector. A travel/retraction estimate includes projected entries and a conservative changed-endpoint allowance. Equal or worse candidates retain the admitted order. Canonical sorts/scans retain their cumulative O(n log n) allowance; the optional O(n²) search owns a separate finite cumulative allowance and skips when it cannot fit. Optional search cannot consume later canonical work or introduce a resource refusal. Its bounded temporary inverse permutation is released before event streaming; no second motion/index stream is retained. Both strategies retain cancellation. This changes command traversal only, not roof architecture or mechanical qualification.

Native controls expose **Cathedral**, **Base webs (%)**, **Automatic roof spacing** and a disclosed **Roof spacing (mm)** override. Joint reinforcement is engine-owned and automatic; there is no reinforcement control. Imported explicit research architectures carry a compact study marker and Reset action that clears only that override; Apply still commits the draft. Width and outer wall count use the existing recipe controls, avoiding a duplicate wall-density setting. Selecting the pattern from an unusable zero/solid fraction sets the visible draft base fraction to 8%; existing valid fractions are preserved. Apply/history/save/reopen/compile/inspect/export carry the policy; the UI does not construct paths. Scope and absent strength qualification are disclosed on hover. There is no service-load credit, buckling/interface qualification or physical printability claim. The offline research harness may propose this production pattern, but mechanical ranking remains subject to its independent numerical and physical gates.

## Combined sparse bands

Required `sparse_infill` is `{kind:"fine"}`, `{kind:"grid"}`, `{kind:"web_roof",roof_spacing_mm:null,architecture:null}` or `{kind:"combine_eligible",max_height_mm}`. Fine is rectilinear; Grid retains two orthogonal families at fine cadence, as specified below. The existing global fine schedule remains. Single-tool jobs retain their nominal width/material; explicitly bound uniform fixed-dual occurrences consume their larger interior widths and per-tool profiles. This is not an independent scheduler. The declared software height envelope bounds all paths. The current consumer stages at most three contiguous eligible intervals and4MiB of affected alternatives; H is terminal true upper minus first true lower, including unequal intervals. Native proof caps remain unchanged; replay transfer has its own schema6 allowance. Fixed-dual cycles and unbound jobs refuse combined bands; both tools retain supported waste on every fine interval. Fine schedule limits intersect every primary owner and both waste tools; bulk occupied-height limits do not enlarge the fine schedule.

[`planner::combined`](../../crates/manufacturing/src/planner/combined.rs) starts from a valid admitted all-fine plan. Full-band native material, independently invariant final D/finish, mandatory skin/event neighborhoods and effective source/profile ownership are separate gates. Layer-local regional indices alone never identify a physical band. A band spans layers that print one source prism (identical held section and process domains); the whole filled candidate superset, and separately the aligned landing rows with their anchor extensions, must pass the same full-height inner-material query as permanent plates over the band's physical interval. Regional cuts and actual hatch footprints remain inside that superset plus its admitted bead/projection radius. Outer occupancy and sampled endpoints are not inner-material authority.

Regenerate hatch pitch from `A(w,H)/(H*fraction)` and E from `A(w,H)=(w-H)*H+pi*(H/2)^2`; `A(w,sum(h))` is not `sum(A(w,h))`. Remove only the same effective region's original SparseFill or genuine Regional Bridge once, preserving positive material, holes, walls, skins, relief and other owners. Coarse strokes deposit at the band top, never by lowering into already printed fine walls. Actual base strokes below the first interval supply anchors: supported rows stay SparseFill; only a genuine direct-end-anchored gap within existing span/feed/flow policy becomes regional Bridge. Terminal previous fine fill that was removed is not an anchor. Reprove affected printing-time fine walls/separators and the following landing; an affected landing sparse row may acquire genuine fine Bridge process without changing its endpoints, width, interval or owner.

When authored coarse rows fail their base, the bound mixed-tool route may instead regenerate the complete next fine landing row set at the coarse physical interval. This preserves hole-split XY rows and constant captured width; it does not move the landing or infer material from matching bounds. Exact source invariance or a fresh full-height native inner-prism query over conservative whole-row supersets, including anchor extensions, is mandatory. The coarse rows must freshly prove their actual earlier base, local process roles and nominal dose at least equal to the requested area-fraction/physical-height target. `landing_aligned:true` requires an exact direction-independent row bijection, no duplicate/omitted row and an untouched fine landing before the next band. Ordinary supported aligned rows use the retained landing’s authored axis; other ordinary rows keep their own scheduled axis. Slightly greater nominal dose is construction evidence, not a strength guarantee or measured porosity.

Adjoining fine separators stay fine. Eventual coarse contact and printing-time stability are distinct: the future coarse stroke is never lower support. For each covered fine separator interval independently prove strict positive overlap with the coarse guaranteed nominal stadium core: conservative width `w-H`, inner vertical interval and XYZ6/arithmetic enclosures. Also retain the independent outer maximum-overlap/permission proof. Tangency, zero core or insufficient nominal overlap retains fine; outer full-height clearance prisms are not inner material or adhesion authority.

`SparseBandDecision {first_layer,last_layer,owner,disposition,landing_aligned}` records Combined or RetainedFine with `process_height`, `mandatory_neighborhood`, `changing_domain`, `missing_lower_anchor`, `unsupported_landing`, `flow_limit`, `interface_bond` or `nominal_dose`. Only proved local eligibility/support/bond failures veto optimization. Hatch numerical/topological refusal, indeterminate work, resource exhaustion, stale work and cancellation propagate; an error is not an inferred fine fallback. Stage borrowed retained paths and bounded landing-role overrides, then commit removals/moves only after all proofs.

Fresh actual-byte verification separately binds every emitted path/interval/owner, actual A/E/feed/flow, base Bridge anchors, printing-time fine stability and nominal separator core bonds. SPLR dimensions are validated expected-plan associations, not measured physical beads. Compare against the actual admitted fine alternative using the same source/profile/zones/fine schedule; independent native jobs may adaptively refine differently within declared bounds. Whole-command accounting includes travel, prime and retract/recover overhead. A commanded-motion lower bound omits acceleration, heating and homing and is not measured print speed, physical quality or machine readiness.

## Source-bound whole-volume and bounded box intent

Schema3 process intent is separate from finish and recipe: at most64 ordered layers/256 operations with exact checked definition/occurrence scope, WholeOwnedVolume or finite definition-local BoxMm, Replace/Erase and positive SparseFraction. Definition layers precede occurrence layers, with transparent layer-local Erase. Whole-owned resolution is explicitly a BASE excluding boxes, not a uniform spatial answer. Reset clears all exact-scope contributions. Normal setters reuse their intended stable operation; ambiguous boxes diagnose. Nullable permission is a tagged object: `{"kind":"separator_wall"}` or `{"kind":"horizontal_diaphragm","thickness_mm":positive_f64}`. Only Box Replace may carry permanent-construction permission; it is not implicit in a density request.

Native capture retains certified per-occurrence M before union and realizes each occurrence's finished D without duplicated native preparation. Shared singleton allocations remain shared. Box admission inverse-transports an outward finished-D enclosure including relief: exactly one X, Y or Z face may intersect eligible interior; the other five faces must be outside finished D. Local Z must remain parallel to build Z, with admitted signed-axis XY transport. Active requirements derive from actual winning contributors, never append order or shadowed consent. Unsupported/unrealizable enabled requirements block export; empty eligible selection reports visible NoEligibleInterior. Equal-density no-op intent reports NoDensityChange and preserves the ordinary lattice.

The finite separator reserves its band exactly once inside original eligible interior; complementary cuts must cover that original set without excess, overlap or dropped positive slivers. Exterior walls, physical skins and source holes remain unchanged. Sparse regions retain source/value attribution; positive overlap involving spatial profiles explicitly refuses unsupported composition. A skin recovery corridor stays within its admitted material owner and excludes exact reserved bands; sparse recovery remains profile-owned.

XY separator construction requires an explicitly fixed compatible hatch axis (X for an X boundary, Y for a Y boundary), not alternating or silent rotation. Adjoining ordinary generators share the physical definition-origin phase transported by occurrence. Genuine orthogonal Bridge recovery uses the same interface frame, mirrored into each owner: first parallel row at three-quarter bead distance, then that region's normal pitch. This supplies bounded positive longitudinal contact without tacks, solid fallback or independently centered zones. Every such joining row retains its regional owner and independently proves a real Bridge span and actual previous-deposition anchors.

Explicit SeparatorWall permission authorizes one permanent open rib, not two closed zone perimeters. Maximum physical overlap is half a bead; inward construction targets quarter-bead overlap where shortening is needed. Actual decimal XYZ6 capsule predicates certify positive contact and maximum overlap separately from representation error (exact rational comparison for canonical decimal dimensions, otherwise conservative outward bounds). A terminal bonds directly to an actual same-occurrence innermost exterior/hole wall, or to an earlier continuous mandatory skin path itself bonded to that wall. Genuine Bridge may carry that unanimous skin operation only with full independent lower-anchor/span/order/flow proof. Arbitrary sparse, mixed-owner Bridge or floating skin is not substitute terminal authority.

Final binding/shortening occurs at actual recovered per-layer acceptance BEFORE later layers borrow its ends as lower anchors. Final points drive coverage/witnesses/actual volume. SeparatorWall never becomes Bridge or receives a support exemption. Hole-split ribs need valid actual material attachment at every endpoint; unsupported/floating ribs refuse. No brushes, six-face box zones, gradients, pattern switching or material routing are implied.

### Centered permanent horizontal plate

One source-Z boundary can request a plate centered at `source_plane_mm ± thickness_mm/2`. Capture maps those source endpoints through the actual occurrence pose to an ordered world `physical_z_mm`; source coordinates are never reported as world coordinates. Native/field/support stops, the density boundary and both plate edges enter one schedule before global balancing. Process-dependent D/classification spans split at those events while borrowing unchanged native M. Values on opposite sides resolve independently with the existing source resolver.

Every actual interval in the requested slab has exactly one matching `ProcessDiaphragmCoverage`: occurrence/source winner, permission, adjacent authored values, source plane, requested thickness, world slab, eligible extent with holes, area and exact regional index. Its separately authorized construction region is solid even though neither authored density is1. It owns existing Regional SolidFill or genuine Bridge paths, never a fabricated SparseFraction1 override, separator bond or joined flag. `diaphragm_area_mm2` charges that construction once in the interior partition, separately from authored density; regional fraction1 is construction coverage, not an authored solid override.

The complete slab must lie in eligible interior without consuming walls or skins. The whole filled plate owner, grown by the full nominal bead/XYZ6 projection radius less the profile tolerance, must lie in the contributing occurrences' full-height inner material over the plate's world slab: each occurrence's tolerant section minus the projection of every source facet crossing the slab's interior, padded by the model's support enclosure, with flat faces within one Z step of either end bounding the slab rather than crossing it (`tolerant::inner_material_admits`). Residue above the profile tolerance squared refuses (`UnsupportedInterface`); protected source holes stay outside the section. The plate's layers share one held section when their own sections agree within the profile tolerance, so the plate keeps one extent; otherwise the extent changes across the slab and the plate refuses. Checked inward XY endpoint construction retains each original row/axis/phase and exactly one positive printable interval, leaves raw schedule Z unchanged, and independently proves exact whole-line coverage by the original raw owner. Lost/split/sub-printable rows refuse without phase search, tolerance relaxation or dropping positive material.

After final coordinates, prove the first plate interval from actual earlier strokes, every later plate interval and the first upper landing; only a genuine anchored gap within existing Bridge/span/flow policy receives Bridge. Separately prove actual exterior/hole-wall attachment. Plate-owned residual support demand and an unsupported upper interface refuse before temporary-support generation; ordinary unrelated support retains its existing policy. A future plate or coarse stroke is never an earlier anchor. H1 retains fine at plate/intent events and their neighborhoods. Independent byte replay checks exact ownership/extent, holes, role/order, lower anchors, upper landing and attachment rather than trusting planned labels.

Unrepresentable requests may report bounded **schedule-admissible** nearby thickness choices under current native/field stops and physical height limits. Each still **requires Compile** for material and support admission; the request is not silently rounded and no native candidate-search loop runs. Numerical, indeterminate, resource, cancellation and stale failures propagate.

## Source-backed whole-face finish construction

Captured `surface_overlays` is the single ordered whole-face finish channel, not a profile flag or recipe material change. Enabled definition layers resolve in stored order, then enabled occurrence layers; the last matching layer wins. Smooth explicitly overrides lower ribs; disabling/removing it reveals the lower finish. Disabled and shadowed layers remain captured provenance/fingerprint content, and every saved target must name an actual face in the exact rebuilt native table.

Finishes run on the single [ADR012](../adr/012-tolerant-brep-curves-and-feature-heights.md) pipeline. Face eligibility, carrier frames and field parameters come from the imported BREP's analytic faces; M at each layer is the finished occurrence's tolerant section, kept apart from the union of unfinished occurrences. Every eligible carrier and its neighbours are analytic planes or vertical cylinders, whose sections lie on the BREP to rounding, so the chain matching, retention, simplicity and positive-addition checks below apply to that section unchanged. They are checks on the realized result, not native certificates; a section that fails them refuses the job explicitly, never prints it untextured.

VerticalRibs requires the immutable native certificate for one four-line, unholed, full-height rectangular exterior vertical planar face with an outward global supporting material side. Both planar and cylindrical finish lanes additionally require an upright bed-anchored invariant extrusion: local Z maps to either sign of build Z, without tilt, intermediate shelves or sloped/tilted carriers. Source-local coordinates and phase follow the occurrence pose. Sloped, partial-height, recessed, ambiguous co-carrier and unsafe/unsupported combinations refuse explicitly, never succeed without the requested finish. A cylindrical carrier is not eligible for the planar finish variant.

The outward field is a smooth raised-cosine periodic profile multiplied by a smooth end taper, constant through the full source height. Protected end bands have nominally zero displacement, original end vertices remain exact, and newly constructed guard/sample coordinates reserve an explicit arithmetic bound; additions close at bed/top with no height expansion. Require pitch ≥ four bead widths, protected margin ≥ two bead widths, depth ≥ four finish approximation bounds and depth ≤ half a bead width; the face must fit both margins and a full pitch. Conservative second-derivative sampling bounds chord error to 0.00099 mm, with 0.00001 mm reserved for native-frame agreement and bounded outward-directed rounding, totaling at most 0.001 mm independently of native/offset tolerances. Compile also requires a resolvable rib peak surviving ordinary first-wall erosion; lost or unresolvable depth refuses.
Sampling includes both half-pitch taper breakpoints. Each ramp uses the conservative product second-derivative bound; the constant-amplitude interior uses `depth * (2π/pitch)^2 / 2`. Each interval count independently satisfies the same 0.00099 mm chord bound. Native chain parameter insertion only subdivides those intervals; it does not flatten the waveform or enlarge the budget.

For planar ribs, each addition A extends inward by one quarter bead width; strictly interior overlap witnesses plus native-boundary intersection/exclusion checks prove the positive collar belongs to original M and contains no protected hole. The native certificate must match one unique full-span contiguous monotone exterior sample chain by both trimmed endpoints, global supporting material side and material-left winding, including closed-ring wrapping. All requested faces are proved against original M; profile sampling merges every original chain parameter and anchors each interval to its actual native segment. Bounded outward samples replace only the active face intervals. Every original material point remains included in D; untouched/protected vertices, full-span endpoints and hole rings retain their exact coordinates. Active-face native samples may become interior M instead of explicit D boundary vertices. Exact robust orientation, local/global progression, original-point inclusion and nonintersection establish mathematical `D = M union A`, with positive added area and unchanged island/hole ownership, without a floating Boolean re-quantizing unrelated geometry. Directed rounding applies only to new samples and refuses beyond the construction bound; missing-material slivers are not accepted with epsilon tolerances.

CylindricalRibs instead requires an exact admitted full-cylinder exterior face. The geometry adapter retains face-associated oriented analytic trim references before modeled circles become NURBS: top/bottom rims cover one continuous full turn at analytic source-Z extrema; remaining axial seams have opposite paired native uses; no additional trim or co-carrier face remains. An inward bore or the original corpus cylinder's two separate half-faces is not silently merged into a valid target. Source-local +X/right-handed +Z defines `theta`; the field is `depth/2 * (1-cos(rib_count*theta - 2π*phase_fraction))`, independent of native seam/UV. Integer count closes the circumference; derived pitch `2π*radius/count` must be at least four bead widths. No artificial angular endpoint margin is stored.

Cylindrical construction matches one full convex native exterior ring with an interior axis, retains its native edge parameters and all protected hole coordinates, and displaces actual native edges radially rather than replacing them with an analytic circle. Per-edge `q(t)=p_native(t)+f(theta(t))*u(theta(t))` bounds use actual supporting-line distance and edge length; native endpoints, field peaks and exact copied seam closure are retained. Chord error is at most0.00099 mm plus0.00001 mm bounded evaluation/pose/directed-rounding reserve. Robust source inclusion, noncrossing, positive simple output and no ring contacts are mandatory; no missing-material epsilon or Boolean re-quantization. First-wall radial peak survival compares against analytic radius with separate native0.005+finish0.001+offset0.005 mm bounds, requires depth>0.022 mm and refuses an unresolved ordinary erosion. It is not physical grip or swept-bead certification. See the [reviewed decision and derivation](../adr/002-cylindrical-grip-ribs.md).

### Mapped image relief

ImageRelief uses the same certified rectangular exterior planar walls or complete exterior cylinders, but the field genuinely varies in source U and source Z. Its captured canonical image is evaluated with periodic pixel-node bilinear interpolation, no half-pixel shift, explicit physical Z period/phase, U repeats/phase, depth and protected planar taper. Source-frame mapping follows upright and source-Z-flipped occurrences; native UV/seams are not authority. Exact formulas, canonical PNG normalization and local derivative/error bounds are in [ADR003](../adr/003-spatial-image-relief.md).

At each admitted sample Z, lift the field on the actual native edge parameters, partitioning all crossed texel/seam/native/taper boundaries. Preserve protected native coordinates and holes, prove M is retained and D is simple/noncrossing, and apply the separately bounded XY construction certificate. Blank image rows return unchanged M. Do not apply the old rib-specific half-bead depth limit or peak-survival refusal to image fields: fine intended features can be below nominal path resolution and require honest diagnostics, not erased source intent.

Wall admission explicitly accepts requested D and required reachable material C. Image-owned islands require native M plus every non-image finish addition, including ribs sharing that island, to remain reachable by the same D-derived first-wall E/coverage proof. Non-image islands retain the full-D gate; rib peak checks are additional, not a replacement for full rib coverage. C is independently cached per native M slab under the current invariant rib envelope, shares M for image-only occurrences, and is attributed by native witnesses rather than polygon ordering. Only image-added sub-bead lobes become nominal-resolution advisories. No duplicate erosion, relaxed reachability threshold or silently discarded source/rib material is permitted.

Scheduling partitions native/image/support event intervals before nominal-height subdivision and local vertical-error refinement. Enabled breakaway construction also proposes XYZ6-enclosed post-image-row minimum-gap floors before balancing, so native-anchor startup and later contact demand need not occupy one slab. These bounded, deduplicated proposals preserve every original event, derive no recursive support stops and grant no anchor/contact authority; actual deposited strokes, complete clearance and final physical-gap replay still decide acceptance. Do not union a global nominal grid with texture events: this creates avoidable sliver layers and can collapse serialized extrusion. Preserve genuine event boundaries with arithmetic-identity-only coalescing; keep minimum thickness and command precision checks unchanged. Admit event/layer/minimum-thickness/retained-point/work limits before native intersections. A narrow bright row between nominal sample planes cannot be silently aliased away; excessive refinement fails explicitly. True scheduled heights drive flow, skin horizons, supports and independent replay.

Physical-skin horizons intersect consecutive final-D spans through a bounded lazy aligned-range cache whose positions are hatch courses: the spans of a split course form one leaf, and a range ending inside one is intersected span by span, so a refinement never changes another range's decomposition. Cached identities refer only to immutable realized spans; neither native invariance nor source M authorizes reuse. Leaves borrow D, internal intersections share geometry, and ascending-range combination is deterministic. Surface proofs, required-material attribution, wall candidates and actual padded Boolean admission share the16,384,000,000 cumulative construction-work ceiling; candidate/layer changes never reset it. Native interval work remains a separate resource. Cached and live query geometry are included in retained/staging bounds; the retained-storage and job-wide deposition limits remain independent. Travel separately admits the three moves required by each lifted entry; [SPLR admission](replay.md) retains its byte-derived aggregate bound. Both support-regenerated alternatives reuse the same D neighborhoods.

Optional cached intersections reclaim unpinned least-recently-used geometry under combined retention pressure; live shared operands remain counted. Eviction scans and recomputation remain charged. Support planning reserves a frozen cache-retention pool, so regenerated queries may reclaim/rebuild within it but cannot silently increase support-stage retention. Ownership classification reports its complete live scratch to the reclaiming retention authority. Post-support regeneration additionally includes live support/interface/protected-domain geometry; neither a frozen cache pool nor borrowed geometry hides newly owned query/classification temporaries.

Support demand/contact follows final D and actual deposited object strokes. Every relevant lower D participates in support-column/corridor exclusion; changing a lower relief can invalidate an upper support route. Both support-regenerated hatch alternatives consume the same D, not native M. A project/field edit invalidates the complete derived artifact; there is no fixed last-N-mm invalidation shortcut. Z gap remains a conservative minimum, not an exact physical attachment certificate. Future fin placement intent must be distinct from a realized contact/shaft and re-realized against changed D; no fin planner is claimed here.

Support/interface realization erodes the combined clearance-clipped physical support domain S once. Interface subset I selects centerline roles and full-density interface hatching, not a separately eroded material boundary. Before any wall/stroke authority, optional convex ears or bounded local inward/Steiner moves may resolve sub-command geometry or genuine original-I spans only with complete containment of every replacement segment in unchanged E(S), preserved simple rings/hole/component topology and exact original-I role feasibility. Inward search coordinates grant no authority from proximity or offset samples: independently prove each segment in the current proposal as well and exclude every other boundary vertex from the discarded pocket. Candidate counts, edits, scratch and every attempted proof remain bounded, charged and cancellable. Feasibility shares wall construction's analytic role cuts and midpoint ownership; every genuine role span must meet the unchanged XYZ6 command floor, while same-role tangencies grant no permission. Original S/I, hatches and required object/skin remain unchanged. Construct every resulting exterior/hole wall in source order and partition its role without duplicate seam outlines. Sub-minimum commands may fit these frozen wall proposals within 0.001 mm, only inside the unchanged eroded physical domain, preserving endpoints, closure, width, physical interval and role boundaries. Every frozen intermediate point participates in the deviation bound; holes and separate roles cannot be bridged. Where no complete chain of admissible commands exists, the wall keeps the admissible chords covering the most original length and omits only the remaining sub-minimum edges, splitting the wall there (a closed ring continues through its seam); the layer reports the omitted length as `region_coverage.support_omitted_wall_mm` and the verifier lists it as a limitation. Optional support/interface hatch proposals shorter than the actual XYZ6 command minimum are omitted before anchor admission; this does not filter required object strokes or grant support to any unmet demand. Actual XYZ6 commands retain the declared minimum length and still pass full-bead clearance and preceding-deposition anchor checks. Allow bead overlap across the internal role seam while retaining full-bead containment in S. This prevents a sub-bead I strip from silently erasing previous-layer anchors. Candidate hatch axes run the unchanged actual preceding-support capsule/bridge proof; count all attempted work/staging and publish only selected output segments. Support is build-plate only: when neither axis anchors every stroke on the support below (an upper footprint ending between lower rows, or orthogonal rows parallel to them), the layer prints the axis with the smaller worst unanchored span as authored (the preferred axis on a tie) and reports that span in `region_coverage.unsupported_overhang_mm`, never refusing the job.

`surface_resolution` records bounded per-layer/face/field nominal-bead witness diagnostics with source U/Z, cell/repeat, residual lower/upper bounds and resolved/ambiguous/below_nominal_resolution/unavailable classes. Representatives are capped at8 per record; coverage/completeness and truncation are explicit. Compute against the attributed actual first-wall exterior, not a scalar peak or a nearby unrelated ring. These sampled advisories neither duplicate the production R/E pipeline nor certify full Hausdorff error, physical texture or printability. Mandatory field/retention/chord/Z/support failures remain hard errors. Summary totals preserve advisory counts/completeness separately from fresh emitted-byte verification.

Source M remains separate for area/provenance accounting. In overlapping source Z interiors, finished occurrences must have disjoint closed XY bounds; their exact rings are appended without another source/derived union overlay. Untextured occurrences are unioned as usual. The same D enters invariant-span identity, ordinary wall/fill/skin ownership, actual deposited-stroke bridge/support checks, regenerated hatch alternatives and clearance corridors. No jitter of prebuilt perimeters, emitted-byte mutation, mesh matching, source-only regeneration or new path role is used.

Preflight admits exact native face metadata and process bounds, expands conservative placed extents, and excludes another occurrence, fixed component bounds and complete-job priming; actual section/overlap/topology/offset/anchor proof remains a Compile obligation. Conservative expanded-AABB exclusions may reject geometrically separated configurations whose boxes overlap or touch in XY within overlapping Z interiors. Sampling, staging, retained geometry, cumulative Boolean/boundary-proof work and final object/support segments retain existing bounded admission/cancellation. Exported construction is not an edited BREP, physical texture/grip certification or printer qualification.

## Provenance, reports and immutable bundles

`ManufacturingProvenance` contains `project_id`, captured `input_revision`, `input_hash`, `definitions:StoredDefinition[]`, `occurrences:OccurrenceRecord[]`, explicit `intent`, full ordered `surface_overlays` and `process_document`, `surface_images:SurfaceImageContent[]`, `kernel:{name,version,revision}`, and explicit planner/emitter/verifier versions. The bounded unique image content table contains canonical PGM bytes once per referenced image, including disabled layers, making exported provenance self-contained. Immutable capture shares canonical content. Inspection/status provenance contains metadata only. Captured source labels/units/uncertainty remain authoritative; native reimport labels must not replace them.

The [native-first DEVELOPMENT route](../adr/ADR005-native-first-development-geometry.md)
uses source21 and the imported SolidCore BREP as its single material authority M.
The realization binds native spline-chart construction, literal active-domain queries,
coefficient-projected height-event exclusion, polynomial derivative-control bounds
with exact component-axis invariance, same-sign scalar control hulls,
continuous first-jet native product reuse, native material endpoint support and
strict closed-slab exclusion from bounded directional material support, plus
same-cell scalar/Cartesian height-gradient intersection, centered implicit-graph
enclosures and chord-owned native section error bounds, endpoint-qualified
native direct/atlas dispatch, immutable radial calibration owners and continuous
closed-strip trim contact and locally centered native PCURVE construction
with literal endpoints and retained constant chart coordinates. Directional
support intersects same-cell scalar/Cartesian coefficient and centered bounds
before trim classification, without creating additional feasible witnesses.
The native torus carrier contains the complete lifted face chart. Height census
can consume an exactly matching owned physical-boundary height without treating
boundary membership as an interior UV witness or a closed material certificate.
Closed material queries retain same-cell centered coefficient bounds separately
from original Hessians. Complete fiber census requires C1 on each accepted
root owner, not on unrelated knot endpoints; unresolved boundary roots refuse.
Planner59 rejects previously compiled artifacts whose geometry producer differs.
The current planner identity is specified with the normalized plan above.
Original STEP bytes, immutable definition/face/use identity, units,
uncertainty and original finish operands remain captured inputs and provenance,
not a second reconstructed solid. Query-private mutation, caches and derived
trims cannot silently redefine M. Material realization identity binds the native
producer and admitted query/tolerance policy; geometry-dependent checks rely on
that kernel rather than an independent original-STEP topology/embedding proof.

Native source leaves and actual union candidates retain independent per-source,
pose, interval/reference, policy and candidate bindings. Adjacent regular slabs
require actual-pair authority at the identical shared cut, including independent
material/void buffers, complete Jordan coverage and bounded nonzero collars.
Invariance or one good section cannot supply closed-cut, critical-endpoint or
attachment authority. Pending receipts and scratch share the live byte bound.
Realized D, continuous occupancy and deposited anchors remain separate obligations.

Producer/kernel versions are checked independently of the authoring input hash.
Earlier source5/planner23–24 evidence does not qualify this material route, and
older bundles cannot be relabeled into current authority. Independent emitted
command/process replay remains mandatory and reports native-kernel reliance; it
does not certify original STEP topology or physical printability. Fresh operator
qualification uses exact published dependency pins, never a diagnostic override.

`input_fingerprint(project_id,definitions,occurrences,intent,surface_overlays,process_document)` validates these inputs, sorts definitions/occurrences by identity, then SHA-256 hashes compact UTF-8 Rust serde_json in this field order:

```text
{schema_version:MANUFACTURING_SCHEMA_VERSION,project_id,units:"mm",frame:"right_handed",definitions,occurrences,intent,surface_overlays,process_document}
```

`MANUFACTURING_SCHEMA_VERSION` denotes the numeric value of the Rust constant in [`spiling-contracts::manufacturing`](../../crates/contracts/src/manufacturing/mod.rs), not a separate fingerprint revision.

Every record/intent field uses its Rust declaration order and serde spelling, without whitespace. Source bytes enter through source SHA-256 and definition identity; source units, retained labels and declared uncertainty are included. Revision/allocator/history/artifacts/timing are excluded. Algorithm/kernel provenance is additional bundle content identity. This is a Rust-defined canonical serialization algorithm, not a promise that arbitrary JavaScript JSON number formatting is interchangeable. Get/status does not rehash geometry on its hot path.

`VerificationReport` preserves bounded verified/coverage/limitations and position/extrusion errors, object-only deposition segments/volume/filament and actual travel count. Separate required support segment/volume/filament fields exclude support from object totals; lifecycle/prime totals remain separate. `retraction_count` and `retracted_filament_mm` account withdrawn feeder distance; paired recovery never counts as deposition or prime. Measurements are finite nonnegative, labels bounded, and stored verified:true never substitutes for fresh independent actual-program replay.

`ManufacturingSummary` contains schedule layers, paths, object-only deposition/volume/filament, separate support deposition/volume/filament, `surface_resolution` advisory totals and software_only:true. Whole-event/deposition budgets include supports, but reported object material ownership never does.

`ManufacturingBundle` contains exactly `schema_version` (the Rust `spiling-contracts::manufacturing::MANUFACTURING_SCHEMA_VERSION` value), provenance, plan, `program` and verification. `program` is a `ProgramDescriptor {hash, byte_count}` naming the exact emitted program bytes; those bytes are a separate immutable artifact, not bundle content. Immutable asset hash covers exact compact UTF-8 JSON without self-reference. `ManufacturingArtifactRecord` binds bundle hash, byte count, summary and the same program descriptor as checked manifest metadata. Raw bundle JSON is bounded at 768 MiB and program bytes separately at 512 MiB, below PTAC's 1 GiB object and SQLite's ~1 GB blob ceilings; neither is ever buffered whole. Jobs admit up to 2,000,000 deposition and 6,000,000 travel segments, with retraction and process-event counts bounded by those totals. These are checked counts, not allocations: internal batch and scratch thresholds have their own constants. Core's 256 MiB active/staging/history capacity counts held compressed frames of bundle and program; a compiled result may use the remaining allowance not held by current and history. Actual-motion replay is a complete indexed SPLR stream, spooled with bounded scratch rather than limited to 32 MiB. Its representation bound derives from admitted deposition/travel/process/tool/lifecycle counts; it does not reserve the whole stream in memory. The 4,096-layer bound is unchanged. Review retains compact shared CPU geometry when machine headroom permits and uses checked disk backing otherwise. Display preparation failure preserves independent verification and export with an explicit diagnostic. Durable source/storage authority is unchanged.

The Rust producer, Core and clients hold both the exact bundle JSON and the program bytes as independently compressed 256 KiB frames (`spiling-chunks`): the engine encodes the bundle straight into frames, Core retains and stores frames, and `decode_bundle_from` parses from a frame reader without a raw buffer. The emitter writes program frames once; the verifier and export read them sequentially. Frames never cross a process or storage boundary: the project stores the raw program as its own object and ByteStream serves raw bytes. `spiling-manufacturing::ManufacturingResult {bundle, program}` pairs a bundle with its program and refuses a mismatched descriptor before replay. A program whose bytes no longer match its descriptor is a corrupt artifact; a consistently re-described edit still fails independent replay.

Public schema APIs are printer/recipe/component/capability/intent `validate()`, plan `validate(&intent)`, provenance/report/summary/artifact/bundle `validate()`, `decode_bundle(bytes)`, bundle `summary()` and `validate_record(&record)`. Summary/record matching require an already validated bundle. Record matching checks semantic input hash, summary and program descriptor; callers separately check exact bundle and program byte hashes and lengths. Schema validation does not parse/replay the program or confer replay success.

## Emitted program and independent replay

Programs remain conspicuously SOFTWARE VALIDATION ONLY, NOT MACHINE READY. Explicit modal/lifecycle commands establish the declared software coordinate/thermal/extrusion contract. XYZ travels/deposition carry explicit F, stationary retract/recover use real E-only G1 commands, and actual first-layer/bridge feed/flow/fan policies are realized. Fan S0..255 commands bind the next queued motion, not current physical cooling. Complete configuration declares ordinary default cooling fan, no laser synchronization and external E-only retraction; no firmware truth is inferred from a hash. Six-digit XYZ/feed and eight-digit E quantization must retain representable movement/extrusion. Complete jobs include independently checked prime, barriers, clearance lift/park and shutdown. No submission/connector or arbitrary macros exist.

For a printer declaring `arc_moves`, each arc run is one `G2` (clockwise) or `G3` with X/Y/Z, I/J (the projected center minus the actual start, six decimals), E and F. The commanded arc is the circle about start plus I/J through the start; its length times the bead area owns the run's E. Replay requires the declaration, consumes the run's planned edges in order and checks the commanded center and direction against the plan, the end against the run's projected last point, every planned point on the commanded circle within the run allowance and in sweep order, and the whole arc inside the build envelope. Each edge then replays as a virtual straight deposition at its projected planned point, with E interpolated along the arc and its arc length, so feed, width, flow, volume, wall-gradient and ownership checks apply unchanged.

An independent verifier decodes command state from emitted bytes, not emitter helpers or a copied move buffer. It checks exact workflow modes/lifecycle/order, malformed/duplicate/nonfinite fields, thermal targets/waits, envelope, speed, nominal flow, object-excluded priming, positive deposition versus travel, expected path ordering/counts, positions and extrusion. Missing/extra/tampered commands fail verification. Coverage includes complete-job command semantics, not physical startup/homing or sensor truth. Replay explicitly excludes physics, collisions, component swept volumes, actual firmware/configuration behavior, calibration and physical printer support. A successful replay proves only the declared software semantics.

Current Cartesian XYZ admission binds independently parsed coordinates to the checked XYZ6 projection of each captured expected point. Raw-plan half-step tolerance cannot authorize an alternate neighboring lattice point. The explicit `contracts::manufacturing::program_coordinates` module owns this fixed output precision, coordinate-conditioning range and outward decimal/height enclosures; backend serialization and verifier command parsing remain independent. Projection multiplies the binary64 input by `1_000_000`, rounds ties away from zero, divides by `1_000_000` and normalizes signed zero; it is not exact-real decimal rounding of the original input. Accepted absolute coordinates are at most `2^52 / 1_000_000` mm, preserving distinct adjacent parsed micrometre lattice points. Compensated gap subtraction checks every intermediate and its outward result for finiteness. Unrepresentable arithmetic refuses with `program_coordinate_conditioning_failure`, not a widened clearance tolerance.

Current inspection/reopen requires planner98, slicing every job, including permanent process interfaces and fixed-dual routing, on the single tolerant pipeline with held prism sections and full-height tolerant inner material, keying alternating hatch axes to physical courses, binding retained analytic/faceted source interpretations to shared prepared sections, support corridors that also exclude actual object beads reaching outside their material and, at the top support layer, the next section's touching occupancy, and additionally binding effective surface finishes realized on tolerant sections, complete tolerant support occupied/void projections, tolerant contact selection at the requested Z gap, stitched wall lanes and authored overhangs where refinement is exhausted or an actual-anchor split shortened no unanchored span by more than the profile tolerance, and explicit new-job Automatic creation. It retains native-first material authority, automatic Cathedral joints and wall/finish/hatch construction, mandatory physical per-occurrence material events and query-local vertical products for terraced sources. Local reuse excludes every cap in the closed queried interval; whole-source and touching-cap ownership remain separate. Input fingerprints bind complete ordered authoring documents, source/pose and printer/recipe/routing policies. Fresh replay independently rechecks actual decoded dimensions, contacts, role/order/flow, ownership, permanent plate extent/attachment and lower anchors; fixed-dual replay additionally reconstructs source/material and staged tool/frame/thermal/debt/waste state. It does not reconstruct transient native source/finish certificates. Project10/manufacturing11/process3/surface3 remain stable. Emitter16 retains the complete automatic-joint construction; verifier41 additionally counts genuine regional Grid bridges in both-family completeness while retaining actual earlier-anchor gap checks; verifier42 resolves alternating axes from each layer's declared hatch course; these identities invalidate superseded programs; SPLR6 keeps its existing binary interpretation. Older development assignment shapes refuse without aliases or missing-policy defaults.

Final normalized-plan admission checks ordinary projected axes during its existing coordinate pass. Independent fresh verification separately checks closed-wall seam scores, same-tool history and distance/coordinate ordering before matching actual emitted commands. A mutually matching program and plan that ignore either consumed policy do not earn semantic success; stored `verified` metadata remains descriptive.

`inspect_bundle(&result,&bundle_hash,output,&cancel)` takes the paired `ManufacturingResult` and produces `ManufacturingInspection {report,provenance,replay}`. `VerificationOutput` is explicitly `report_only` or `report_and_motion`; `verify_bundle` retains its report-only API. The same independent command decoder streams validated actual XYZ, absolute E and F into an optional disk-backed SPLR record sink. Layer/kind attribution comes from the validated expected-point iterator, never emitter comments. Report-only work allocates no display buffer, index or display/program hash. Failed decoding or cancellation drops the unpublished optional sink; no partial display result exists.

`ManufacturingInspectionProvenance` captures `project_id,input_revision,input_hash,surface_overlays,process_document,kernel,planner_version,emitter_version,verifier_version,workflow,surface_color` from the actual bundle, not the current project. `ReplayDisplayDescriptor {pages,info,resource}` binds packed actual motion to the original bundle/input hashes. Its immutable MIME is `application/x-spiling-replay-motion`; native/client validators bind exact size/hash/counts and the chronological raw-record page index. Streaming admission validates every record with continuous state and independently regenerates index metadata without a motion-object vector. [Replay contracts](replay.md) own backing, index and compact rendering semantics.

## Native services and immutable retrieval

The typed `spiling.engine.Manufacturing` service owns Get/SetIntent Execute and standard Google-operation Preflight/Compile/Verify. Every long request captures explicit project base revision, session and optional canonical request UUID. Preflight is nonmutating, uses the same bounded native worker/revision/cancellation ownership and returns no manufacturing artifact.

The Rust/TypeScript shell adapter uses `ManufacturingCommand` for `get`, `set_intent`, `compile` and `inspect`; `inspect` maps to Verify with captured project revision and required explicit `output`. Protobuf output unspecified or unknown is rejected, not implicitly report-only. Status contains project info, optional intent, optional artifact record and an optional immutable bundle resource descriptor. The adapter reports accepted work as `operation_accepted {operation:NativeOperationView}`, with a Google operation name, never a public numeric job handle.

`PreflightReport {input_revision,eligible,software_only,checks}` has at most 256 checks. Each check contains bounded code/message, severity error/warning/info and optional occurrence/face IDs. Eligible means inexpensive input/profile/orientation/placement admission only, not native slicing or physical printability. Explicit informational checks state that native layer/support/bridge/offset analysis occurs during Compile and physical qualification is absent. Native result `ManufacturingPreflight {report}` must match captured revision and have no immutable outputs.

Native metadata and result use the protobuf `NativeOperationMetadata` and `NativeOperationResult` Any types. Compiled results contain project info, record and resource; verified results contain record, fresh report, bundle resource, captured inspection provenance and optional replay display descriptor. All bundle and replay resources must be present in immutable outputs; conversions reject missing provenance, unsupported frames, malformed counts/hashes/MIME/size or record bindings. Standard Operations Get/Wait/Cancel and Engine WatchOperation provide observation and explicit cancellation. Dropping or timing out a unary RPC does not cancel accepted work. Unfinished retained operations become interrupted/aborted on engine restart; old session results are historical, not active native handles.

Compilation captures immutable inputs and their persistent revision. Core remains sole writer. After compile/replay, ACK publication rechecks project/session/revision/cancellation. Geometry, intent, process or finish edits invalidate the complete artifact atomically, including support/bridge obligations; undo restores coherent inputs/artifact without rewinding allocators. Publication advances persistent revision while provenance keeps the compiler input revision. Read-only permits fresh inspect/replay, never mutation or Compile publication. Durable operation success appears only after native promotion ACK/Core commit.

Artifacts.GetArtifact and standard Google ByteStream.Read retrieve immutable bundle bytes by the returned resource descriptor. Descriptor size/hash must match the manufacturing record; validate streamed size, exact actual-byte SHA-256, strict UTF-8/schema, captured semantic input hash and summary. The bundle and separately addressed program use the streamed limits in the [bundle contract](#provenance-reports-and-immutable-bundles); neither requires a whole-object allocation. There is no ReadChunk command, correlation frame header or kind-4 payload. ByteStream flow control is separate from job observation/cancellation.

Inspection reserves the bundle and program allowances for single-job ledger publication, plus the packed replay allowance when ReportAndMotion is requested. Fresh report/provenance/display descriptor and packed bytes publish together in the terminal SQLite transaction only after native promotion ACK. Pre-ACK cancellation drops staging; late cancellation cannot roll back an ACK-winning commit. Replay is transient/runtime-ledger state, not a Core manifest asset or saved verified authority. ByteStream replay chunks remain at most1MiB; clients bound and validate complete immutable bytes before display/export. The renderer never replans or reparses the machine program.

Canonical gRPC/Google Status carries typed domain error details. Domain errors remain recoverable; transport/contract failures do not fabricate success or retry uncertain mutations. Saved bundle JSON and the printer-authoring JSON are domain/storage formats, deliberately distinct from protobuf RPC messages.

Manufacturing errors are `program_coordinate_conditioning_failure`, `native_kernel_failure`, `native_conditioning_failure`, `native_analysis_indeterminate`, `native_singular_geometry`, `native_shape_error_exceeded`, `invalid_specification`, `unsupported_capability`, `unsupported_geometry`, `no_intent`, `empty_project`, `stale_revision`, `verification_failed`, `resource_limit`, `cancelled`, `io`, `corrupt_artifact`, `read_only`, `busy`. Program-coordinate refusal is distinct from native-geometry conditioning failure; neither permits tolerance slack or an unsupported-geometry catch-all. Messages are bounded to 2,048 UTF-8 bytes. Invalid specifications are not replaced by defaults or fake fallbacks.
