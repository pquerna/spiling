<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Software-only manufacturing evidence

## Physical qualification gate — not yet exercised

Successful compilation, preflight, independent replay and export establish software
properties only. A complete Marlin program is not permission to execute it.
No printer, measured material calibration or supervised physical measurements are
available in the recorded evidence. Profile configuration/calibration hashes bind
declared bytes; they do not certify the contents or a machine.

Before a physical evaluation, freeze and retain:

| Input | Required record |
| --- | --- |
| Machine and firmware | Exact machine/extruder/nozzle identity, firmware build and complete configuration; coordinate/homing/endstop conventions, usable envelope, extrusion mode, leveling state and temperature limits |
| Process configuration | Ordinary print-cooling fan mapping with laser synchronization disabled; permitted externally commanded retraction, E-feed limits, prime/park clearance and permitted Z-hop |
| Material calibration | Material/batch, filament diameter, extrusion calibration, temperatures, measured flow ceiling, first-layer settings, cooling, retraction and bridge-span settings with procedures and raw measurements |
| Software and geometry | Spiling commit/kernel pin, exact inert intent, source hashes, normalized plan, actual program, fresh independent report and immutable export/provenance hashes |
| Evaluation criteria | Operator-approved dimensional/first-layer/bridge/support-removal criteria and tolerances fixed before printing; fixture identities, repetitions, instruments and measurement procedure |

The supervised sequence is machine/configuration inspection, first-layer and
extrusion calibration, dimensional box and protected-hole coupons, sparse-roof
and bounded bridge coupons, then a cantilever/support-interface coupon. Retain
failures as well as successes, photographs and raw dimensional measurements.
Compare emitted object, support, prime and recovered-retraction accounting
separately; recovered filament is not newly deposited material. Verify shutdown,
fan behavior, travel/hop clearance and absence of thermal/extrusion faults under
the actual firmware. Do not loosen frozen criteria after a failed coupon.

