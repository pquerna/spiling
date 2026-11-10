# Routing algorithm research

The [Rust crate](../../crates/deposition-routing/README.md) creates
all route decisions and synthetic deposition fixtures. The trial runner only
snapshots, builds, runs and records; it proposes no route and edits no source.
`capture.py` is a separate cohort-preparation tool: it converts slicer G-code or
retained native plan/program pairs into frozen stroke text. It never runs during
a trial.
The independent Rust judge, corpus, planner constraints and compiler stay frozen.
Candidates own the whole route through `solve(Session)`. Both
`crates/deposition-routing/src/search.rs` and Rust helper modules under
`src/search/` may change, including additions/removals. Everything else stays
frozen. This admits graph construction, CNC pocket/stay-down strategies, robot
configuration-space portals, Snake-style escape preservation, trail decomposition,
backtracking, beam search and local repair. Required strokes cannot change; new
extrusion/scarf/helical construction still needs a separate frozen problem/judge.
This is trusted algorithm code, not a security sandbox.

```sh
python3 tools/route-lab/loop.py --root .cache/route-search freeze
python3 tools/route-lab/loop.py --root .cache/route-search trial nearest \
  --hypothesis 'Shortest admissible next travel'
# Edit src/search.rs and/or Rust modules under src/search/, then format.
python3 tools/route-lab/loop.py --root .cache/route-search trial lookahead \
  --parent nearest --hypothesis 'Avoid ending at a leaf before its connected loop'
```

Use an external memory/CPU/tasks bound around each trial (for example Linux
`systemd-run --user --scope -p MemoryMax=2G -p TasksMax=128 -p CPUQuota=200%`).
These are small research-fixture bounds, not production workstation limits.
The command deadline applies to each build/test/run. Cargo uses the frozen pinned
toolchain, offline dependencies and one build/test worker by default.

`freeze` assembles a small workspace in the cohort: the routing crate, its local
dependencies `crates/compute` and `crates/contracts`, the repository lockfile
and toolchain, and the repository manifest restricted to those members. All of
it is frozen except the algorithm files. It primes Cargo's normal cache at one
fixed build path. CubeCL can take several minutes and multiple GB on a cold
build; allow sufficient memory for `freeze`.

Every trial uses that same workspace and persistent target. `CARGO_TARGET_DIR`
is honored when freezing; subsequent trials must retain that target. Only changed
source bytes are written, with ordinary current filesystem timestamps; unchanged
files and all compiler artifacts are left alone. Cargo decides which units to
reuse or rebuild. No artifacts are deleted or copied per trial. Earlier snapshot
copying preserved stale source timestamps; normal writes repair that harness
mistake without overriding Cargo freshness. A real compile/run regression covers
changed, unchanged and restored source against the same cache. Receipts record
Cargo's reused/compiled unit counts alongside source and executable hashes. A
per-cohort lock serializes trials, and this target stays separate from the live
native kernel target.

`SPILING_COMPUTE_BACKEND` (`cpu` default, `vulkan` or `auto`) is fixed when the
cohort is frozen; every trial and sealed run must use the same value, and
receipts record the device the matrix reports. Do not pool cohorts frozen
with different backends. Local device evidence is not GPU or platform qualification.

The 45-case matrix has four kinds of positive case and one negative:

- **Mechanism fixtures** (17): connected roads, a direction-locked junction/cycle,
  islands, nested variable-width walls, mixed physical height bands, a tall
  obstacle, a multi-hop corridor under a ceiling, inside-hole access, incompatible
  joins, access traps and low-ceiling local lifts, with size/mirror/translation
  holdouts. They exist to catch missing capability, and have little quality headroom.
- **Seeded junction lattices** (2): every grid edge is its own stroke, so the
  fewest uninterrupted starts is a trail-cover problem with a known bound.
- **Captured slicer layers** (15, `slicer-*`): real PrusaSlicer layers of
  project-authored shapes: perforated grilles (sparse and top layers), bolted
  brackets, pin arrays, embossed code plates, fins, a plain block control and two
  four-layer pin stacks. Walls keep the slicer's direction and seam; fill may
  reverse. Variable-width walls keep every per-edge width. Stroke order is the
  slicer's print order, so the frozen input-order control approximates that
  slicer's sequencing under this lab's travel model. It does not replay the
  slicer's own travel moves or retraction choices.
- **Captured Cathedral layers** (10, `cathedral-*`): native web and roof
  deposition on curved, concave and holed footprints, plus two four-band stacks
  of 676 and 597 strokes. Captures retain physical intervals and per-edge widths,
  check path-start order against emitted extrusion runs, and keep walls and
  bridges directed. Headers bind the historical producer; these are routing
  inputs, not current-engine or whole-part replay qualification.
