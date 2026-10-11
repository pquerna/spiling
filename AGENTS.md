<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Spiling: repository guide

Spiling is a native BREP manufacturing workbench, not merely a model viewer or a mesh-only slicer. Its purpose is to turn authoritative geometry and explicit manufacturing requirements into inspectable, verified manufacturing artifacts. Rust owns the native engine; a cross-platform desktop workbench and CLI use the same services.

This file defines repository-wide working rules and architectural boundaries. Read it before changing the project. **Keep it compact, current, and canonical—not a development diary.**

## 1. Status and scope

The engine/CLI implements authenticated gRPC services for source-backed project authoring and software-only planar manufacturing: declared-profile STEP import, shared rigid occurrences, bounded undo/redo, save/open/recovery, explicit printer/recipe intent, native-section compilation, independently replayed program and durable derived-bundle export. Standard Google Operations and ByteStream own native job observation, cancellation and immutable output retrieval. Core owns persistent project state/storage; native geometry/display caches remain derived. Manufacturing is limited to declared vertical planar/cylindrical, integral fixed-height, solid-fill geometry; programs are **not machine ready** and establish no physical printer support or printability. The thin CEF/WebGPU workbench provides project/inspection controls, not manufacturing UI. Physical visible/pickable acceptance, platform durability and packaging remain open gates. Synthetic Triangle is an explicit diagnostic, never an import fallback. [B0 evidence](docs/quality/B0-evidence.md), [geometry evidence](docs/quality/geometry-evidence.md), [historical authoring evidence](docs/quality/authoring-evidence.md), [operation evidence](docs/quality/engine-operations.md) and [software manufacturing evidence](docs/quality/manufacturing-evidence.md) govern claims. Source layout or CI targets alone do not establish support. Do not create placeholder workspaces as part of documentation work.

Initial implementation direction:

