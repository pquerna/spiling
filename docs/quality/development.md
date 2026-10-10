<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Development and packaging

## Prerequisites

- Node **24.x** (reference **24.15.0**) and pnpm **10.32.1**: `npm install --global pnpm@10.32.1`.
- Rustup; the repository pins Rust **1.99.0**, rustfmt and clippy in `rust-toolchain.toml`.
- C/C++ linker/build tools. Windows uses MSVC build tools; macOS uses Xcode command-line tools.
- Linux GTK4 and CEF runtime libraries. On Ubuntu 24.04:

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config cmake ninja-build libgtk-4-dev libx11-dev libxrandr-dev libxcomposite-dev libxdamage-dev libnss3-dev libasound2-dev libgbm-dev libsecret-1-dev libssl-dev patchelf desktop-file-utils
```

For headless Linux diagnostics also install `xvfb xauth dbus-x11 mesa-vulkan-drivers`. Debian 13's optional FUSE package is `libfuse2t64`; Ubuntu uses `libfuse2`. B0 currently packages a Debian artifact on Linux, not an AppImage.

The native CEF runtime is a substantial download. `tauri-runtime-cef` pins CEF **152.3.0**; its build utility obtains the matching native distribution, and Tauri's CLI detects the dependency and bundles CEF rather than a system webview. Exact dependency resolution lives in Cargo.lock. See the upstream [CEF example](https://github.com/tauri-apps/tauri/tree/tauri-v3.0.0-alpha.4/examples/cef) for the runtime/packaging mechanism. Do not set an arbitrary `CEF_PATH` without matching the pinned distribution and recording its checksum/provenance.

Root tools use ignored `.cache/cef` for the shared build/package distribution and default to four Cargo build jobs; `CARGO_BUILD_JOBS` can explicitly override concurrency. Native archive integrity metadata is retained in provenance. The upstream downloader verifies the CDN archive SHA-1; this is not signature authentication. Cargo and pnpm lockfiles independently pin their package integrity.

## Running the development shell

After checking out the source and installing the prerequisites:

```sh
pnpm bootstrap
pnpm dev
```

This launches unfinished durable STEP project authoring, not a manufacturing product. Import declared-profile parts, share/place rigid instances, save/open projects and use bounded session undo/redo through the same engine as the CLI. Restart creates a fresh child and reopens an attached checkpoint; unsaved edits and prior-session undo are not replayed. The complete OSL license is available offline and assent precedes entry. WebGPU initialization gates normal engine startup; there is no WebGL, synthetic-geometry or system-webview fallback.

## Root commands

```sh
pnpm bootstrap      # install locked deps, generate contracts, stage engine, build frontend + native CEF
pnpm dev            # Tauri development application with engine stderr in the same console
pnpm contracts      # regenerate Rust-owned TypeScript contracts
pnpm check          # drift, Rust formatting/clippy, TypeScript and repository formatting
pnpm test           # Rust behavior tests and real CLI / TypeScript binary interoperability
pnpm smoke:cli      # real child handshake, triangle decode and protocol mismatch
pnpm bench:smoke    # ten fresh-process diagnostic transfers; not a geometry benchmark
pnpm package        # current-platform distributable with native sidecar and notices
```

macOS development must use Tauri CLI so it launches the CEF application bundle and helper processes correctly. A bare Rust executable is not an equivalent macOS development workflow.

`pnpm package` selects Debian on Linux, `.app` on macOS and NSIS on Windows. It compiles release without bundling, collects native CEF notices, then bundles the already-built binaries; a separate debug bootstrap is not required after installing prerequisites and locked dependencies. Artifacts are under `target/release/bundle/`. These are development packages until platform acceptance, source access, third-party licensing, signing and distribution assent have been reviewed.

## CI cost policy

Material source/configuration changes run one Linux job: generated-contract drift, TypeScript consumers, contract/shared-transport Rust tests and independent TypeScript binary tests. Markdown-only changes do not launch builds. Automatic CI deliberately excludes native engine/CLI corpus workloads, desktop/CEF, Clippy and installers; these are explicit local or opt-in checks, not replaced by mock geometry. No cross-platform support is inferred.

Run `pnpm check && pnpm test` locally before merging or preparing release evidence. These retain full workspace formatting, Clippy and behavior checks; cheaper CI is not permission to skip them.

Native packaging is opt-in through the workflow's `platform` input (`linux`, `macos`, `windows`, or explicit `all`; default Linux). For example, `gh workflow run build.yml -f platform=windows` requests Windows distribution evidence. Each selected platform compiles release once, without repeating debug bootstrap or deep workspace checks.

Automatic checks have a 10-minute cap; manual packaging has a 30-minute cap. Cache keys follow the pinned Rust toolchain and lockfile, not each commit. Automatic CI caches lean Rust build artifacts and downloads; packaging caches downloads only, avoiding large CEF distributions and native build trees. Development package uploads expire after three days. CI packages are neither signed releases nor physical hardware certification.

Protocol fixture changes under `fixtures/protocol/**` trigger both material path filters, including native-derived goldens. Native corpus/engine/CLI checks remain deliberate local evidence so routine CI does not build the CAD kernel.

## Native geometry inspection

Protocol v3 implements shared project and geometry controls; SPLM/SPLS remain packed schema v1. Rust owns all DTOs and frozen limits. Exercise generation and independent decoders:

```sh
cargo run --locked -p spiling-contracts --bin generate
cargo run --locked -p spiling-contracts --bin generate -- --check
cargo test --locked -p spiling-contracts
pnpm --filter @spiling/protocol check
pnpm --filter @spiling/protocol test
```

The contracts generator owns the small algebraic fixtures. Actual native box/through-hole goldens use `cargo run --locked -p spiling-geometry --example generate_native_goldens`, with `-- --check` for byte drift. Decoder tests check native source hashes, extents, residual, area/winding and chunk order; they do not launch the engine or certify GPU behavior. [Geometry evidence](geometry-evidence.md) separates those surfaces.

### Native facade and original corpus

```sh
cargo run --locked -p spiling-geometry --example generate_corpus --
cargo run --locked -p spiling-geometry --example generate_corpus -- --check
cargo run --locked -p spiling-geometry --example inspect_corpus -- fixtures/geometry/box-mm.step fixtures/geometry/box-inch.step fixtures/geometry/cylinder.step fixtures/geometry/through-hole.step
cargo run --locked -p spiling-geometry --example inspect_corpus -- --mesh-only fixtures/geometry/perforated-plate.step
cargo test --locked -p spiling-geometry
```

The inspector captures source bytes in the example, then calls the public byte-based facade. It reports source hashes, native face identity/carriers/bounds, actual packed chunk counts/errors and native midplane loop area/closure/residual. `--mesh-only` is the bounded transfer sizing surface, not a substitute for mandatory curved native sections. Corpus generation preserves deterministic original source recipes and license sidecars; source hashes and the workload recipe must be frozen before acceptance.

### Real engine and CLI

```sh
cargo build --locked -p spiling-engine -p spiling-cli
target/debug/spiling-cli geometry --scene fixtures/geometry/scenes/two-parts.scene.json --section 0,0,4:0,0,1 --engine "$PWD/target/debug/spiling-engine"
target/debug/spiling-cli geometry --scene fixtures/geometry/scenes/repeated-128.scene.json --engine "$PWD/target/debug/spiling-engine"
target/debug/spiling-cli geometry --scene fixtures/geometry/scenes/large-origin.scene.json --section 1000000000,1000000000,1000000004:0,0,1 --engine "$PWD/target/debug/spiling-engine"
target/debug/spiling-cli geometry --source fixtures/geometry/through-hole.step --section 0,0,4:0,0,1 --out NEW_DIRECTORY --engine "$PWD/target/debug/spiling-engine"
cargo test --locked -p spiling-engine -- --test-threads=1
SPILING_TEST_ENGINE="$PWD/target/debug/spiling-engine" cargo test --locked -p spiling-engine-client -p spiling-cli -- --ignored --test-threads=1
```

Both metadata-only and --out modes fetch/hash/layout-validate every unique chunk. Output exclusively creates a new directory and completion manifest; failures roll back only owned output. Recipes are original evaluation scenes, not saved projects or general STEP hierarchy. Use the actual configured CARGO_TARGET_DIR instead of `target` when overridden.

Durable authoring uses protocol-v4 / format-2 project services, not geometry evaluation recipes. All commands start a fresh supervised engine and emit one JSON report only after observed clean engine shutdown. `project create PATH` creates a new destination and saves the final state; `project edit PATH` opens a writer, applies ordered operations and saves; `project save PATH` makes an explicit checkpoint. `project open PATH` opens a writer without implicit save, `project inspect PATH` opens a read-only committed snapshot, and `project recover PATH` explicitly opens the validated previous checkpoint dirty without repairing the current manifest. Add `--save` to commit recovered content deliberately. `--read-only` is available with open/recover; read-only workflows reject authoring flags before spawning.

```sh
CLI="$PWD/target/debug/spiling-cli"
ENGINE="$PWD/target/debug/spiling-engine"
# NEW_PROJECT_DIRECTORY must not already exist.
"$CLI" project create NEW_PROJECT_DIRECTORY --import fixtures/geometry/box-mm.step --import fixtures/geometry/cylinder.step --engine "$ENGINE"
"$CLI" project inspect NEW_PROJECT_DIRECTORY --section 0,0,4:0,0,1 --engine "$ENGINE"
# Use the stable definition_id printed in report.occurrences/definitions:
"$CLI" project edit NEW_PROJECT_DIRECTORY --add DEFINITION_ID '{"translation_mm":[40,0,0],"rotation_xyzw":[0,0,0.7071067811865476,0.7071067811865476]}' --pose 3 '{"translation_mm":[60,0,0],"rotation_xyzw":[0,0,0,1]}' --undo --redo --save --pose 3 '{"translation_mm":[40,0,0],"rotation_xyzw":[0,0,0,1]}' --undo --section 0,0,4:0,0,1 --engine "$ENGINE"
"$CLI" project open NEW_PROJECT_DIRECTORY --remove 3 --undo --redo --engine "$ENGINE"
"$CLI" project save NEW_PROJECT_DIRECTORY --engine "$ENGINE"
"$CLI" project inspect NEW_PROJECT_DIRECTORY --inspect-face OCCURRENCE_ID FACE_ID --out NEW_INSPECTION_DIRECTORY --engine "$ENGINE"
"$CLI" project recover NEW_PROJECT_DIRECTORY --engine "$ENGINE"
```

Authoring flags execute left-to-right: `--import STEP` imports at identity, `--add DEFINITION_ID POSE_JSON` places a shared definition, `--pose OCCURRENCE_ID POSE_JSON` changes placement, `--remove ID` removes an occurrence, and `--undo`/`--redo` act on this engine session only. A pose uses explicit mm translation and normalized xyzw quaternion. `--save` checkpoints at that point without clearing undo; `--save-as NEW_DIRECTORY` attaches a new exclusive destination for open/edit/recover workflows. Create always uses its positional PATH. Open without save discards unsaved changes on child shutdown; use edit or an explicit final `--save` for durable changes. Repeated CLI invocations preserve project references, not undo history, session IDs, scene epochs or native artifact handles.

Reports expose `project` (identity, persistent/saved revisions, dirty/read-only/recovered and undo/redo availability), `operations` (observed project state after every operation), `scene`, stable definition/occurrence/source-face records, optional `face_inspections`, independently verified unique mesh counts and native per-occurrence section metrics. `--section` and `--out` use the same mandatory chunk validation/output ownership as geometry inspection. Original STEP source paths are not required to reopen a saved project.

On a recoverable save error the CLI queries authoritative project status before terminating/reaping the child. Stderr JSON includes `operation: "save"` and `project`, including `save_uncertain`. A post-replacement durability failure may have attached the actual committed destination while retaining the prior confirmed saved revision; dirty and save_uncertain remain true. This is neither a confirmed durable success nor an assumed rollback. No success report/output completion manifest is emitted, and the CLI never retries automatically. Inspect the committed snapshot explicitly before deciding on another save.

The opt-in actual-child client/CLI tests above implement create/import/shared-add/pose/undo/redo/save, dirty/undo-through-save, fresh-session reopen after source removal, persistent references/native sections, native non-UTF-8 paths, read-only rejection, second-writer lock and kill/shutdown lock release, missing/corrupt snapshots and explicit recovery. They must be run against the built engine; these commands are procedures, not claims of executed acceptance. Engine/core tests additionally own process-crash commit-stage and storage-integrity evidence.

### Software-only manufacturing

Printer specifications are trusted TypeScript-authored data against generated Rust contracts; the engine never executes that code. The authoring tool type-checks the source, executes it with the operator's permissions, rejects nonfinite/non-JSON output and exclusively creates a bounded JSON file. Rust performs semantic/capability validation when intent is submitted. Do not run untrusted author modules.

```sh
pnpm manufacturing:intent fixtures/manufacturing/solid-fill.intent.ts NEW_INTENT.json
"$CLI" project create NEW_PRINT_PROJECT --import fixtures/geometry/box-mm.step --intent NEW_INTENT.json --compile --verify --manufacturing-out NEW_SOFTWARE_EXPORT --engine "$ENGINE"
"$CLI" project inspect NEW_PRINT_PROJECT --verify --manufacturing-out NEW_REOPENED_EXPORT --engine "$ENGINE"
```

All destinations above must be new. `--intent JSON`, `--compile` and `--verify` execute left-to-right; there is no hidden printer or recipe default. Compilation is revision-bound and invalidates/replaces artifacts only through core transactions. Geometry/intent mutation invalidates the active bundle; undo restores coherent inputs and artifact. Save retains both undo and redo. Read-only inspection independently replays the exact persisted program; it never trusts stored `verified`.

`--manufacturing-out NEW_DIRECTORY` exclusively writes `plan.json`, `program.gcode`, fresh `verification.json`, captured `provenance.json` and a completion `manifest.json` after observed engine cleanup. Immutable files may be compared byte-for-byte on fresh reopen; completion metadata includes transient session/timing/project state and is not semantic artifact identity. Original external STEP paths are unnecessary after checkpointing. Errors preserve authoritative project/artifact status, emit no success completion and never retry automatically.

Current support is bed-anchored fixed-height, world-vertical planar/cylindrical native geometry, integral layer-aligned horizontal features, 1..8 walls and 100% solid fill; no slopes, generated supports/bridging, partial top layers, unresolved thin regions or erased requested wall topology. Native tolerance, controlled offsets and rounded nominal bead accounting are declared separately. Component box/cylinder shapes are retained metadata, not collision verification. Programs carry `SOFTWARE VALIDATION ONLY, NOT MACHINE READY`; no firmware initialization, heating, homing, execution, physical printer support or printability is claimed.

See [manufacturing schema](../protocol/manufacturing.md) and [measured software proof](manufacturing-evidence.md). Actual-engine regressions include persisted/reopened source labels and hashes, independent replay/tampering, read-only/resource rejection, stale/cancelled publication, artifact invalidation and history restoration:

```sh
cargo test --locked -p spiling-manufacturing -p spiling-core -- --test-threads=1
SPILING_TEST_ENGINE="$ENGINE" cargo test --locked -p spiling-engine -p spiling-engine-client -p spiling-cli -- --include-ignored --test-threads=1
```

On constrained Linux hosts, set `CARGO_BUILD_JOBS=1`, `RUST_TEST_THREADS=1`, `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0` and `CARGO_PROFILE_TEST_DEBUG=0` before building. Keep those settings consistent to avoid duplicate fingerprint/artifact trees; run compiler, native plate workloads and CEF diagnostics sequentially. Prefer disk-backed build storage: tmpfs artifacts consume RAM and compete with compiler/kernel/GPU allocations. Do not infer available memory from free disk bytes.

### Independent original exporter corpus

`fixtures/geometry/independent-exporters/generate.py` uses optional `cadquery-ocp==7.9.3.1`/OCCT tooling to construct and BRepCheck-validate original analytic box/cylinder/through-hole solids before AP214IS export. It is not another runtime kernel:

```sh
uv run --python /usr/bin/python3 --with cadquery-ocp==7.9.3.1 python fixtures/geometry/independent-exporters/generate.py --check
cargo run --locked -p spiling-geometry --example inspect_corpus -- fixtures/geometry/independent-exporters/occt-box-mm.step
```

Frozen STEP bytes are never silently overwritten. Independent generation validity is not native admission: the box passes import/mesh/section, while the independently exported cylinder and hole currently fail converted native topology certification. Exact hashes and rejections are in geometry evidence. OCP bindings are [Apache-2.0](https://github.com/CadQuery/OCP); OCCT has [LGPL-2.1 with exception](https://dev.opencascade.org/resources/licensing). Neither optional tool is bundled.

### Kernel source maintenance

The Monstertruck source is [pquerna/monstertruck](https://github.com/pquerna/monstertruck), branch `spiling-dev`. Workspace dependencies pin published commit `d87b4d9ced1f3baf31aa771ac0e7c663efb1c001`, based on upstream `1fbc7a52555df6ccfe66aee7c877097ab1852146`. The public fork removes the private agent-guidance submodule and repairs native analytic boolean/projection/loop ownership plus bounded cylinder meshing and trim lookup. The public `resources` submodule and Apache-2.0 licensing remain; exact numerical evidence is recorded separately.

Upstream's default branch is `master`, not `main`. The local checkout `/root/monstertruck` tracks `upstream/master` and pushes `spiling-dev` to the fork. Incorporate upstream updates without rewriting the published integration branch:

```sh
git fetch upstream
git switch spiling-dev
git merge upstream/master
git push origin spiling-dev
```

For another checkout, configure `upstream` as `https://github.com/virtualritz/monstertruck.git`, `branch.spiling-dev.remote=upstream`, `branch.spiling-dev.merge=refs/heads/master`, and `branch.spiling-dev.pushRemote=origin`; push explicitly to `origin spiling-dev`. After testing an update, change all five workspace kernel revisions together and regenerate the corpus manifest's kernel identity. `crates/geometry` consumes those declarations, and Cargo.lock records the resolved kernel packages. Local path overrides are diagnostic-only and must never become the shipped dependency.

## Architecture and contributing

The [bootstrap architecture](B0-plan.md#build-boundaries) describes current module responsibilities and dependency boundaries. The [control protocol](../protocol/control.md) defines the engine/client wire contract; [acceptance evidence](B0-evidence.md) separates implemented code from verified behavior.

Before changing an area, read the [repository guide](../../AGENTS.md), then its `AGENT-RULES.md` and `AGENT-VISION.md` along the directory ancestry. Update affected contracts, consumers, and canonical rules together. Keep development history in version control and measured milestone evidence, not the README or area rules/vision.

## Packaged desktop diagnostics

The desktop loads the license offline and asks for local explicit assent. WebGPU must initialize before starting the engine. The initial viewport is empty; admitted native definitions are shared across rigid occurrences, with source-face picking and native section overlays. Kernel identity reports the actually linked exact public fork; geometry capabilities are bounded implemented operations, not physical hardware claims.

Exercise the actual CEF application, either a compiled Linux development executable with its native CEF resources or a packaged/installed executable—not an unrelated browser. macOS still requires the proper Tauri/CEF application bundle and helpers:

```sh
pnpm smoke:desktop --executable /path/to/packaged/spiling
```

```sh
pnpm smoke:desktop --executable /path/to/spiling-desktop --scenario geometry --scene fixtures/geometry/scenes/two-parts.scene.json --fixture-root fixtures/geometry
```

The normal scenario exercises the empty native session and lifecycle. Geometry adds real DOM admission/import, poses/shared instances, canvas face picks, native section overlays, actual plate job cancellation and multi-chunk transfer pause/resume, then fresh-session restart/stop/close. Original frozen recipes can be selected with --scene. Screenshots are under ignored `artifacts/`; source paths use a startup-fixed canonical fixture root, the same session-bound token store and actual native jobs/decoders/uploads. This privileged relative-path gate requires valid opt-in CEF debugging and is disabled in ordinary startup; production UI accepts native dialog tokens, never arbitrary paths.

Additional scenarios are `--scenario mismatch` and `--scenario gpu-unavailable`; the latter injects GPU unavailability before frontend startup and does not prove physical hardware support. A job stuck inside an upstream call is interrupted by the independent watchdog, not reported as successful cooperative cancellation.

`--scenario project-bridge` is a separate qualified native/storage diagnostic. It captures the actual project controls and unsupported surface, then deliberately invokes the real CEF/native bridge without requiring a WebGPU viewport. It imports copied original box/cylinder sources, saves shared placed occurrences, proves undo-to-saved, removes the original source paths, restarts/reopens, independently decodes raw mesh transfers, checks persistent face identity/read-only rejection and observes child cleanup. It uses and removes only its exclusive scratch project. This is not UI authoring, native-dialog, picking or physical WebGPU certification:

```sh
SPILING_CEF_UNSANDBOXED=1 xvfb-run -a pnpm smoke:desktop --executable /path/to/spiling-desktop --scenario project-bridge
```

Root/headless containers cannot establish normal sandbox or hardware acceptance. For a **diagnostic-only** software-rendered container run, explicitly opt in:

```sh
SPILING_CEF_UNSANDBOXED=1 SPILING_CEF_SOFTWARE_GPU=1 xvfb-run -a pnpm smoke:desktop --executable /path/to/packaged/spiling
```

Do not ship those settings as defaults. `SPILING_CEF_DEBUG_PORT` opens a privileged local debugging endpoint and is off by default. `SPILING_PROTOCOL_VERSION=5` deliberately exercises a mismatch against current v4; older protocols are also rejected. `SPILING_GEOMETRY_FIXTURE_ROOT` is a startup-only privileged fixture admission root and has no effect without valid debugging opt-in. `SPILING_ENGINE_PATH` is an absolute debug-build development override. Installed packages resolve the engine beside the actual executable, never the development target tree.

## Release evidence

Use [B0-plan.md](B0-plan.md) for acceptance and [SOURCE.md](SOURCE.md) for source distribution. Record only exercised results in the milestone evidence; missing Windows/macOS/hardware access remains an unmet gate. Preserve [license requirements](../license.md) and do not turn local rules/vision into a chronological release log.