- The sealed cavity must still refuse.

Each family has a held-out sibling with different dimensions or seed. Pin stacks
declare a dependency on overlapping material one band below and use a declared
nozzle-under-block head, so finishing one pin several bands high is admissible
only where the judge finds clearance. That is a declared model, not a printer.

Regenerate captures only to begin a new cohort:
`python3 tools/route-lab/capture.py` (needs `prusa-slicer`; shapes, slicer
version and options are recorded in each file). For retained native exports, use
`python3 tools/route-lab/capture.py native --native-root CAPTURE_DIRECTORY`;
that directory must contain `producer.json` and the configured case exports.
See the [capture boundary](../../crates/deposition-routing/examples/matrix/captured/README.md).
Add a shape when it can expose a named weakness; pick the layer by measured
headroom, not appearance.

A trial passes formatting, the frozen adversarial tests and the entire matrix
three times. Every positive must complete and the negative must refuse. The
judge reconstructs original deposition, dimensions/permissions, dependencies and
continuous head/box intersection without calling search or planner helpers.
The Rust matrix emits a complete census even when a candidate fails a case.
Complete failed trials are `invalid`, with raw rows retained and no quality grade;
build failures, timeouts or missing rows are `unknown`. Neither can score. A
complete invalid parent can establish a capability change, never a cost ratio over
a successful subset. The frozen case inventory catches omissions even on a first
trial. Frozen adversarial tests validate the judge/reference and final replay;
experimental quality is evaluated by the matrix, not by assuming candidate success
inside those tests. Receipts retain exact source/compiler/binary
identity, commands, full per-case results and discovery/holdout grades. Each
candidate must preserve the matched parent's entire frozen baseline inventory.
The cohort workspace exists only to isolate this research build from unrelated
native work; it is not another application runtime.

Grades retain geometric-mean candidate/baseline travel, uninterrupted starts,
nominal machine time and motion lower bounds, plus every case's ratios and worst
regression. Machine time is stop-to-stop travel under a declared acceleration plus
a declared allowance per travel move; it shows when shorter travel does not buy
time, and is not a firmware or printer model.

`gap_to_bound` is absolute headroom, independent of any control: starts over a
trail-cover bound and travel over a spanning-tree bound. Both are relaxations
(they ignore widths, locked directions, prerequisites and obstacles), so 1.0
proves optimality and a larger value need not be reachable. A judged route below
either bound is a harness fault and stops the trial. `planning_us_min3` sums each
case's least planning time over the three repetitions; the comparison reports its
ratio to the parent. Charged work is voluntary for custom algorithm code and is
not a cost measure. A parent
comparison marks aggregate dominance/tradeoff/equality; that is not automatic
promotion or a claim of per-case dominance. Longest chains, work counts and
planning time stay visible. Retain useful Pareto alternatives; no 5% gain gate.
Timing excludes pressure, acceleration lookahead, tool switching and heating.

These are declared planar route fixtures and FP64 envelope checks with permitted
boundary contact. They do not prove native source occupancy, anchor construction,
calibrated clearance or physical printability. Native-deposition capture,
independent emitted-program replay and saved-return/export are mandatory before
production adoption. Scarf/helical/new-connector construction is not enabled.

Before retaining a change, run both parent and candidate on a panel neither has
seen: `python3 tools/route-lab/loop.py --root COHORT sealed PARENT CANDIDATE`.
It draws a fresh seed, regenerates the seeded families and applies an axis swap,
mirror and translation to every case, then judges the retained binaries. Quality
on an isometry should not change; `isometry_travel_drift` lists cases where it
did, which can indicate coordinate-sensitive tie breaking or input-layout dependence.
Mirroring rounded captured coordinates can change floating-point ties, so investigate
the listed rows before interpreting a drift as overfitting. Pass `--seed N`
to reproduce a recorded panel. Do not tune against a sealed seed.

Generate a standalone PNG/SVG/HTML report for one frozen cohort with
`python tools/route-lab/report.py --root COHORT --output NEW_REPORT_DIRECTORY`.
Optional `--control NAME --selected NAME` selects the per-case comparison. Reports
show invalid trials as completion censuses and plot quality only for valid trials.
This optional plotting command uses matplotlib (already pinned by the sibling
infill-lab environment); it is not a runtime dependency of the Rust crate or
stdlib trial runner. Do not pool different judges/corpora into one grade.
