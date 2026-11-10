# Deposition routing

A dependency-free Rust route-planning library, separately compiled for algorithm
research and a future manufacturing adapter. Production manufacturing does not
call it yet. No kernel, project writer, G-code, storage or renderer belongs here.

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

The frozen input-order control uses bounded depth-first recovery and local-height
portals. It is a reproducible reference, not an optimal or complete solver. Travel
primitives use a checked visibility graph over caller and candidate portals plus a
high plane proposal. Continuous FP64 intersection uses Minkowski-expanded declared
head/obstacle boxes and earlier bead boxes. Boundary-only contact is permitted.
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
clearance independently of planner helpers. They are declared Rust route fixtures,
not accepted BREP imports. [The research harness](../../tools/route-lab/README.md)
freezes these files and permits edits only to `src/search.rs` and Rust modules
under `src/search/`.
