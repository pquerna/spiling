<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Resource envelopes protect availability

Status: adopted. `crates/geometry/src/compute.rs` owns the default native
geometry byte envelopes; `GeometryWorkBudget::query()` and `::compile()` select
cancellable cumulative monitoring without a work-unit ceiling. Explicit
`GeometryWorkBudget::new(maximum)` retains deterministic caller-selected
exhaustion. Display also accepts a borrowed guard and phase
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
- resource admission, including optional cumulative work limits, simultaneous-live bytes,
  overflow/allocation refusal and cancellation;
- responsiveness, including worker isolation, stage reporting and cancellation.

Small fixed byte allowances and elapsed-time thresholds are not geometric or
manufacturing validity rules. Bound in-flight buffers, queues and uploads; use
backpressure and spill/stream derived data rather than rejecting a complete job
at a buffer-sized aggregate threshold. Resident cache and worker allocations
should adapt to actual machine/container headroom, with explicit operator
constraints, while checked record sizes, offsets and fallible allocations remain
mandatory. Existing aggregate source, scratch and display ceilings are capacity
debt, not intended product-size targets; removing them requires the actual
owning consumer changes, not setting constants to the largest integer.

Ordinary native jobs and their watches have no elapsed-time deadline. Only an
explicit study limit requests automatic cancellation. User cancellation,
owner-loss/shutdown and durable commit/ACK supervision remain active. The
acknowledgement grace bounds recovery after cancellation; it is not a runtime
allowance for a healthy job. External test campaign deadlines are explicit
operator constraints and never qualify a product rejection as correct.

Current transitional geometry-library envelopes remain enforced; responsiveness
is a separate obligation:

| Envelope                                  | Default                                                          |
| ----------------------------------------- | ---------------------------------------------------------------- |
| one interactive query or explicit section | monitored work; no unit ceiling                                  |
| one complete compile                      | monitored work; no unit ceiling                                  |
| prepared-section pool                     | `MAX_PREPARED_SECTION_BYTES` = 512 MiB live                      |
| standalone import query proof             | `MAX_QUERY_PROOF_BYTES` = 256 MiB live; explicit caller override |
| directional owner/query scratch           | `MAX_DIRECTIONAL_SCRATCH_BYTES` = 256 MiB live                   |
| one display candidate proof               | `DISPLAY_PROOF_BYTES` = 256 MiB live                             |

Work is cumulative per operation and charged through one guard; a retry, cache
reuse, layer or API boundary does not reset it. Byte limits are live bounds for
the owner named by each constant, not additive process-RSS promises. Existing
borrowed-work APIs permit explicit finite work limits; byte-limit APIs permit
narrower storage limits. Convenience APIs do not expose every byte allowance.
One atomic counter owns work admission and reporting. Limited guards never
over-admit under contention; monitored counters saturate rather than wrap.
For monitored guards, `remaining()` returns `u64::MAX` for the native candidate
bridge, not a whole-operation quota. Native candidate consumption still debits
the same operation on success or failure.
`tessellate_with_work` accepts the display caller's guard and a synchronous
phase observer. The engine retains one display guard across all definitions
rebuilt by an open operation as well as all faces and refinement attempts.
Deliberate exhaustion, overflow, cancellation and stale-result tests keep their narrow
selectors; ordinary positive tests use the default policy. Heterogeneous work
units do not share a universal CPU duration. Memory, allocation, overflow, finite
output and genuine convergence/depth safeguards remain enforced. The engine watchdog supervises explicit cancellation and selected study limits;
ordinary operation runtime is not bounded by a hidden default timer.

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

The engine owns source/proof admission for import, open and missing previous-root
captures during Save As. It observes available system RAM, the current process's
container/cgroup ancestor headroom and finite POSIX address-space headroom. Half
of the smallest observed headroom admits additional source/proof growth, leaving
room for opaque native transients, the workbench and other applications. This is
proportional to the current machine, not a fixed model-size allowance. Optional
`SPILING_SOURCE_MEMORY_BYTES` selects a positive total byte allowance for retained
raw sources and measured native proofs. Invalid configuration refuses startup.
Existing residency already reduces observed OS headroom; subtract measured
retained bytes again only from that explicit total. The worker passes the
remaining growth into native proof construction without a hidden standalone
256MiB clamp. Other scratch, mesh, inventory and packed-output limits remain
separate capacity work.

