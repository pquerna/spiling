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

The native CEF runtime is a substantial download. Cargo pins `cef`/`cef-dll-sys` **152.3.0+152.0.6**: Rust bindings version 152.3.0 and native CEF distribution **152.0.6**. Its build utility obtains the matching native distribution, and Tauri's CLI detects the dependency and bundles CEF rather than a system webview. Exact dependency resolution lives in Cargo.lock. See the upstream [CEF example](https://github.com/tauri-apps/tauri/tree/tauri-v3.0.0-alpha.4/examples/cef) for the runtime/packaging mechanism. Do not set an arbitrary `CEF_PATH` without matching the pinned distribution and recording its checksum/provenance.

Root tools use ignored `.cache/cef` for the shared build/package distribution and default to one Cargo build job; `CARGO_BUILD_JOBS` can explicitly override concurrency. Native archive integrity metadata is retained in provenance. The upstream downloader verifies the CDN archive SHA-1; this is not signature authentication. Cargo and pnpm lockfiles independently pin their package integrity. Job count is not a hard memory bound; use externally enforced limits on resource-sensitive hosts.

## Running the development shell

After checking out the source and installing the prerequisites:

```sh
pnpm bootstrap
pnpm dev
```

This launches the unfinished project/inspection workbench, not a machine-ready manufacturing product. The desktop displays the complete OSL license offline and requires assent before entering the shell. WebGPU initialization gates normal engine startup; there is no WebGL or system-webview fallback. Software manufacturing is currently an engine/CLI workflow, not desktop manufacturing UI.

## Headless operation diagnostics

The engine uses local authenticated gRPC with standard Google Operations and ByteStream. No separate protoc installation is needed: the build uses a pinned vendored binary. Engine and shell are built together; cross-version compatibility and protocol overrides are not supported.

```sh
cargo build --locked -p spiling-engine -p spiling-cli
target/debug/spiling-cli job --chunks 4 --delay-ms 250
# Retain operations across engine runs; retry with the same request ID/inputs:
target/debug/spiling-cli job --store /absolute/path/store --request-id UUID
```

Progress is stderr JSON; the final stdout report follows orderly shutdown. The shell stores operation records in its application local-data directory and displays complete partial output as it arrives. Cancellation requests stop the job independently of observation/transfer calls. Stores retain bounded operations/output reservations; reaching capacity is reported explicitly. Diagnostic and native output budgets are separate. See [the protocol](../protocol/control.md) for semantics and implementation limits.

## Root commands

```sh
pnpm bootstrap      # install locked deps, generate contracts, stage engine, build frontend + native CEF
pnpm dev            # Tauri development application with engine stderr in the same console
pnpm contracts      # regenerate Rust-owned TypeScript contracts
pnpm check          # drift, Rust formatting/clippy, TypeScript and repository formatting
pnpm test           # Rust behavior tests and real CLI / TypeScript binary interoperability
pnpm smoke:cli      # real child readiness, operation progress and triangle decode
pnpm bench:smoke    # ten fresh-process diagnostic transfers; not a geometry benchmark
pnpm package        # current-platform distributable with native sidecar and notices
```

macOS development must use Tauri CLI so it launches the CEF application bundle and helper processes correctly. A bare Rust executable is not an equivalent macOS development workflow.

`pnpm package` selects Debian on Linux, `.app` on macOS and NSIS on Windows. It compiles release without bundling, collects native CEF notices, then bundles the already-built binaries; a separate debug bootstrap is not required after installing prerequisites and locked dependencies. Artifacts are under `target/release/bundle/`. These are development packages until platform acceptance, source access, third-party licensing, signing and distribution assent have been reviewed.

## CI cost policy

Material source/configuration changes run one Linux job: generated-contract drift, TypeScript consumers, selected engine/protocol Rust tests, real CLI incremental transfer/cleanup and TypeScript binary tests. Markdown-only changes do not launch builds. Automatic CI does not compile the desktop/CEF, run Clippy, build installers or claim cross-platform support.

Run `pnpm check && pnpm test` locally before merging or preparing release evidence. These retain full workspace formatting, Clippy and behavior checks; cheaper CI is not permission to skip them.

Native packaging is opt-in through the workflow's `platform` input (`linux`, `macos`, `windows`, or explicit `all`; default Linux). For example, `gh workflow run build.yml -f platform=windows` requests Windows distribution evidence. Each selected platform compiles release once, without repeating debug bootstrap or deep workspace checks.

Automatic checks have a 10-minute cap; manual packaging has a 30-minute cap. Cache keys follow the pinned Rust toolchain and lockfile, not each commit. Automatic CI caches lean Rust build artifacts and downloads; packaging caches downloads only, avoiding large CEF distributions and native build trees. Development package uploads expire after three days. CI packages are neither signed releases nor physical hardware certification.