- One Cargo workspace and one pnpm workspace.
- A supervised Rust engine process, independent of the desktop runtime.
- Tauri 3 with CEF, React/TypeScript/Vite, and a Three.js WebGPU viewport. Pin tested versions; packaged viability on declared Windows, macOS, and Linux reference machines is a gate, not an assumption.
- [Monstertruck](https://github.com/pquerna/monstertruck/tree/spiling-dev) is the default CAD/BREP kernel, behind a narrow, capability-aware geometry adapter. Spiling patches live on the public fork's `spiling-dev` branch, based on [upstream](https://github.com/virtualritz/monstertruck) `master` (its default branch). Pin all Monstertruck dependencies to the same tested exact fork commit, not the moving branch, and validate required capabilities against the corpus. Do not introduce an alternate kernel or silent fallback without an explicit architecture decision.
- Protobuf-defined gRPC services, standard Google Operations/ByteStream, Rust-derived TypeScript shell views, and separately specified packed binary payloads.
- Printer specifications are TypeScript-authored data against generated Rust-defined domain contracts: configuration, capabilities, coordinate frames, component shapes and process limits. Rust validates inert data and owns planning, backend execution and mandatory verification. Ordinary printer additions should be data; genuinely new behavior requires a reviewed Rust backend extension. Arbitrary TypeScript profile execution does not belong in the engine, and a declared capability does not establish machine support.

The first printable product imports a declared STEP subset, preserves face identity, supports editing and recovery, compiles a constrained planar print, previews the emitted program, and exports for one tested machine configuration. A viewer or attractive toolpath preview does not meet that scope.

The longer-term vision includes large assemblies, incremental compilation, supervised TypeScript extensions, execution connectors, load-informed planning, indexed reorientation, and experimental optical or foam processes. Continuous multi-axis deposition requires its own later evidence gate. Representing a capability in a schema does not make it supported.

Do not expand the initial product into cloud services, collaboration, a general CAD editor, a plugin marketplace, continuous robot control, or a full structural solver. Do not maintain a parallel Electron implementation or silently substitute a system webview for CEF.

## 2. Canonical local documentation

### Required files and placement

Every crate, application, shared package, and substantial module or code area MUST have two files at its owning directory:

- `AGENT-RULES.md`: the **current binding rules** for working in that area.
- `AGENT-VISION.md`: the **current intended purpose and direction** of that area.

This root `AGENTS.md` is the shared entry point; it does not replace local pairs. Add the pair when an area is introduced, before or alongside its first implementation. Do not pre-create directories or pairs for hypothetical future areas.

A substantial area has a distinct responsibility, contract, invariants, or independent change surface. Crate roots always qualify; modules such as project persistence, artifact storage, job scheduling, kernel adapters, manufacturing verification, and major UI features qualify when introduced. A small private helper does not need a pair merely because it is in a separate file.

For a directory module, put the pair beside its implementation, usually under `src/<module>/`. Prefer an owning directory for a substantial single-file module rather than inventing ambiguous document names. Nested substantial areas get their own pairs even when the parent has one. Do not scatter a pair beside every source file.

### Reading and authority

Before editing an area:

1. Read root `AGENTS.md`.
2. Read both local files along the directory ancestry, from broadest scope to the target area.
3. Read the pairs for affected dependency or consumer areas when changing their contracts.

Local rules refine repository-wide rules; they cannot silently override them. Vision explains intent, not permission to bypass rules or advertise unimplemented behavior. If documentation conflicts with another document or observed code, resolve the conflict explicitly rather than choosing the convenient version. An intentional architecture change updates the governing documents and affected consumers together.

These filenames are mandatory even if an agent tool does not automatically discover them. Agents are responsible for reading them explicitly; do not rely on automatic `AGENTS.md` discovery to enforce the system. Avoid additional nested `AGENTS.md` files that duplicate the local pair.

### What each file contains

Keep `AGENT-RULES.md` short and actionable:

- Responsibility, scope, allowed dependencies, and prohibited coupling.
- Public contracts, authority boundaries, invariants, and error behavior.
- Relevant units, frames, tolerances, revisions, concurrency, safety, and resource ownership.
- How to exercise the area and what evidence a meaningful change requires.
- Links to canonical protocol specifications or decisions rather than copied specifications.

Keep `AGENT-VISION.md` short and specific:

- Purpose and the user or downstream consumer it serves.
- Intended capabilities, architectural direction, and explicit non-goals.
- Current support envelope and clearly labeled intended or experimental capabilities.
- Acceptance conditions and unresolved decisions that still affect the direction.

Neither file is a backlog, changelog, session transcript, implementation journal, or collection of superseded proposals. No dated progress entries, completed-task lists, or “previously we…” narratives. Current limitations and genuinely open decisions belong; their chronological story does not.

### Maintenance is part of the change

When responsibilities, contracts, invariants, support limits, or direction change, update the affected pair and any governing parent in the same change. Replace obsolete statements; remove resolved questions, stale guidance, and duplicate material. Do not append a contradictory new paragraph beneath an old rule.

A change is incomplete if the documentation describes a different system. Read both files even when neither needs an edit; do not manufacture documentation churn for implementation-only changes.

Keep historical rationale in `docs/adr/`, measured milestone evidence in `docs/quality/`, and chronological changes in version control or a project changelog when present. An ADR records a decision and evidence; the agent files state the resulting current rules and vision. Link history only when it helps explain a current constraint. Do not require the session paste or a private external conversation to understand binding rules.

Private research, planning, ideas, Markdown, and agent knowledge live in the separate `/root/spiling-brain` checkout (`pquerna/spiling-brain`), not this repository's `docs/reserach/`. Read its own agent guide before changing it; every brain change must be committed and safely pushed to main, fetching/rebasing concurrent updates without force-pushing. Brain proposals do not override implementation contracts. Adopt accepted constraints into the relevant public canonical documents without exposing private material or requiring private-repository access to understand the code.

## 3. Repository shape and dependency boundaries

Use directories to express ownership first. Create a separate crate only for justified dependency isolation, separate compilation, or another consumer—not for every conceptual subsystem.

B0 establishes contracts and a shared `crates/engine-client` used by the CLI and shell; that second crate is justified by two consumers. The domain areas below describe ownership as they are introduced, not a requirement to create empty future crates.

| Planned area                                   | Ownership                                                                        |
| ---------------------------------------------- | -------------------------------------------------------------------------------- |
| `apps/desktop/src/`                            | Workbench features and transient UI state                                        |
| `apps/desktop/src-tauri/`                      | Shell, engine supervision, native dialogs, binary bridge, packaging              |
| `apps/engine/`                                 | Sidecar entry point and composition of native services                           |
| `apps/cli/`                                    | Client commands using the same protocol and services; no alternate planner       |
| `crates/contracts/`                            | IDs, revisions, commands, events, errors, manifest schemas                       |
| `crates/engine-client/`                        | Shared protocol client, child-process lifecycle, request deadlines               |
| `crates/core/`                                 | Project transactions, undo, persistence/recovery, artifacts, jobs                |
| `crates/geometry/`                             | Kernel adapter, import, tessellation, sections, capability contract              |
| `crates/manufacturing/`                        | Planning, operations/resources, backend, packaging, emitted-program verification |
| `packages/protocol/`                           | Generated TypeScript contracts and binary decoder; no UI dependencies            |
| `packages/viewport/`                           | Scene, picking, GPU resources, mesh/toolpath rendering                           |
| `fixtures/`, `bench/`                          | Redistributable correctness corpus and reproducible workloads                    |
| `docs/adr/`, `docs/protocol/`, `docs/quality/` | Decisions, wire specifications, support and evidence                             |
| `tools/`                                       | Bootstrap, generation, and release tooling                                       |

Dependencies flow inward:

- Contracts have no Tauri, React, kernel, or renderer dependencies.
- Core depends on contracts; geometry depends on contracts and Monstertruck through its kernel adapter.
- Manufacturing consumes geometry services and contracts; engine composes these services with core.
- Shell and CLI are protocol clients, not owners of geometry or planning logic.
- Viewport uses display contracts and rendering libraries, never Tauri directly.

Keep backend, packager, connector interfaces, and verifier as distinct logical modules within manufacturing initially. Separate contracts do not require separate crates. Tauri-specific dependencies stay in the shell. Generated TypeScript must not become a second hand-maintained domain model.

## 4. Architectural invariants

### Authority, identity, and recovery

- The engine is the single project writer. Transactions own mutation, undo, and revision changes; the UI owns only transient interaction state.
- Initially use one engine per open project; CLI operations start an engine. An exclusive writer lock prevents concurrent project mutation. Read-only inspection uses a committed snapshot.
- Mutations carry base revisions. Jobs carry input revisions and cancellation. Stale results may populate immutable caches but cannot replace the active plan or preview.
- BREP geometry, units, frames, tolerances, and source identity remain authoritative. Display meshes are disposable approximations with declared bounds.
- No kernel object or pointer crosses IPC. Session handles expire on engine restart; persistent references identify immutable assets and revisions.
- Persistence uses a versioned manifest, immutable assets, content-addressed derived artifacts, and bounded recovery checkpoints. Commit validates references; save writes assets before atomic manifest replacement with platform-appropriate durability.
- Undo/redo is bounded session transaction history; save does not clear it. Camera and playback state are outside project undo. A drag commits once on release.
- Recovery yields a complete committed state, never a mixed manifest. Restart never automatically replays printer operations.

### Protocol, resources, and display

- Use authenticated gRPC on ephemeral IPv4 loopback, with standard Google Operations and ByteStream. Inherited pipes carry bounded startup metadata, the per-launch capability and owner liveness; logs go to stderr.
- Engine and shell ship together. Validate child/instance identity and limits; evolve their API together without cross-version compatibility shims.
- Bound queues by bytes; provide backpressure and cancellation that large transfers cannot starve. Release pending transfers and GPU resources on cancellation or closure.
- Validate binary sizes, arithmetic overflow, alignment, finite coordinates, index bounds, versions, and allocation limits before views or GPU upload. Never serialize arbitrary Rust memory layouts.
- Mesh chunks carry revision, source-face mapping, bounds, local origin, hash, and schema identity. Native precision queries own exact measurements; float32 display buffers do not.
- Keep picking revision-aware. Topology changes require explicit reference remapping, not guessed identity preservation.
- Render packed mesh/toolpath batches, not an object per segment. Camera, hover, selection, and playback do not regenerate geometry. Account separately for engine, renderer, and GPU memory.
- Measure copies and boundary transfers. Do not claim zero-copy or add shared-memory/V8/GPU interop before profiling establishes a need.

### Manufacturing and safety

- Keep authoring requirements, normalized plans, machine realization, export bundles, and verification logically distinct.
- General manufacturing interfaces share geometry and intent but allow process-specific planners and distinct operation kinds: paths, poses, exposure/raster maps, and process cycles. A toolpath is not every manufacturing operation, and G-code is not every output format. Keep provenance, error budgets, calibrated process inputs, and machine capabilities explicit across boundaries without expanding current process support.
- Unsupported geometry, requirements, commands, and machine behavior produce explicit diagnostics; do not invent successful fallbacks.
- Verification decodes the emitted machine program independently of the planner preview. Share primitive types where useful, not assumptions that conceal planner errors.
- Report verification coverage and provenance to the operator. A recorded load annotation is not an enforced load constraint until the mechanical gate passes.
- Initial print scope: one named machine/firmware/configuration, nozzle and material profile, one extruder, fixed layer height, no generated supports, no arbitrary mesh repair.
- Use explicit units and coordinate frames. Do not bake a global rising-Z assumption into general operation/resource contracts; a nonplanar representation fixture is not executable multi-axis support.
- Later plugins are supervised, permission-limited, and use batched APIs. They cannot bypass mandatory host verification. Execution connectors must reconcile uncertain submission outcomes rather than blindly retrying.

## 5. Rust and implementation patterns

- Prefer explicit types, small modules, ordinary functions, and narrow traits at real service or adapter boundaries. Keep domain logic usable without Tauri.
- Newtypes should distinguish IDs, revisions, units, and frames wherever confusion could invalidate geometry or manufacturing behavior. Keep approximation and tolerance policy explicit.
- Make ownership, cancellation, and cleanup visible. Keep kernel-specific unsafe code or FFI isolated behind reviewed adapters with documented safety invariants.
- Avoid avoidable allocations, copies, per-point IPC, and unbounded work in native hot paths. Optimize from measured workloads without weakening correctness.
- Prefer standard derives and the single selected contract generator. Use a declarative macro only for genuine repetitive structure with a clear expansion; add procedural macros only when ordinary Rust or generation cannot reasonably express the requirement.
- Macros must not conceal mutation authority, blocking work, allocation, unsafe operations, or error policy. Do not build a custom macro framework to anticipate future crates.
- Pin toolchains and dependencies, commit lockfiles, and review kernel/slicer/native-runtime licensing before distribution. Do not commit private customer CAD; fetch large benchmark assets by documented checksums.
- Original Spiling code and documentation use `OSL-3.0`; [LICENSE.md](LICENSE.md) is the canonical unmodified license. Follow [docs/license.md](docs/license.md) for short SPDX headers, ownership notices, package metadata, and release obligations. Preserve third-party licensing and attribution; never relabel imported material as project-owned.

## 6. Working and acceptance discipline

Before implementation, identify the owning area, read its pair, and state the boundary and acceptance behavior being changed. Reuse existing patterns. Do not introduce alternate protocols, planners, type definitions, or compatibility shims as shortcuts.

Verify the changed behavior through the real surface: native engine/CLI operations, packaged desktop integration, geometry corpus, or emitted-program replay as appropriate. Unit tests alone do not establish end-to-end behavior. For documentation-only changes, check scope, consistency, and document references; no application smoke run is implied.

Use tests for consumer-visible invariants: geometry closure/orientation, units, source mappings, approximation bounds, revision ownership, cancellation, recovery, extrusion accounting, and invalid input rejection. Binary interoperability uses cross-language golden fixtures. Generated contracts are checked in and CI rejects drift.

Distinguish deterministic normalized-plan comparisons from semantic equivalence within declared floating-point tolerances. Exclude nonsemantic timing metadata. Do not promise universal bitwise geometry equality across kernels or platforms.

Progress through evidence gates:

1. **Packaged bootstrap:** reproducible workspaces, contracts, handshake, CLI, diagnostics, CEF packaging, and WebGPU detection on declared platforms.
2. **Geometry and transfer:** frozen valid/adversarial corpus, import/tessellation/section invariants, face picking, bounded transfer, and measured validation of Monstertruck's capabilities and limits.
3. **Recoverable authoring:** editing, save/open, undo/redo, jobs, stale-result rejection, cancellation, and fault-injected recovery.
4. **Printable MVP:** planar compiler, one backend, independent replay, equivalent UI/CLI plans, and repeated supervised physical prints.
5. **Measured expansion:** scale, extensions, execution, load-informed recipes, indexed reorientation, and experimental processes through their own gates.

Numerical targets from the implementation RFC are proposed until frozen for a gate; they are not achieved results. Freeze fixture hashes, support envelope, tolerances, hardware, drivers, procedures, and thresholds before evaluation. Record failures and explicit exceptions. Never silently weaken a failing gate or claim support from a synthetic demo.

Keep evidence sufficient to reproduce the result: commit and dependency versions, environment, fixtures, procedure, correctness outcomes, cold/warm timing and memory/transfer data, plus physical setup and measurements when manufacturing is involved. Review the complete operator workflow, not just microbenchmarks.

Root commands are `pnpm bootstrap`, `pnpm dev`, `pnpm contracts`, `pnpm check`, `pnpm test`, `pnpm bench:smoke`, and `pnpm package`; real-surface smoke runners are documented in [development.md](docs/quality/development.md). Keep commands consistent with tools/run.mjs and report only commands actually exercised. Native runtime/toolchain pins and dependency lockfiles are repository contracts.

Finish a change only when affected callers and contracts agree, relevant behavior is exercised, and the canonical rules and vision still describe the current system. Report observed results and remaining support limits plainly.
