<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
-->

# Engine G-code comparison

`pnpm compare:slicers` builds the matching native CLI/engine and compares actual
exported commands with an installed PrusaSlicer **2.9.4**. This is a bounded
software comparison, not a printer qualification or an assertion that another
slicer's decisions are correct. It exercises native STEP admission, intent,
compilation, mandatory emitted-program replay and exclusive CLI export; no UI is
involved. Each native case checkpoints and opens in a new read-only engine,
freshly replays, and exports byte-identical G-code.

```sh
pnpm test:parity
pnpm compare:slicers --reference /usr/bin/prusa-slicer
```

`CARGO_TARGET_DIR` and `CARGO_BUILD_JOBS` follow the root tooling conventions.
The comparison runner can also consume already built matching binaries:

```sh
node tools/compare-slicers.mjs --cli /path/to/spiling-cli \
  --engine /path/to/spiling-engine --reference /path/to/prusa-slicer
```

Use `--reference-sha256 HEX` to require a previously captured executable identity.
The reference is an external development dependency; this command does not
install or distribute its executable, profiles or implementation. The workflow
has been exercised with the Linux distribution executable. Other platforms and
reference versions require their own evidence; do not silently substitute one.

Every invocation owns a new ignored `.cache/slicer-parity-*` directory. It retains
source bytes, authored and resolved profiles, commands/logs, executable and input
hashes, engine build/implementation identities, raw programs, native verification,
independent audits and an interactive `report.html` with physical-band overlays.
Exit **1** means a missing case, compilation/refusal, parser error or unmet audit
obligation. Exit **2** means audits passed but differences remain unresolved.
`--observe` permits exit zero for unresolved differences only; it never makes a
refused case or failed audit pass. The complete matrix currently exposes unresolved hollow-reference audit
obligations, so a nonzero result is expected until those differences are resolved.
Do not exclude a failing case or turn a promised positive into an expected refusal.

## Native bridge and captured-profile comparison

```sh
pnpm compare:slicers --bridges --reference /usr/bin/prusa-slicer
```

This mode reuses the same bounded process runner and independent command decoder.
[`bridge-corpus.mjs`](../../tools/gcode-parity/bridge-corpus.mjs) owns the original
analytic STEP author; immutable sources, explicit X/Z polygons, dimensions,
hashes and OSL sidecars live in
[`bridges/manifest.json`](../../fixtures/manufacturing/bridges/manifest.json).
Two 2 mm posts separated by 2/6/10 mm support a 1 mm roof at Z3 mm; the one-ended
source deliberately omits the second post. Both importers receive the same
unchanged source bytes and source frame. Production code never recognizes a
coupon or substitutes this author for native geometry queries.

Matched explicit controls precede the two captured synthetic material selections.
Supports and retraction are disabled, with one wall and solid rectilinear fill.
The declared native span limit admits the short/middle two-ended sources and
refuses the long or missing-anchor sources. Nearby overrides on the unchanged
middle source straddle its actual deposited gap; all admitted limit variants
must retain byte-identical G-code. Required refusals must carry the actual anchor
diagnostic and publish no export. An arbitrary native geometry failure is never
an expected bridge refusal.

[`bridges.mjs`](../../tools/gcode-parity/bridges.mjs) independently observes the
analytic solid posts/roof, layer order, complete duplicate strokes, extrusion in
the source void, declared prime, nominal capsule coverage and actual XYZ/E dose.
Its 0.15 mm grid, nominal 0.45 mm width and 0.002 mm observation allowance are
bounded diagnostics, not exact physical coverage. Every layer retains coverage
sample/miss counts and witnesses; actual dose is separate from nominal footprint.
An uninterrupted collinear deposition run can cross G-code command partitions.
Travel, reversal and corners close it. Raw command endpoint counts remain visible
but cannot turn a partition point into a missing physical anchor. Lower-layer
capsules and complete straight runs independently expose gap span and both ends.
Role comments supply none of these facts.

Positive native cases require fresh export verification, expected actual thermal,
bridge feed/cooling commands, complete two-ended observed gaps and byte-identical
fresh read-only reopen/export. Synthetic profile changes must preserve decoded
geometry and change total E by the declared flow ratio. This is process plumbing,
not calibrated PLA/PCTG-CF performance. Reference integer fan percentages retain
the explicit PWM rounding mismatch. A selected native maximum span has no matched
reference admission control; reference emission beyond it is an explainable
policy difference, not evidence that either print succeeds physically.