## Native geometry and recoverable authoring

The native engine uses the exact public Monstertruck fork pin in Cargo.toml/Cargo.lock. Frozen original/adversarial fixtures and independent-exporter admission limits are recorded in [geometry evidence](geometry-evidence.md). STEP admission is deliberately restricted; native conversion failure never substitutes a synthetic triangle or alternate kernel.

```sh
cargo build --locked -p spiling-engine -p spiling-cli
CLI="$PWD/target/debug/spiling-cli"
ENGINE="$PWD/target/debug/spiling-engine"
"$CLI" geometry --source fixtures/geometry/box-mm.step --section 0,0,4:0,0,1 --engine "$ENGINE"
"$CLI" geometry --scene fixtures/geometry/scenes/two-parts.scene.json --section 0,0,4:0,0,1 --engine "$ENGINE"
"$CLI" project create NEW_PROJECT_DIRECTORY --import fixtures/geometry/box-mm.step --import fixtures/geometry/cylinder.step --engine "$ENGINE"
"$CLI" project inspect NEW_PROJECT_DIRECTORY --section 0,0,4:0,0,1 --engine "$ENGINE"
```

Use the actual `CARGO_TARGET_DIR` instead of `target` when overridden. Create/export destinations must not already exist. Authoring flags execute left-to-right: import, shared add, pose, remove, undo/redo and save are explicit operations. A drag commits once. Core owns project revision/history/persistence, while the native session owns derived scene identity. Save retains undo/redo; original source paths are unnecessary after checkpointing. Opens/saves/imports/sections are Google operations, not a second job service. See [project format](../protocol/project.md).

Project `open` is an explicit writer without implicit save; `edit` applies ordered operations and saves; `inspect` reads a committed snapshot; `recover` explicitly opens the previous validated checkpoint dirty. Read-only rejects mutations but permits native inspection. Save uncertainty after actual manifest replacement must be reconciled against authoritative status; the CLI does not assume rollback or retry automatically. Success reports and completion manifests follow observed child exit.

## Software-only manufacturing

Printer specifications are trusted TypeScript-authored data against generated Rust contracts. The authoring command type-checks and executes a source module with operator permissions; it is not a sandbox. It rejects nonfinite/non-JSON output and exclusively creates a bounded JSON file. Rust validates inert intent and owns planning, backend behavior and independent replay; the engine never executes the TypeScript module.

```sh
pnpm manufacturing:intent fixtures/manufacturing/solid-fill.intent.ts NEW_INTENT.json
"$CLI" project create NEW_PRINT_PROJECT --import fixtures/geometry/box-mm.step --intent NEW_INTENT.json --compile --verify --manufacturing-out NEW_SOFTWARE_EXPORT --engine "$ENGINE"
"$CLI" project inspect NEW_PRINT_PROJECT --verify --manufacturing-out NEW_REOPENED_EXPORT --engine "$ENGINE"
```

There are no hidden printer/recipe defaults. Compile and Verify capture the persistent project revision and return Google operations. Artifact invalidation and undo restoration are core transactions. Fresh read-only inspection independently replays exact stored program bytes; it never trusts a stored `verified` flag.

Export exclusively writes `plan.json`, `program.gcode`, fresh `verification.json`, captured `provenance.json` and a completion `manifest.json` after engine cleanup. Compare immutable files, not transient completion/session/timing metadata. See [manufacturing contracts](../protocol/manufacturing.md) and [measured evidence](manufacturing-evidence.md).

The measured three-part scenario copies `box-mm.step`, `cylinder.step` and `through-hole.step` to an exclusively created operator-source directory. In a new project, import the box, import the cylinder and pose occurrence 2 with `{"translation_mm":[40,20,0],"rotation_xyzw":[0,0,0,1]}`, then import the hole and pose occurrence 3 with `{"translation_mm":[80,0,0],"rotation_xyzw":[0,0,0,1]}`. Apply the authored intent, compile, verify and export. Remove only the owned operator-source directory; inspect the saved project in a fresh read-only process, verify and export to another new directory. Check captured provenance and all four immutable files byte-for-byte; assert both reported engine PIDs no longer exist. Fixture hashes and observed measurements are in [manufacturing evidence](manufacturing-evidence.md).

Support is world-vertical planar/cylindrical native geometry, bed-anchored integral fixed-height layers and nominal 100% solid fill. Slopes, tilted cylinders, nonaligned shelves, partial top layers, generated supports, unresolved thin regions and erased wall topology are rejected. Box/cylinder component shapes are retained metadata, not collision verification. Programs say `SOFTWARE VALIDATION ONLY, NOT MACHINE READY`; no heating, homing, firmware initialization, printer execution, physical support or printability is claimed.

