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

Travel uses a checked visibility graph over caller-supplied waypoints plus a high
plane proposal. Clearance uses continuous FP64 segment intersection against the
Minkowski-expanded declared head/obstacle boxes and earlier bead boxes. Boxes are
conservative approximations; boundary-only contact is permitted by this model.
This is not native material authority, a certified numerical predicate, calibrated
head geometry, physical collision freedom or machine readiness. Dependency edges
are supplied requirements, not an independently inferred deposition-anchor proof.
`NoRoute` means this search did not find a complete route in the declared graph;
it is not a proof that the part cannot be manufactured. Resource/cancel/input
errors remain distinct. Greedy ordering can miss a feasible complete schedule.

Caller-sized `Limits` and cancellation bound search. This first reference
implementation intentionally scans candidates/obstacles directly; the benchmark
records charged work and timing before spatial-index or lookahead changes.
Scarf ramps, new connectors, flow/thermal/tool-change realization, helical paths
and production integration need their own construction/verification contracts.

Run `cargo test -p spiling-deposition-routing --all-targets` or
`cargo run -p spiling-deposition-routing --example route_matrix --release`.
The matrix and independent judge live under `examples/matrix/`; they reconstruct
inventory, permissions, dependencies, travel binding and complete segment/head
clearance independently of planner helpers. They are declared Rust route fixtures,
not accepted BREP imports. [The research harness](../../tools/route-lab/README.md)
freezes these files and permits edits only to `src/search.rs`.