The mode writes `report.json`, per-case audits/comparisons and the common manifest
with raw/resolved inputs, output/log and observer/executable hashes. It reports
`nativeSoftwarePassed`, `referenceAuditsPassed`, `correctnessPassed` and
`differencesExplained` separately. Failed reference nominal coverage remains a
baseline failure and exit 1 even while native gates pass. Exit 2 requires passed
baseline audits with unresolved comparisons; `--observe` cannot hide an audit
failure. The selected external terminal-roof rule has independent forward/control
explanation through `selectedPolicyExplained`; complete-comparison
`differencesExplained` remains distinct. Other coupon policies and strict audit
flags retain their own open disposition. Cube circular internal-bridge flow
evidence does not automatically explain an exterior coupon roof.

The middle matched coupon also retains one-setting reference probes for 15%
and 25% infill overlap, full-height top skin, half bridge flow and a 90-degree
fill-angle control, plus a native
full-height-skin compile with fresh verification.
[`bridge-probes.mjs`](../../tools/gcode-parity/bridge-probes.mjs) refuses missing
or duplicate reference settings rather than silently failing to change them.
Coverage counts distinguish the overlap and solid-classification contributions
without dismissing remaining baseline or probe misses.

A full-height skin request on this already-solid native coupon preserves the
complete unoriented deposition/process/dose multiset, but changes roof order and
the following layer's nearest seam and path direction. The witness retains the
raw byte difference, the actual changed physical heights, the prior endpoint
and the independently observed nearest admissible corner. It never reports
byte-identical programs. Per-stroke E comparison allows only the two-coordinate
rounding bound of 2.1e-8 mm filament; total dose and fresh replay remain mandatory.
The cube's stronger byte-invariance witness has its own narrower scope.

For the reference exterior roof with `thick_bridges=0`, the half-flow probe
observes a smaller dose per length and tighter fitted pitch together. Long-row
E/length divided by measured pitch must recover the actual 0.2 mm interval;
bridge path length increases while bridge-process volume stays close to the
baseline. Non-roof intervals must remain unchanged. This identifies a local
flow/spacing contribution; it does not predict a halved whole-job volume or
transfer the cube's circular internal-bridge model to this roof. The source
selection in the pinned reference `LayerRegion::bridging_flow` distinguishes
ordinary modified flow from forced thick bridges; retained actual commands are
the deciding evidence for fitted pitch and dose.

These probe bounds are confined diagnostic witnesses, never broad baseline
coverage or total-material tolerances. The runner fault-checks them against
changed native material, an unrelated reference geometry failure, a missing
half-flow control and shifted interval material with conserved total. Every
fault must reject acceptance. Accepted contribution witnesses remain separate
from policy explanations and failed baseline observations.

## Bridge material and coverage witnesses

[`bridge-ledger.mjs`](../../tools/gcode-parity/bridge-ledger.mjs) accounts for every
model stroke in actual physical intervals and common 0.2 mm bands. It clips
centerlines into left-post/span/right-post X regions, independently of source
material presence below the roof. Boundary-parallel, X/Y-aligned and oblique
classes describe commanded geometry, not inferred wall/fill roles. These are
centerline partitions, not bead-volume intersections. Per-case ledgers and
`material-difference.json` retain the complete native/reference distribution;
the matched middle case's `material-accounting.json` also retains every control.

Each cell separates common-height nominal stadium path volume, the correction
for its actual interval height, and remaining commanded E dose. All cells,
layers and bands must reconcile; terminal half-height intervals cannot be
compared by layer index. This algebra localizes material differences without
proving their policy or physical validity. The frozen native profiles use unity
bridge flow: every native stroke must independently match its selected
0.45 mm stadium/flow model within 2e-7 mm³ command quantization, preventing
balanced local dose defects from disappearing in layer or job totals. Fitted
reference flow has its own evidence and is not required to match that model.

`coverage-witnesses.json` retains every baseline missed coordinate and the
nearest actual stroke, endpoint projection, distance and E/length for each
control. An explicitly labelled countermodel inverts dose/height to stadium
width; this is never measured bead width or a widened baseline tolerance.
Every control retains repaired, retained and newly introduced misses. More
overlap can close one fill-corner gap while hatch refitting creates another;
coverage across several different programs cannot qualify a single program.
All baseline and probe audits remain available with complete missed points.

