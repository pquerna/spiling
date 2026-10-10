<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Software manufacturing checkpoint evidence

## Decision and branch boundary

This is a preserved implementation checkpoint, **not an accepted release or completed manufacturing gate**. It implements software-only planar manufacturing on the pipe-framed protocol-v4 / project-format-2 authoring branch based on `97f9df1`. It has not been integrated with main commit `4ac3d97` (`Implement durable engine operations with standard Google RPCs`). That commit replaces the transport and operation contracts across the engine, contracts, clients and desktop. Integration must migrate the manufacturing services and their consumers to that architecture; a mechanical merge or compatibility shim does not establish correctness.

The operator reported a host failure during verification. The host subsequently rebooted. No compiler, test runner, engine or CEF process remained after restart. Previous-boot kernel journal inspection returned no entries, so an out-of-memory cause is **unconfirmed**. Heavy verification was stopped; the checkpoint was published at the operator's request. The interrupted final workspace run has no confirmed result.

No physical printer support, machine readiness, printability, component collision verification, packaged desktop acceptance or physical WebGPU acceptance is claimed. Programs are labeled `SOFTWARE VALIDATION ONLY, NOT MACHINE READY` and do not initialize firmware, heat, home or execute a machine.

## Implemented software workflow

Explicit printer specifications and recipes are TypeScript-authored inert data against generated Rust contracts. The trusted authoring tool type-checks and executes the source with operator permissions, then creates bounded JSON without overwriting an existing destination. Rust owns semantic validation, native-section planning, emission and mandatory independent replay; the engine does not execute TypeScript profiles.

Core owns intent, transactions, bounded history, artifact invalidation and immutable bundle storage. Compilation uses native sections of admitted world-vertical planar/cylindrical geometry, integral fixed-height layers, controlled offsets and nominal 100% solid fill. Emission and replay are separate implementations. Save stores captured source assets and the derived bundle; fresh-process read-only inspection independently replays the persisted program. CLI export exclusively creates plan, program, fresh verification and provenance files, followed by a completion manifest after observed child cleanup.

See [manufacturing contracts](../protocol/manufacturing.md), [project persistence](../protocol/project.md) and [reproduction commands](development.md#software-only-manufacturing).

## Observed real engine / CLI workflow

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

## Correctness findings and exercised fixes

- Independent replay previously admitted an empty deposition layer. A public-API smoke reproduced admission before the fix and rejection afterward. Typed validation and nested decoding now require paths in every declared layer.
- Nested decoding previously admitted a 200,000-segment document despite the 100,000-segment cumulative limit. The same public-API smoke rejected it after introducing a shared admission budget during point allocation; serialized input was 2,800,270 bytes.
- After 32 imports and 32 undos, setting manufacturing intent previously left discarded redo-history sources pinned in the native registry, causing the next import to fail with `ResourceLimit`. The actual CLI reproduction succeeded after synchronizing pins at commit; its final state retained one definition and occurrence 33. The corresponding actual-engine regression passed.
- The complete workspace run before the final erosion correction failed in `native_holes_cylinders_and_repeated_occurrences_compile_without_display_rebuild`. Two separated native circles buffered together produced two valid centers plus two spurious triangles, each approximately `4.973799150320701e-14` mm². Self-union retained those fragments. Independently eroding each source island produced exactly two centers without discarding contours or changing tolerance. Negative offsets now operate per disconnected island; positive offsets retain aggregate semantics. The same actual-engine regression passed afterward: **1 passed, 4 filtered**, 32.13 s. Thin-region and topology rejection remain in place.

## Verification record and outstanding checks

Before the final erosion correction, the contracts/core/manufacturing test run passed **96 tests**, including 42 core and 20 manufacturing tests. Workspace Clippy with warnings denied passed; TypeScript checks passed; TypeScript tests passed **141 tests**; generated-contract drift checking passed. The trusted authoring command produced the committed intent bytes, rejected nonfinite output and refused an existing output destination. These observations are not a final all-checks pass of the published checkpoint.

The earlier complete Rust workspace run failed on the repeated-circle regression described above. The targeted actual-engine regression then passed after correction. The subsequent full workspace run was interrupted by the reported host failure and is **unverified**. A current native desktop build / CEF smoke and a repeated three-part smoke after the final correction were not completed. No aggregate workspace-pass claim is made.

The changed Markdown and authoring tool were formatted before the host failure. This evidence file is added for checkpoint publication. Resume native verification only on a suitably resource-controlled host; one Cargo job does not bound test or native-kernel memory. Native calls remain nonpreemptible, and bounded retained buffers are not allocator-level RSS guarantees.

## Reproducibility identity and limits

The exercised environment was Linux x64, Intel i7-10710U (6 cores / 12 threads), approximately 8 GiB RAM and 512 MiB swap, Rust 1.99, Node 24.15.0 and pnpm 10.32.1. Native commands used a single Cargo job, disabled incremental compilation and debug information, and single-threaded tests. Monstertruck dependencies were pinned to public fork commit `d87b4d9ced1f3baf31aa771ac0e7c663efb1c001`; no alternate CAD kernel was introduced. The new `geo` dependency supplies 2D region operations, not BREP authority.

| Input                                           | SHA-256                                                            |
| ----------------------------------------------- | ------------------------------------------------------------------ |
| `fixtures/geometry/box-mm.step`                 | `1b23042e12183748e12bc4e0195ae546c80296ce4170709eb3f836dce2f4ac98` |
| `fixtures/geometry/cylinder.step`               | `95df1da444dc9c49529054f1825c9013c135301df677e30269c3a789ee2b464b` |
| `fixtures/geometry/through-hole.step`           | `eb2112f9ef4be523615f237b86b69569026cef8e1cafbc20e2fdca7e3459cb29` |
| `fixtures/manufacturing/solid-fill.intent.json` | `97ca7ea395b7c44aef1e15ff61d505dab42b1d64b6849204f07dd120d297e58b` |

Support remains restricted to the declared import and planar manufacturing envelope. Slopes, tilted cylinders, non-layer-aligned shelves, partial top layers, generated supports, arbitrary repair and unresolved thin geometry are rejected. Component shapes are metadata. Independently exported cylinder/hole admission and physical visible/pickable acceptance remain separate unresolved geometry gates; see [geometry evidence](geometry-evidence.md). Private Blender-format research is nonbinding and introduces no importer implementation.
