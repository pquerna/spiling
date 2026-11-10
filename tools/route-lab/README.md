# Routing algorithm research

The dependency-free [Rust crate](../../crates/deposition-routing/README.md) creates
all route decisions and synthetic deposition fixtures. Python only snapshots,
builds, runs and records trials; it generates no roads and edits no source.
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
toolchain, offline dependencies and one build/test worker by default. Honor
`CARGO_TARGET_DIR` as the parent of isolated per-snapshot build directories;
otherwise use the campaign's own CPU-only target. Never reuse a standalone
trial's target for another snapshot: preserved source mtimes can fool Cargo
freshness checks even with different manifest paths. This isolation avoids
building or contending for the live native kernel target.

The 18-case matrix contains 17 positives and an inaccessible cavity negative:
connected roads, a direction-locked junction/cycle, islands, nested variable-width
walls, mixed physical height bands, a tall obstacle, multi-hop corridor with a
ceiling, inside-hole access, incompatible joins, and six held-out size/ordering/
mirror/translation cases. Access traps require reconsidering a locally cheap
deposition order; low ceilings require a local lift rather than a global high
plane. The frozen input-order baseline now has bounded DFS recovery and local
height portals, so it remains a complete comparison control on this panel. Mutations exercise omitted/duplicate work, locked direction,
unauthorized continuation, clear-endpoint collision and prerequisite order.
Resource/cancel/cycle negatives remain distinct from obstructed geometry.

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
The source snapshot compiles standalone only to isolate this existing workspace
crate from unrelated native work; it is not another application runtime.

Grades retain geometric-mean candidate/baseline travel, uninterrupted starts and
motion lower bounds, plus every case's ratios and worst regression. A parent
comparison marks aggregate dominance/tradeoff/equality; that is not automatic
promotion or a claim of per-case dominance. Longest chains, work counts and
planning time stay visible. Retain useful Pareto alternatives; no 5% gain gate.
Timing excludes pressure, acceleration lookahead, tool switching and heating.

These are declared planar route fixtures and FP64 envelope checks with permitted
boundary contact. They do not prove native source occupancy, anchor construction,
calibrated clearance or physical printability. Native-deposition capture,
independent emitted-program replay and saved-return/export are mandatory before
production adoption. Scarf/helical/new-connector construction is not enabled.

Generate a standalone PNG/SVG/HTML report for one frozen cohort with
`python tools/route-lab/report.py --root COHORT --output NEW_REPORT_DIRECTORY`.
Optional `--control NAME --selected NAME` selects the per-case comparison. Reports
show invalid trials as completion censuses and plot quality only for valid trials.
This optional plotting command uses matplotlib (already pinned by the sibling
infill-lab environment); it is not a runtime dependency of the Rust crate or
stdlib trial runner. Do not pool different judges/corpora into one grade.
