# Deposition routing

A Rust route-planning library for algorithm research and a future manufacturing
adapter. Production manufacturing does not call it yet. It depends on contracts
and `spiling-compute`, the shared CubeCL accelerator; no kernel, project writer,
G-code, storage or renderer belongs here. Algorithms may use compute batches
(for example exact `box_overlaps` masks) to screen candidates. A batch that
declines or fails falls back to the CPU path, and every returned step is still
checked by frozen replay and the independent judge. The matrix configures the
backend from `SPILING_COMPUTE_BACKEND` (`cpu` by default) and reports its device.

`plan` consumes immutable identified planar strokes, per-edge widths, occupied Z
intervals, process keys, direction/seam permissions and earlier-work dependencies.
It returns original stroke references and checked travel waypoints. Required
geometry is unchanged; it never inserts extrusion across a gap. `continuous`
means exact compatible endpoints without travel, not calibrated pressure or
firmware lookahead. Physical Z intervals, not layer numbers, define compatibility.

The editable algorithm owns a complete route. `Session` exposes immutable scene
geometry, checked prefixes, ready orientation/seam choices, arbitrary proposed
travel portals, segment-clearance queries and custom step admission. A candidate
can build a graph, backtrack, use a beam/frontier, plan pockets or perform bounded
local repair; it need not use the prepared choices. `src/search/` may contain new,
changed or removed Rust helper modules. A fresh frozen replay checks every returned
step before the independently implemented matrix judge checks it again.

The retained search constructs the lowest unfinished physical band first, with
bounded backtracking when that construction stalls. It then improves visit order
with band-local relocation, reversal and iterated local search. A second stage
proposes cross-band moves; only a complete checked rebuild with less travel and
no additional starts replaces the incumbent. Optional search exhaustion preserves
the admitted incumbent; cancellation still aborts. The objective is travel, with
starts as a guard; nominal time remains a separate matrix metric.

The frozen input-order control uses bounded depth-first recovery and local-height
portals. It is a reproducible reference, not an optimal or complete solver. Travel
primitives use a checked visibility graph over caller and candidate portals plus a
high plane proposal. Continuous FP64 intersection uses Minkowski-expanded declared
head/obstacle boxes and earlier bead boxes. Boundary-only contact is permitted.
Two exact shortcuts skip material that cannot matter: a segment at or above the
top of all earlier material, and disjoint segment/box bounds.
These are conservative nominal boxes, not native material authority, a certified
predicate, calibrated head geometry, physical collision freedom or machine
readiness. Supplied dependencies are not inferred deposition-anchor proofs.
`NoRoute` means the bounded search found no complete route; it is not global
manufacturing impossibility. Resource/cancel/input errors remain distinct.

Caller-sized `Limits` bound charged work, input edges, travel portals and requested
live search prefixes. Search and final replay each have a `work` allowance; reported
work includes both, at most twice that allowance. This reserve lets optional search
return an incumbent after exhausting its own allowance. Cancellation still aborts.
Candidate code is trusted Rust with process permissions, not a sandbox: external
memory/tasks/time limits remain mandatory. Prefix clones copy route/occupancy
metadata but borrow source geometry. Search charges structural work; work units
are not elapsed CPU instructions or a hard allocation bound. This research
implementation scans candidates/obstacles directly. Scarf ramps, new extrusion
connectors, thermal/tool-change realization, helical paths and native integration
need separate construction/verification contracts.

Run `cargo test -p spiling-deposition-routing --all-targets` or
`cargo run -p spiling-deposition-routing --example route_matrix --release`.
The matrix and independent judge live under `examples/matrix/`; they reconstruct
inventory, permissions, dependencies, travel binding and complete segment/head
clearance independently of planner helpers. Cases are declared Rust route fixtures
plus `captured/` layers converted from PrusaSlicer G-code of project-authored
shapes; neither is an accepted BREP import or Spiling deposition. The judge also
reports two sound lower bounds (trail cover for starts, spanning tree for travel)
and a declared nominal machine time. `route_matrix --sealed SEED` regenerates
seeded families and applies an isometry to every case. [The research harness](../../tools/route-lab/README.md)
freezes these files and permits edits only to `src/search.rs` and Rust modules
under `src/search/`.
