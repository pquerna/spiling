<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Native opening and manufacturing performance

`crates/manufacturing/benches/pipeline.rs` exercises the production compiler
and inspection facades. `pnpm bench:slice` builds the `profiling` Cargo
profile: release-derived optimization/LTO with symbols retained. It selects the
engine/CLI/workbench packages too, keeping the native application's unified
dependency feature set. Build time is outside reported samples. Keep that
package selection for focused library tests too; omitting a package can change
transitive features and rebuild the kernel even with the same profile and
kernel revision. Record the exact revision, dirty diff, executable hash,
toolchain, profile overrides, CPU/affinity/quota and resource envelope with
runs. Do not compare different profiles as an algorithm improvement.

The `native-manufacturing-v1` workload imports the unchanged through-hole source
and shared box definitions, places 1/2/8/32 separated uniform occurrences and
captures the existing mixed-width/combined intent. Source/intent hashes, input
identity, selected strategy, actual program hash/counts and available parallelism
accompany each sample. This is a declared software cohort, not general geometry,
physical printability or maximum plate-size qualification.

## Short performance loop

Observe the complete operator journey before choosing a kernel: saved return,
fresh inspection/replay, artifact retrieval/export and automatic compilation
have different costs. Orient with separate CPU samples, allocation/live-byte
evidence and logical versus physical I/O; symbol-family shares are not exclusive
phase timers. Decide on one measured mechanism, then change it without weakening
admission, cancellation, immutable identity or independent actual replay. Act by
rerunning the same successful and refusal controls, comparing output bytes and
resource use, and shipping the independently useful cut before choosing another.
Keep the remaining opportunity ranking current in its research owner, not as a
second implementation backlog here.

CLI native operations use the shared checked operation watch, without periodic
completion polling. Attribute short-journey savings from observation separately
from numeric planner, kernel and verifier work; end-to-end latency includes both.
Watch establishment has the client deadline, while the engine retains native
work/deadline/cancellation authority.

Artifact-transfer investigations must separate ledger reads from RPC copying.
The operation ledger uses incremental BLOB reads, independent of project PTAC
storage; [the control protocol](../protocol/control.md#incremental-artifacts-and-binary-reads)
owns reader lifetime and backpressure. Compare logical reads against artifact
size over several sizes/ranges before proposing shared memory or a storage
schema change. Cached filesystem reads are not physical disk traffic.

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

`--compute-backend auto|cpu|vulkan` configures the immutable compute session
before source preparation; the default is `auto`. The benchmark calls native
services directly, so engine-launch environment overrides do not select its
backend. Records include requested preferences, resolved devices and separate
compute initialization time. First kernel compilation remains inside the first
consuming sample. Keep backend/device selection unchanged for before/after
algorithm comparisons; the comparator refuses changed or missing compute
configuration. Cross-backend experiments are separate cohorts, not a
like-for-like algorithm speedup.

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
JSON, using the same package set rather than remembering an executable hash:

```sh
cargo build --locked --profile profiling \
  -p spiling-manufacturing -p spiling-engine -p spiling-cli -p spiling-workbench \
  --bench pipeline --message-format=json
```

Run `perf record -e cycles:u -F 99 --call-graph dwarf,16384 -- EXECUTABLE ...`
where permitted.
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
its executor. Native definitions remain confined to the engine worker. A synchronized kernel
cache alone does not qualify concurrent queries; parallel source work needs an
exercised ownership boundary, not a thread-count setting or an unsafe assertion.

## Actual native opening

`pnpm bench:open` builds matching optimized engine/CLI/workbench binaries and
measures fresh-process, read-only opening of a saved `.spil` with at least one
renderable source. The automation
adapter currently requires Linux X11, Python 3 and an accessible `DISPLAY`.
Use isolated XDG data where the native application license has already been
accepted interactively. Each sample gets a copy of those preferences in its own
XDG data directory; the supplied preferences remain untouched. The tool never
accepts the license on the operator’s behalf.
Run inside the same external memory/task/CPU envelope for both revisions.

```sh
pnpm --silent bench:open --project SAVED.spil --user-data ISOLATED_XDG_DATA \
  --output .cache/open-before --samples 3 --warmups 1 > .cache/open-before.ndjson
# Repeat after the change, using a new output directory/file.
node tools/compare-open-bench.mjs .cache/open-before.ndjson .cache/open-after.ndjson
```

The opt-in `SPILING_OPEN_TRACE=1` records bounded static milestones on stderr:
initial UI, first GPU readback, engine/storage/native admission/tessellation,
saved fresh replay, operation publication, display transfer/staging and source
readback. The workbench requests two extra GPU readbacks when tracing; completion
bounds rendered pixels, not compositor presentation. These clocks are
process-local; compare stage durations within one process, not absolute engine
and workbench timestamps. Complete open still validates all saved content and
freshly replays saved manufacturing. No partial-open or stored-verification shortcut
is part of this measurement.

Each sample retains the real window screenshot, trace, binary/project/preferences hashes,
adapter, effective CPU affinity/parallelism, cgroup resource limits and peak process
RSS. It closes through the window manager, requires successful shutdown and child
reaping, and confirms that the read-only input is unchanged. Warmups are excluded;
each sample starts a new process. Filesystem cache state is uncontrolled, so this
is not a cold-disk benchmark. Compare the same optimized profile and environment;
the comparator refuses incomplete samples and changed inputs, producer, graphics
or recorded resource configuration. Timing remains separate from correctness and
hardware/platform qualification.

`node tools/prepare-open-bench.mjs NEW_DIRECTORY profiling` captures a reusable
cohort through the matching CLI/engine: shared occurrences, distinct definitions,
curved sources, multiple plates with saved programs and a 32-part mixed-width job
with protected bore. Build the matching
binaries first. Preparation is outside sample timing; external source copies are
removed before fresh return. `cohort.json` retains source hashes and explicit
geometry/display refusals. A project with some unavailable definitions is not
complete display qualification for those sources. Use additional real saved jobs
for manufacturing-heavy return; do not substitute the small cohort for them.

Fresh fine-path replay prepares a bounded borrowed-path inventory per prior
layer. Conservative projected bounds and maximum local bead radius reject only
paths that cannot contribute centerline or endpoint support. Surviving edges use
the original exact capsule calculation and order; skipped edges still charge the
same cumulative work ceiling. No copied motion plan or new verification authority
is retained. Native operation observation uses the existing checked operation
stream immediately; the host timer remains for liveness, not completion polling.
