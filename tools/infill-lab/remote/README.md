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

Use a new run ID and `--backend warp` for the GPU. Optional `--memory-gib` and `--cpus` can lower or explicitly select resources within the measured host envelope. Defaults use 60% of total RAM bounded by 75% of currently available RAM, and 75% of affinity-visible CPU cores. These are host-adaptive research headroom policies, not product limits. One prefix lock permits one build/job at a time; keep one worker prefix per host. Compare two independent candidates across two hosts before attempting distributed solves.

Each job uses an immutable image ID, non-root UID, read-only root/input, no network, dropped capabilities, a bounded temporary filesystem, CPU/PID/RAM limits and no extra swap. Writable mounts are only its output and shared kernel cache. Docker's RAM accounting is not a proven hard bound on all driver-managed GPU allocations; GPU working sets need separate measurement. The baseline uses small fixed allocations.

`runs/ID/` retains launch/host/resource metadata, Docker state, container log and result. `peak_process_rss_kib` excludes driver/device allocations and is not total system/GPU peak. Cold setup/transfer/JIT and solve times are separate; do not turn this smoke into a performance claim. Copy results back over SSH/SCP. Deadlines, errors and handled HUP/TERM/INT signals remove the owned container; cleanup failure is itself an error. An SSH disconnect is not a completion receipt: reconnect and inspect the run before retrying. The remote deadline remains responsible for its lifetime. GPU kernels compile only in the prefix cache or temporary filesystem.

## Cleanup

Inspect owned containers with `docker ps -a --filter label=org.spiling.purpose=offline-interior-evaluator`. Normal jobs remove themselves. For an interrupted launcher, inspect and remove only the exact named owned container. After retrieving evidence and confirming no owned job is active, remove that prefix and the exact evaluator image IDs/tags recorded in its receipts. Do not use global Docker prune, remove other images, modify the daemon or change host CUDA/driver packages.

Validation: `python -m pytest tools/infill-lab/remote/test_remote.py`, followed by actual CPU/GPU cases and an externally interrupted/deadline job on the intended host. Unit tests cannot establish CUDA or container lifecycle acceptance.
