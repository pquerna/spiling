<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Spiling: repository guide

Spiling is a native BREP manufacturing workbench, not merely a model viewer or a mesh-only slicer. Its purpose is to turn authoritative geometry and explicit manufacturing requirements into inspectable, verified manufacturing artifacts. Rust owns the native engine; a cross-platform desktop workbench and CLI use the same services.

This file defines repository-wide working rules and architectural boundaries. Read it before changing the project. **Keep it compact, current, and canonical—not a development diary.**

## 1. Status and scope

The engine, CLI and CEF workbench implement source-backed authoring and a software-only operator slicer through shared native services. Current formats are project10/manufacturing11/process3/surface3 with source6/planner26, emitter8/verifier10 and SPLR6; superseded development documents/artifacts refuse without compatibility aliases. Automatic walls consume constant/per-edge nominal widths and physical intervals, with independent source/void/occupancy/command qualification. One nozzle/material can combine eligible sparse-interior bands while exterior walls, skins, holes and authorized permanent interfaces remain fine. Core-owned definition/occurrence Replace/Erase layers retain source-bound sparse fractions and finite boxes separately from face finishes. Authorized separators/physical-thickness plates retain independent native/core/actual-anchor/attachment obligations. A separate bounded fixed-dual Marlin software route binds exactly two upright source occurrences and loaded materials through exclusive reservations, supported transition waste and independently decoded tool/offset/E/debt/thermal state. It does not inherit the single-tool finish/support/coarse envelope or confer physical printer/color qualification. Owning contracts and evidence distinguish exercised workflows, refusals and unqualified operations; source layout or unit checks alone do not establish delivery.

Programs are **NOT MACHINE READY**: physical printability/grip, swept-tool collision, cross-platform durability and packaging remain unqualified. Local hardware evidence is not universal GPU or platform qualification. General STEP documents/assemblies, branching/fin supports, dedicated fuzzy/helical/crossed controls, material/color painting, multi-axis deposition and execution remain outside support. Synthetic Triangle is an explicit diagnostic, never an import fallback. [Native interfaces](docs/adr/ADR005-native-first-development-geometry.md), [image interfaces](docs/adr/003-spatial-image-relief.md), [geometry evidence](docs/quality/geometry-evidence.md), [operation evidence](docs/quality/engine-operations.md), [manufacturing evidence](docs/quality/manufacturing-evidence.md) and [B0 evidence](docs/quality/B0-evidence.md) govern claims.

