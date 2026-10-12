<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Software-only manufacturing evidence

## Complete-job and planar-region software proof

Based on merged main `0b4a14f`, manufacturing data and SPLR are schema 2; planner/emitter/verifier provenance is version 2. Machine lifecycle, native material classification, Core storage and client display remain separate owners. This gate explicitly accepts software-verified jobs first, not physical machine qualification. Diagnostic bodies remain a distinct workflow; complete Marlin jobs declare exact configuration/calibration record hashes without claiming measurements.

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

No physical printer support, machine readiness or printability is claimed. The implemented software workflow and support envelope below remain authoritative; the earlier checkpoint measurements are preserved only as history.

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
