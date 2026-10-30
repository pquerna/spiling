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

Sparse bridging still needs a quantitative distribution explanation. Actual
21 mm/s plus fan255 identifies the configured process independently of hints.
[`support/strokes.rs`](../../crates/manufacturing/src/planner/support/strokes.rs)
can classify a whole path as Bridge when genuine bounded gaps have two direct
previous-stroke anchors. That policy explains why sparse rows may receive bridge
controls, but does not by itself justify their extent or speed/cooling cost.
Keep this and material-distribution differences unresolved until the deciding
actual-command/anchor evidence is retained.

Global zero infill retains source-bound interior/intentional-void accounting and
emits zero centered/alternate/common-phase hatch rows. Required walls/skins,
actual roof-anchor admission and independent replay still apply. Authored density
overrides retain their positive-only contract. The hollow reference exposes
roof-adjacent material and nominal bridge-footprint audit differences; retain
these as unresolved model/intent questions rather than declaring reference
correctness or weakening the analytic obligation. Broader geometry, retraction, supports and
multi-tool comparisons belong in later explicitly scoped corpus cases.