Headless appearance services provide single-filament palette matching and bounded Prusa v7 reference-estimate studies of ordered physical layer cycles, finite patch exposure and recipe alternatives, with revision-owned acceptance and source-independent recovery. These estimates are not Spiling calibration, backing optics, executable material assignments or physical color qualification. The independently authored manufacturing route is separate authority; [appearance contracts](docs/protocol/project.md#optical-reference-construction-studies) own study scope and workflow.

Runtime SolidCore dependency pins in `Cargo.toml` and `geometry::KERNEL_REVISION` must agree on the same exact published commit. Frozen original-exporter inputs retain historical provenance without requiring their retired builders. Source layout, a successful build or a partial smoke run does not qualify complete geometry/manufacturing support. Current support declarations must distinguish exercised behavior, explicit refusals and unqualified operations.

[ADR005](docs/adr/ADR005-native-first-development-geometry.md) defines native-first **DEVELOPMENT** material authority: one imported SolidCore BREP, immutable original STEP identity/provenance and original finish inputs, exact producer/tolerance binding, no independent original-STEP topology/embedding claim. Source6 remains that authority under the current coordinated planner/emitter/verifier. Qualification requires the actual coherent producer, exact published SolidCore pins, recorded vendored companion sources and preserved operators, not transferred source5 evidence. Original bracket/W1, noncolumn torus critical-band slab capability and the documented image/fixed-axis workbench refusals remain unqualified. Keep those diagnostics and safety gates unchanged; [manufacturing evidence](docs/quality/manufacturing-evidence.md) owns their scope. Independent actual-program/process replay remains mandatory and is not physical or machine-readiness certification.

During active SolidCore work, refresh upstream each UTC workday and before a fork change or pin. Fork `main` mirrors upstream; long-lived `pquerna/*` branches and fork-only draft PRs isolate each logical change. Compose them on `pquerna/integration-spiling-tracking`; Cargo pins an exercised exact commit from that branch. Update published lanes by normal merges; rebase only unpublished work. Never force-push shared/pinned history or open an upstream PR without explicit approval. Follow the [daily fork procedure](docs/quality/development.md#solidcore-fork-freshness) and maintain each change's rationale, SMLib relationship, evidence and upstream/removal criteria in the [assessment](docs/quality/solidcore-upstream-review.md).

Initial implementation direction:

- One Cargo workspace and one pnpm workspace.
- A supervised Rust engine process, independent of the desktop runtime.
- Tauri 3 with CEF, React/TypeScript/Vite, and a Three.js WebGPU viewport. Pin tested versions; packaged viability on declared Windows, macOS, and Linux reference machines is a gate, not an assumption.
- [SolidCore](https://github.com/pquerna/solidcore) is the runtime CAD/BREP kernel behind the geometry adapter; [upstream](https://github.com/tangency-labs/solidcore) and the public fork retain native ownership. Pin SolidCore and solidcore-mesh to the same exact published fork commit, never a moving branch. Only SolidCraft's geometry/STEP companion libraries are vendored; preserve their upstream licensing and `vendor/solidcraft/UPSTREAM.toml` provenance, not a second application fork. The native-first route uses the imported BREP as its single material authority; original STEP preserves identity/provenance and finish inputs, not a second reconstructed solid. Qualification is producer- and operator-specific. Historical Monstertruck-authored fixture bytes retain their original identity, not current runtime authority.
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
| `crates/appearance/`                           | D65 colorimetry, bounded palette matching and optical reference construction studies |
| `crates/manufacturing/`                        | Planning, operations/resources, backend, packaging, emitted-program verification |
| `packages/protocol/`                           | Generated TypeScript contracts and binary decoder; no UI dependencies            |
| `packages/viewport/`                           | Scene, picking, GPU resources, mesh/toolpath rendering                           |
| `fixtures/`, `bench/`                          | Redistributable correctness corpus and reproducible workloads                    |
| `docs/adr/`, `docs/protocol/`, `docs/quality/` | Decisions, wire specifications, support and evidence                             |
| `tools/`                                       | Bootstrap, generation, and release tooling                                       |

Dependencies flow inward:

- Contracts have no Tauri, React, kernel, or renderer dependencies.
- Core depends on contracts; geometry depends on contracts and SolidCore through its kernel adapter.
- Surface-fields depends on contracts, not Core, geometry, renderer or planner. Core owns canonical immutable image storage; engine enables the optional image codec; manufacturing consumes borrowed scalar fields and geometry services. Engine composes these services with Core.
- Appearance depends only on contracts: no Core, kernel, manufacturing, UI or GPU dependency. Core owns immutable original/canonical palettes, prepared identity caches and Arc-owned studies/reports; engine composes typed Projects/Operations/ByteStream for engine-client and CLI. Named reference-model math stays separate from unclipped target colorimetry and does not confer calibration or routing authority. See [appearance contracts](docs/protocol/project.md#optical-reference-construction-studies).
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

### Pre-release format evolution

Before the first **published 0.1.0 release**, keep the current numerical values of unreleased project, process and manufacturing format versions stable—not their data models. Cargo package versions already set to `0.1.0` do not mean that release has happened. Update Rust contracts, protobuf, generated consumers and fixtures in lockstep; do not increment format counters per feature or renumber, unify or roll back existing versions.

Older development files or artifacts may need recreation. Fail clearly rather than reinterpret missing or changed data or weaken validation; do not add backward readers, migrations, aliases or compatibility defaults for nonexistent released-file compatibility. Preserve producer/algorithm identity checks where needed to invalidate stale compiled manufacturing output; change those identities when the affected production or verification semantics require invalidation, not automatically for every data-model edit. Change a binary format revision only for an actual incompatible binary representation, layout or interpretation change. This policy does not introduce a new version or build-ID scheme.

At the initial `0.1.0` release gate, decide the supported on-disk compatibility policy **before publishing the release**; do not assume unlimited backward compatibility.

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
- Single-tool software scope retains actual-height native layers, skins, solid/sparse fill, bounded actual-anchor bridges and bed-rooted breakaway supports. The separate fixed-dual Marlin reference admits exactly two explicitly assigned upright occurrences, fine intervals, both loaded tools each layer, source/waste/carriage reservations and independently decoded transition state; enabled finish/process/support/coarse combinations refuse that route. Every path retains native topology/void/offset/resource/command obligations. No general STEP, branching supports, arbitrary material/color painting, shared-melt mixing, multi-axis or machine execution is implied. Declared profiles, nominal bonds and successful replay are not physical printability, calibrated color or machine readiness.
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

Use tests for consumer-visible invariants: geometry closure/orientation, units, source mappings, approximation bounds, revision ownership, cancellation, recovery, extrusion accounting, and invalid input rejection. Binary interoperability uses cross-language golden fixtures. Generated contracts are checked in; run `pnpm contracts --check` locally to reject drift. Automatic remote CI is disabled; native packaging remains opt-in. Local verification requirements still apply.

Distinguish deterministic normalized-plan comparisons from semantic equivalence within declared floating-point tolerances. Exclude nonsemantic timing metadata. Do not promise universal bitwise geometry equality across kernels or platforms.

Progress through evidence gates:

1. **Packaged bootstrap:** reproducible workspaces, contracts, handshake, CLI, diagnostics, CEF packaging, and WebGPU detection on declared platforms.
2. **Geometry and transfer:** frozen valid/adversarial corpus, import/tessellation/section invariants, face picking, bounded transfer, and measured validation of SolidCore's capabilities and limits.
3. **Recoverable authoring:** editing, save/open, undo/redo, jobs, stale-result rejection, cancellation, and fault-injected recovery.
4. **Printable MVP:** planar compiler, one backend, independent replay, equivalent UI/CLI plans, and repeated supervised physical prints.
5. **Measured expansion:** scale, extensions, execution, load-informed recipes, indexed reorientation, and experimental processes through their own gates.

Numerical targets from the implementation RFC are proposed until frozen for a gate; they are not achieved results. Freeze fixture hashes, support envelope, tolerances, hardware, drivers, procedures, and thresholds before evaluation. Record failures and explicit exceptions. Never silently weaken a failing gate or claim support from a synthetic demo.

Retain reproducible run evidence with commit/dependency identity, inputs, environment, procedure and outcomes in CI or ignored artifacts; summarize exercised checks and known failures in commit/PR verification notes. Committed Markdown should explain the lasting contract, how to navigate the implementation and how to reproduce the workflow—not copy each run's output. Physical qualification additionally requires retained setup and measurements. Review the complete operator workflow, not just microbenchmarks.

Root commands are `pnpm bootstrap`, `pnpm dev`, `pnpm contracts`, `pnpm check`, `pnpm test`, `pnpm bench:smoke`, and `pnpm package`; real-surface smoke runners are documented in [development.md](docs/quality/development.md). Keep commands consistent with tools/run.mjs and report only commands actually exercised. Native runtime/toolchain pins and dependency lockfiles are repository contracts.

Finish a change only when affected callers and contracts agree, relevant behavior is exercised, and the canonical rules and vision still describe the current system. Report observed results and remaining support limits plainly.
