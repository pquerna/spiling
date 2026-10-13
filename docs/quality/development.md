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

This launches the project/inspection and software-manufacturing workbench, not a machine-ready manufacturing product. The desktop displays the complete OSL license offline and requires assent before entering the shell. WebGPU initialization gates normal engine startup; there is no WebGL or system-webview fallback. The manufacturing panel uses the same native services as the CLI and offers no printer execution.

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

Before merging, verify changed interface consumers, relevant behavioral properties and the actual changed surface. Use focused checks on resource-constrained hosts; do not repeat unrelated suites or rebuild release/CEF artifacts merely to satisfy a blanket command. `pnpm check` and `pnpm test` remain the full-workspace checks for broad changes and release preparation. Record exactly which checks ran and any unverified gates.

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

Printer specifications are TypeScript-authored data against generated Rust contracts. The trusted authoring command type-checks and executes a source module with operator permissions; it is not a sandbox. It exclusively creates bounded inert JSON and rejects nonfinite/non-JSON output. The application loads a regular JSON file bounded to 32 KiB through single-use native selection tokens; neither desktop nor engine executes TypeScript. Full-profile loading retains printer identity, capabilities, component declarations, firmware/configuration/calibration records and explicit recipe process/support policy. Apply is the only intent mutation; scalar drafts preserve the full printer declaration rather than rebuilding a reduced profile.

```sh
pnpm manufacturing:intent fixtures/manufacturing/solid-fill.intent.ts NEW_INTENT.json
"$CLI" project create NEW_PRINT_PROJECT --import fixtures/geometry/box-mm.step --pose 1 '{"translation_mm":[15,25,2],"rotation_xyzw":[0,0,0,1]}' --intent NEW_INTENT.json --center-on-bed 1 --preflight --compile --verify --manufacturing-out NEW_SOFTWARE_EXPORT --replay-out NEW_ACTUAL_MOTION.splr --engine "$ENGINE"
"$CLI" project inspect NEW_PRINT_PROJECT --preflight --verify --manufacturing-out NEW_REOPENED_EXPORT --replay-out NEW_REOPENED_MOTION.splr --engine "$ENGINE"
```

Use `fixtures/manufacturing/complete-sparse.intent.ts` instead of the solid diagnostic intent to exercise the typed complete Marlin lifecycle, 25% rectilinear interior and physical-thickness skins. Its configuration/calibration records explicitly contain no measurements; it is not a qualified machine profile. Author into another new output path before using the same CLI workflow.

Flags execute left-to-right: select intent before bed preparation, then Preflight before Compile. `--place-on-bed ID` preserves horizontal placement; `--center-on-bed ID` also centers in the selected printer envelope. Both share engine-client certified native-bound axis-permutation preparation with desktop, preserve quaternion and commit only translation through the existing Core SetInstancePose transaction with `baseRevision`; Undo and artifact invalidation are ordinary Core behavior. Arbitrary tilted orientations or nonfitting bounds are rejected, not estimated from renderer meshes. There are no hidden printer/recipe defaults.

Preflight is a read-only durable Google operation on the same manufacturing worker, ledger and ACK lifecycle. It retains revision-bound checks, including historical results, but creates zero artifacts and changes neither Core nor scene state. `eligible` means inexpensive input/profile/orientation/placement eligibility only; native layer, support, bridge and offset analysis remains Compile's responsibility. Compile and Verify capture input revisions; stale results cannot replace current state. Fresh read-only verification independently replays stored exact program bytes, not a stored `verified` flag.

CLI export exclusively writes `plan.json`, `program.gcode`, fresh `verification.json`, captured `provenance.json` and a completion `manifest.json` after engine cleanup. The desktop uses the same exclusive four-file writer; its completion receipt is transient rather than a session manifest. `--replay-out` requests fresh report-and-motion inspection and exclusively writes a complete validated SPLR file, with its bound descriptor in CLI JSON. Existing destinations are never overwritten. Compare immutable files, not transient completion/session/timing metadata. See [manufacturing contracts](../protocol/manufacturing.md), [packed replay](../protocol/replay.md) and [measured evidence](manufacturing-evidence.md).

