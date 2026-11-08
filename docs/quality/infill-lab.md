<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Interior construction research

[`tools/infill-lab`](../../tools/infill-lab) is offline Linux research tooling. It proposes inert process edits through the ordinary CLI, compiles with the engine, saves and independently replays actual emitted programs. The production checked SPLR decoder supplies the deposition roads. It does not own a planner, kernel, project writer or workbench physics engine.

## Run and resume

Install `uv` and the repository's pinned Node toolchain. Use `pnpm build:native` to build matching engine/CLI binaries under the normal [resource discipline](development.md), then copy them into a private, immutable experiment bin directory. Never replace shared build outputs during a campaign. The optional research dependencies install only through `infill:lab`, outside product builds and distributions. `uv.lock` freezes Python dependencies; `SPILING_UV` can select a local uv executable.

```sh
pnpm infill:lab test
systemd-run --user --wait --pipe --collect \
  --property=WorkingDirectory="$PWD" \
  --property=MemoryMax=12G --property=MemorySwapMax=0 --property=TasksMax=512 \
  /usr/bin/env OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=1 RAYON_NUM_THREADS=1 \
  pnpm infill:lab study --output .cache/interior-study --bin-dir .cache/interior-bin \
  --limit 48 --seconds 900 --trial-seconds 120
pnpm infill:lab report --campaign .cache/interior-study/campaigns/CAMPAIGN_ID \
  --output .cache/interior-report
```

The memory/task values are example externally enforced whole-process bounds, not machine requirements or product worker limits. Select limits from available machine resources, retaining room for concurrent work. The runner requires finite cgroup-v2 memory/tasks limits, executes native trials sequentially and kills its owned process group on deadline/output overflow. No shared Cargo or Git mutation occurs in a trial.

Each campaign freezes evaluator/dependency/producer/intent hashes, material, specimen, mesh, phase, case and seed. Editing them starts another campaign; running again with unchanged inputs resumes the append-only, fsynced trial log. A writer lock prevents two campaign drivers. Failed/refused/timed-out trials remain visible. Successful native artifacts have checked file hashes; interrupted incomplete cache entries fail rather than becoming successful reused data. Candidate JSONL can supply an explicit comparison set with `--candidates FILE`; subsequent trials mutate sampled Pareto candidates. This bounded search is not a claim of superiority to random search or a global optimum.

## Evaluator and scoring boundary

`geometry.py` forms local unions of actual nominal XY footprints over each actual physical interval. Retained mesh cells are fully inside those unions. Only face-connected cells share degrees of freedom; a distant bead contact never ties a local gap. Identical footprint masks may be reused after exact XY identity and interval membership checks. A separate inscribed-stadium diagnostic exposes the sensitivity to idealized bead/contact shape.

`mechanics.py` uses scikit-fem full-integration Q1 elasticity with SciPy/PyAMG. Assumed homogeneous PLA and perfect nominal bonds predict **elastic stiffness only**, not strength, buckling, crushing, fracture, layer adhesion, sag or physical printability. The default is isotropic. `--z-ratio` optionally weakens Z modulus and XZ/YZ shear for an explicitly assumed transversely isotropic sensitivity scenario; reciprocal, positive-definite compliance is checked. Neither scenario is measured layer adhesion. Analytic axial units/energy and the optional independent `oracle.py` CalculiX C3D8 checks qualify the numerical solver, not the material model. CalculiX is externally installed and not distributed with Spiling. A nominal footprint continuum is not a measured deposited bead; its volume stays distinct from the independently verified extrusion dose.

Raw Pareto axes minimize commanded deposited material and feed-based motion time while maximizing stiffness for the declared fixture. Solid-fill and tuned ordinary controls are mandatory comparison inputs. Material uses actual verification totals, including startup prime; time includes stationary retraction/recovery but excludes heating, acceleration and firmware delays. A ratio alone cannot hide an absolutely weak result or make solid plastic win without paying for its material/time. Galleries draw actual replayed paths and explicitly cut away roads for inspection.

Bending fixture datums and imposed strain bind the unchanged source dimensions, not grid-dependent bounding boxes. `--case compression_envelope` retains three independent directional compression results and scores their minimum; it is not simultaneous hydrostatic pressure. The same mesh/material matrix is assembled once for those loads, with separate boundary conditions, residuals and reactions. Matrix reuse across changed meshes/materials is rejected.

Before recommending a construction, repeat promising comparisons with finer cells, shifted mesh phase, material/contact sensitivity and held-out sources/load cases. Separate numerical uncertainty from model uncertainty. A load-specific simulated improvement does not establish a general-purpose interior algorithm. Physical validation requires a captured printer/material cohort and measured stiffness/failure tests; no research ranking grants manufacturing authority.
