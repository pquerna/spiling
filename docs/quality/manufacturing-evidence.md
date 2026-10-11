<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Software-only manufacturing evidence

## Current authenticated gRPC integration

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

Support remains restricted to the declared import and planar manufacturing envelope. Slopes, tilted cylinders, non-layer-aligned shelves, partial top layers, generated supports, arbitrary repair and unresolved thin geometry are rejected. Component shapes are metadata. Independently exported cylinder/hole admission and physical visible/pickable acceptance remain separate unresolved geometry gates; see [geometry evidence](geometry-evidence.md). Private Blender-format research is nonbinding and introduces no importer implementation.
