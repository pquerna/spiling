<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Isolated remote evaluator

This Linux ARM64 research image compares FP64 sparse solves over frozen inert input. It consumes no project mutation API, printer connection, kernel object or executable candidate. SciPy is the CPU baseline; Warp provides an explicitly selected CUDA comparison with no CPU fallback. This is neither a production physics service nor physical strength qualification.

The host needs existing SSH, Python 3, Docker and, for GPU runs, working NVIDIA CDI device injection. Do not change host packages, drivers or Docker configuration as part of this procedure. All source, immutable inputs, results and caches live below a dedicated home-directory prefix. Images and build layers use Docker's existing managed storage. Install Python and OS dependencies **inside the image** only.

## Prepare

Run `pnpm infill:lab campaign context --output NEW_CONTEXT` and copy only that context's explicit source files to a versioned directory under `~/src/spiling-eval/tooling/`. Do not send the whole development checkout, SSH configuration, credentials, Docker socket or private brain as a build context. Build through SSH:

```sh
python3 ~/src/spiling-eval/tooling/RECIPE/launch.py \
  --root ~/src/spiling-eval build \
  --context ~/src/spiling-eval/tooling/RECIPE
```

The official Python base is digest-pinned. `requirements.lock` contains exact package versions and distribution hashes; ordinary dependencies install only from hash-locked wheels. The multi-stage image separately compiles exact-source PyMetis and PyAMG using hash-locked Python build tools and a contained compiler, then copies their wheels into the runtime stage. No host compiler is installed; build and runtime package inventories remain in the image. The exact resulting image ID remains comparison authority. Regenerate it with `uv pip compile requirements.in --python-platform aarch64-manylinux_2_36 --python-version 3.12 --only-binary :all: --generate-hashes --no-header --no-annotate --output-file requirements.lock`. Debian runtime-library resolution stays inside the image and is recorded in `/opt/spiling-eval/os-packages.txt`; Python packages are recorded alongside it. Rebuilds can resolve different Debian updates. Capture the resulting image ID in `image.json`; copy that image with `docker image save`/`load` when comparing machines rather than assuming two rebuilds are identical.

Do not expose a Docker TCP endpoint. A normal SSH command invoking the launcher is sufficient; no API server, ports or agent service is installed. A committed private brain snapshot may live elsewhere under the prefix; it is never mounted in the evaluator.

## Frozen case

Each new input directory under `PREFIX/inputs/` contains:

- `case.json`: `schema: 1`, a `files` map of exact SHA-256 hashes, `physical` and provenance.
- `matrix.npz`: a finite symmetric positive-definite FP64 CSR system after declared fixture elimination/scaling.
- `vectors.npz`: FP64 `rhs` and independently captured `reference` vectors.
- For `physical: true`, `stiffness.npz` retains original interleaved XYZ stiffness; `fixture.npz` retains `free`, `fixed`, `values`, `scaling`, `loaded` and scalar positive `delta` in millimetres. The solution restores as `u[free] = scaling * x`, `u[fixed] = values`.

Retain native source/actual-replay hashes, material assumptions, mesh/fixture descriptions and evaluator identities in provenance. Keep inputs unchanged and read-only after capture. Successful solver residual alone does not qualify an unvalidated material or fixture. Reference capture and geometry qualification remain the originating evaluator's responsibility.

The reader checks inventory, hashes, shape, finite values, symmetry and reference residual. It compares the computed solution, residual and quadratic energy in FP64. Physical cases additionally reconstruct original displacement and independently check free-force residual, fixture reaction, global force balance and boundary-work/strain-energy agreement. Nonconvergence or absent CUDA fails; it never becomes a zero-stiffness or successful fallback result.

## Finite actual-interior jobs

Schema2 `actual_interior_coupon` inputs contain `case.json` and hashed `roads.json` captured by the ordinary native CLI/engine. The manifest binds candidate, source, actual-replay identity, evaluator source/dependency hashes and explicit coupon/material/refinement scenarios. `--backend coupon` constructs conforming nominal-deposition prisms and solves independent frictionless XYZ compression with explicitly selected PyMetis-ordered FP64 SuperLU or elasticity-aware smoothed-aggregation/CG. Choose `campaign pack --solver amg` for the latter; there is no silent solver fallback. Physical residual, force and energy gates remain unchanged. Completed nonspanning directions retain a witnessed zero; mesh/solver/physical-gate failures retain failures. This is not whole-part strength, buckling, bond-failure or calibrated material qualification. All object material and motion costs remain from fresh complete native replay.

