<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
-->

# Shared numeric compute

New numeric consumers should prefer one CubeCL kernel serving CPU/backend
vectorization and GPU over separately maintained scalar/SIMD implementations.
Keep domain orchestration and independent verification separate. Evaluate
complete cold/warm calls and resident-data reuse across workload sizes; batch
size is one policy input, not a blanket GPU admission gate. Keep kernels
portable for future Wasm/WebGPU consumers; each target still needs conformance.

## Batch architecture

A launch has a fixed cost (about 100–250 µs per round trip today, CPU or
GPU) that no kernel tuning removes, so launches must scale with device time,
not with how often code asks. Every kernel follows three layers:

1. **Grouped, pure kernels.** A kernel takes many independent groups as flat
   buffers plus offset tables and runs them in one launch. A group's result
   depends only on that group, so it is identical however groups are batched;
   determinism never depends on scheduling.
2. **Device coalescer (group commit).** Callers make an ordinary synchronous
   call. Requests from every thread queue on the device; whoever finds no round
   running leads one: it takes everything queued, runs one grouped launch per
   segment and kernel kind, publishes each result and wakes the waiters, while
   the next round gathers. Under concurrency, many calls share one launch with
   no caller changes. Exclusive work (a whole-stage batch borrowing the
   caller's data) leads a round the same way, after the queued requests.
3. **Stage batches.** Where the planner's (stage, layer) graph is wide but a
   chain is serial, the stage gathers its groups and submits them in one call
   (`capsule_masks` takes a whole support stage; a layer's skins and
   interiors, its bridge rows and its gyroid curves each make one request).

There are no admission gates or scalar fallbacks: any size runs, and an
empty axis returns without a launch. A new kernel adds a grouped kernel, an
encoded request on the caller's thread and a packing step in the round.

`configure(ComputeSettings)` initializes an immutable process session before native
workers, off the async executor. `status()` lazily initializes default settings for
direct callers. Failed initialization is retained; changing settings requires a
new process. Repeating identical settings is idempotent. FEA selection is only a
reserved device choice, not an implemented FEA service.

Auto chooses Vulkan discrete, then integrated GPU, then native CPU, independent
of CubeCL's default-device environment override. Explicit Vulkan may select a
virtual/other/software adapter after discrete/integrated choices; only a reported
discrete/integrated adapter receives hardware-acceleration credit. Acquisition,
initialization-probe and execution failures refuse rather than silently downgrade.

The three numeric APIs return exact row-major query × input masks:
`row_crossings` uses half-open intervals with either endpoint order;
`interval_membership` uses closed intervals with either order; `box_overlaps`
uses closed axis-aligned bounds and rejects inverted boxes. Nonfinite
inputs refuse. IEEE-754 ordered high/low u32 keys normalize signed zero and
preserve subnormal/extreme comparisons without shader f64. Kernels use four-lane
loads and packed 32-bit masks. `get` bounds-checks; `selected_columns` scans set
bits without walking every column.

The generated CubeCL buffer-argument call is the isolated unsafe boundary.
Before it, the adapter checks padded source-field lengths, query-field lengths,
row/column and total-word bounds, and every segment's length. Inputs and results are
u32 arrays, not serialized Rust structs; readback must have the exact expected
word-byte length before decoding. No kernel object crosses the service boundary.

Callers encode their own requests. A round packs queued mask groups of one
kind into segments of whole rows: at most 1,048,576 output words and
4,194,304 input words (or one group's rows, if larger), each one launch read
back once. A word finds its group by bisection over the segment's word
starts. CPU launches are one cube with one unit per core, since the CPU
runtime runs one worker per unit; GPU launches are grid-stride 64-unit
cubes. Dedicated device allocations bypass adaptive pool growth.
Runtime/compiler/driver overhead is additional to these buffers, not a
claimed total memory ceiling.

One fixed stream per selected device prevents per-caller stream
accumulation. Every round synchronously drains before the next begins. The
first execution failure is retained and refuses every later round. A waiter
that is cancelled leaves the queue, or abandons its result if its round has
started; a running segment is not interrupted. Kernels have no barriers.
Only three predicate specializations are compiled; initialization exercises
the crossing specialization.

`capsule_masks` rasterizes groups of closed capsules (integer ticks) into
each group's dense window of 64-cell words. Decisions are exact i64 tests, so
every backend sets the same cells; a single-precision guess of each row's
interval only chooses where the exact walk starts. The crate derives each
group's window from the kernel's own row and column bounds, and a hit outside
it refuses. A capsule owns a contiguous job range found by bisection, so no
per-job input is uploaded; batches of any size run as segments of whole
groups, with one zeroed output per segment read back once. Callers hand a
whole stage (many groups) to one call. It has no admission gate: CPU launches
use one cube with a unit per core, each unit taking a contiguous job block
(the CubeCL CPU runtime runs one worker per unit), and Vulkan launches stride
64-unit cubes. `capsule_cases` is a test-only generator and reference check.

Run `CARGO_BUILD_JOBS=2 cargo check -p spiling-compute` and
`CARGO_BUILD_JOBS=2 cargo test -p spiling-compute`. Actual Vulkan conformance is
opt-in: `CARGO_BUILD_JOBS=2 cargo test -p spiling-compute --test conformance
vulkan_matches_scalar_oracles_and_independent_fea -- --ignored --nocapture`.
The separate ignored `software_vulkan_is_explicit_and_auto_selects_cpu` test
requires `SPILING_COMPUTE_SOFTWARE_VULKAN_ICD` naming an actual software ICD.
These checks do not qualify manufacturing decisions, speedups, physical GPU
support on other machines or machine readiness.
