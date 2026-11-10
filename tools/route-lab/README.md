# Routing algorithm research

The [Rust crate](../../crates/deposition-routing/README.md) creates
all route decisions and synthetic deposition fixtures. The trial runner only
snapshots, builds, runs and records; it proposes no route and edits no source.
`capture.py` is a separate cohort-preparation tool: it converts slicer G-code or
retained native plan/program pairs into frozen stroke text. It never runs during
a trial.
The independent Rust judge, corpus, planner constraints and compiler stay frozen.
Candidates own the scheduling algorithm through `solve(Context)`. Both
`crates/deposition-routing/src/search.rs` and Rust helper modules under
`src/search/` may change, including additions/removals. Everything else stays
frozen. This admits trail decomposition, dependency-aware construction,
whole-group local repair and alternative tool sequencing. The production API
borrows task terminals, physical intervals, entry permissions and prerequisites;
it returns a checked permutation, tool groups and modeled transition clocks.
It does not propose new extrusion, machine commands or collision-clear travel.
New scarf/helical construction and cross-layer production scheduling need their
own construction and independent actual-program admission.
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
`systemd-run --user --scope -p MemoryMax=16G -p TasksMax=512 timeout 600`).
These are small research-fixture bounds, not production workstation limits.
The command deadline applies to each build/test/run. Cargo uses the frozen pinned
toolchain, offline dependencies and one build/test worker by default. The cohort
uses the repository's dev profile and its captured package settings for macro
algorithm comparisons; no release build or release-performance claim is needed.
The profile and environment overrides are recorded and frozen with the cohort.

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

The scheduling matrix uses the production API. It retains exact roads, split
islands, locked paths and nested loops; two seeded junction lattices; fifteen
captured PrusaSlicer layers/stacks; ten historical native Cathedral web/roof
captures; and declared rigid-dual/three-tool-changer panels with held-out
siblings. Fine/coarse two-tool witnesses retain unequal physical intervals and
roof prerequisites. A contradictory prerequisite cycle must refuse. Former
head-box/corridor fixtures were specific to the deleted research travel model;
there is no collision qualification in this cohort.

The independent judge reconstructs inventory, permitted entries, prerequisites,
barriers, complete mandatory service groups, selection counts and the declared
travel/service/swap/prewarm clock without calling library pricing/checking code.
It rejects forged timing annotations as well as missing work. A prewarm lead
that cannot precede an initial selection produces modeled waiting; this is not
a measured heating law. Recorded `machine_time_s` excludes station approach,
thermal dynamics, acceleration and finalization; it is a scheduling estimate,
not physical print time. Current native product comparisons must use freshly
emitted programs and their independent accounting.

Each family has a held-out sibling with different dimensions or seed. Pin stacks retain their captured prerequisites and physical top barriers; this cohort does not reorder production across fine layers or infer support from overlap.

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
group/transition timing without calling search or planner helpers.
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

Grades retain geometric-mean candidate/baseline travel, interrupted starts,
modeled schedule seconds and motion lower bounds, plus every case's ratios and
worst regression. Schedule seconds include the declared travel/retraction,
deposition, service, selection and unmet prewarm lead costs. They exclude station
approach and calibrated motion/thermal dynamics; this is not a firmware or
printer model.

`gap_to_bound` is absolute headroom, independent of any control: starts over a
trail-cover bound and travel over a spanning-tree bound. Both are relaxations
(they ignore widths, locked directions and prerequisites), so 1.0
proves optimality and a larger value need not be reachable. A judged route below
either bound is a harness fault and stops the trial. `planning_us_min3` sums each
case's least planning time over the three repetitions; the comparison reports its
ratio to the parent. Charged work is voluntary for custom algorithm code and is
not a cost measure. A parent
comparison marks aggregate dominance/tradeoff/equality; that is not automatic
promotion or a claim of per-case dominance. Selection counts, work counts and
planning time stay visible. Retain useful Pareto alternatives; no 5% gain gate.
The timing model excludes pressure and acceleration lookahead; swap and prewarm values remain declared estimates.

These are declared deposition scheduling fixtures with checked FP64 terminals
and physical intervals. They do not prove native source occupancy, anchor construction,
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
