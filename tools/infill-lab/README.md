<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Interior research

Candidate deposition comes from **Spiling's Rust planner**, through the ordinary engine/CLI, independent emitted-program replay and source-absent saved return/export. Python coordinates experiments, runs numerical evaluation and renders reports; it must not construct, replace or mutate candidate roads. Fixed STEP test specimens may be authored by offline CAD tooling before native import. They are inputs, not a second infill generator.

## Source-edit experiments

`algorithm_loop.py` freezes the specimen/recipe panel, Rust compiler and all public source outside its explicit Rust-generator allowlist. During scored iterations edit only:

- `crates/manufacturing/src/planner/region/web_roof.rs`
- `crates/manufacturing/src/planner/region/web_roof/architecture.rs`
- `crates/manufacturing/src/planner/region/web_roof/joints.rs`

The verifier, contracts, source fixtures, numerical evaluators, capture/decoder tools and dependency locks remain frozen. New topology may require a separately reviewed construction-contract/verifier extension **before freezing a new cohort**. A generator edit outside the frozen native contract earns an explicit refusal, not a fabricated successful construction or a changed judge. The controller chooses each next hypothesis from prior outcomes; this is not an autonomous proposal model or a parameter-only sweep.

Freeze from a committed baseline; only the allowed generator files may have working edits. The baseline commit plus the archived Rust snapshots must reconstruct every frozen source file. Use the locked Python environment (`uv sync --extra fixtures --extra test`) and pinned Rust toolchain. Keep the cohort outside the checkout or in ignored `.cache`. For example, a JSONL panel can contain `{"pattern":"web_roof","fraction":0.05}` and `{"pattern":"web_roof","fraction":0.16}`.

```sh
python tools/infill-lab/algorithm_loop.py --root "$COHORT" freeze \
  --candidates "$PANEL" --specimens cube48 beam80 panel48
python tools/infill-lab/algorithm_loop.py --root "$COHORT" prepare control \
  --hypothesis 'Reproduce the native control'
# Run inside an externally enforced memory/task scope; --jobs is a reservation.
python tools/infill-lab/algorithm_loop.py --root "$COHORT" run control --jobs 2
# Edit only the declared Rust generator files. Run before the next source edit.
python tools/infill-lab/algorithm_loop.py --root "$COHORT" prepare candidate \
  --parent control --hypothesis 'Describe the construction intervention'
python tools/infill-lab/algorithm_loop.py --root "$COHORT" run candidate --jobs 2
```

Preparation snapshots every allowed Rust file with its hash, parent and hypothesis. Run requires that exact snapshot and unchanged frozen source, builds engine/CLI with the locked graph, copies immutable binaries and records their hashes, then executes the complete frozen native panel. It checks source identity again after build and capture. A concurrent edit stops acceptance; it never resets, stashes or overwrites another checkout. Build failures, missing/failed captures and native refusals remain named outcomes with `native_replayed=false`. Completed trials cannot be overwritten.

A successful native trial means compile, fresh independent replay, Save, removal of owned external inputs, a fresh engine reopen/replay and exact four-file export. It is **not** a mechanical score. Use the existing `campaign.py pack` / frozen evaluator dispatch and collection commands on `trials/NAME/capture` for finite-coupon mechanics (`cathedral.py pack` separately requires a declared one-source cube panel and its automatic native control); keep its native manifests and code lineage with that separate numerical cohort. Freeze exact physical crops, assumed law, solver, mesh and image IDs before comparing. Never splice Python-generated roads into a native capture or transfer a verified flag to modified geometry.

## Evaluation and promotion

Report directional elastic stiffness alongside any geometric mean, complete-object material, and explicitly labeled motion costs. Failed/OOM/timed-out solves remain unknown. Preserve complete flange/cap extents inside every crop; a clipped rib cannot earn a full-rib score. Any positive measured tradeoff may enter the Pareto set; no minimum percentage gate applies. Numerical sensitivity, held-out shape/load response, buckling, interlayer failure, whole-part response and physical prints remain separate qualification work.

Resource limits protect the host; they are not infill design objectives. Choose external reservations from current machine headroom and measured trials. When a candidate hits an internal count limit, check its expansion and actual program/replay bytes before changing the construction. Keep the failure, fix incorrect accounting separately, and freeze a new cohort after a limit or judge change. Do not reward a design for avoiding an artificially small budget or report a resource refusal as poor structural performance.

Numerical jobs consume inert hashed deposition in exact existing Docker images; they execute no candidate code and install no host packages or services. Retain external CPU/RAM/task/time bounds and combined live reservations. Native builds/captures remain sequential and independently bounded.

The superseded Python source-edit campaign is retained in its immutable archive and published Git history. `algorithm_report.py` and its historical judges can reproduce that frozen report; their derived rows are not native verification evidence and cannot supply future generation. There is no current Python candidate mutator.

Promotion requires the reviewed Rust implementation, appropriate producer identity, independent emitted-program replay, source-absent saved return/export, complete costs and held-out native geometry. Attractive coupon results alone do not justify a production default or strength claim. Production authoring remains typed Rust/protobuf; JSON here is inert experiment interchange and archived output.
