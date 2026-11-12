# Ideas backlog: pipeline speed with supports on (from baseline perf profiles, 2026-11-12)

Baseline profile shares (parts 13/15/03/22, supports on): offset `region::boundary_tube` capsule-union
32% (inside arachne::cover openings, support offset_polygon_owned, region offset_admitted);
routing `search::solve` 16–20% (40% of that is libm `hypot`); arachne cover unions ~8%; skeleton
construct 8–12%; support Booleans 10%+; strokes::analyze 7%; budget accounting (polygon_budget, sift,
sweep_active, material_bytes, remaining_geometry_budget) 5–8%. Planner is fully single-threaded on a
32-core host.

## A. Offset engine (largest single lever)
- A1 One-sided offset band: erosion/dilation only need the inner/outer half of the boundary tube. Build
  per-edge rectangles on one side + disk wedges only at vertices convex toward that side; ~5x fewer
  points and far fewer overlaps in the union. Same disk polygon, same error budget.
- A2 Direct offset curve (Clipper-style): offset each ring's edges, round joins with the circumscribed
  disk's vertices, then one self-union (NonZero) to remove loops. O(n log n) single overlay.
- A3 i_overlay native OutlineOffset (round join, angle from the error budget, radius inflated to the
  circumscribed bound) as the engine; validate against capsule union on census layers.
- A4 Opening = erode∘dilate fused: skip intermediate canonicalization/admission between the two passes.
- A5 Positive offset of several polygons: one union of all tubes + polygons instead of incremental
  per-polygon `expanded.union(tube)` (quadratic in island count).
- A6 Cull: an opening of a region whose minimum feature width > 2r is the region itself; detect cheaply
  (e.g. medial-axis/skeleton radius already computed by arachne) and skip.

## B. Do less work: memoize identical layers
- B1 Content-keyed wall cache: tolerant mode makes every layer its own span, so `wall_cache` (keyed by
  span) never hits. Key by hash of canonical material bits + envelope/required owner -> reuse WallPlan
  across identical prismatic layers (deck panels, brackets).
- B2 Same content cache for classification/skin/infill region work and support per-layer corridors.
- B3 Section-level dedup in tolerant::collect_span: identical consecutive sections share one Arc.

## C. Parallelism (rayon, deterministic ordered results)
- C1 Precompute per-layer walls (arachne plan incl. cover) in parallel before the sequential schedule.
- C2 Routing: groups per layer are independent except the start point; route in parallel with the
  start fixed to the previous layer's planned end via two-phase (construct ends first), or accept the
  previous layer's construct-only end as start (semantic change, gate it).
- C3 Parallel tolerant sectioning (section_at per layer).
- C4 Parallel support per-layer candidate/raster generation where layers do not depend on each other;
  parallel final clearance certification (pure checks).
- C5 Parallel verifier/proof passes (certify_support_clearance, certify_polygon_capsules).

## D. Routing search
- D1 hypot -> sqrt(dx*dx+dy*dy) (or squared-distance compare where only ordering matters).
- D2 Neighbor lists (k nearest by entry/exit, k~8–12) for Or-opt insertion instead of O(n) scan:
  O(n*k) per sweep instead of O(n^2).
- D3 Cache gap cost of candidate insertion pairs; avoid recomputing entry_exit for unmoved visits.
- D4 Spatial grid for nearest-neighbour construction.

## E. Support stage
- E1 strokes::analyze: spatial index (uniform grid / sorted x-sweep) over previous-layer capsules
  instead of pairwise screening blocks.
- E2 Boolean ops: union many small shapes with one NonZero overlay instead of chained unions; reuse
  column unions across layers (immutable candidate column cache already exists; extend).
- E3 Remove per-operation accounting estimators now that ceilings are gone (ADR009: count, don't cap):
  polygon_budget/sift/sweep_active pre-estimation, remaining_geometry_budget scans, material_bytes walks.
- E4 offset_polygon_owned: same offset engine (A) and cache offsets of identical inputs.

## F. Skeleton / Voronoi
- F1 inside_material / ray_crossing: replace per-vertex ray casting over all edges with one
  edge-indexed sweep or y-bucket grid (O(log n) per query).
- F2 Feed boostvoronoi integer input once per layer; reuse across identical layers (B1 covers).
- F3 (done: y-bucket rows; G3 CubeCL PIP unnecessary, PIP now <1% of walls.) Next skeleton lever is
  boostvoronoi's lazy->exact circle-formation fallback (~95% of lazy pss/sss calls end in ExtendedInt):
  find which site configurations fail the 128-ulp check (near-collinear consecutive sites? origin-near
  results?) and fix upstream or condition input semantically.
- F4 Other nested-rayon i_overlay calls inside parallel stages (convolution offsets, geo Booleans in
  cover) could pick the serial solver when rayon::current_thread_index() is Some; small today.

## G. CubeCL (aggressive; screen on GPU, decide exactly on CPU)
- G1 GPU broad phase for strokes::analyze: candidate-vs-previous capsule bbox overlap masks via the
  existing box_overlaps kernel, then exact CPU intervals only on hits.
- G2 GPU k-nearest candidate lists for routing Or-opt (all-pairs entry/exit distances per layer).
- G3 GPU point-in-polygon (winding) batch for skeleton inside_material.
- G4 GPU batch segment–segment distance screen for certify_support_clearance /
  certify_polygon_capsules (prove far pairs cheaply in f32 with conservative margin; exact CPU on rest).
- G5 GPU conservative raster pre-filter: rasterize material/candidate masks per layer to bitmaps to
  skip Boolean ops whose result is provably empty/identical.
- G6 GPU hashing/compare of layer sections for B1 keys (silly but cheap).
- G7 GPU capsule rasterization to find which offset tube capsules actually touch others (prune union
  input to the overlapping ones).

## H. Allocation / constant factors
- H1 Avoid MultiPolygon clones in hot paths (support column polygons, section material clones).
- H2 mimalloc as global allocator in the engine.
- H3 Build with target-cpu=native for profiling comparisons only (not for shipping).

## Parallelism rule (owner, 2026-11-12)
Every rayon loop must first be evaluated as a CubeCL kernel: data-parallel numeric work (distance
matrices, k-nearest lists, point-in-polygon, capsule/segment screens, interval candidates) goes to
CubeCL so it scales with GPU/SIMD width; rayon is for coarse irregular tasks (per-layer walls,
per-layer routing), possibly with CubeCL screens inside. Record the CubeCL evaluation in the log
entry of any rayon experiment.
