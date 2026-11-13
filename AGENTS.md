# Spiling: agent guide

Spiling is a native BREP slicer and manufacturing workbench: it turns an imported solid and explicit manufacturing intent into an inspectable, independently verified machine program. A Rust engine owns geometry, planning and verification; an egui workbench and a CLI are clients of the same services.

This file is the critical information an agent needs to work here correctly, plus an index. Keep it short and current. It is not a status report, design record or changelog.

**This repository is not open for outside contribution yet.** Product direction, priorities, current capability, research, plans and reasoning live in the private brain repository `pquerna/spiling-brain` (use a local checkout; read its `AGENTS.md` first). Start there for _what to do and why_; start here for _how to do it without breaking things_.

## 1. Non-negotiables

- **One authority per source.** STEP retains its imported SolidCore BREP; STL/3MF retain one adopted analytic/faceted surface complex ([ADR013](docs/adr/013-hybrid-imported-material.md)). Display buffers are disposable approximations. No second material interpretation or planner.
- **Tolerance, not proof.** [ADR012](docs/adr/012-tolerant-brep-curves-and-feature-heights.md) governs acceptance: one recorded tolerance policy (join, profile 0.01 mm, height 0.001 mm) and checks on the result. There is one slicing pipeline, within tolerance; no exact native lane or certification gate. Do not add per-site micro-tolerances or proof preconditions that refuse printable parts.
- **Independent verification is mandatory.** The verifier decodes the emitted program on its own and checks it against the plan. Never share assumptions that would hide a planner error. Replay success is not physical printability.
- **No invented success.** Unsupported geometry, requirements or machine behaviour produce explicit diagnostics. No silent fallback, healing, hole filling or dropped feature. A decision to print something imperfect (an overhang, an unprintable sliver) must be reported in the plan.
- **No compatibility shims before 0.1.0.** See [pre-release format evolution](#pre-release-format-evolution) below.
- **Resources bound working sets, not part validity.** Small fixed aggregate ceilings are capacity debt ([ADR009](docs/adr/009-resource-envelope-policy.md)); do not add one or write tests that pin one. The aim is never OOMing a reasonable machine: caches, indexes and data carried across steps are fine when byte-admitted, shared rather than copied, and released with their last consumer.
- **Protocol and display safety.** Authenticated gRPC on loopback with Google Operations/ByteStream; engine and clients ship together with no cross-version shims. Validate every binary size, index, alignment and finite coordinate before building views or uploading to the GPU. Bound queues by bytes, support cancellation, render packed batches, keep picking revision-aware.
- **Rust conventions.** Explicit types and newtypes for IDs, units and frames; small modules and ordinary functions; visible ownership and cancellation; unsafe/FFI only behind reviewed adapters; no custom macro frameworks; pinned toolchains and committed lockfiles.
- **Parallelism: CubeCL first, rayon for coarse tasks.** For any hot loop, first evaluate a CubeCL kernel (`crates/compute`): data-parallel numeric batches (distances, point-in-polygon, capsule/interval screens, candidate lists) scale with wider GPUs and CPU SIMD; the CPU decides exactly on what the kernel screens. Use rayon only for coarse, irregular tasks CubeCL cannot express (a whole layer's walls, a layer's routing), with results reduced in input order so output stays deterministic. Count work, never cap it.
- **The engine is the single project writer.** Transactions own mutation, undo and revisions; stale results never replace the active plan; restart never replays printer operations.
- **SolidCore pins** in `Cargo.toml` and `geometry::KERNEL_REVISION` name the same exact published fork commit. Follow the [fork procedure](docs/quality/development.md#solidcore-fork-freshness); never force-push shared history or open upstream issues/PRs without approval.
- **Vendored SolidCraft sources are byte-exact.** Never reformat them (`cargo fmt -p <crate>`, not `--all`).
- **Privacy and licensing.** Never commit credentials or private CAD. Original code is `OSL-3.0` with SPDX headers ([docs/license.md](docs/license.md)); preserve third-party attribution.
- **Programs are not machine ready.** Nothing here establishes physical printing, collision safety or machine qualification.

### Pre-release format evolution

Before the first **published 0.1.0 release**, keep the current numerical values of unreleased project, process and manufacturing format versions stable—not their data models. Cargo package versions already set to `0.1.0` do not mean that release has happened. Update Rust contracts, protobuf, generated consumers and fixtures in lockstep; do not increment format counters per feature or renumber, unify or roll back existing versions.

Older development files or artifacts may need recreation. Fail clearly rather than reinterpret missing or changed data or weaken validation; do not add backward readers, migrations, aliases or compatibility defaults for nonexistent released-file compatibility. Preserve producer/algorithm identity checks where needed to invalidate stale compiled manufacturing output; change those identities when the affected production or verification semantics require invalidation, not automatically for every data-model edit. Change a binary format revision only for an actual incompatible binary representation, layout or interpretation change. This policy does not introduce a new version or build-ID scheme.

At the initial `0.1.0` release gate, decide the supported on-disk compatibility policy **before publishing the release**; do not assume unlimited backward compatibility.

## 2. Where things live

| Path                                           | Owns                                                                                          |
| ---------------------------------------------- | --------------------------------------------------------------------------------------------- |
| `xtask/`                                       | Cargo-native build, launch, checks, bundles, provenance and macOS tooling; no Node dependency |
| `apps/engine/`                                 | Engine process; composes native services                                                      |
| `apps/cli/`, `apps/workbench/`                 | Clients of the engine protocol; no planning logic                                             |
| `crates/contracts/`                            | IDs, revisions, commands, errors, manifests, plan/program contracts                           |
| `crates/engine-client/`                        | Protocol client and child-process lifecycle                                                   |
| `crates/core/`, `crates/core-ptac/`            | Project transactions, undo, recovery; the `.spil` SQLite container                            |
| `crates/chunks/`                               | Compressed immutable byte frames for large outputs                                            |
| `crates/geometry/`                             | Kernel adapter, STEP import, tessellation, sections, tolerant sections                        |
| `crates/manufacturing/`                        | Planner (walls, fill, skins, supports, Cathedral), realization, emitter, verifier             |
| `crates/deposition-routing/`                   | Task ordering, seams and tool sequence                                                        |
| `crates/compute/`                              | CubeCL CPU/GPU numeric batches; never geometry authority                                      |
| `crates/appearance/`, `crates/surface-fields/` | Colour matching; image fields for finishes                                                    |
| `packages/protocol/`                           | Generated TypeScript contracts (do not hand-edit)                                             |
| `fixtures/`, `bench/`, `tools/`                | Test corpus, workloads, generation and corpus harnesses                                       |

Dependencies point inward: contracts depend on nothing project-specific; geometry on contracts and SolidCore; manufacturing on contracts, geometry, compute and routing; core never on geometry or manufacturing; clients never own geometry or planning. Do not add a crate without a second consumer or a real isolation need.

## 3. How to work

1. Read this file, then the brain's `AGENTS.md` and the live card for your task, then any `AGENT-RULES.md` along the directories you will touch.
2. Change the owning area; reuse its patterns. Update every affected caller, contract and generated file together (`cargo xtask contracts`; optional TypeScript authoring checks use `pnpm check:tooling`).
3. Verify through the real surface, not unit tests alone:
   - `cargo test --locked -p <crate>` for touched crates (set `RUST_MIN_STACK=8388608`; use `--profile profiling` for manufacturing).
   - Real parts: `tools/realistic-corpus/run.py` against PrusaSlicer, with and without supports (`SPILING_CORPUS_SUPPORTS`); the wall generator census is `wall_census` in `crates/manufacturing/src/planner/walls/tests.rs`.
   - `cargo xtask contracts --check`; optional TypeScript/observer checks use `pnpm check:tooling` and `pnpm test:tooling`. Clippy with warnings denied on touched crates.
4. Commit with a message that states what changed, why, and what was verified, including known failures. Push to `main` after rebasing; never force-push. Brain changes are committed and pushed to the brain's `main` the same way.

There is no remote CI. Report only commands you actually ran.

## 4. Index

| Topic                                                           | Canonical document                                                                                     |
| --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| Tolerance policy, curves, feature heights                       | [ADR012](docs/adr/012-tolerant-brep-curves-and-feature-heights.md)                                     |
| BREP authority and its limits                                   | [ADR005](docs/adr/ADR005-native-first-development-geometry.md)                                         |
| Resource policy                                                 | [ADR009](docs/adr/009-resource-envelope-policy.md)                                                     |
| Construction goals, layer heights                               | [ADR008](docs/adr/008-construction-goals.md)                                                           |
| Workbench architecture                                          | [ADR006](docs/adr/006-native-egui-workbench.md)                                                        |
| Project storage                                                 | [ADR007](docs/adr/007-ptac-project-storage.md), [project-storage](docs/protocol/project-storage.md)    |
| Alternatives, print layouts                                     | [ADR010](docs/adr/010-manufacturing-alternatives.md), [ADR011](docs/adr/011-print-layouts.md)          |
| Manufacturing plan, walls, fill, supports, Cathedral, materials | [protocol/manufacturing.md](docs/protocol/manufacturing.md)                                            |
| Projects, finishes, appearance                                  | [protocol/project.md](docs/protocol/project.md)                                                        |
| Hardware, library, control, replay, display, analysis           | [docs/protocol/](docs/protocol/)                                                                       |
| Build, run, smoke and corpus procedures                         | [quality/development.md](docs/quality/development.md)                                                  |
| Geometry and manufacturing evidence                             | [geometry](docs/quality/geometry-evidence.md), [manufacturing](docs/quality/manufacturing-evidence.md) |
| SolidCore fork changes                                          | [solidcore-upstream-review.md](docs/quality/solidcore-upstream-review.md)                              |

## 5. Local guidance files

A directory may have an `AGENT-RULES.md` (what you must know to change it correctly) and rarely an `AGENT-VISION.md`. Add one only for non-obvious, durable local rules; aim for about 300 words. State rules, not history: no changelogs, build results, test counts or narratives of how a fix was found. When a change makes guidance stale, rewrite it in place. Status, rationale and plans belong in the brain or an ADR, not here.