The public `campaign capture/context/pack/dispatch` commands separate sequential native production from independent remote numerical work. Use one frozen image across hosts. Completed per-job `trials.jsonl` and launch receipts are retrieved; outputs from different material/refinement/cut scenarios must be reported separately. The old `scipy`/`warp` frozen-matrix comparison remains available and does not inherit a measured GPU speedup from the coupon CPU path.

## Run and retrieve

```sh
python3 ~/src/spiling-eval/tooling/RECIPE/launch.py \
  --root ~/src/spiling-eval run \
  --input ~/src/spiling-eval/inputs/CASE \
  --run-id case-cpu --backend scipy --seconds 300
```

Use a new run ID and `--backend warp` for the GPU. Optional `--memory-gib` and `--cpus` can lower or explicitly select resources within the measured host envelope. Defaults use 60% of total RAM bounded by 75% of currently available RAM, and 75% of affinity-visible CPU cores. These are host-adaptive research headroom policies, not product limits. A prefix admits one build/job at a time. Multiple prefixes on one host require explicit CPU/RAM allocations whose combined bounds leave host headroom; automatic defaults apply to a single worker, not an admission scheduler. Set `--blas-threads` independently within the allocated CPU envelope when benchmarking solver threading. Compare independent candidates across both hosts before attempting distributed solves.

Each job uses an immutable image ID, non-root UID, read-only root/input, no network, dropped capabilities, a bounded temporary filesystem, CPU/PID/RAM limits and no extra swap. Writable mounts are only its output and shared kernel cache. Docker's RAM accounting is not a proven hard bound on all driver-managed GPU allocations; GPU working sets need separate measurement. The baseline uses small fixed allocations.

`runs/ID/` retains launch/host/resource metadata, Docker state, container log and result. `peak_process_rss_kib` excludes driver/device allocations and is not total system/GPU peak. Cold setup/transfer/JIT and solve times are separate; do not turn this smoke into a performance claim. Copy results back over SSH/SCP. Deadlines, errors and handled HUP/TERM/INT signals remove the owned container; cleanup failure is itself an error. An SSH disconnect is not a completion receipt: reconnect and inspect the run before retrying. The remote deadline remains responsible for its lifetime. GPU kernels compile only in the prefix cache or temporary filesystem.

## Cleanup

Inspect owned containers with `docker ps -a --filter label=org.spiling.purpose=offline-interior-evaluator`. Normal jobs remove themselves. For an interrupted launcher, inspect and remove only the exact named owned container. After retrieving evidence and confirming no owned job is active, remove that prefix and the exact evaluator image IDs/tags recorded in its receipts. Do not use global Docker prune, remove other images, modify the daemon or change host CUDA/driver packages.

Validation: `python -m pytest tools/infill-lab/remote/test_remote.py`, followed by actual CPU/GPU cases and an externally interrupted/deadline job on the intended host. Unit tests cannot establish CUDA or container lifecycle acceptance.

## Independent bending-element qualification

`element_study.py` packs analytical pure-bending/axial controls and the retained
native cube/beam crossing-road controls into an explicit `element_reference`
cohort. It verifies the original native capture manifest before copying inert
roads. `reference.py` exports existing prism topology as C3D6/C3D15, or a
conforming three-tetrahedra subdivision as C3D10; added edge nodes follow node
identity, never coordinate welding. An independent structured C3D8I beam is a
formulation control. None of these elements replaces the existing coupon judge.

The adapter uses an external CalculiX process, not a new FE implementation or a
product dependency. The optional `Dockerfile.reference` installs exact Debian
`calculix-ccx=2.20-1` inside a selected existing evaluator image. Generate a context
with `campaign context`; on the host use `launch.py ... build --context CONTEXT
--reference-base sha256:EXACT_EXISTING_IMAGE_ID`. The launcher verifies inherited
base layers; retain its immutable result ID and copy that image to the other host.
No host packages, drivers, services or network listeners are installed. CalculiX
is separately licensed GPL software; it is not linked into the Spiling application.

```sh
python tools/infill-lab/element_study.py --capture RETAINED_NATIVE_CAPTURE \
  --context FROZEN_CONTEXT --output NEW_INPUTS
python tools/infill-lab/campaign.py dispatch --backend reference \
  --input NEW_INPUTS --output NEW_RESULTS --hosts HOST1 HOST2 \
  --remote-root DEDICATED_PREFIX --run-id element-n1 --seconds 1200 \
  --cpus 15 --memory-gib 64 --blas-threads 1
```

The example resource overrides apply only when the measured host envelope admits
them; ordinary adaptive defaults remain available. Each case retains its exact
input deck, solver output, reaction work, integrated energy/volume and errors.
DAT precision is checked at 1e-5 for these independent output gates; that does not
relax the existing FP64 coupon gates. Child RSS is explicitly a cumulative
high-water mark. The original 5% response/refinement criterion stays separate.
Analytical success alone cannot qualify an actual crossing-road or whole part.

