<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Manufacturing performance measurements

`crates/manufacturing/benches/pipeline.rs` exercises the production compiler and
inspection facades. `pnpm bench:slice` builds the `profiling` Cargo profile:
release-derived optimization/LTO with symbols retained. It selects the engine/CLI
packages too, keeping their unified dependency feature set. Build time is outside
reported samples. Record the exact revision, dirty diff, executable hash,
toolchain, profile overrides, CPU/affinity/quota and resource envelope with runs.
Do not compare different profiles as an algorithm improvement.

The `native-manufacturing-v1` workload imports the unchanged through-hole source
and shared box definitions, places 1/2/8/32 separated uniform occurrences and
captures the existing mixed-width/combined intent. Source/intent hashes, input
identity, selected strategy, actual program hash/counts and available parallelism
accompany each sample. This is a declared software cohort, not general geometry,
physical printability or maximum plate-size qualification.

## Reproduce and compare

Run native workloads sequentially with externally enforced memory/task/time
limits. On Linux, for example:

```sh
mkdir -p .cache/performance
systemd-run --user --scope -p MemoryMax=8G -p TasksMax=256 \
  env CARGO_BUILD_JOBS=2 RAYON_NUM_THREADS=1 \
  pnpm --silent bench:slice --parts 32 --samples 3 --warmup 0 --phase compile \
  --write-bundle .cache/performance/before.bundle.json \
  > .cache/performance/before.ndjson
```

The benchmark emits one JSON record per measured sample. The pnpm command
resolves bundle paths against the repository root; a direct executable uses its
current working directory. `--write-bundle` creates
a new file exclusively. Existing files never overwrite. Use a new process with
one sample to measure first-use compiler state; use multiple samples to observe
same-process repetition. Neither clears the OS page cache nor proves incremental
compilation. `--warmup` explicitly excludes initial iterations. `--policy primary`
is a separate authored control, not the automatic job or a free speedup.

The compiler timer includes ordinary planning, candidate selection, realization,
emission and mandatory independent verification. Source import is separately
reported as setup. Other phases consume a captured checked bundle without source
preparation or slicing inside their sample timer:

```sh
pnpm bench:slice --phase validate --bundle .cache/performance/before.bundle.json --samples 5
pnpm bench:slice --phase route --bundle .cache/performance/before.bundle.json --samples 5
pnpm bench:slice --phase verify --bundle .cache/performance/before.bundle.json --samples 5
```

`parse` measures typed deserialization and validates the result outside its timer;
`decode` includes production integrity validation; `validate` checks bundle/plan
integrity without independently replaying the program; `route` resolves complete
projected-bead reservations; `verify` performs fresh report-only inspection;
`motion` adds actual replay generation; `encode` serializes the retained bundle.
Setup and untimed guards still appear in whole-process profiles and CPU totals.
Errors abort the run. Repeated results must retain the same program hash.

After a change, rerun the same commands/profile/envelope to new paths, then:

```sh
node tools/compare-slice-bench.mjs .cache/performance/before.ndjson .cache/performance/after.ndjson
```

The comparator rejects incomplete samples, changed input/producer/output or
recorded CPU configuration. It reports medians, median absolute deviations and
raw sample counts; it does not impose a universal timing gate. Retain raw samples,
resource output and the environment record. Timing cannot replace semantic
acceptance or establish correctness of a legitimately changed program.

## CPU profiles and real workflow costs

Build first. Resolve the benchmark executable from Cargo's `compiler-artifact`
JSON (`cargo build --locked --profile profiling -p spiling-manufacturing --bench
pipeline -p spiling-engine -p spiling-cli --message-format=json`), rather than remembering a hash. Run `perf record
-e cycles:u -F 99 --call-graph dwarf,16384 -- EXECUTABLE ...` where permitted.
Retain self and inclusive views, lost-sample counts and unwind quality; an
unresolved stack does not support an inclusive attribution. Compare timings
without the profiler separately. Do not change system profiling policy to make a
benchmark pass. A privileged profiler can launch a workload under the normal
user when that access is already authorized; keep raw stack captures private.

Build matching optimized engine/CLI binaries to measure saved open, fresh
verification, replay retrieval, export and save separately. The pure compiler
benchmark excludes IPC, SQLite, UI and file export; those costs require the real
CLI/native surfaces. Open freshly validates/replays saved output, so its cost is
not merely SQLite I/O. Preserve writer leases, exact-byte admission, revision
binding, cancellation and child cleanup when removing repeated work. Stored
verification flags never authorize skipping fresh independent replay.

CPU utilization and phase work identify serial computation versus waits. Count
all native/control/render/library workers and coexisting scratch before choosing
parallelism. Tokio handles asynchronous control/I/O; native CPU work remains off
its executor. BREP definitions are not `Sync`; parallel source work needs a real
ownership boundary, not a thread-count setting or an unsafe assertion.
