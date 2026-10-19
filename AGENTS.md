<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Spiling: repository guide

Spiling is a native BREP manufacturing workbench, not merely a model viewer or a mesh-only slicer. Its purpose is to turn authoritative geometry and explicit manufacturing requirements into inspectable, verified manufacturing artifacts. Rust owns the native engine; a cross-platform desktop workbench and CLI use the same services.

This file defines repository-wide working rules and architectural boundaries. Read it before changing the project. **Keep it compact, current, and canonical—not a development diary.**

## 1. Status and scope

The engine, CLI and CEF workbench implement source-backed authoring and a software-only operator slicer through shared native services. Current formats are project8/manufacturing9/process2/surface3 with planner13, emitter/verifier5 and SPLR4. Previous manifests and artifacts are refused without compatibility aliases. Core-owned definition/occurrence Replace/Erase layers author whole-owned sparse fractions and bounded finite source-local boxes, separately from face finishes. A box admits one fixed-axis internal boundary with explicit permanent open SeparatorWall permission; its reserved band, actual bonds and lower anchors are verified independently while global exterior walls, skins and source holes remain protected. The rounded-housing reference has a denser end and lighter body within ONE part, not a bracket or general spatial painting. Whole-owned base values are not uniform-density claims for zoned output. Required job-wide seam/axis policies remain explicit; adjoining regional lattices share the transported physical source frame. Generic trimmed-face descriptors, optional exact analytic metadata and independent mesh availability support bounded native inspection; import/display does not authorize manufacturing. Independent tapered fitting and rounded housing are representative operator sources, not general STEP or full G0–G3 qualification. Core owns project/history, process intent, ordered finishes and canonical immutable images; upright full-height exterior planar/cylindrical carriers retain narrow rib/image-relief eligibility.

Programs are **NOT MACHINE READY**: physical printability/grip, swept-tool collision, cross-platform durability and packaging remain unqualified. Local hardware evidence is not universal GPU or platform qualification. General STEP documents/assemblies, branching/fin supports, dedicated fuzzy/helical/crossed controls, material/color painting, multi-axis deposition and execution remain outside support. Synthetic Triangle is an explicit diagnostic, never an import fallback. [Native interfaces](docs/adr/004-native-geometry-parity.md), [image interfaces](docs/adr/003-spatial-image-relief.md), [geometry evidence](docs/quality/geometry-evidence.md), [operation evidence](docs/quality/engine-operations.md), [manufacturing evidence](docs/quality/manufacturing-evidence.md) and [B0 evidence](docs/quality/B0-evidence.md) govern claims.

Native dependency pins in `Cargo.toml` and `geometry::KERNEL_REVISION` must agree on the same exact published commit. Source layout, a successful build or a partial smoke run does not qualify complete geometry/manufacturing support. Current support declarations must distinguish exercised behavior, explicit refusals and unqualified operations.

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

### Add local guidance only when it earns its place

This root guide supplies shared rules. A directory MAY have `AGENT-RULES.md`, `AGENT-VISION.md`, both, or neither. They are not a mandatory pair. A crate, package, module, or new directory does not automatically need either file.

Add a local file only when it contributes useful, non-obvious guidance beyond its ancestors and canonical specifications. Keep it at the directory that owns those facts. Do not create directories, empty templates, or extra documentation layers just to satisfy a documentation convention.

### Content: current facts, patterns, rules and direction

- **`AGENT-RULES.md`**: what a contributor needs to change this area correctly—responsibility, ownership and dependency boundaries, important invariants, established patterns, non-obvious failure/resource behavior, and the relevant way to exercise it. Include only what matters locally; do not fill a checklist of categories.
- **`AGENT-VISION.md`**: why this area exists, whom it serves, the durable direction and explicit non-goals. Distinguish current capability from intended capability; include open architectural decisions only while they still affect that direction.

Neither file is a history, changelog, backlog, session transcript or implementation journal. No dated progress, completed-task lists, “previously we…” narratives, build results, fixture hashes or accounts of how a fix was discovered. State the resulting rule or direction, not the story.

Prefer a few short bullets or paragraphs. As a review signal, aim for roughly 300 words or fewer for rules and 150 for vision; these are ceilings to question, not quotas to fill. A shorter useful file wins. If more is needed, keep only essential local guidance and link the detailed specification rather than creating more agent files.

Do not repeat inherited rules, copy protocol/API specifications, enumerate every function/test, or narrate code that is easy to inspect. Link canonical documents for detail. An important safety or ownership invariant still belongs here when it would otherwise be easy to miss.

### Read, maintain and prune

Before editing, read this guide and any existing local rules/vision along the target's directory ancestry, broadest first. Read affected dependency/consumer guidance when changing their contracts. Missing local files are normal; do not create them as a prerequisite to work.

Local rules refine inherited rules; they cannot silently override them. Vision does not authorize unsupported behavior. Resolve conflicts with code or governing specifications explicitly, updating affected guidance and consumers together.

