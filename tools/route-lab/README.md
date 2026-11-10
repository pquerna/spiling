# Routing algorithm research

The dependency-free [Rust crate](../../crates/deposition-routing/README.md) creates
all route decisions and synthetic deposition fixtures. Python only snapshots,
builds, runs and records trials; it generates no roads and edits no source.
The independent Rust judge, corpus, planner constraints and compiler stay frozen.
Only `crates/deposition-routing/src/search.rs` may change in one cohort.

```sh
python3 tools/route-lab/loop.py --root .cache/route-search freeze
python3 tools/route-lab/loop.py --root .cache/route-search trial nearest \
  --hypothesis 'Shortest admissible next travel'
# Edit only src/search.rs, then format that file.
python3 tools/route-lab/loop.py --root .cache/route-search trial lookahead \
  --parent nearest --hypothesis 'Avoid ending at a leaf before its connected loop'
```

Use an external memory/CPU/tasks bound around each trial (for example Linux
`systemd-run --user --scope -p MemoryMax=2G -p TasksMax=128 -p CPUQuota=200%`).
These are small research-fixture bounds, not production workstation limits.
The command deadline applies to each build/test/run. Cargo uses the frozen pinned
toolchain, offline dependencies and one build/test worker by default. Honor
`CARGO_TARGET_DIR`; otherwise use the campaign's own CPU-only target, without
building or contending for the live native kernel target.

The 14-case matrix contains 13 positives and an inaccessible cavity negative:
connected roads, a direction-locked junction/cycle, islands, nested variable-width
walls, mixed physical height bands, a tall obstacle, multi-hop corridor with a
ceiling, inside-hole access, incompatible joins, and four held-out size/ordering/
mirror cases. Mutations exercise omitted/duplicate work, locked direction,
unauthorized continuation, clear-endpoint collision and prerequisite order.
Resource/cancel/cycle negatives remain distinct from obstructed geometry.

A trial passes formatting, the frozen adversarial tests and the entire matrix
three times. Every positive must complete and the negative must refuse. The
judge reconstructs original deposition, dimensions/permissions, dependencies and
continuous head/box intersection without calling search or planner helpers.
Unknown/failed trials cannot score. Receipts retain exact source/compiler/binary
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
This optional plotting command uses matplotlib (already pinned by the sibling
infill-lab environment); it is not a runtime dependency of the Rust crate or
stdlib trial runner. Do not pool different judges/corpora into one grade.
