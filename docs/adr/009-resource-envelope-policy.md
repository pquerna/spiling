<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Resource envelopes protect availability

Status: adopted. `crates/geometry/src/compute.rs` owns the default native
geometry envelopes; `GeometryWorkBudget::query()` and `::compile()` bind them
to ordinary work. Display also accepts a borrowed caller allowance and phase
observer rather than imposing a second hidden ceiling. Geometric
tolerances, material/void/incidence obligations, independent emitted-program
verification and physical qualification are unchanged.

## Context

Resource limits were scattered across public entry points, planner stages and
tests: a 2M test-local collar selector, a 50M section allowance, a 20M display
allowance, a 32 MiB display proof workspace, a 64 MiB prepared-section pool and
a 512M whole-compile allowance. Those numbers had different meanings and no
measured end-to-end policy. Valid collar work completed well above the 2M
selector but failed it. The limit had become an accidental capability ceiling
rather than an availability guard.

## Decision

Keep three contracts separate:

- geometric and manufacturing correctness, including declared tolerances and
  independent replay;
- resource admission, including cumulative work, simultaneous-live bytes,
  overflow/allocation refusal and cancellation;
- responsiveness, including worker isolation, stage reporting and cancellation.

Default envelopes bound resource admission while allowing valid work to finish;
responsiveness remains a separate obligation:

| Envelope | Default |
| --- | --- |
| one interactive query or explicit section | `QUERY_WORK_UNITS` = 4B cumulative work |
| one complete compile | `COMPILE_WORK_UNITS` = 8B cumulative work |
| prepared-section pool | `MAX_PREPARED_SECTION_BYTES` = 512 MiB live |
| import query proof | `MAX_QUERY_PROOF_BYTES` = 256 MiB live |
| directional owner/query scratch | `MAX_DIRECTIONAL_SCRATCH_BYTES` = 256 MiB live |
| one display candidate proof | `DISPLAY_PROOF_BYTES` = 256 MiB live |

Work is cumulative per operation and charged through one guard; a retry, cache
reuse, layer or API boundary does not reset it. Byte limits are live bounds for
the owner named by each constant, not additive process-RSS promises. Existing
borrowed-work and explicit byte-limit APIs permit narrower caller limits;
convenience APIs choose fixed defaults and do not expose every allowance.
`tessellate_with_work` accepts the display caller's guard and a synchronous
phase observer. The engine retains one display guard across all definitions
rebuilt by an open operation as well as all faces and refinement attempts.
Deliberate exhaustion, overflow, cancellation and stale-result tests keep their narrow
selectors; ordinary positive tests use the default policy.

Manufacturing passes its captured guard into planar/cylindrical finish and
surface-color face certification as well as native material queries. The
standalone face-certificate convenience wrappers are separate queries and
must not be used to reset work within a compile.

A refusal remains a typed `ResourceLimit`. It is not invalid or unsupported
geometry, and it must not be converted into an empty success, a mesh repair or
a tolerance change. Raising an allowance does not establish convergence for an
invalid chart, stalled enclosure or combinatorial blowup.

## Consequences

Valid but expensive work may now run for tens of seconds or minutes. It must
run off the UI thread from first dispatch, report real stages or indeterminate
progress, and remain cancellable; consumed allowance divided by the maximum is
not progress. Native display reports edge preparation, candidate generation,
independent auditing and completed-face counts for the current attempt. The
engine coalesces these through one job-bound slot into existing Operations
phase metadata, without a second scheduler or per-cell IPC. Stale progress
cannot override cancellation, promotion or terminal status.
A later engine-owned aggregate reservation still has to account
for coexisting definitions, jobs, meshes and outputs rather than summing these
independent maxima.

These are geometry-library ceilings, not new engine residency limits. Engine
import still reserves incoming sources and retained proof/index allocations
against the remaining 64 MiB source/history envelope; it passes that smaller
proof allowance into import. The 256 MiB standalone proof ceiling does not
override that reservation. Packed output, source residency, native proof
scratch and renderer/GPU limits retain distinct owners.

Section polygon certification uses a conservative outward-error-expanded grid
before the unchanged exact interval proof. Its projected chords and sorted
cell entries share source/output admission; all setup, sorting and pair scans
remain charged and cancellable. Each ordinary candidate is checked once;
wide or unindexable chords retain all-pairs checks. The 512-hole plate motivates
the query default, not a per-fixture exception or a promise that every large
section fits. Preparation cost and full-engine resource qualification remain
separate obligations.

Display trim circuits similarly use a closed UV box sweep before unchanged
interval joint/crossing proofs. It prunes only strictly disjoint boxes across
all source loops, with charged cancellable sorting and coexisting storage
admission. A high work count can expose avoidable candidate enumeration, not
intrinsically expensive geometry. Phase-scoped diagnostics must identify that
distinction before selecting either an algorithm change or a larger allowance.

These are software resource defaults, not measured hardware requirements,
universal GPU/platform qualification, printability or machine readiness.

## Alternatives rejected

- Treating a subsecond timeout or a small allowance as a correctness criterion.
- Per-fixture exemptions or resetting the same operation's allowance at each
  stage.
- Adaptive retry machinery before measuring the ordinary positive route.
- Requiring deep optimization before allowing valid work to finish.
- Reporting consumed work as percentage complete.