The engine also owns manufacturing working-set admission. When a compile,
alternative, layout trial, preflight or inspection job starts on the worker, it
observes the same headroom fresh and passes half of the smallest observation to
manufacturing as a `WorkingSet`. Inputs, sources and held artifacts are already
resident in that observation, so the planner and verifier count only their own
simultaneously live geometry, caches and staging against it; there is no fixed
256MiB planner ceiling. Optional `SPILING_MANUFACTURING_MEMORY_BYTES` selects a
positive explicit allowance; invalid configuration refuses startup. Concurrent
planning tasks share the allowance rather than each admitting all of it. Format
and protocol representation limits (bundle, program, layer and segment counts)
remain separate constants, and Core's held compressed-artifact allowance
remains separate capacity debt.

Resident memory, not only counted live data, consumes that headroom. On glibc
the engine bounds allocator arenas to four before any thread starts (an operator
`MALLOC_ARENA_MAX` wins), so parallel per-layer planning does not keep a
per-thread arena high-water resident beside its live working set.

Core owns semantic source identity and persistence, measures distinct actual
source Vec capacities across Current/history/prepared snapshots and accepts a
caller-selected extra allocation budget. Equal hashes with different allocations
still consume RAM. Saved return freshly streams and verifies actual stored
chunks/object hashes before sharing unchanged source Arcs; native definitions
and display caches retain their existing identity-bound reuse. Required BREP and
tessellation residency is useful work, not waste to eliminate with tiny limits.
Avoid duplicate allocations and repeated native/display construction instead.
One source object is bounded by PTAC's 1 GiB immutable-object representation, not
an arbitrary 16 MiB RAM gate. The native parser/BREP still requires a complete
source; this policy does not provide streaming BREP construction or exact RSS
enforcement. Packed output, source residency, native proof scratch and
renderer/GPU memory retain distinct owners.

Section assembly sorts original endpoint uses and builds an exact successor
permutation instead of repeatedly scanning all arcs. Degree-one incidence,
stable canonical order and parallel provenance/error ownership remain mandatory.
Section polygon certification expands bounds outward for curve error before
the shared closed-box index and unchanged exact interval proof. Nonfinite
expanded bounds retain complete pair checks. Coexisting chord/index/source/output
capacities are admitted. No fixture grants a resource exemption.

Display trim circuits share the closed-box sweep. An active Y-interval tree
avoids all-pairs scans when X ranges overlap but Y ranges are disjoint.
Both axes use literal closed bounds; touching pairs survive and each candidate
is emitted once. Sorting, updates and traversal remain counted and cancellable,
with admitted coexisting storage. Dense actual overlap can still require
quadratic candidate output. A high work count can expose avoidable enumeration, not
intrinsically expensive geometry. Phase-scoped diagnostics must identify that
distinction before selecting an algorithm change or an explicit caller limit.

Native continuation matches exact role/source-face keys by sorted bijections,
then checks mapped parents. Conditional confinement/interface fallbacks use
outward-expanded shape bounds before unchanged kernel-containment and separation
predicates; every possible owner is counted, never a nearest owner. Display trim
segments share immutable partition validation, not range-local coefficient,
denominator or continuity proofs. Local surface queries binary-search closed
knot spans, retaining both incident spans at exact knots. A failing display face
requests only its own finer tolerance; shared edges inherit incident requests.
Each refinement round collects all independently failing faces in an admitted
face-sized inventory, not a fixed-size exception list or one restart per face.
After an audit failure the current packed prefix cannot publish; remaining faces
are audited for the next round, and only a complete fresh successful pass packs
every face in source order. The eight-round convergence guard is not a limit on
the number of faces that may request refinement.
Fresh face audits remain required until complete candidate/shared-edge dependency
reuse is independently qualified.
This does not establish end-to-end subquadratic continuation: complete-boundary
clearance still enumerates segment pairs before some fast paths. Uncached
surface queries still validate global knot inventories; local span lookup does
not supply a reusable surface-validation receipt.

These are software resource defaults, not measured hardware requirements,
universal GPU/platform qualification, printability or machine readiness.

## Alternatives rejected

- Treating a subsecond timeout or a small allowance as a correctness criterion.
- Per-fixture exemptions or resetting the same operation's allowance at each
  stage.
- Adaptive retry machinery before measuring the ordinary positive route.
- Requiring deep optimization before allowing valid work to finish.
- Reporting consumed work as percentage complete.