When a change affects documented facts or direction, rewrite the relevant guidance in place. Do not append a paragraph for each feature or fix. Remove stale, redundant or resolved material; implementation-only changes need no documentation churn.

When touching an area, question whether its local files still earn their place. Remove an unnecessary file once any unique, still-relevant guidance has a suitable canonical owner; do not preserve boilerplate or a pair merely for symmetry. Do not discard safety or contract facts just to hit a word target.

Keep durable design rationale in `docs/adr/`. `docs/quality/` is a compact current reference: code ownership and entry points, non-obvious invariants/failure paths, support limits and reproduction procedures. It is not a diary of builds, test counts, fixture hashes, timings or individual machines. Put change-specific verification in commit/PR notes and detailed run output in CI or ignored artifacts; git retains superseded documentation. Replace stale facts rather than append checkpoints or move them into another Markdown archive. A reader must not need a session paste or private conversation to understand current binding rules. Avoid nested `AGENTS.md` files that duplicate this guidance.

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
| `crates/surface-fields/`                       | Bounded image normalization and shared source-frame scalar-grid evaluation       |
| `crates/manufacturing/`                        | Planning, operations/resources, backend, packaging, emitted-program verification |
| `packages/protocol/`                           | Generated TypeScript contracts and binary decoder; no UI dependencies            |
| `packages/viewport/`                           | Scene, picking, GPU resources, mesh/toolpath rendering                           |
| `fixtures/`, `bench/`                          | Redistributable correctness corpus and reproducible workloads                    |
| `docs/adr/`, `docs/protocol/`, `docs/quality/` | Decisions, wire specifications, support and evidence                             |
| `tools/`                                       | Bootstrap, generation, and release tooling                                       |

Dependencies flow inward:

- Contracts have no Tauri, React, kernel, or renderer dependencies.
- Core depends on contracts; geometry depends on contracts and Monstertruck through its kernel adapter.
- Surface-fields depends on contracts, not Core, geometry, renderer or planner. Core owns canonical immutable image storage; engine enables the optional image codec; manufacturing consumes borrowed scalar fields and geometry services. Engine composes these services with Core.
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
- Current software print scope: one explicit named machine/firmware/configuration, nozzle/material profile and extruder; actual-height native layers, skins, solid/sparse infill, bounded bridges from actual deposited anchors and bed-anchored vertical breakaway supports with interfaces/contact gaps. Eligibility requires per-layer topology/offset/resource gates; no arbitrary mesh repair, general/freeform STEP, branching supports, color, multi-axis deposition or execution support. Declared profiles and successful replay do not establish physical printability or machine readiness.
- Source-face finishes are inert Core-owned project data, resolved by immutable definition/face identity and checked occurrence scope. Surface edits carry persistent ProjectRevision plus current native FaceRef; metadata changes do not mutate BREP or scene epochs. Canonical images are content-addressed shared assets, not filesystem paths or renderer-normalized pixels. Additive construction preserves original material/protected holes and precedes ordinary wall/fill/skin/anchor/support analysis. Native M and realized D have independent reuse identities; support/contact/corridor calculations depend on final D, including relevant lower geometry, never a fixed last-N-mm shortcut. XY/native/vertical approximation budgets, original/realized/added geometry, bounded nominal-bead advisories and fresh replay provenance remain distinct; no perimeter jitter, untextured fallback or inferred physical grip.
- Process resolution retains original occurrence operands before native union and each finished D contribution before classification. Different effective sparse fractions cannot overlap in positive material volume; compatible values may share existing admitted material union without new attachment authority. Regional coverage records source/winner provenance and sums solid, sparse and nominal void areas; actual extrusion remains stroke-derived. Density edits invalidate complete manufacturing input, including anchors/support and publication eligibility.
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

Before implementation, identify the owning area, read its applicable existing guidance, and state the boundary and acceptance behavior being changed. Reuse existing patterns. Do not introduce alternate protocols, planners, type definitions, or compatibility shims as shortcuts.

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

Retain reproducible run evidence with commit/dependency identity, inputs, environment, procedure and outcomes in CI or ignored artifacts; summarize exercised checks and known failures in commit/PR verification notes. Committed Markdown should explain the lasting contract, how to navigate the implementation and how to reproduce the workflow—not copy each run's output. Physical qualification additionally requires retained setup and measurements. Review the complete operator workflow, not just microbenchmarks.

Root commands are `pnpm bootstrap`, `pnpm dev`, `pnpm contracts`, `pnpm check`, `pnpm test`, `pnpm bench:smoke`, and `pnpm package`; real-surface smoke runners are documented in [development.md](docs/quality/development.md). Keep commands consistent with tools/run.mjs and report only commands actually exercised. Native runtime/toolchain pins and dependency lockfiles are repository contracts.

Finish a change only when affected callers and contracts agree, relevant behavior is exercised, and the canonical rules and vision still describe the current system. Report observed results and remaining support limits plainly.
