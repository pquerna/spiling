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