CalculiX C3D6 uses two integration points, whereas the existing Wedge1 judge uses
an explicit full triangle-by-line rule. They describe the same continuum geometry
but are **not identical discrete matrices**. C3D15 uses nine integration points;
C3D10 provides a different quadratic formulation on the preserved solid. Keep
formulation, quadrature, geometry/contact assumptions and refinement in the
record; do not pool their scores. The official CalculiX manual owns these element
and `*ELASTIC`/`*EL PRINT` definitions. This remains perfectly bonded nominal
linear elasticity, with no calibrated strength, buckling or adhesion claim.

Reference runs default to one OMP/BLAS thread. A controlled ARM64 CalculiX 2.20
cohort returned completed but unbalanced reactions/energy with four threads;
same-input single-thread controls passed. This is an environment-specific
reliability observation, not an attribution to a particular solver subsystem.
`--ccx-results-threads`, `--ccx-stiffness-threads` and
`--ccx-equation-threads` permit explicit phase-isolation controls; these are not
admitted parallel optimizations. Thread profiles retain OMP/BLAS, result,
stiffness and equation-solver overrides; the report refuses to pool them. Keep
failed runs and deliberate thread controls separate from candidate comparisons.
`--extra-divisions 4` (or `4 8`) packs additional planar refinements with explicit
1800-second per-solve budgets. The report checks the finest required pair, even
when that solve failed, and retains the entire panel's failures.

`element_report.py --input INPUTS --results RESULTS --output REPORT.json
--plot convergence.png` retains failed/missing cases and exports a diagnostic of
accepted responses. This is element convergence, not an infill strength chart.

For the Debian 2.20 ARM64 cohort, isolated controls with a single equation-solver
thread and four assembly/result threads match the single-thread reference;
result-only and result-plus-stiffness restrictions still reject other cases.
This narrows the investigation to the parallel equation-solve path for those
controls. It is not universal solver qualification or a reason to change the
single-thread default. Retain solver logs, failures and exact complete panels.
The official reference is the [CalculiX manual](https://www.dhondt.de/ccx_2.22.pdf);
its element rules and environment controls do not replace testing the exact
installed executable/image.

## Scalable quadratic reference

`quadratic_study.py` binds a TetP2 panel to the exact retained C3D10 case/result
bytes. `quadratic.py` consumes the same conforming tetrahedral split and node
identities as the external reference. scikit-fem supplies the quadratic basis
and exact affine-tetrahedron quadrature; bounded sparse chunks assemble its
matrix without retaining global basis/gradient arrays. The existing SciPy/PyAMG
solver uses all corner/edge displacement DOFs, explicit rigid-body candidates,
and the unchanged physical/scaled residual, reaction and energy checks. There
is no fallback to a cheaper element, different contact model or looser tolerance.

Build an optional image with `launch.py ... build --context CONTEXT
--quadratic-base sha256:EXACT_EXISTING_IMAGE_ID`. This recipe copies only reviewed
source into the exact existing scientific image: no packages, compiler, host
software or drivers are added. Copy the resulting immutable image to the other
host and verify its ID before dispatch. The earlier CalculiX cohort stays frozen.

```sh
python tools/infill-lab/quadratic_study.py --inputs FROZEN_CCX_INPUTS \
  --results RETAINED_CCX_RESULTS --context FROZEN_CONTEXT --output NEW_INPUTS \
  --solvers amg --divisions 1 2 4 8
python tools/infill-lab/campaign.py dispatch --backend quadratic \
  --input NEW_INPUTS --output NEW_RESULTS --hosts HOST1 HOST2 \
  --remote-root DEDICATED_PREFIX --run-id quadratic --seconds 1800 \
  --blas-threads 1
python tools/infill-lab/quadratic_report.py --input NEW_INPUTS \
  --results NEW_RESULTS --output REPORT.json
```

Reference-parity cases require the original component inventory and selected
response within 1e-5 (external DAT precision), alongside the stricter existing
FP64 solve checks. Cases lacking an external response are explicitly refinement
work, not external positives. Missing or rejected fine cases remain required.
Admit the largest cases from observed matrix/AMG memory, with external CPU/RAM/PID
and wall limits; unused physical RAM is not a reason to shrink the geometry.

`--solvers superlu_mmd amg` permits a controlled same-matrix cost pair. The report
only provides a speed ratio for matching image/thread policy, host and resource
limits, with agreeing complete numerical responses. Order, background work and
single-case scope still belong in the evidence. Numerical parity and a refined
TetP2 response do not by themselves qualify independent-formulation convergence,
whole-part behavior, buckling, calibrated interfaces or physical strength.