Fan and extrusion interpretations must match the configured firmware, not merely
command spelling: [Marlin linear moves](https://marlinfw.org/docs/gcode/G000-G001.html)
use filament-axis position and modal feed, while [Marlin fan control](https://marlinfw.org/docs/gcode/M106.html)
affects the next queued motion and can control a laser in an incompatible
configuration. Replayed fan state is commanded state, not measured airflow.
Component metadata and nominal bead coverage do not establish collision freedom
or swept-bead physics. Physical qualification remains open until the frozen
configuration passes supervised measurements; the workbench has no execution
connector and must not infer permission from a verified report.

## Spatial image relief — project 4 / surface 3 / planner 6

**Software workflow gate passed:** genuine PNG import and source-local U/Z
mapping on native planar and full-cylinder carriers, immutable captured images,
ordinary walls/fill/skin/support, G-code and independent actual-program replay,
history/save, source deletion, stored-image remapping and fresh read-only
preview/export. Physical printer, GPU/picking/dialog and packaged-platform gates
remain open. No arbitrary freeform carrier, brush/material painting, fuzzy skin,
helical/crossed texture, branching support or fin planner is claimed.

This implementation is based on public main `3a4011a`; its reproducible source
is the commit containing this section. The accepted
[mini-RFC](../adr/003-spatial-image-relief.md) preceded implementation and was
iterated against independent review and actual failures. Environment:
Linux7.0.14-23-pve x64, Rust1.99.0 (`b940084d7`,2026-09-28), Node24.15.0,
pnpm10.32.1. Monstertruck remains
`5a4ddb7582c3d7f75a39e800a9fef345cdfe435e`. Current identities are project4,
surface3, manufacturing5, planner6, emitter/verifier4 and SPLR3. Old formats
have no compatibility reader.

### Frozen inputs and procedure

| Input | SHA-256 |
|---|---|
| `fixtures/geometry/box-mm.step` | `1b23042e12183748e12bc4e0195ae546c80296ce4170709eb3f836dce2f4ac98` |
| `fixtures/geometry/independent-exporters/occt-cylinder-mm.step` | `21ea8ea395b4dd38d04dbe901d2538dd781f6567b404031148302164c85a8a9a` |
| `fixtures/manufacturing/image-relief.pattern.json` | `a2bc3a7ece26cdcb15f9819148d78de1df9ca75014b418ec8d857ad279eace0b` |
| `fixtures/manufacturing/solid-fill.intent.json` | `f0638ecdadb8b50e85af8af4c8c8da26aaa61d9e6972ceee67de14f853448d4f` |
| `fixtures/manufacturing/complete-sparse.intent.json` | `b91e7054907216e4ea6ecaf472cadeea950fb20e642586d4be15815893918c77` |

The CEF runner encodes the checked-in8×8 pattern as a genuine PNG. Canonical
Gray8 PGM is75 bytes, SHA-256
`c33953fe2846c40a4d81b0c84a9eca0fba830575592e4c689aecd5befb16da99`;
source PNG SHA-256 is
`5437a77302079c7223693deb4ac71d261f6fb3029b84f048057afe8b1d04aae0`.
Mapping is depth0.35mm, U repeat1/phase0.125, Z period7.3mm/phase0,
vertical error0.05mm, planar margin1mm or cylinder margin0.

Run the resource-capped native/CEF procedures in
[development.md](development.md). Actual CLI integration cases are
`planar_image_relief_survives_source_deletion_and_fresh_readonly_retrieval`
and `cylindrical_image_relief_survives_source_deletion_and_fresh_readonly_retrieval`
in `apps/cli/tests/project.rs` (ignored real-child scenarios, explicitly run).
Their unchanged demanding4×32 asymmetric PNG uses depth0.08mm, U repeat2/
phase0.137, Z period3.7mm/phase0.213 and error0.01mm; rows include a black
interval and a narrow bright band. Both workflows passed. The final cylinder
case took68.87s, peak84.6MiB.

Actual CEF `image-relief-planar` and `image-relief` scenarios exercised the
production controls/native bridge, canonical Canvas pixels, Undo/Redo, PNG
deletion followed by stored-image remapping, Compile, fresh ReportAndMotion,
cancellation, save/export, STEP deletion, read-only reopen, engine restart and
native-window shutdown. The planar diagnostic passed in17.017s/857.5MiB.
The final cylinder diagnostic passed in63.295s/672.7MiB using owned Linux app
data; native close exited both application and engine. The fresh read-only
canonical image and disabled mutation controls were visually inspected.
CEF used explicit unsandboxed/SwiftShader/no-GPU diagnostic settings. GTK/DBus
and GPU-process-exit11 messages were present; this proves the actual panel and
2D Canvas workflow, not hardware rendering, picking, dialogs or normal-sandbox
packaging.

### Observed final cylindrical CEF output

| Property | Observed value |
|---|---|
| Immutable bundle SHA-256 | `7f8a2e88de239f465512f48d56f8f5ac6700173e32e415e46c067de0c781723f` |
| Bundle bytes | 2223740 |
| Layers / paths / object deposition segments | 44 /425 /17600 |
| Nominal object volume | 343.65968622023036mm³ |
| Filament | 142.87709467mm |
| Replay events / packed bytes | 18813 /903120 |
| Maximum decoded position / extrusion error | 7.011189039420439e-7mm /9.966481183745768e-9mm |
| Nominal surface witnesses | 10850 ambiguous;0 resolved/below-resolution/unavailable; coverage complete |

These are one captured artifact's values, not universal deterministic hashes
across new project IDs or native sampling choices. Ambiguous surface witnesses
are honest bounded-error advisories, not physical feature-survival certificates.

### Geometry, support and interface proof

`steep_image_support_succeeds_from_real_bed_corridor_and_lower_image_rebuild_blocks_it`
passed through public native compilation and independent replay for both solid
box and protected-hole/sparse objects: disabled support refuses; enabled support
deposits real bed-anchored Support and SupportInterface; changing only lower
image rows blocks the previously usable upper corridor. Final run127.54s,
peak8.7MiB. Parameters remain depth5mm, protected margin1.2mm,2×16 image,
Z period32mm/phase1.4mm, vertical error0.8mm, nominal height/bead0.6mm,
support density1, XY/Z gaps0.2mm and interface0.8mm. All relevant lower D
participates; no last-N-mm shortcut or native-M anchor substitution exists.
Z gap remains a conservative minimum, not an exact physical attachment claim.

Focused properties passed for periodic seam roundoff, PNG/canonical limits,
source retention/protected holes/actual native cylinder chords, asymmetric
phase placement and non-grid vertical bands, requested versus mandatory mixed
image/rib material, physical skin horizons with changing outer rings/holes,
charged range-cache eviction and pinned operands, simultaneous classification
scratch, balanced event scheduling/decimal shifted-bed arithmetic, and shared
support-role transitions. The three new private support raster properties
passed; existing native cantilever/separate-volume, exact floating contact-gap
and blocked-plus-valid-component regressions also passed. The obsolete floating
test's fixed first-object-layer0.4mm grid pin was deleted; physical interface
height0.8mm and contact gap0.2mm checks remain unchanged.

Actual engine cases passed for atomic image history/read-only retrieval,
error/cancel/stale non-mutation, canonical-budget admission, and
`spatial_image_compilation_captures_self_contained_content_and_fresh_replay`
(2.70s actual-child runtime). Core's initial71-case run found two failures;
both were fixed and their targeted regressions passed. Contract generation,
generated drift checking, protocol canonical-image checks, desktop frontend
build and native desktop build were exercised successfully. No full workspace,
release or packaging sweep was run.

### Failures corrected without weakening gates

- Cylindrical boundary proof and repeated skin horizons exhausted cumulative
  work. Exact boundary broad-phase admission, padded Boolean candidate accounting
  and immutable final-D aligned-range reuse remove redundant work; the20M cap
  is unchanged.
- Cache retention and classified scratch exhausted staging. Reclaim only
  unpinned optional ranges, retain/count live operands, freeze the support cache
  pool and include all live support/query/classification geometry exactly once.
  Geometry/segment limits remain unchanged.
- A global nominal-grid/event union created needless thin slabs and zero E8
  extrusion. Balanced subdivision of genuine event intervals removes those
  artificial slivers; exact events, native minimum thickness, XYZ6/E8 precision,
  strict bead height and independent positive-deposition checks remain intact.
- Independent support/interface erosion erased a0.425mm role strip and left a
  real15.924mm unanchored stroke. Erode physical S once, assign centerline roles
  across I without fake seam walls, and prove staged axes against actual prior
  deposition. The same native box/hole fixture now passes without changing its
  parameters or bridge/clearance/anchor limits.
- Mixed image/rib admission had accidentally made ribs on another face advisory.
  Required C now preserves native M plus every non-image finish addition; only
  image-added sub-bead detail is advisory, with unchanged reachability.
- Tiny negative periodic coordinates rounded to the wrapped image extent.
  Modular integer-node addressing fixes seam indexing without clamping the field.
- Vec geometric growth and retained source-history expectations failed Core
  limits; exact admission/capacity handling and real pinned-history refusal were
  corrected, not bypassed.
- CEF's exact baseline-area assertion compared different certified native sample
  planes. The oracle now uses the declared source-circle sampling certificate
  and invariant captured source accounting, not bitwise cross-sample geometry.
- Repeated CEF runs filled the operator's existing128-operation durable ledger.
  Linux smoke runs now isolate owned app data while preserving intra-run restart
  history; the operator ledger and admission cap were not cleared or increased.

Native commands ran serially with one Cargo/Rayon/test worker, enforced3GiB
RAM/128MiB swap/64 tasks; CEF separately used1536MiB/128MiB/256 tasks.
Largest observed build peak was1.3GiB (native desktop); no OOM was observed.
Final warm target10797400KiB (10.30GiB) remains below the11GiB ceiling;
filesystem headroom was1706MiB before owned failed-smoke cleanup. No target
clean, second target, release build or unrelated data deletion was needed.

## Historical cylindrical checkpoint `3a4011a` — surface 2 / planner 5

**Software gate passed:** native full-cylinder face authoring through actual CLI
and CEF, ordinary manufacturing, fresh independent actual-program replay,
history, durable save/source removal/read-only reopen and immutable export.
This checkpoint was based on public main `0b8a09b` and recorded in `3a4011a`.
Rust 1.99.0 (`b940084d7`), Linux x64 7.0.14-23-pve, Node24.15.0 and
pnpm10.32.1 were exercised. Monstertruck was pinned to
`5a4ddb7582c3d7f75a39e800a9fef345cdfe435e`. Checkpoint identities were project3,
surface2, manufacturing4, planner5, emitter/verifier4 and SPLR3; its measured
results below are historical, not current schema support claims.

The [reviewed mini-RFC](../adr/002-cylindrical-grip-ribs.md) preceded code and
was revised after independent geometry/data-flow review. This is not an
exporter/face-number recognizer: the importer retains generic native
face/wire/oriented-coedge analytic references before modeled circles become
NURBS. Planar and cylindrical construction share one allocation-free periodic
profile, robust material-retention/simplicity proofs and a private prepared
carrier boundary. One realized material domain feeds ordinary walls,
fill/skins, anchor/support reasoning, emission and independent verification.
The first cylindrical consumer remains deliberately bounded to a complete
outward, full-height cylinder in an upright bed-anchored invariant extrusion.
Split/partial/recessed faces, bores, unknown identities and unsafe combinations
refuse; no helical/crossed knurling, arbitrary painting or physical grip claim.

Frozen inputs, unchanged during evaluation:

| Input | SHA-256 |
| --- | --- |
| `geometry/independent-exporters/occt-cylinder-mm.step` — admitted full face | `21ea8ea395b4dd38d04dbe901d2538dd781f6567b404031148302164c85a8a9a` |
| `geometry/cylinder.step` — two native half-faces, explicit refusal | `95df1da444dc9c49529054f1825c9013c135301df677e30269c3a789ee2b464b` |
| `manufacturing/solid-fill.intent.json` — actual CLI | `99a5033a65020d206b673b70a572c41bcd60850610426e0215187ec52d2ddb29` |
| `manufacturing/complete-sparse.intent.json` — actual CEF | `ef2f2cb07d36f9a92917c5a3125b8103145c69b02efb8615e3eaee3cef38f176` |

Focused properties passed: five contract, six Core, two client and eleven
selected geometry cases; eleven native manufacturing cases; two new Boolean
admission cases. Evidence covers typed parameter/storage/history boundaries,
native ownership/corruption refusal, translated/phased/radius/axis variations,
three-/seven-part native rim subdivisions, short-step refusal, native polygon
inclusion, exact protected holes, unequal-radius edge derivative/chord bounds,
copied cyclic closure, phase/seam independence, ordinary first-wall survival,
actual replay and bounded precision/work/cancellation failures. Synthetic
hole-ring proofs are not an imported annular-cylinder support claim.

The first actual CLI run exposed a production integration failure missed by
the smaller native configuration: the two-wall fill offset's capsule self-union
hit the coarse Boolean bound. A temporary diagnostic observed `region.rs:232`,
314 capsules /14758 points per operand and19998077 accumulated whole-ring pairs
before the next47×47 charge. General edge broad-phase admission now tightens
that conservative estimate without changing the20M work ceiling, material
coordinates, sampling or native/finish/offset tolerances. Input/index growth,
cancellable heapsort, every active scan and intra-/cross-operand candidate are
charged. Independent review caught and closed shared integer-adapter
quantization in comparison boxes. Dense/near-grid refusal and thick-neighborhood
exterior/hole/clearance properties pass. The diagnostic was removed. Separately,
an invalid generated stepped-cylinder test source used the wrong revolution-axis
anchor; its source construction was corrected, not the native admission gate.

Actual CLI tests
`ordered_cylindrical_ribs_compile_history_save_fresh_read_only_motion_and_reveal`
and `ordered_surface_ribs_compile_save_fresh_read_only_export_and_metadata_reveal`
both pass against the rebuilt engine. Cylindrical parameters are depth0.1 mm,
count12 and phase0.25; the formerly failing parameters were retained.
`cylindrical_finish_refuses_split_native_face_without_smooth_fallback` also passes.
The workflow compares actual baseline/finished paths and program bytes, checks
original/realized/added material accounting, then exercises history, checkpoint,
owned source removal, fresh read-only replay/motion/export and engine reaping.

Actual CEF `cylindrical-finish --diagnostic-no-gpu` passed using the production
panel/native bridge and confined native source admission. It inspected the real
radius5 mm carrier (`step:17` in this source), authored occurrence depth0.1 mm /
count8 / phase0, compiled and independently inspected/exported, tested
cancellation/invalidation/history, removed the owned source and reopened
read-only with a fresh engine. Source identity/material remain unchanged.
Measured output:40 layers,395 paths,22195 object deposition segments,
335.23185258662926 mm³ nominal object deposition,23286 replay events and
1117824 packed replay bytes; support deposition was zero. The fresh report
was verified with maximum position error7.000453293648492e-7 mm and maximum
extrusion error9.983345534807231e-9 mm. The native close exited both app and
engine. Actual authoring and fresh-inspection PNGs in
`artifacts/cylindrical-finish-{native-face-authoring,production-panel}.png`
were visually inspected; these are local ignored artifacts, not committed
binary evidence.

One warm target, one Cargo/Rayon/test worker, incremental/debug info disabled;
no release/package/full-workspace sweep. Native gates used 3 GiB RAM / 128 MiB
swap / 64 tasks; frontend 2 GiB / 128 MiB / 64 tasks; CEF separately 1536 MiB /
128 MiB / 256 tasks. No builds ran concurrently.

| Observed stage | Wall time | Unit memory peak | Unit swap |
| --- | ---: | ---: | ---: |
| Geometry retry + eleven native manufacturing properties | 232.014 s | 440.1 MiB | 0 |
| Two causal/adversarial Boolean properties | 4.386 s | 434.9 MiB | 0 |
| Corrected engine build + both actual CLI workflows | 69.346 s | 505.3 MiB | 0 |
| TypeScript + production Vite build | 1.523 s | 356.1 MiB | 0 |
| Matching native CEF debug build | 16.617 s | 1.2 GiB, systemd-rounded | 0 |
| Actual CEF cylindrical workflow | 54.454 s | 860 MiB | 0 |
| Smoke syntax + `pnpm contracts --check` (no generated drift) | 1.329 s | 141.7 MiB | 0 |

Observed target usage rose from 10430356 to 10551188 KiB, below the 11 GiB
ceiling at each stage check; filesystem headroom finished at 2.0 GiB.
These are stage observations, not a continuous disk-peak measurement.
The CEF run was explicitly unsandboxed software-GPU/no-WebGPU diagnostic
under Xvfb; GPU process failures and absent desktop D-Bus services were visible.
No hardware rendering/picking, real OS dialogs, packaging/platform durability,
machine execution, measured texture/grip or physical printability was established.


## Planar source-face finish checkpoint — surface 1 / planner 4 (historical)

**Historical gate decision: the selected native-face planar-rib workflow passed
in software at `0b8a09b`.** Physical texture/grip, printer qualification,
swept-tool collision, real dialogs, GPU display/picking and distribution/platform
durability remained open. This checkpoint was based on public main `415342e`,
Rust 1.99.0 (`b940084d7`), Linux x64, Node 24.15.0, pnpm 10.32.1 and unchanged
Monstertruck pin `5a4ddb7582c3d7f75a39e800a9fef345cdfe435e`. Its project format
was3, surface schema1, manufacturing/planner/emitter/verifier4 and SPLR3.
Measurements below retain those original identities, not the current cutover.

### Decision and exercised interfaces

Whole-face directional ribs were selected before random fuzzy skin, cylindrical
knurling or image mapping: a source-local constant-Z additive domain can feed
the existing native ownership, skins/infill and actual-anchor/support pipeline,
instead of disguising perimeter jitter as derived geometry. No external texture
implementation/catalog was copied; no speculative crate or inactive texture
channel was introduced.

Core owns bounded ordered finish documents, immutable history, monotonic layer
IDs, source-deletion cascade and save/open/recovery. Typed project RPCs require
current native FaceRef plus persistent ProjectRevision; scene identity and source
BREP remain unchanged. Definition inheritance composes before occurrence
overrides; Smooth/disable/remove reveal lower fills. Compiler capture, input
fingerprints and exported/replayed provenance include the full document.

Native certification admits only a full-height four-line rectangular exterior
supporting wall; finishing additionally requires an upright bed-anchored
invariant extrusion. The additive boundary matches the entire native sampled
carrier chain, including ring wrapping. Original material remains included;
untouched/protected coordinates and hole rings remain exact. New samples satisfy
the source-local waveform and the fixed 0.001 mm construction bound. Original,
realized and added areas are distinct; geometric addition is not extrusion or
measured grip. Expanded exclusions are conservative nominal metadata checks.

### Focused observed checks

| Exercised surface | Result |
| --- | --- |
| Surface/manifest/fingerprint, required area/bound and typed protobuf contracts | 6 PASS |
| Core transaction, cascade, allocator/capacity, persistence/recovery and artifact history | 6 PASS |
| Native face identity, outward side, trim/height/recess/co-carrier refusal, corruption/cancellation | 4 PASS |
| Shared client typed edits and mixed-build readiness rejection | 2 PASS |
| Native ribs, phase/chord bound, protected holes, poses, multi-face/occurrence, scope/limits, actual-anchor/support and verifier gap ownership | 8 PASS |
| Actual engine metadata revision/scene/invalidation and reused/rebuilt native-open targets | 2 PASS |
| Actual CLI create→native face inspection→definition finish→Preflight/Compile/Verify→save/export→delete owned source→fresh read-only replay/export→Smooth/disable/remove | PASS; observed each engine reaped |
| Generated TypeScript consumed by production frontend build | PASS |
| Actual compiled CEF `surface-finish --diagnostic-no-gpu` scenario | PASS; observed native window and engine exit |

Reproduction uses the selective `surface`, `wall`, `native_surface`,
`certified_native_ribs`, `multiple_certified_finishes`, `finished_native_cantilever`,
`support_only_layers` and `contact_gap_layer` filters, engine project/manufacturing
`surface` cases, and the CLI project's explicitly ignored `ordered_surface_ribs`
case with `SPILING_TEST_ENGINE` pointing to the fresh native engine. These are
focused results, not a full-workspace or packaged-release acceptance claim.
The [operator procedure](development.md#source-face-vertical-ribs) uses the same
production controls/services as the actual CEF runner.

### Frozen inputs and real workbench observation

| Input | SHA-256 |
| --- | --- |
| Original native `box-mm.step` | `1b23042e12183748e12bc4e0195ae546c80296ce4170709eb3f836dce2f4ac98` |
| Original protected-hole source | `eb2112f9ef4be523615f237b86b69569026cef8e1cafbc20e2fdca7e3459cb29` |
| Schema-4 diagnostic solid intent | `99a5033a65020d206b673b70a572c41bcd60850610426e0215187ec52d2ddb29` |
| Schema-4 complete sparse intent | `ef2f2cb07d36f9a92917c5a3125b8103145c69b02efb8615e3eaee3cef38f176` |

The actual CEF controls selected occurrence 1 / native face `step:25`, applied an
occurrence layer with depth 0.1 mm, pitch 2 mm, protected margin 1 mm and phase
0 mm, then exercised Undo/Redo with unchanged native FaceRef/material intent.
Preflight, native Compile, fresh independent replay, no-overwrite export,
metadata invalidation/history restoration, save, source-copy deletion, native
restart, read-only reopen/export and cancellation completed. Every layer retained
the native 200 mm² source footprint, positive bounded addition ≤2 mm² and exact
source-plus-addition accounting. Screenshots visually showed the real selected
native controls and disabled read-only controls; no GPU/picking proof is claimed.

| Recorded complete-job observation | Value |
| --- | --- |
| Layers / paths | 40 / 749 |
| Object deposition / travel segments | 14,429 / 977 |
| Object volume / filament | 805.1545255017074 mm³ / 334.74435314 mm |
| Support deposition | 0 |
| Separate prime filament | 1.5 mm |
| Retractions / retracted filament | 114 / 91.20000000000014 mm |
| Actual replay events / packed bytes | 15,638 / 750,720 |
| Bundle bytes / SHA-256 | 1,790,168 / `c1d1bbfc1a9f111cb217aac8c7950d76839448b1d3c52895c893e79947c57499` |
| Input hash at project revision 10 | `5d59c779fd52acf8e3b78105c09e5d2b181c265af0459a22efca42effe176ce6` |

Recorded hashes bind this run's project identity; separately created projects do
not promise identical hashes. Fresh reopen compares immutable outputs within the
same saved project. Complete lifecycle/modal checks are still software-only.

### Corrected failures and resource envelope

The first native compilation failed on unsupported cgmath array conversions;
explicit physical XYZ extraction corrected it. Initial floating finish union
re-quantized untouched protected-hole boundaries; strict retention correctly
refused. Source-preserving sampled-chain construction replaced that union,
including native line subdivisions rather than weakening containment tolerance.
Two existing support/contact-gap fixtures required zero original source area
when their object area is zero.

The actual CLI then exposed an incompatible 0.8 mm margin for its 0.45 mm bead;
the explicit operator preset was corrected to 1 mm without relaxing process
limits. Uniform worst-taper sampling also exhausted the unchanged 20-million
Boolean-pair limit in boundary-capsule construction. Region-local curvature
sampling and exact monotone collinear *construction-primitive* coalescing removed
redundant work; original material arrays, 0.001/0.005 mm bounds, point limits and
the Boolean-pair cap were not weakened. The failing CLI passed afterward; native
retention, phase/chord and support regressions also passed.

Native compilation/checks were sequential, one Cargo/Rayon/test worker, no
incremental/debug variants, under 3 GiB memory / 128 MiB swap / 64-task cgroups.
The cached debug desktop build peaked at 1.4 GiB (1.6 MiB swap), without CEF
download, release build or packaging. CEF ran separately under 1.5 GiB / 128 MiB
swap / 256 tasks: 17.568 s, 873.2 MiB peak, zero swap. Upstream GTK/D-Bus and
headless GPU-process warnings were retained; explicit diagnostic attachment did
not substitute a renderer or establish hardware support. The actual CLI smoke
completed in 11.40 s after compilation.

Authorized workspace-only cache cleanup recovered 2.3 GiB before work. The final
warm target measured 10 GiB, below the 11 GiB goal, with 2.1 GiB free disk. No
installer, release/benchmark duplicate or additional native distribution was built.



## Schema-3 operator workflow software evidence

**Historical gate decision: the frozen schema-3 software operator workflow passed;
physical and GPU qualification remained open.** At that checkpoint,
manufacturing/planner/emitter/verifier and SPLR were version 3; project storage
was format 2. Native-performance and workbench proofs below retain their
published kernel `5a4ddb7582c3d7f75a39e800a9fef345cdfe435e` and original versions.
Baseline focused checks, box, nonintegral and cantilever observations used
`f6e8a88d08d998970ab17aefd53bd2946408363f`; prepared-native/contact-gap,
shelf and ramp observations used `4dd2890a20b27b489c0f6d32490d90bb02727bc1`.
Those earlier measurements retain their original pins and are not current
performance claims. Older schema-2 and desktop measurements are historical.

### Interfaces and scope

The full-profile loader accepts inert regular JSON bounded to 32 KiB through
single-use native tokens, without executing TypeScript in desktop or engine.
Explicit Apply preserves the entire printer declaration. Shared engine-client
axis-permutation bed preparation uses native certified bounds, then the existing
Core SetInstancePose/baseRevision transaction and normal undo/invalidation.
Preflight uses the same durable worker, ledger and ACK path as manufacturing:
it is read-only, revision-bound, historical-result-safe, creates zero artifacts
and mutates neither Core nor scene. Eligibility does not include native Compile
analysis or physical qualification.

Actual-height scheduling, physical skin horizons and explicit process/support
policy feed native planning. Support/interface material and contact-gap layers
are separate from object ownership; gap height is not deposited bead height.
Independent emitted-byte verification binds process behavior and accounting.
SPLR3 carries bridge/support/interface roles and stationary XYZ retract/unretract
with actual absolute E, not fabricated travel; commanded fan state is not motion
or measured cooling. Object, support, prime and retracted/recovered filament are
separate. Fresh read-only verification and exclusive export do not trust stored
verification flags. Reproduction uses the [ordered CLI workflow](development.md#software-only-manufacturing);
all output paths must be new, and source-deletion exercises remove only owned
copies, never frozen originals.

### Frozen focused checks

| Exercised surface | Observed result |
| --- | --- |
| Contracts | 52 PASS |
| Protocol, viewport and desktop TypeScript consumers | All three checks PASS |
| Desktop manufacturing state | 4 PASS |
| Core manufacturing | 12 PASS |
| Independent verifier | 29 PASS |
| Packed TypeScript consumers after actual native SPLR3 golden | 174 PASS |
| Planner/support, excluding already-tested slow tilt | 32 PASS |
| Actual-engine durable Preflight | 2 PASS |

The verifier proof includes the actual exact-E endpoint large-E counterexample:
it failed before correction and passed afterward. The actual-engine Preflight
proof covers durable/historical reports, nonmutation, queued cancellation and
stale inputs. Counts are focused observations, not a full-workspace acceptance
claim. Native prisms initially had reversed winding in the original generator;
construction was corrected rather than changing numerical tolerances. Section
sampling remains 0.005 mm, boolean tolerance 1e-5 and plane tolerance 1e-4.

| Corrected native source | SHA-256 |
| --- | --- |
| Nonintegral prism | `322c486eece35cdd306e530b089e99eb060f92948cf48b29d27775ef3575d263` |
| Shelf prism | `dc8bdd90518575bdaf3c51f49935201617d177b38033180ce92a884e593655ff` |
| Ramp prism | `d8aa2bcb2baa178dfaf886f8cfcd1af2d4aa61741b105606da222ceaac8048e9` |

### Actual CLI observations

The complete-box run imported the original source, posed occurrence 1 at
`[15,25,2]` mm, applied full intent, then centered it on the bed at `[100,105,0]`.
Preflight was eligible at input revision 4 with unchanged Core/scene state.
Compile, independent verification, SPLR3 inspection, save and exclusive export
completed. Trusted authoring produced 3,767 bytes of JSON, SHA-256
`1a68d731e89a5332d260a15b81e9d4f3821ed4f16f00de4fc61f7f7aedd31e28`.
Declared configuration hash was
`c46cf9a35f2916b3109dbbae2a06dc0cb212d4fe611b511bf1c72220bbcc147c`;
calibration hash was
`322df893a20557507671a732334464bebf7164d9c547e6ef09eea356e99b3a8b`,
with `physical_measurements=[]`.

| Observation | Complete box | Nonintegral source |
| --- | ---: | ---: |
| Scheduled layers | 40 | 42 |
| Object deposition segments | 3,432 | 3,329 |
| Object deposited volume, mm³ | 882.4183774865178 | 863.2515508721464 |
| Object filament, mm | 366.86693003 | 358.89828951 |
| Prime filament, mm (separate) | 1.5 | 1.5 |
| Retractions / retracted filament, mm | 119 / 95.2 | 122 / 97.6 |
| Replay events | 4,544 | 4,546 |
| Elapsed / peak memory | 1.931 s / 13.1 MiB | 1.929 s / 13.3 MiB |

The box run had 238 process events and used zero swap. The nonintegral source was 7.85 mm high with explicit
first-layer flow 1.05 and bridge flow 0.95. Its first layer was 0.2 mm; its last
actual height was `0.049999999999998934` mm at Z=7.85 mm.
After deleting only the owned source copy with the frozen nonintegral hash,
a fresh read-only process completed Preflight, verification and export in
1.306 s / 12.2 MiB. Actual `cmp` established byte identity of all four immutable
files and SPLR. All three reported engine PIDs returned `ESRCH` to `kill(pid,0)`
after reaping; transient manifests/session/timing metadata are not identity
comparands.

| Nonintegral immutable output | SHA-256 |
| --- | --- |
| `plan.json` | `3c0e53eaf5c2b8e347d92e9529b1c5123a6ef7add5851c5016a7458a101f5df2` |
| `program.gcode` | `db673300bf75f78ae4018bce04a00a54c8a59d3b3a2f3bb9130f3aad8d3ce6d5` |
| `verification.json` | `568922de6adf38cc65e248f4f1fb1fc5ec435d8196b61873199365e5f866a876` |
| `provenance.json` | `8c05ad6dc7ead871e6e8843dd8833b8159b307fa8056c831ba36a3cc1d363619` |
| Replay SPLR | `b3925f71bc7b235934b24c65e3294c50da6f141c5187504dadf55e7624f5a18c` |

### Sparse cantilever and actual packed replay

The same authored complete-sparse profile compiled the original box at
`[40,40,0]` and a shared occurrence at `[50,40,8]` mm, independently verified,
saved and exported successfully. No density, skin, width, endpoint tolerance or
bridge-span relaxation was used to make this operator case pass. Runtime was
2.629 s, peak memory 21.8 MiB, zero swap; the child process was observed reaped.

| Property | Observed value |
|---|---|
| Schedule layers / paths | 80 / 1,590 |
| Object segments / volume / filament | 6,826 / 1,677.0133951476005 mm³ / 697.2211499600002 mm |
| Support segments / volume / filament | 481 / 367.7842359157383 mm³ / 152.9069169299998 mm |
| Separate prime / retractions | 1.5 mm / 422 |
| Actual replay events / stationary feeder events | 10,589 / 844 |
| Program SHA-256 | `e57731f431ce87dcd625d81aef096207afb935b60fd321c13ddc86a7083f003a` |
| Bundle SHA-256 | `2ac5ef3104585ed3d12c053e9a86fe48749871824e6e00e74ed0d1138a820d8c` |

The actual box, nonintegral and cantilever SPLR bytes passed the production
TypeScript decoder and packed viewport constructor. Cantilever transport retained
694 bridge, 426 support and 55 interface events, with retract/unretract represented
by stationary Points rather than invented XYZ segments. Travel visibility left
feeder events visible; final-layer selection and owned-buffer admission agreed.
The capped smoke used 125.7 MiB peak, zero swap and 745 ms runtime. This is CPU
construction evidence, not physical GPU/visual acceptance.

### Prepared-native and durable contact-gap proof

Published kernel `4dd2890a20b27b489c0f6d32490d90bb02727bc1` preserves original
BREP curve parameters and coedge provenance through bounded, cancellable native
plane clipping. Prepared source seeds borrow immutable definition geometry;
job-wide retained source bytes and independent query/intermediate quotas are
explicit. They are derived acceleration data, never replacement geometry or
display-mesh manufacturing input. Native focused coverage passed 38 library,
one interval-coverage, five preparation-limit and two strict native-parameter-cut
cases. Five original tilted-section cases passed without tolerance changes.
The source exact-fit quota/cancellation regression also passed.

The actual floating box at `[40,40,2]` mm, non-grid shelf and inward sloped prism
used the same full complete-sparse intent, shared bed preparation where requested,
read-only Preflight, native Compile, independently decoded program verification,
save and exclusive export. All runs used zero swap and remained software-only.

| Actual CLI property | Floating box | Non-grid shelf | Inward ramp |
| --- | ---: | ---: | ---: |
| Scheduled layers / paths | 50 / 807 | 45 / 738 | 42 / 665 |
| Object deposition segments | 3,432 | 2,331 | 3,658 |
| Object deposited volume, mm³ | 882.4183774865178 | 728.6029779488283 | 790.769379820963 |
| Support segments / volume, mm³ | 202 / 230.33379795276542 | 0 / 0 | 0 / 0 |
| Retractions | 145 | 133 | 115 |
| Runtime / peak memory | 1.793 s / 13.7 MiB | 1.415 s / 11.2 MiB | 1.848 s / 13.7 MiB |

The floating plan has support interface at Z=1.8 mm, an empty contact-gap layer
at Z=2.0 mm and first object deposition at Z=2.2 mm. The whole-plan validator,
not the individual layer decoder, authorizes that internal gap from explicit
support policy and surrounding ownership. Its boundary regression passed.
Actual durable admission initially failed because ordinary JSON parsing changed
support volume from `230.33379795276542` to `230.33379795276545`. Enabling the
workspace's `serde_json/float_roundtrip` fixed exact stored-record identity;
Core equality was not weakened. The exact-total admission/one-ULP changed-record
rejection regression passed.

A fresh read-only engine verified and exported the floating job in 1.404 s /
13 MiB, zero swap. Actual `cmp` proved byte identity of all four immutable files
and SPLR. Bundle SHA-256 is
`8156a61c71d01c06bc37532f2d8ad996ab10b67b5e9bcd1bd563a5b717645f5a`;
program SHA-256 is
`6d117e2cf1c862f7ab9b0a6ff8ff6703965b62a772db3431b800b1662273b498`.
The actual production TypeScript decoder admitted 5,025 events, including 303
bridge, 93 support, 109 interface and 290 stationary feeder events. The packed
viewport constructor rendered zero vertices for gap layer 9 and retained the
adjacent interface/object layers. This disposable CPU smoke passed in 669 ms /
118.3 MiB, zero swap; it does not establish GPU rendering or physical support
behavior. The script was removed.

The build target was measured at 10.86 GiB before cleanup. Removing obsolete
owned test/profiling executables, while preserving current engine/CLI binaries,
dependency caches and CEF, reduced it to 9.97 GiB with 2.52 GiB free disk. No
blanket clean, release build, packaging or dependency/runtime download was used
to obtain these CLI proofs.


### Current native-performance and workbench software gate

Published kernel `5a4ddb7582c3d7f75a39e800a9fef345cdfe435e` retains prepared
native clipping and preserves adaptive curve probes, accepted parameter/point
order and arithmetic while appending into one output allocation. Compact
surface position/first-derivative evaluation preserves existing custom batch
semantics and removes the coupled projection solver's repeated 32×32 scratch
matrices. Original five tilted-section cases, exact sampling and compact/custom
surface arithmetic regressions passed. Native verification used Rust 1.99.0;
the fork's floating nightly currently fails in cached `branches` before project
code because that dependency calls removed `core::intrinsics::abort`. No
dependency or toolchain workaround is presented as a passed nightly gate.

The original 1° tilted cylinder now completes native Compile, independent
emitted-byte verification, save, exclusive four-file export and production
TypeScript SPLR admission within the unchanged engine deadline. Source,
pose, recipe, sampling/Boolean/plane tolerances and deadline were not weakened.
The typed frozen recipe is
[`tilted-cylinder-complete.intent.ts`](../../fixtures/manufacturing/native/tilted-cylinder-complete.intent.ts);
its generated inert JSON is 3,534 bytes, SHA-256
`67833eac365badbc73b0664fdf16e9970302c057a9bdc34d02a868916d6025ff`,
and was structurally compared against the actual compiled full input.
The exact source/pose/cap-sample provenance is retained in the
[native corpus rules](../../fixtures/manufacturing/native/AGENT-RULES.md).

A signed finite-edge buffer had produced three islands for the native cap,
including two approximately zero-area fragments; incremental capsule unions
then introduced a sub-grid perimeter edge. Neither was accepted by weakening
thin-feature/path checks or pruning fragments. Object offsets now uniformly
subtract/union the boundary's disk neighborhood, using bounded convex segment
capsules and one shared-coordinate-frame union. Only construction primitives
are hulled; authoritative rings remain unchanged. The retained native-cap
regression checks one material island, protected topology and bead-center
clearance within the declared offset bound. Complementary skin intersections
use original operands rather than re-quantizing rounded masks; the strict
`1e-7` area-partition invariant remains unchanged, including growing floors.

| Published-pin CLI observation | Tilted cylinder | Floating support/contact gap |
| --- | ---: | ---: |
| Scheduled layers / paths | 21 / 394 | 50 / 807 |
| Object deposition segments | 3,009 | 872 |
| Object deposited volume, mm³ | 613.4708430886585 | 882.4183778713631 |
| Support segments / volume, mm³ | 0 / 0 | 202 / 230.33379795276542 |
| Actual replay events | 3,407 | 2,465 |
| Runtime / peak memory | 58.469 s / 15.1 MiB | 1.653 s / 10.5 MiB |

Both runs used zero swap. Tilted program SHA-256 is
`cc07c44d8f22f7ca5150ed60060c8c34b846fd7f052cba3e86feb81e9e5b7fbf`;
bundle SHA-256 is
`b4f37e7b9dff5ea4c01a064e01be7ce3457679c0a8b090608202e03ce48f9707`.
Fresh floating read-only inspection completed in 1.025 s / 9.8 MiB; byte
comparison proved identical four-file exports and SPLR. Production TypeScript
admitted both actual payloads and confirmed interface Z=1.8 mm, zero deposition
in contact-gap layer 9 and object Z=2.2 mm. The final geometry/ownership group
passed 17 tests; deposited-anchor/whole-row boundaries passed seven. Scoped
all-target clippy passed for manufacturing, engine, engine-client and CLI with
`-D warnings`; no full-workspace test claim is implied.

The real production CEF workbench passed `tools/smoke-desktop.mjs --scenario
manufacturing --diagnostic-no-gpu`, using the compiled native shell and current
engine rather than a browser mock. Native inert-profile loading, malformed-input
retention, shared bed preparation, read-only Preflight, sparse/skin Compile,
fresh Verify/replay/export, exclusive no-overwrite rejection, undo/historical
invalidation, source-copy deletion and read-only reopen, cancellation, restart
and native window-close/engine reaping all passed. The run took 9.014 s,
471.6 MiB peak and zero swap. Its actual report verified 40 layers, 809 object
segments, 130 retract/recover pairs and 1,902 packed events. The captured
production panel was inspected; diagnostic mode explicitly reported no GPU
display readiness. Ordinary restart retained the product GPU admission gate.

Canonical `pnpm build` also passed with cached CEF
`152.3.0+152.0.6`, one Cargo job and a 3 GiB process cap: 49.008 s,
2.2 GiB peak, zero swap. Native/Node/CEF checks ran sequentially with enforced
memory, task and time limits; no release/package build or CEF download was used.
After final native qualification, the build target measured 10.90 GiB with
1.52 GiB free disk; obsolete owned diagnostic executables/source/local-patch
scaffolding were removed without deleting current binaries or dependency caches.
These are software/interface proofs on this Linux host, not GPU-visible/pickable,
native-dialog, platform durability, packaging, firmware or physical printer
qualification. No measured calibration or supervised physical prints are
available; all programs remain **not machine ready**.

## Historical schema-2 complete-job and planar-region software proof

At merged main `0b4a14f`, manufacturing data and SPLR were schema 2; planner/emitter/verifier provenance was version 2. The measurements in this section establish that historical software gate, not the current schema-3 contract. Machine lifecycle, native material classification, Core storage and client display remained separate owners. Diagnostic bodies remained a distinct workflow; complete Marlin jobs declared exact configuration/calibration record hashes without claiming measurements.

The focused proof below used the previous exact published kernel `d87b4d9ced1f3baf31aa771ac0e7c663efb1c001`, independently of the periodic-edge kernel work. Trusted TypeScript authoring of `complete-sparse.intent.ts` produced 3,147 bytes, SHA-256 `d37102c5c8c2528d41d81d3384d30e466e8a700829764645878b1ed0e7a49b43`. The actual CLI imported the original native box, compiled complete Marlin startup/waits/prime/object/lift/park/shutdown with 25% sparse interior and 0.8 mm top/bottom skins, independently inspected actual motion, saved and exclusively exported. A fresh read-only engine independently replayed the stored job.

| Complete sparse-box observation             | Result                                  |
| ------------------------------------------- | --------------------------------------- |
| Layers / object paths / deposition segments | 40 / 632 / 3,432                        |
| Object volume / filament                    | 882.4183774865178 mm³ / 366.86693003 mm |
| Separate priming / lifecycle events         | 1.5 mm filament / 4                     |
| Actual replay events / bytes                | 4,068 / 195,360                         |
| Compile/replay/save/export runtime / peak   | 1.420 s / 12.2 MiB; zero swap           |
| Fresh read-only runtime / peak              | 1.134 s / 11.3 MiB; zero swap           |

The four immutable export files and packed replay were byte-identical across reopen. Both reported engine PIDs were reaped. The public TypeScript decoder admitted the actual native bytes and observed layer-free prime/lift/park, endpoint-only unknown first start, known first object travel, and perimeter/bottom-skin/sparse-fill/top-skin roles. No homing trajectory was fabricated.

A disposable smoke projected those actual bytes through the production packed renderer: layer filtering hid lifecycle, travel hiding retained priming/deposition, and the empty event prefix drew no marker/segments. Accounted retained CPU/raw bytes were 309,248 and GPU-buffer bytes 97,620; the Node process peaked at 51.1 MiB under a 512 MiB cap with zero swap. This is CPU construction/visibility proof, not a GPU upload, rendered-motion or picking claim. The disposable script was removed.

Focused manufacturing coverage passed 37 distinct behavioral tests across corrected targeted runs; the native golden refresh is separate. Coverage includes conservative physical skin horizons, local roofs/floors, protected holes/islands, density extremes, shared-layout equivalence, independent nominal-volume binding, complete-job thermal/modal/order mutations, rounding-aware object exclusion, barriers and parser/resource/cancellation boundaries. A newly exposed empty-operand overlay re-quantization changed nominal partition area; preserving mathematical empty-set identities removed the unnecessary computation without weakening the original absolute partition assertion. Independent analytic area and offset-sagitta obligations remain.

The actual-engine complete sparse source-deletion/fresh-read-only case passed, as did stale/cancelled/resource-limited no-publication behavior. Existing Core coverage passed 42 tests; contract coverage passed 45 across full and corrected targeted runs. Protocol/viewport/desktop type checks and the two workbench state cases passed. Native builds were sequential under 3 GiB memory, 128 MiB swap and 64-task limits, one Cargo/Rayon worker and disk-backed scratch storage; the matching CLI workload used a 1 GiB/64 MiB cap. The Go-backed author type checker defaults to one worker after task exhaustion was observed; limits were not raised. No release, packaging or blanket workspace rerun establishes support.

The refreshed schema-2 native rounding/coincident-travel golden passed its Rust generation contract; the independent TypeScript packed suite passed all 171 cases, including 30 replay admission cases. The first whole-suite invocation used concurrent Node test children under a 32-task cap and reported one failed test file without an assertion diagnostic; direct replay execution passed, then `--test-concurrency=1` passed the complete suite at 52.1 MiB with zero swap. No test expectation was weakened.

The real production CEF panel was opened through its local debugging endpoint after the explicit no-GPU diagnostic attachment. Visible controls admitted the original native box, selected the reference draft explicitly, applied 0.6 mm top skin / 0.4 mm bottom skin / 25% infill, compiled and independently inspected actual transported motion. The fresh report was verified with 40 layers, 569 paths, 3,369 object deposition events, 3,938 total replay events / 189,120 bytes, object volume 815.9341319315249 mm³ and object filament 339.22599271 mm. The displayed workflow was explicitly diagnostic body-only, lifecycle/prime totals were zero, and `display_ready` stayed false. Changing layer/cursor retained the same bundle/revision and exposed actual source-line-6 first travel `[0.225,0.225,0.2]` mm with unknown start and zero E. Screenshots confirmed the skin/infill editor. Explicit discard/Stop reaped the owned engine PID. This proves native-panel controls and coordinate inspection, not WebGPU motion/picking, dialogs or hardware qualification.

Canonical `pnpm build` staged the matching engine before embedding the frontend and building the desktop, reusing cached CEF without packaging or downloading it. The earlier build peaked at 1.6 GiB with zero swap; the final exact-kernel build after private interface factoring peaked at 1.5 GiB. The exact Git-pinned native replay/executable build peaked at 941.2 MiB. The completed kernel investigation's owned 1.6 GiB build cache was removed; Spiling retained one warm 9.8 GiB target with 3.0 GiB free disk. All observed runs used zero swap.

### Final exact-kernel three-part integration

The final exact published kernel is `f6e8a88d08d998970ab17aefd53bd2946408363f`.
The actual CLI imported unchanged independent OCCT box/cylinder/through-hole
sources, preserving 6/3/7 source faces, posed the cylinder at `[40,20,0]` mm and
hole at `[80,0,0]` mm, applied the same explicit complete/sparse intent, and
compiled, independently replayed, saved and exclusively exported. This includes
the original failing first-layer `z=0.1` native cut, not a substituted midplane.
After deleting only the owned operator-source copies, a fresh read-only engine
reopened persisted sources/program and independently verified/exported again.

| Final independent OCCT complete-job observation   | Result                                         |
| ------------------------------------------------- | ---------------------------------------------- |
| Layers / object paths / deposition segments       | 40 / 2,256 / 30,536                            |
| Object volume / filament                          | 2,827.375123713008 mm³ / 1,175.48598051 mm     |
| Separate priming / lifecycle events               | 1.5 mm filament / 4                            |
| Replay events / bytes                             | 32,796 / 1,574,304                             |
| Compile/replay/save/export runtime / peak         | 38.705 s / 53.9 MiB                            |
| Fresh source-independent read-only runtime / peak | 5.425 s / 47.4 MiB                             |
| Nominal section / protected-hole area             | 650.2553349444219 mm² / 28.247784358680462 mm² |
| Interior intentional-void area at 25% density     | 364.1298861737432 mm²                          |

All four immutable export files and packed replay were byte-identical.
Actual program SHA-256 is
`58f742c75f15d98c1fdd0a88e609f3124fc245edcaa22c897ea540bd4bb23bfa`;
packed replay SHA-256 is
`8b7b40084e2d4fa211e41125a7e8e86f536ec2ec8bcd52d3aafae9b93ee2f991`.
Bundle/input hashes are
`fc199f8e140b0f333e0396ffcbaf8589107ef2b57b2c4559b1373e78dcd2fd1d` /
`5826f847fa32df8d64c04014fd20c10489e13e0a34e87a2503e7dc46ed93a128`.
The independent public TypeScript decoder admitted seven actual roles:
travel/perimeter/bottom skin/top skin/sparse/lifecycle travel/prime. All 40 layers
preserved the existing ownership partition bound and protected hole; the 0.8 mm
skin horizon occupied four bottom and four top layers, with bottom precedence.
Intentional void is a reported subset of sparse ownership, not another material
partition or protected source hole. Both native child PIDs were reaped.

The final real CEF production panel opened that persisted complete-job project
through confined native tokens. Scalar Apply changed top skin to 0.6 mm while
preserving every printer/job/configuration/calibration declaration exactly and
invalidating the old artifact. Core Undo restored the complete intent without
making historical verification fresh. A new compile after private hatch-layer
and independent motion-state factoring produced the same input hash and object
totals; fresh actual-program inspection transported all 32,796 events. Visible
first/last endpoints were `[10,210,0.2]` mm at source line 15 with unknown start and
`[0,210,20]` mm at source line 32,811 with known start and absolute E
1,176.98598051 mm. Neither has an object-layer attribution. Priming remained
separate from object totals. The negotiated UI kernel was the exact published
revision. The explicit no-GPU diagnostic retained the normal startup rejection
and `display_ready=false`; this is not GPU/picking or physical machine evidence.
The owned unsaved smoke changes were discarded, the native child was observed
reaped, and the diagnostic service was stopped; its peak was 621.9 MiB with zero
swap. Readable recipe/report evidence is retained under ignored `artifacts/`.

Selected contracts/core/geometry/manufacturing/engine/CLI/desktop all-target
Clippy passed with warnings denied at 542 MiB peak. Bounded control/native result
oneofs retain the existing allocation-free inline policy rather than adding
per-call boxes. After factoring, the lifecycle modal/thermal/barrier/extrusion
mutation regression, local physical-skin/slab-transition regression and exact
native rounding/coincident-travel golden passed; that focused run peaked at
550.7 MiB. Final-revision TypeScript replay admission passed all 30 cases.
Existing generator `ts-rs` attribute diagnostics and third-party future-compiler
warnings remain visible.

## Prior manufacturing review and actual-program inspection

The manufacturing-workbench change builds on merged main `49a07d2`. The desktop feature and native bridge are separate owning modules; CLI and shell share one exclusive export writer. The engine remains the only intent/planning/verification authority. Report-only inspection retains no display bytes; report-and-motion publishes bounded SPLR schema 1 from the existing independent actual-command decoder through the same durable operation ledger and ByteStream. The original verified-result resource remains the manufacturing bundle. Descriptor/provenance and resource bindings are documented in [packed replay](../protocol/replay.md).

Focused verification passed: real native motion read-only publication, cancellation and corrupt-transfer boundaries; three actual CLI manufacturing cases; six shared exclusive-output/ownership cases; packed corruption, strict wire/provenance validation and the 200,000-event sink boundary. The native actual-rounding/coincident-travel fixture uses fixed identity and checks golden drift. All TypeScript consumers typechecked, the production frontend bundled, 167 protocol tests passed, and the regenerated replay fixture passed its 26 decoder cases. Two workbench state cases cover unchanged/failed drafts and session/content/reopen freshness. A throwaway renderer smoke exercised the actual packed module's endpoint-only first travel, empty prefix, travel toggle, precision rejection and disposal without claiming GPU execution. Generated-contract drift passed without another compilation.

The real CLI imported the frozen independent OCCT box, applied the explicit reference intent, compiled, freshly inspected with motion and exclusively exported. After removing only the owned operator source, a fresh read-only process reopened the saved project and produced identical replay bytes and four immutable export files. The 615,931-byte bundle reported 40 layers, 1,400 paths and 3,920 deposition segments. Circular independent-exporter rejection remains explicit; both cylinder and through-hole now expose the native identical-vertex closed-edge failure rather than a generic manifold message. No admission widening, seam repair or fallback was introduced.

The actual CEF production-panel scenario passed explicit selection/Apply, unchanged suppression, failed-draft retention, scalar editing, Core Undo/Redo, compile, fresh report-only/motion inspection, cancellation, actual endpoint controls, unchanged-checkpoint freshness, exclusive four-file export/no-overwrite, content invalidation, source-independent read-only reopen/export and fresh-session child restart/reaping. It used the real panel/native adapter and independently validated transported bytes, not API mocks.

| Native CEF panel observation                 | Result                                                         |
| -------------------------------------------- | -------------------------------------------------------------- |
| Compiled bundle                              | 647,544 bytes                                                  |
| Layers / paths / deposition segments         | 40 / 1,400 / 4,200                                             |
| Actual replay events / bytes                 | 5,600 / 268,896                                                |
| First actual endpoint                        | `[0.225, 0.225, 0.2]` mm; source line 7; travel; start unknown |
| First actual feed                            | 6,000 mm/min                                                   |
| Passing whole-scenario runtime / cgroup peak | 8.825 s / 453 MiB; zero swap                                   |

Native workloads were sequential under 3 GiB memory / 256 MiB swap limits, with one Cargo job and one Rayon worker; CEF used a separately chosen 256-task allowance. The matching expanded CEF runtime was reused, not downloaded or rebuilt. The largest observed build peak was 911.9 MiB with zero swap. Development/test profiles now disable debug symbols and incremental caches; obsolete build variants were removed while preserving current dependency/runtime caches. No release build, installer, benchmark or blanket workspace test rerun was performed.

The first desktop check exposed a missing Tauri `Manager` import; the corrected bridge compiled. Initial CEF probes exposed stale embedded frontend assets and an old staged pipe-protocol sidecar copied over the current gRPC executable; current frontend and matching sidecar staging corrected those integration prerequisites. The scenario then exposed a checker race when a clean checkpoint still had a pending Save; it now waits for completed Save/control readiness before reopen rather than treating an already-clean label as completion.

CEF evidence deliberately injected GPU unavailability, checked normal startup rejection, then used an explicitly startup-gated native diagnostic attachment. `display_ready` remained false. The actual panel screenshot is under ignored `artifacts/manufacturing-production-panel.png`. This establishes native-panel software behavior, not rendered motion, picking, native-dialog interaction, normal sandboxing, physical hardware/printer support or packaging certification. Existing GTK/D-Bus/CEF diagnostics remain visible. The physical and independent circular-part gates remain open.

## Prior authenticated gRPC integration

The integration branch is based on main `4ac3d97`, not the earlier pipe-framed checkpoint. Typed Geometry, Projects and Manufacturing RPCs use the existing authenticated engine, single durable SQLite operation ledger, standard Google Operations and immutable ByteStream resources. No protocol-v4 compatibility shim or second public scheduler remains. Saved project format 2 and packed SPLT/SPLM/SPLS version 1 remain separately versioned contracts.

The current real engine/CLI run exercised all three native fixtures **after the per-island erosion correction**. Source/build identity is the commit containing this record, with the exact Monstertruck fork pin below. A new project imported copied box/cylinder/hole sources, posed the cylinder at `[40,20,0]` mm and hole at `[80,0,0]` mm, applied the committed explicit intent, compiled, independently replayed, saved and exported. The operator-source directory was then deleted. A fresh read-only process reopened persisted sources/bundle, independently replayed again and exported to a new directory.

| Current gRPC observation                           | Recorded result                               |
| -------------------------------------------------- | --------------------------------------------- |
| Compile, replay, save and export                   | 27.614 s                                      |
| Fresh source-independent reopen, replay and export | 5.162 s                                       |
| Compact stored bundle                              | 4,458,908 bytes, retrieved through ByteStream |
| Layers / paths / deposition segments               | 40 / 5,040 / 34,040                           |
| Travel moves                                       | 5,040                                         |
| Nominal deposited volume                           | 5,122.252442416237 mm³                        |
| Filament length                                    | 2,129.58509969 mm                             |
| Maximum replay position error                      | 6.932172994209306e-7 mm                       |
| Maximum replay extrusion error                     | 9.95061000090558e-9 mm                        |
| Whole scenario cgroup peak                         | 73.7 MiB; zero swap                           |

Current stored bundle SHA-256: `56f361b7789c496178bfb28130126c0ed4c31d2e12ed38ae4b82865b9ab0ef5a`. Captured provenance and all four immutable export files were byte-identical after fresh reopen; the exported verification exactly matched fresh replay. Both reported engine PIDs had already been reaped. Transient completion manifests were not compared. Owned scratch directories were removed.

Verification reused disk-backed `/root/spiling/target` with one Cargo job, no incremental/debug information, single-threaded tests and one Rayon worker for multi-engine/scenario runs. Native commands used an externally enforced 3 GiB memory / 256 MiB swap / 64-task cgroup. This is observed scenario evidence, not a cold/warm benchmark or an allocator-level kernel guarantee.

Current environment: Linux `7.0.14-23-pve` x86_64, Rust `1.99.0` (`b940084d7`, 2026-09-28), Node `24.15.0` and pnpm `10.32.1`. The host has approximately 8 GiB RAM and 512 MiB swap; the lower per-command cgroup limits above were enforced.

Current TypeScript checks and frontend build passed. All 141 TypeScript packed-payload tests passed, and the existing contract generator's `--check` passed without another build. Trusted TypeScript authoring produced the exact 2,002-byte intent fixture, rejected nonfinite output and refused an existing destination without modifying it.

Current Rust behavioral coverage passed **236 distinct tests**, completed in targeted passes rather than repeating already-passed suites. All actual-child ignored cases were explicitly included.

| Current exercised Rust area                               | Passing tests |
| --------------------------------------------------------- | ------------: |
| CLI unit and actual-child workflows                       |            26 |
| Contracts and protobuf/domain invariants                  |            49 |
| Core transactions/storage/recovery                        |            42 |
| Engine ledger, child, geometry, project and manufacturing |            46 |
| Shared client, including real-child cases                 |            13 |
| Native geometry corpus/admission                          |            34 |
| Manufacturing planning/emission/independent replay        |            20 |
| Native shell token/route invariants                       |             6 |

Integration regressions passed for sticky cancellation between dispatcher read and progress publication, saturated control-queue ACK/output ownership, queued-save project-revision changes and failed committed-save receipt persistence after reconnect. Old pipe-era tests were migrated to durable accepted-operation failures while retaining exact no-mutation/read-only/stale/reference invariants; obsolete capability-string assertions were removed rather than re-pinned. A multi-engine run initially exhausted the 64-task limit while initializing independent default Rayon pools; the affected writer/reader/contender scenario passed with `RAYON_NUM_THREADS=1`, without lifting the task or memory limit. Focused native verification peaked at 795.1 MiB with zero swap across initial and corrected passes; the final missing-coverage pass succeeded at 565.1 MiB.

The final cached workspace `cargo clippy --locked --offline --workspace --all-targets -- -D warnings` passed at 517.5 MiB peak with zero swap. Rustfmt and repository Prettier checks passed. Upstream `ts-rs` attribute-parser and `proc-macro-error2` future-compatibility diagnostics remain visible; this is not a warning-free dependency claim. Bounded inline control enums have narrow, explained size-lint expectations instead of new per-observation boxing allocations. The exact CI `tools/smoke-cli.mjs` runner also passed without rebuilding: authenticated diagnostics/durable operations, cross-language binary rejection, native geometry/sections, software compilation/replay, source-independent project export, non-UTF paths and child cleanup; 37.8 MiB peak, zero swap.

The actual CEF `project-bridge` diagnostic passed native source tokens, import/placement, save, child restart/reopen, persistent face identity, independently decoded 12,720 mesh bytes, read-only rejection and native-window child cleanup. The captured workbench surface reported WebGPU unavailable; GPU admission was deliberately bypassed only for native bridge/storage proof. It is **not visible geometry, picking, dialogs, hardware WebGPU or packaging certification**. Sandbox and software-GPU diagnostics were explicitly enabled; upstream GPU-process/GTK/D-Bus diagnostics remained visible. Peak memory was 703.2 MiB with zero swap. One desktop bridge build reused the cached matching CEF 152.0.6 native distribution and Rust bindings 152.3.0+152.0.6; no installer or release build was run.

Main's actual CEF `engine-operations` diagnostic also passed partial packed-byte retrieval, cancellation without killing the engine, interrupted-operation recovery after a reaped/restarted child and native-window cleanup. It reused the existing executable and cached CEF distribution; 329 MiB peak, zero swap, 1.921 s. Both CEF diagnostics used a separate 256-task cgroup under the same memory/swap limits, with explicit GPU-admission bypass and no rendering claim.

The injected `gpu-unavailable` CEF startup path passed separately: the workbench displayed the unsupported-WebGPU diagnostic, native status remained `stopped` with no negotiated hello, and native-window close terminated the application. Peak 298.9 MiB, zero swap, 1.121 s. This proves the admission failure behavior, not hardware availability.

No physical printer support, machine readiness or printability is claimed. The schema-3 operator section above governs current software claims; earlier checkpoint measurements below are preserved only as history.

## Historical checkpoint `f3a0898`

### Checkpoint decision and branch boundary

The preserved checkpoint was **not an accepted release or completed manufacturing gate**. It implemented software-only planar manufacturing on the pipe-framed protocol-v4 / project-format-2 authoring branch based on `97f9df1`, before integration with main `4ac3d97` (`Implement durable engine operations with standard Google RPCs`). The measurements in this historical section describe that checkpoint, not the current gRPC integration above.

The operator reported a host failure during verification. The host subsequently rebooted. No compiler, test runner, engine or CEF process remained after restart. Previous-boot kernel journal inspection returned no entries, so an out-of-memory cause is **unconfirmed**. Heavy verification was stopped; the checkpoint was published at the operator's request. The interrupted final workspace run has no confirmed result.

No physical printer support, machine readiness, printability, component collision verification, packaged desktop acceptance or physical WebGPU acceptance is claimed. Programs are labeled `SOFTWARE VALIDATION ONLY, NOT MACHINE READY` and do not initialize firmware, heat, home or execute a machine.

### Implemented software workflow

Explicit printer specifications and recipes are TypeScript-authored inert data against generated Rust contracts. The trusted authoring tool type-checks and executes the source with operator permissions, then creates bounded JSON without overwriting an existing destination. Rust owns semantic validation, native-section planning, emission and mandatory independent replay; the engine does not execute TypeScript profiles.

Core owns intent, transactions, bounded history, artifact invalidation and immutable bundle storage. Compilation uses native sections of admitted world-vertical planar/cylindrical geometry, integral fixed-height layers, controlled offsets and nominal 100% solid fill. Emission and replay are separate implementations. Save stores captured source assets and the derived bundle; fresh-process read-only inspection independently replays the persisted program. CLI export exclusively creates plan, program, fresh verification and provenance files, followed by a completion manifest after observed child cleanup.

See [manufacturing contracts](../protocol/manufacturing.md), [project persistence](../protocol/project.md) and [reproduction commands](development.md#software-only-manufacturing).

### Historical real engine / CLI workflow

The three-part smoke used the original box, cylinder and through-hole fixtures, with the cylinder translated by `[40,20,0]` mm and the hole by `[80,0,0]` mm. The authored recipe used 0.2 mm layers, 0.45 mm bead width and two perimeters. All destinations were exclusively created scratch paths.

| Observation                                                      | Recorded result                                                                     |
| ---------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| Original repeated-section implementation                         | Cancelled after 65.68 s; no successful bundle publication                           |
| Certified vertical-slab reuse implementation                     | Compile, independent replay, save and export completed in 17.70 s                   |
| Fresh read-only process after original source paths were removed | Reopen, independent replay and export completed in 3.16 s                           |
| Compact stored bundle                                            | 4,459,936 bytes; exceeds the 4 MiB single-frame limit and exercises chunk streaming |
| Layers / paths / deposition segments                             | 40 / 5,040 / 34,040                                                                 |
| Travel moves                                                     | 5,040                                                                               |
| Nominal deposited volume                                         | 5,122.252458868365 mm³                                                              |
| Filament length                                                  | 2,129.58510653 mm                                                                   |
| Maximum replay position error                                    | 6.872574256745515e-7 mm                                                             |
| Maximum replay extrusion error                                   | 9.933381925765051e-9 mm                                                             |

The four immutable exported files were byte-identical after fresh reopen. Completion manifests were not compared as semantic identity: they contain transient session, timing and project state. Both observed engine children had exited before the successful reports were consumed.

This smoke **predates the final per-island erosion correction** described below. It proves the exercised workflow at that point, not a fresh three-part smoke of the final checkpoint.

Recorded bundle SHA-256: `5f96dafbf5ff66118270a72c3cf916e2189c41919e3422a015fd7a0379748199`.

### Historical correctness findings and exercised fixes

- Independent replay previously admitted an empty deposition layer. A public-API smoke reproduced admission before the fix and rejection afterward. Typed validation and nested decoding now require paths in every declared layer.
- Nested decoding previously admitted a 200,000-segment document despite the 100,000-segment cumulative limit. The same public-API smoke rejected it after introducing a shared admission budget during point allocation; serialized input was 2,800,270 bytes.
- After 32 imports and 32 undos, setting manufacturing intent previously left discarded redo-history sources pinned in the native registry, causing the next import to fail with `ResourceLimit`. The actual CLI reproduction succeeded after synchronizing pins at commit; its final state retained one definition and occurrence 33. The corresponding actual-engine regression passed.
- The complete workspace run before the final erosion correction failed in `native_holes_cylinders_and_repeated_occurrences_compile_without_display_rebuild`. Two separated native circles buffered together produced two valid centers plus two spurious triangles, each approximately `4.973799150320701e-14` mm². Self-union retained those fragments. Independently eroding each source island produced exactly two centers without discarding contours or changing tolerance. Negative offsets now operate per disconnected island; positive offsets retain aggregate semantics. The same actual-engine regression passed afterward: **1 passed, 4 filtered**, 32.13 s. Thin-region and topology rejection remain in place.

### Checkpoint verification record and outstanding checks

Before the final erosion correction, the contracts/core/manufacturing test run passed **96 tests**, including 42 core and 20 manufacturing tests. Workspace Clippy with warnings denied passed; TypeScript checks passed; TypeScript tests passed **141 tests**; generated-contract drift checking passed. The trusted authoring command produced the committed intent bytes, rejected nonfinite output and refused an existing output destination. These observations are not a final all-checks pass of the published checkpoint.

The earlier complete Rust workspace run failed on the repeated-circle regression described above. The targeted actual-engine regression then passed after correction. The subsequent full workspace run was interrupted by the reported host failure and is **unverified**. A current native desktop build / CEF smoke and a repeated three-part smoke after the final correction were not completed. No aggregate workspace-pass claim is made.

The changed Markdown and authoring tool were formatted before the host failure. This evidence file is added for checkpoint publication. Resume native verification only on a suitably resource-controlled host; one Cargo job does not bound test or native-kernel memory. Native calls remain nonpreemptible, and bounded retained buffers are not allocator-level RSS guarantees.

### Reproducibility identity and limits

The exercised environment was Linux x64, Intel i7-10710U (6 cores / 12 threads), approximately 8 GiB RAM and 512 MiB swap, Rust 1.99, Node 24.15.0 and pnpm 10.32.1. Native commands used a single Cargo job, disabled incremental compilation and debug information, and single-threaded tests. Monstertruck dependencies were pinned to public fork commit `d87b4d9ced1f3baf31aa771ac0e7c663efb1c001`; no alternate CAD kernel was introduced. The new `geo` dependency supplies 2D region operations, not BREP authority.

| Input                                           | SHA-256                                                            |
| ----------------------------------------------- | ------------------------------------------------------------------ |
| `fixtures/geometry/box-mm.step`                 | `1b23042e12183748e12bc4e0195ae546c80296ce4170709eb3f836dce2f4ac98` |
| `fixtures/geometry/cylinder.step`               | `95df1da444dc9c49529054f1825c9013c135301df677e30269c3a789ee2b464b` |
| `fixtures/geometry/through-hole.step`           | `eb2112f9ef4be523615f237b86b69569026cef8e1cafbc20e2fdca7e3459cb29` |
| `fixtures/manufacturing/solid-fill.intent.json` | `97ca7ea395b7c44aef1e15ff61d505dab42b1d64b6849204f07dd120d297e58b` |

At this historical checkpoint, support was restricted to the declared import and planar manufacturing envelope. Slopes, tilted cylinders, non-layer-aligned shelves, partial top layers, generated supports, arbitrary repair and unresolved thin geometry were rejected; component shapes were metadata. Its independently exported cylinder/hole admission gate was unresolved. The current exact-kernel software gate is recorded above and in [geometry evidence](geometry-evidence.md); physical visible/pickable and machine acceptance remain open. Private Blender-format research is nonbinding and introduces no importer implementation.