The historical exact-kernel three-part scenario copied `box-mm.step`, `cylinder.step` and `through-hole.step` to an exclusively created operator-source directory, imported/posed the cylinder at `[40,20,0]` mm and hole at `[80,0,0]` mm, then applied intent, compiled, verified and exported. Source deletion followed by fresh read-only verification/export established immutable-file identity and child cleanup at that checkpoint. Its measurements are historical, not a current schema-3 run; the current box/nonintegral observations and remaining gates are recorded in [manufacturing evidence](manufacturing-evidence.md#schema-3-operator-workflow-software-evidence).

Schema-3 planning uses nominal layer spacing plus native horizontal events and exact top, recording each layer's actual height rather than folding a final partial layer into nominal accounting. Physical skin horizons use distances. Explicit first-layer flow/speed, fan, bridge and retraction policies govern realization; breakaway support and interface ownership remain separate from object ownership. Contact-gap schedule layers have zero object/support deposition and do not increase the next deposited bead height. These are implemented software contracts, not universal native-geometry acceptance: the actual nonintegral-height CLI case passed, while sparse cantilever endpoint/partial-top semantics and the actual 1° tilted engine case remain unpassed. See the [current operator evidence and gates](manufacturing-evidence.md#schema-3-operator-workflow-software-evidence).

Report-only inspection performs fresh independent verification without display allocation. Report and motion publishes SPLR schema 3 derived from independently decoded emitted bytes, with distinct bridge/support/interface roles and stationary XYZ retract/unretract events carrying actual absolute E. Fan state is process state, not invented motion; homing trajectories and an invented first start remain absent. Object, support, prime and recovered-retraction totals are separate; recovery is not new deposition. Replay controls do not regenerate geometry. Fresh export checks current session and server-selected bundle/input hashes again after transport; Save bookkeeping does not make old verification fresh.

The desktop exposes full intent loading/Apply, bed preparation and actionable Preflight through the same services. The new production CEF controls, native profile selection and complete workbench workflow have not yet been exercised; historical panel evidence does not establish their acceptance. Component shapes remain metadata, not collision verification. Diagnostic and complete Marlin jobs say `SOFTWARE VALIDATION ONLY, NOT MACHINE READY`; actual firmware, sensor truth, physical calibration, execution and printability remain unqualified.

## Resource-controlled local verification

Keep native builds, actual-engine workloads and CEF diagnostics sequential. The workspace development/test profile disables debug information and incremental compilation to avoid duplicate bulky artifact variants. Reuse that profile and disk-backed Cargo storage; `/tmp` may be tmpfs and consume the same RAM needed by compiler/kernel workloads. Check free disk before native builds and do not infer memory capacity from apparent tmpfs free space. Cargo's target directory is a disk cache, not a RAM allowance.

On Linux with systemd, enforce limits on the entire command/child cgroup rather than only Cargo concurrency. This example uses a disk-backed target directory and limits memory, swap and task count:

```sh
systemd-run --wait --pipe --collect \
  --property=WorkingDirectory="$PWD" \
  --property=MemoryMax=3G --property=MemorySwapMax=256M --property=TasksMax=64 \
  /usr/bin/env PATH="$PATH" CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 \
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 RUST_TEST_THREADS=1 \
  RAYON_NUM_THREADS=1 CARGO_TARGET_DIR="$PWD/target" \
  cargo test --locked -p spiling-manufacturing actual_motion_inspection_rounding_coincident_travel_and_fixture -- --test-threads=1
```

System limits may require administrator privileges. A limit-triggered failure is a failed check, not permission to lift limits or weaken acceptance silently. Nonpreemptible upstream calls and bounded retained buffers are not allocator-level RSS guarantees. Actual-child suites must point `SPILING_TEST_ENGINE` to the newly built integrated engine and explicitly include ignored cases. Unit tests alone do not establish the native CLI/desktop workflow.

Reusing the same disk-backed target and profile avoids duplicate dependency builds. Set `RAYON_NUM_THREADS=1` as well as single-threaded Rust tests: actual writer/reader/contender scenarios launch several engines, each otherwise initializes its own Rayon pool and may exhaust the task limit. For focused integration verification, run only missing or corrected test targets rather than repeating suites that already passed; preserve the complete exercised coverage in evidence. Do not rebuild release/CEF distributions, installers or benchmarks unless their surfaces changed.

Trusted TypeScript intent authoring defaults the Go-backed type checker to `GOMAXPROCS=1`, retaining an explicit operator override. Under a tight task-count cgroup, also use `UV_THREADPOOL_SIZE=1 NODE_OPTIONS=--v8-pool-size=1` for the Node command rather than lifting the cap after thread exhaustion.

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

For development-native checks, build current frontend assets and stage the matching engine **before** building the desktop: `pnpm build` implements that order. A direct desktop Cargo build can copy an old staged sidecar over the engine executable; sharing a warm target does not validate sidecar identity.

`--scenario manufacturing` exercises the actual production panel, native intent/history/compile/verification, motion transport, exclusive export, source-independent read-only reopen and child restart/cleanup. It requires working WebGPU by default. On a headless Linux diagnostic host, `--diagnostic-no-gpu` explicitly preserves the startup GPU rejection, then enables a startup-gated native-panel attachment:

```sh
SPILING_ENGINE_PATH="$PWD/target/debug/spiling-engine" SPILING_CEF_UNSANDBOXED=1 \
  xvfb-run -a node tools/smoke-desktop.mjs --executable "$PWD/target/debug/spiling-desktop" \
  --scenario manufacturing --diagnostic-no-gpu --port 9222
```

This mode proves panel/native software behavior, not rendered motion, picking, native-dialog interaction, normal sandboxing, hardware support or packaging certification. Use the resource limits above, with a separately chosen CEF task allowance; do not overlap it with native compilation.

## Release evidence

Use [B0-plan.md](B0-plan.md) for acceptance and [SOURCE.md](SOURCE.md) for source distribution. Record only exercised results in the milestone evidence; missing Windows/macOS/hardware access remains an unmet gate. Preserve [license requirements](../license.md) and do not turn local rules/vision into a chronological release log.