## Resource-controlled local verification

Keep native builds, actual-engine workloads and CEF diagnostics sequential. Disable incremental compilation/debug information on constrained hosts and use disk-backed Cargo storage; `/tmp` may be tmpfs and consume the same RAM needed by compiler/kernel workloads. Do not infer memory capacity from apparent tmpfs free space.

On Linux with systemd, enforce limits on the entire command/child cgroup rather than only Cargo concurrency. This example uses a disk-backed target directory and limits memory, swap and task count:

```sh
systemd-run --wait --pipe --collect \
  --property=WorkingDirectory="$PWD" \
  --property=MemoryMax=3G --property=MemorySwapMax=256M --property=TasksMax=64 \
  /usr/bin/env PATH="$PATH" CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 \
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 RUST_TEST_THREADS=1 \
  RAYON_NUM_THREADS=1 CARGO_TARGET_DIR="$PWD/target" \
  cargo test --locked --workspace -- --include-ignored --test-threads=1
```

System limits may require administrator privileges. A limit-triggered failure is a failed check, not permission to lift limits or weaken acceptance silently. Nonpreemptible upstream calls and bounded retained buffers are not allocator-level RSS guarantees. Actual-child suites must point `SPILING_TEST_ENGINE` to the newly built integrated engine and explicitly include ignored cases. Unit tests alone do not establish the native CLI/desktop workflow.

Reusing the same disk-backed target and profile avoids duplicate dependency builds. Set `RAYON_NUM_THREADS=1` as well as single-threaded Rust tests: actual writer/reader/contender scenarios launch several engines, each otherwise initializes its own Rayon pool and may exhaust the task limit. For focused integration verification, run only missing or corrected test targets rather than repeating suites that already passed; preserve the complete exercised coverage in evidence. Do not rebuild release/CEF distributions, installers or benchmarks unless their surfaces changed.

## Architecture and contributing

The [bootstrap architecture](B0-plan.md#build-boundaries) describes current module responsibilities and dependency boundaries. The [control protocol](../protocol/control.md) defines the engine/client wire contract; [acceptance evidence](B0-evidence.md) separates implemented code from verified behavior.

Before changing an area, read the [repository guide](../../AGENTS.md), then its `AGENT-RULES.md` and `AGENT-VISION.md` along the directory ancestry. Update affected contracts, consumers, and canonical rules together. Keep development history in version control and measured milestone evidence, not the README or area rules/vision.

## Packaged desktop diagnostics

The desktop loads the license offline and asks for local explicit assent. WebGPU must initialize before normal engine startup. The native scene starts empty and displays admitted shared definitions/rigid occurrences, source-face picking and native sections. Synthetic Triangle remains an explicit diagnostic, never an import fallback. Kernel identity reports the linked exact fork pin and implemented bounded capabilities, not physical hardware support.

To exercise a built application automatically, pass its **packaged/installed executable**, not an unrelated browser:

```sh
pnpm smoke:desktop --executable /path/to/packaged/spiling
```

The smoke opens opt-in CEF debugging, checks the real UI/engine, captures screenshots under ignored `artifacts/`, kills the real engine to exercise crash detection, restarts/stops it, and closes the native window to check cleanup. `--scenario geometry` exercises admitted geometry and renderer interactions. `--scenario project-bridge` separately exercises native source-backed save/restart/reopen, persistent face identity, raw mesh decoding and read-only rejection without requiring a functioning WebGPU surface; it is not UI/picking/physical GPU certification. `--scenario gpu-unavailable` injects unavailability before frontend startup and likewise does not prove hardware support.

Root/headless containers cannot establish normal sandbox or hardware acceptance. For a **diagnostic-only** software-rendered container run, explicitly opt in:

```sh
SPILING_CEF_UNSANDBOXED=1 SPILING_CEF_SOFTWARE_GPU=1 xvfb-run -a pnpm smoke:desktop --executable /path/to/packaged/spiling
```

Do not ship those settings as defaults. `SPILING_CEF_DEBUG_PORT` opens a privileged local debugging endpoint and is off by default. `SPILING_ENGINE_PATH` is an explicit development override. Installed packages resolve the engine beside the actual executable, except CEF Debian packages: their desktop is in `share/Spiling` and their sidecar is in `bin` under the same installation prefix.

## Release evidence

Use [B0-plan.md](B0-plan.md) for acceptance and [SOURCE.md](SOURCE.md) for source distribution. Record only exercised results in the milestone evidence; missing Windows/macOS/hardware access remains an unmet gate. Preserve [license requirements](../license.md) and do not turn local rules/vision into a chronological release log.
