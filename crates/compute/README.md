<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
-->

# Shared numeric compute

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
uses closed axis-aligned bounds and rejects inverted boxes. Nonfinite admitted
inputs refuse. IEEE-754 ordered high/low u32 keys normalize signed zero and
preserve subnormal/extreme comparisons without shader f64. Kernels use four-lane
loads and packed 32-bit masks. `get` bounds-checks; `selected_columns` scans set
bits without walking every column.

The generated CubeCL buffer-argument call is the isolated unsafe boundary.
Before it, the adapter checks padded source-field lengths, query-field lengths,
row/column and total-word bounds, and every tile range. Inputs and results are
u32 arrays, not serialized Rust structs; readback must have the exact expected
word-byte length before decoding. No kernel object crosses the service boundary.

Admission is 4,096 or more pairs, at most 65,536 items per axis and 262,144
mask words including row padding (1 MiB; at most 8,388,608 comparisons). Outside
this optional budget, `Ok(None)` leaves the original scalar algorithm authoritative.
Cancellation, invalid inputs and runtime failures never mean `None`.
Each tile holds at most 4,096 words (16 KiB/131,072 comparisons). Host encoded
inputs and queries each cap at 2 MiB; the result caps at 1 MiB. Device inputs and
queries are uploaded once per batch, not per tile; there are only two immutable
input handles and one output tile live. Dedicated device allocations bypass
adaptive pool growth. Runtime/compiler/driver overhead is additional to these
buffer limits, not a claimed total memory ceiling.

One cancellable gate admits a batch before scratch allocation; no application
work queue is created. One fixed stream per selected device prevents per-caller
stream accumulation. Every tile synchronously drains, and completion/cancellation
drains resource release before the next batch. CPU launches have one cube/unit,
no barriers and no pool-growth requirement. CubeCL's independent LLVM runner
still creates its initial worker pool from OS-active cores, not Rayon settings;
these kernels queue at most one unit at a time. Vulkan launches have one cube of
64 units. Cancellation is checked during validation/encoding, while waiting for
admission, between tiles and before result publication, not inside a running tile.
Only three predicate specializations are compiled; initialization exercises the
crossing specialization.

Run `CARGO_BUILD_JOBS=2 cargo check -p spiling-compute` and
`CARGO_BUILD_JOBS=2 cargo test -p spiling-compute`. Actual Vulkan conformance is
opt-in: `CARGO_BUILD_JOBS=2 cargo test -p spiling-compute --test conformance
vulkan_matches_scalar_oracles_and_independent_fea -- --ignored --nocapture`.
The separate ignored `software_vulkan_is_explicit_and_auto_selects_cpu` test
requires `SPILING_COMPUTE_SOFTWARE_VULKAN_ICD` naming an actual software ICD.
These checks do not qualify manufacturing decisions, speedups, physical GPU
support on other machines or machine readiness.
