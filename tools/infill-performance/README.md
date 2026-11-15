# Native infill performance measurements

This measures production Rectilinear, Grid, Gyroid and Cathedral hatch dispatch.
It is a developer workload, not another generator or a timing/resource gate.
Use it to locate work, then confirm useful changes through complete compilation
and independent verification. Nominal density does not make different patterns
mechanically equivalent or guarantee equal material.

## Build once, reuse the executable

From the repository root, with adequate space in the owning worktree target:

```sh
mkdir -p .cache/infill-perf
CARGO_BUILD_JOBS=4 CARGO_PROFILE_DEV_DEBUG=1 RUST_MIN_STACK=8388608 \
  cargo test --locked -p spiling-manufacturing --lib --no-run \
  --message-format=json > .cache/infill-perf/build-test.jsonl
CARGO_BUILD_JOBS=4 CARGO_PROFILE_DEV_DEBUG=1 \
  cargo build --locked --profile dev -p spiling-manufacturing --bench pipeline \
  --message-format=json > .cache/infill-perf/build-pipeline.jsonl
```

Select the `compiler-artifact.executable` whose target name is
`spiling_manufacturing` or `pipeline` from those successful Cargo outputs. Never
run a remembered executable after a failed build. All measurements should keep
the same compiler, profile, features, backend and source. The dev profile already
optimizes manufacturing/geometry numerical work; the debug override adds symbols
without release/LTO. Other dependencies retain their configured optimization.

Given the first executable as `infill_test`:

```sh
systemd-run --user --scope -p MemoryMax=16G \
  python3 tools/infill-performance/measure.py "$infill_test" \
  .cache/infill-perf/baseline --samples 3
```

`--quick` selects the small 15% cases; the full matrix includes larger interiors,
three densities, curved holes, concave/disconnected outlines, and Cathedral roof
closure. `--backend vulkan` explicitly selects the configured GPU path. Setup and
one warm iteration precede timed samples. Cases run sequentially. Timers include
hatching and output destruction, excluding output observation. Cathedral's
isolated measurement includes local seed discovery; it is not a measurement of
production part-level prepared-column reuse.

Results retain each sample, counts, length, nominal volume and a coordinate
fingerprint, plus timing ranges, producer identity and source receipts (including
untracked files). These observations screen accidental output changes; they do
not replace independent geometric and program verification. Completed cases are
checkpointed; a failure stops immediately and retains the diagnostic. A distinct
output directory is required for each run. No sample is silently discarded.

Compare a candidate using `--reference BASELINE/summary.json`. The harness
requires matching geometry/material observations before reporting a geometric
mean speedup. This is a guard for geometry-preserving implementation changes,
not a permanent prohibition on better representations. Geometry-changing
experiments need an independently qualified tolerance/topology/coverage/anchor
judge before their performance can be admitted. Very fast cases need larger,
frozen repetitions (`--layers N`) and paired runs before interpreting small improvements.

## Native perf capture

Keep build-ID caches with the owned experiment output as well as raw profiles:

```sh
perf_cache="$PWD/.cache/infill-perf/buildid"
SPILING_INFILL_PATTERN=gyroid SPILING_INFILL_SHAPE=annulus \
SPILING_INFILL_SIZE=192 SPILING_INFILL_FRACTION=0.15 \
SPILING_INFILL_REPEATS=6 SPILING_INFILL_COMPUTE=cpu \
SPILING_INFILL_OBSERVE=0 RUST_MIN_STACK=8388608 \
  perf --buildid-dir "$perf_cache" record -e cycles:u -F 99 --call-graph dwarf,16384 \
  -o .cache/infill-perf/gyroid.data -- \
  "$infill_test" measure_infill_generation --ignored --nocapture --test-threads=1
perf --buildid-dir "$perf_cache" report --stdio --no-children --call-graph none \
  -i .cache/infill-perf/gyroid.data
```

Output observation is disabled for the profile so checksum/volume work does not
become an invented product bottleneck. Profile capture and unprofiled timing
are separate runs. Inspect sample loss and unwind quality. Flat symbol shares
are not exclusive stage wall times; unavailable callers must stay unknown.
Explicitly separate first-use compute/JIT costs from warmed work.

## Complete jobs

The existing `pipeline` executable accepts `--pattern
rectilinear|grid|gyroid|cathedral` with `--box X,Y,Z`; it uses the production
compiler, emitter and independent verifier:

```sh
"$infill_pipeline" --box 48,48,8 --infill 0.15 --pattern gyroid \
  --phase compile --samples 3 --warmup 1 --compute-backend cpu \
  --write-bundle .cache/infill-perf/gyroid.bundle
"$infill_pipeline" --bundle .cache/infill-perf/gyroid.bundle \
  --phase verify --samples 3 --warmup 1 --compute-backend cpu
```

For matched slicer comparisons of a box, add `--supports disabled`. The default
`--supports fixture` keeps the frozen fixture's breakaway policy, which may refine
physical layers even when it emits no support roads. Match the support policy
and inspect the actual schedule before attributing material or print-time
differences to the sparse pattern. The override also applies to `residency`;
loading `--bundle` always uses its captured intent.

Bundle and raw program stay together. The benchmark checks repeated program
identity. Its allocator also collects residency counters; this is a native
facade workload, not measured workbench responsiveness. Complete compilation
already includes verification: the second timing is a fresh verification pass,
not another additive compile phase. Real imported-part saved returns and UI
acceptance remain necessary for a claimed product performance improvement.

Keep metrics descriptive. Do not change source tolerance, deposited material,
roof support, bridge/anchor predicates, cancellation or independent verification
to improve a score. Retain compact evidence before retiring task-owned raw
profiles, build-ID copies, bundles and build caches.