The matched witness requires native full-skin band/region invariance, later-roof
invariance under the reference skin control and non-roof invariance under the
half-flow control. It additionally retains the nonmonotone overlap response.
Fault checks reject shifted spatial contributions despite conserved band totals,
balanced stroke dose defects despite conserved layer/job material, omitted
coverage witnesses and missing controls. Accounting acceptance remains separate
from baseline correctness and complete policy explanation; no broad material
or coverage tolerance closes either gate.

## Forward external-roof policy

[`bridge-policy.mjs`](../../tools/gcode-parity/bridge-policy.mjs) predicts the
frozen reference terminal roof from source dimensions and captured controls,
without measuring a pitch, counting decoded rows or reading role comments first.
The prediction is written before reference emission. For nominal width `w`,
actual interval height `h`, cross-fill extent `C` and flow multiplier `m`:

```text
s = w - h(1 - pi/4)           nominal stadium pitch
b = (w + s)/2                source-to-fill boundary inset
n = floor((C - 2b)/s)        fitted row count
p = (C - 2b)/n               fitted pitch
row center i = b + (i+1/2)p
fill dose = h*p*m
inner turn inset = b + s/2
free-end inset = b + 0.05s
```

The model admits only the frozen zero-overlap, classic single-wall, external
rectilinear 0/90-degree roof and captured profiles. Its fit stays below the
reference's 20% spacing cap. The pinned source distinguishes
[solid spacing adjustment](https://github.com/prusa3d/PrusaSlicer/blob/69c8e569410efe82259e706cd09a05ea82da9359/src/libslic3r/Fill/FillBase.cpp),
[rectilinear contour offsets and linking](https://github.com/prusa3d/PrusaSlicer/blob/69c8e569410efe82259e706cd09a05ea82da9359/src/libslic3r/Fill/FillRectilinear.cpp)
and [external versus internal flow realization](https://github.com/prusa3d/PrusaSlicer/blob/69c8e569410efe82259e706cd09a05ea82da9359/src/libslic3r/Fill/Fill.cpp).
The independent observer is not a second production planner.

An unchanged-source control changes only `fill_angle` from 0 to 90. Complete
resolved INI comparison must confirm that single change, excluding only comments
and blank lines. The forecast swaps cross/along extents and predicts every row,
link, free end and local dose, while the perimeter remains invariant. Two
opposite seam alternatives are explicit. Uninterrupted straight command
partitions normalize to one geometric run; each original command retains its
own dose obligation, so cancellation cannot conceal a local defect.

Position allowances derive from reference XYZ serialization and micrometre-grid
rounding; local E-volume allowances additionally include five-decimal absolute-E
rounding. They do not change the baseline nominal-coverage tolerance. Before the
control emits, the terminal missed-point forecast must predict the baseline
gaps and coverage of those same coordinates under **both** rotated seam
alternatives. The actual control must also retain its complete audit; covered
coordinates from different programs never merge into one passing program.

Per-case `roof-policy-prediction.json` and `roof-policy-observation.json` retain
all expected/observed geometry and E. The middle case additionally retains the
rotation prediction, coverage forecast, complete `roof-policy-witness.json` and
raw fault programs. Complete independent reference audits must preserve the
declared physical intervals and reject every unrelated geometry failure; only
the retained nominal-coverage flags are admitted to this explanation gate.
No-op controls, unrelated resolved-setting changes, fixed
nominal dose on fitted paths, balanced E defects and removed links with conserved
terminal material must reject. `selectedPolicyExplained` requires this deciding
control, every frozen reference roof observation, complete accounting and all
native positive/refusal/recovery gates. It qualifies this bounded construction
explanation; strict reference coverage, complete-comparison explanation and
physical printability remain separate.

## Comparison contract

[`corpus.mjs`](../../tools/gcode-parity/corpus.mjs) derives a coherent 10 mm cube
from the original analytic `box-mm.step`, changing both point coordinates and
line-vector lengths. It refuses a changed/nonaxial source rather than attempting
general STEP transformation. The same derivative goes through each slicer's own
importer. The matrix varies one/two walls and 0/25/100% rectilinear infill, with
0.2 mm layers, 0.45 mm nominal widths, a 0.4 mm nozzle, 1.75 mm filament and
0.8 mm top/bottom skins. Retraction, supports, gap fill, skirts and reference
infill anchors are disabled. Reference small-perimeter speed and seam gaps are
explicit, preventing those defaults from silently changing the comparison.

These are matched **nominal inputs**, not identical intent semantics. Reference
classic wall spacing, hatch fitting, vertical shell enforcement and bridge
construction differ from native material/actual-anchor qualification. The full
resolved reference configuration remains evidence. Ordinary fan commands are
128 versus reference 50%=127.5; the observer retains both commanded values without
pretending to simulate firmware quantization. Complete-job startup differs;
only the exact declared Spiling prime is excluded from model accounting.

## Independent observations and obligations

[`decode.mjs`](../../tools/gcode-parity/decode.mjs) imports no production parser,
plan or generated domain model. It tracks explicit units, XYZ/E modes, E resets,
retraction debt, feed/flow overrides and cooling from actual linear commands.
E arithmetic is exact through eight decimal places. Recovery is not deposition;
`G92 E` does not cancel debt. Unknown commands/fields, inches, coordinate-frame
resets, arcs, tool changes, moving retract/recovery and nonplanar deposition
refuse. Files, lines, logs, child duration and analytic work have explicit bounds;
these are application limits, not a measured process-tree RSS guarantee.

[`analyze.mjs`](../../tools/gcode-parity/analyze.mjs) checks the analytic source
box, actual elevation intervals, three wall probes per side, allowed speed/fan
pairs, local nominal width/flow limits, repeated complete strokes and required
straight-wall/solid-skin footprint samples. Hollow intermediate cores must remain
empty under this analytic contract. All commanded extrusion volumes derive from
E and filament area. Wall/core volume zones are **centerline partitions**, not
physical bead-volume intersections. Volume is conservatively resampled by actual
interval overlap into common 0.2 mm physical bands; comparing layer indices or
only total filament would hide local differences. Role comments are diagnostic
hints and cannot supply geometry, extrusion or bridge-process authority.

The 0.1 mm footprint raster assumes a stadium-equivalent bead inferred from
commanded E/length/height. It excludes square corners, uses 0.002 mm command
position tolerance and refuses widths outside the declared 0.30–0.75 mm range.
It cannot prove physical coverage, fusion, collision freedom or absence of every
possible duplicate/underfilled path. Reference circular/oblique bridge models,
roof anchoring and extra shell material can violate these particular analytic
obligations without establishing a reference defect. Keep the violation and
investigate its assumption; never silently broaden coverage to obtain green.
Fault tests demonstrate missing walls/roofs, reduced flow, outside extrusion,
duplicate strokes, hidden speed defaults and corrupt modal/debt handling.

## Explanations and unresolved differences

The report separates matched observations, evidence-backed explanations and
unresolved differences. Exact-like volume equality is diagnostic, not an
acceptance target; even equal totals remain unresolved when physical bands or
wall/core distributions differ.

Wall placement has a narrow explanation only when all probes fit each declared
pitch: native nominal width versus reference classic rounded-bead spacing
`width - height * (1 - pi/4)`. The reference implementation is
[`Flow.cpp` at version_2.9.4](https://github.com/prusa3d/PrusaSlicer/blob/version_2.9.4/src/libslic3r/Flow.cpp).
This explains centers, not physical fusion or the whole volume difference.

A terminal schedule difference is explained only with an actual same-case
controlled probe: change native shape error from 0.100 to 0.101 mm, freshly
compile/replay/audit, retain both native geometry reports, and show that the
achieved diagnostic bound exceeds 0.100 mm while commanded heights match the
reference. The baseline keeps its stricter bound. Native certification/refinement
lives in [`planner/native.rs`](../../crates/manufacturing/src/planner/native.rs).
No extra layer is automatically excused from its count or a planner comment.

The independent `tools/gcode-parity/anchors.mjs` observer intersects each emitted
centerline with nominal declared-width capsules from the immediately preceding
actual deposition elevation. Endpoint-disk overlap is separate from interior
coverage. It retains bounded line-number witnesses, unsupported spans, row
counts/lengths and E-derived volumes. Its command-precision assumptions and
0.002 mm substantial-gap threshold are explicit; it is not the production
anchor certificate or a physical bead model. No role comments supply geometry.

For the 25% cube's common sparse bands, a narrow policy explanation requires
actual two-ended gaps in both programs, native bridge-process signatures and
reference ordinary-process signatures, with native spans below the configured
limit. The native whole-row Bridge policy lives in
[`support/strokes.rs`](../../crates/manufacturing/src/planner/support/strokes.rs).
The report separately exposes fitted row count/length and extrusion, so process
classification is not mistaken for extra material. A speed/fan signature is a
command observation, not proof of a semantic role. Roof construction, full
bridge-process totals and material-distribution differences remain unresolved
until their own deciding evidence exists; this explanation does not approve the
whole comparison or establish either policy's physical superiority.

Global zero infill retains source-bound interior/intentional-void accounting and
emits zero centered/alternate/common-phase hatch rows. Required walls/skins,
actual roof-anchor admission and independent replay still apply. Authored density
overrides retain their positive-only contract. The hollow reference exposes
roof-adjacent material and nominal bridge-footprint audit differences; retain
these as unresolved model/intent questions rather than declaring reference
correctness or weakening the analytic obligation. Broader geometry, retraction, supports and
multi-tool comparisons belong in later explicitly scoped corpus cases.

## Controlled roof probes and construction ledger

The runner retains three separate reference-only diagnostics for every hollow
and 25% case: request three rather than four top layers, halve bridge flow, and
disable vertical-shell assurance. Each changes one explicit setting; baseline
inputs and audit obligations remain fixed. Raw G-code, resolved configuration,
audit and file hashes are retained for each diagnostic. A deliberately thinner
roof can fail the original roof obligation; it never replaces the baseline.

[`probes.mjs`](../../tools/gcode-parity/probes.mjs) requires the extra material at
Z9.2 and the bridge at Z9.4 to move together to Z9.4/Z9.6 when roof thickness
changes, with the earlier ordinary layer restored. Disabling vertical-shell
assurance must preserve the preparation and first bridge. This identifies the
reference's bridge preparation rather than blaming that shell option or treating
all interior extrusion as sparse infill. The reference's
[`PrintObject::bridge_over_infill`](https://github.com/prusa3d/PrusaSlicer/blob/69c8e569410efe82259e706cd09a05ea82da9359/src/libslic3r/PrintObject.cpp)
adds ensuring material near the next bridge's perimeter. This explains the
construction policy; the stricter hollow-core diagnostic remains visible.

[`construction.mjs`](../../tools/gcode-parity/construction.mjs) independently
buckets actual model strokes into geometric wall centerlines, long fill strokes
(over 6 mm) and all remaining links/short fill. Every layer reconciles all
E-derived volume; the buckets are diagnostic geometry, not slicer semantic roles.
Wall accounting tolerates subdivision and reversal. Parallel long strokes retain
row count, projected pitch, length, volume and bounded line/XYZ/E witnesses. The
observer is bounded to 100 elevations and 100,000 decoded strokes. Geometry
buckets can change with subdivision of fill; total extrusion is conserved.

A circular bridge hypothesis uses the observed projected row pitch minus the
reference's explicit 0.05 mm bridge gap. All admitted long bridge rows must fit
that area within 0.001 mm², with pitch spread at most 0.004 mm. The observer also
retains the layer-height stadium countermodel and both volume residuals. The
half-flow diagnostic must change actual extrusion and fitted diameter while
leaving earlier layers invariant. No role comments identify bridges. Native
bridge rows continue to fit the layer-height stadium; fan/feed signatures alone
cannot establish a circular model or physical bead shape.

The reference source at commit `69c8e569410efe82259e706cd09a05ea82da9359`
explains this hypothesis: [`Fill.cpp`](https://github.com/prusa3d/PrusaSlicer/blob/69c8e569410efe82259e706cd09a05ea82da9359/src/libslic3r/Fill/Fill.cpp)
forces thick flow for internal bridges and adjusts non-internal-fill flow after
row fitting; [`LayerRegion.cpp`](https://github.com/prusa3d/PrusaSlicer/blob/69c8e569410efe82259e706cd09a05ea82da9359/src/libslic3r/LayerRegion.cpp),
[`Flow.cpp`](https://github.com/prusa3d/PrusaSlicer/blob/69c8e569410efe82259e706cd09a05ea82da9359/src/libslic3r/Flow.cpp)
and [`Flow.hpp`](https://github.com/prusa3d/PrusaSlicer/blob/69c8e569410efe82259e706cd09a05ea82da9359/src/libslic3r/Flow.hpp)
define circular area, diameter and bridge gap. The installed executable identity
and actual-byte observations are retained separately; a source tag is not build
attestation. Source is inspected as reference research, not imported into the
production planner.

For the forty common interior intervals, the report separately checks native
centered ceiling row counts and reference cropped/fitted floor counts against
the declared cube/wall dimensions. Both retain nominal extrusion per length for
internal infill; dense reference pitch is fitted, while sparse pitch stays
nominal. This explains that interior volume contribution through measured row
length/count, rather than mistaking bridge fan/feed changes for extra extrusion.
Whole-job material distribution, variable-width internal solids and physical
roof footprint remain separate questions. A reconciled ledger alone never turns
an unexplained policy or failed baseline audit green.

The 100% cases additionally extend only top skin through the full cube height
(50 reference layers / 10 mm native thickness). This probe preserves the already
solid material request. The native program must remain byte-identical and pass
fresh emitted replay. The reference's 42 ordinary interior intervals must change
to its independently observed natural internal-skin template, with one additional
long row, short connecting extrusion and bounded variable widths. The outermost
layers and existing skins must retain their volume. The repeated-template
prediction must match the observed whole-program reference delta within 0.01 mm³;
that bound is confined to this causal diagnostic, never a baseline volume or
coverage tolerance. Failed invariance, geometry or template checks cannot explain
the difference.

The reference [`Fill.cpp`](https://github.com/prusa3d/PrusaSlicer/blob/69c8e569410efe82259e706cd09a05ea82da9359/src/libslic3r/Fill/Fill.cpp)
selects `ipEnsuring` for internal solids separately from perimeter generation;
[`FillEnsuring.cpp`](https://github.com/prusa3d/PrusaSlicer/blob/69c8e569410efe82259e706cd09a05ea82da9359/src/libslic3r/Fill/FillEnsuring.cpp)
uses straight fill and variable-width residual construction. Thus “classic
perimeters” does not imply uniform-width internal skins. The diagnostic retains
actual nominal width ranges inferred from E/length/height, row/link accounting,
and the controlled programs. It identifies a specific material-volume contribution
without adopting that filler or claiming the full remaining distribution resolved.

## Frozen cube-family material explanation

[`material-ledger.mjs`](../../tools/gcode-parity/material-ledger.mjs) closes a
**specific policy explanation**, never a claim of equal material or physical
quality. It requires the complete 0/25/100% family for each wall count, successful
native audits, and all independently established schedule/wall/hatch/bridge/roof
and internal-solid witnesses. Its cross-case checks require unchanged native
walls/skins, unchanged reference walls and ordinary skins, empty hollow interiors,
and the observed reference internal-solid template. Reference first/top external
skin row volume must fit measured pitch times actual height/length within
0.005 mm³; this diagnostic accounts for XYZ3/E5 precision and fitted flow, not a
whole-job tolerance. Native E must retain its nominal stadium model.

Every layer's geometric buckets must reconcile with actual E-derived volume.
Every common physical band's volume/core/wall-zone values are reprojected from
actual intervals; equal totals cannot hide shifted material. The ledger exposes
separate wall, external/internal bottom, ordinary-interior, pre-roof, first-roof,
internal-roof and terminal-band deltas. The policy witnesses establish causes;
the ledger alone cannot establish an explanation.

The real runner fault-checks each admitted family by removing a density control,
removing the internal-solid witness, shifting material between bands while
conserving total extrusion, and injecting an unrelated reference audit failure.
Each must reject the explanation. Reference hollow flags remain binding at their
original Z9.2 preparation and Z9.4 footprint obligations; any other reference
violation prevents this bounded family explanation. The frozen comparison can
therefore report `policyDifferencesExplained:true` while its baseline audit still
fails and its exit remains 1. `correctnessPassed` and the overall
`differencesExplained` success condition keep their original stricter meaning.
A policy explanation does not certify a reference roof, adopt another algorithm,
or establish physical printability.
