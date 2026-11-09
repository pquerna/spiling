<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Interior research

Offline studies consume the engine's actual deposition. They do not provide a second product planner, calibration, strength certification or machine readiness. Production pattern/architecture authoring remains typed Rust/protobuf; JSON here is inert experiment interchange and archived numeric output.

## Source-edit experiments

`algorithm_loop.py` freezes an evaluator cohort, snapshots proposals, performs local admission, dispatches bounded Docker jobs over SSH, and collects append-only outcomes. During scored iterations, edit **only `interior_algorithm.py`**. The human/agent selects the next hypothesis from the last result; this is not an autonomous proposal model or a parameter-only sweep. `algorithm_report.py` reports one cohort and refuses incompatible judge identities.

The current judges are deliberately specific: a 48mm native cube, 0.45mm roads, retained outer walls/skins, and either a complete lower chamber/finite piers or a declared roof crop. They are not general geometry or support authorities. Preserve complete flange/cap extents inside the crop. A cropped rib cannot earn a full-rib score. Costs charge the complete object; mechanical responses apply only to the declared finite coupon.

Use the locked Python environment (`uv sync --extra fixtures --extra test`), an exact built engine/CLI pair, and the existing `campaign.py capture` / `cathedral.py pack` commands to prepare native roads and an explicit scenario. Then:

```sh
python algorithm_loop.py --root "$COHORT" freeze \
  --hosts "$EVALUATOR_HOST" --remote-root "$REMOTE_PREFIX" \
  --image "$EXACT_IMAGE_ID" --baseline "$ROADS" \
  --context "$REVIEWED_CONTEXT" --scenario "$SCENARIO"
# Edit interior_algorithm.py; evaluator files remain frozen.
python algorithm_loop.py --root "$COHORT" prepare identity --hypothesis 'Reproduce the native control'
python algorithm_loop.py --root "$COHORT" prepare joint_ribs --parent identity --hypothesis 'Add short flanges at intersections'
python algorithm_loop.py --root "$COHORT" run identity joint_ribs --seconds 600
python algorithm_report.py --cohort "$COHORT" --control identity --output "$REPORT"
```

Run each `prepare` immediately after its corresponding source edit. Preparation executes trusted local Python with a timeout; it is not a sandbox. Apply external memory/task bounds to the controller. Frozen code/hash records and inert sparse rows travel together, but candidate code never executes in the numerical container. Remote jobs require existing Docker, disable networking, mount inputs read-only, and bound CPU/RAM/tasks/time. They create no network service or host package installation. Dispatch currently reserves up to two 40GiB/3CPU jobs per host, after checking live headroom; the physical/numerical scenario is independent of that reservation.

`--kind roof` selects the subset-only roof judge. `--roof-bounds` must include the changed corbels and actual top skin. The explicit TetGen adapter preserves oriented boundary identity, physical material containment and face connectivity before mechanics. `max_source_prisms` is a frozen source-mesh resource limit; it never relaxes boundary, residual, force-balance or energy checks. A judge fix starts a new cohort with paired controls. Archive the prior failures; never rewrite them as successful results.

The report uses normalized **elastic stiffness**, complete deposited material and an explicitly labeled motion proxy. Keep individual fixture responses beside the geometric mean. Failed/OOM/timed-out solves remain unknown. Any positive measured tradeoff may enter the Pareto set; no minimum percentage gate applies. Numerical sensitivity, whole-part response, buckling, interlayer failure and physical prints are separate qualification work.

Promotion requires a Rust implementation through the existing native planner/realization boundaries, appropriate producer identity, exact unchanged geometry where claimed, independent emitted-program replay, source-absent save/reopen/export, and held-out native shapes. Attractive or cheap coupon geometry alone does not justify a production default.
