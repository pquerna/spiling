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

The development profile disables debug information and incremental compilation.
Only the certified numerical crates, `monstertruck-modeling` and
`spiling-geometry`, use optimization level 2; application/control crates retain
the ordinary development settings. This keeps native interaction practical
without an optimized desktop-stack rebuild. Use one warm target and serialize
compiler, native workload and CEF verification on constrained hosts. Numerical
optimization does not relax source, work, memory, tolerance or watchdog limits.

Linux hardware startup enables CEF's native Vulkan compositor without overriding driver or adapter safety blocks. The executable directly links the shipped `libvulkan.so.1`: GTK's transitive Vulkan import does not inherit executable `$ORIGIN` RUNPATH, and mixing its system loader with Dawn's pinned loader can prevent device initialization. Keep the pinned distribution and its loader together; no `LD_LIBRARY_PATH` or replacement native library is required.

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

Flags execute left-to-right: select intent before bed preparation, then Preflight before Compile. `--place-on-bed ID` preserves horizontal placement; `--center-on-bed ID` also centers in the selected printer envelope. Both request a revision-bound read-only native-bound proposal from the engine, preserve the quaternion and explicitly apply only translation through the existing Core SetInstancePose transaction with `baseRevision`; Undo and artifact invalidation are ordinary Core behavior. Desktop shows the proposal before Apply. Tilted placement uses certified directional bounds, not renderer meshes or an axis-permutation shortcut; unresolved or nonfitting bounds refuse. There are no hidden printer/recipe defaults.

Preflight is a read-only durable Google operation on the same manufacturing worker, ledger and ACK lifecycle. It retains revision-bound checks, including historical results, but creates zero artifacts and changes neither Core nor scene state. Its `disposition` is `admitted`, `refused` or `unresolved`; required admission checks must pass, while explicitly identified Compile-only obligations remain `not_evaluated`. Native layer/slab completeness, support, bridge and offset analysis remain Compile's responsibility. Compile and Verify capture input revisions; stale results cannot replace current state. Fresh read-only verification independently replays stored exact program bytes, not a stored `verified` flag.

CLI export exclusively writes `plan.json`, `program.gcode`, fresh `verification.json`, captured `provenance.json` and a completion `manifest.json` after engine cleanup. The desktop uses the same exclusive four-file writer; its completion receipt is transient rather than a session manifest. `--replay-out` requests fresh report-and-motion inspection and exclusively writes a complete validated SPLR file, with its bound descriptor in CLI JSON. Existing destinations are never overwritten. Compare immutable files, not transient completion/session/timing metadata. See [manufacturing contracts](../protocol/manufacturing.md), [packed replay](../protocol/replay.md) and [measured evidence](manufacturing-evidence.md).

The historical exact-kernel three-part scenario copied `box-mm.step`, `cylinder.step` and `through-hole.step` to an exclusively created operator-source directory, imported/posed the cylinder at `[40,20,0]` mm and hole at `[80,0,0]` mm, then applied intent, compiled, verified and exported. Source deletion followed by fresh read-only verification/export established immutable-file identity and child cleanup at that checkpoint. Its measurements are historical, not a current schema-3 run; the current box/nonintegral observations and remaining gates are recorded in [manufacturing evidence](manufacturing-evidence.md#schema-3-operator-workflow-software-evidence).

Schema-6 compilation combines nominal layer limits, original-source critical events, mapped-image row/seam events and adaptive shape-error refinement. Complete native section/slab certificates must meet the recipe's required `native_shape_error_mm`; unresolved geometry refines within the bounded policy or refuses. Actual layer heights govern bead/flow and physical skin horizons. First-layer/fan/bridge/retraction policy and separate breakaway support/interface/contact-gap ownership remain explicit. Native-section XY chord error, between-plane native shape error, offset error, finish XY error and image vertical error are distinct budgets. Native M, realized D and added geometry stay separate; an invariant source extrusion never licenses reuse of a varying-Z derived field. Earlier schema-3/4/5 measurements remain historical, not automatic acceptance of schema-6 inputs or physical qualification. See [manufacturing evidence](manufacturing-evidence.md).

Report-only inspection performs fresh independent verification without display allocation. Report and motion publishes SPLR schema 3 derived from independently decoded emitted bytes, with distinct bridge/support/interface roles and stationary XYZ retract/unretract events carrying actual absolute E. Fan state is process state, not invented motion; homing trajectories and an invented first start remain absent. Object, support, prime and recovered-retraction totals are separate; recovery is not new deposition. Replay controls do not regenerate geometry. Fresh export checks current session and server-selected bundle/input hashes again after transport; Save bookkeeping does not make old verification fresh.

The desktop exposes full intent loading/Apply, bed preparation and actionable Preflight through the same services. The recorded production CEF manufacturing workflow passed with explicit no-GPU diagnostic attachment; it does not establish physical WebGPU/picking, real-dialog or platform-package acceptance. Source-face finish controls retain original geometry and use fresh emitted-program replay for manufactured paths. Component shapes remain nominal metadata exclusions, not collision certification. Diagnostic and complete Marlin jobs say `SOFTWARE VALIDATION ONLY, NOT MACHINE READY`; actual firmware, sensor truth, physical calibration, execution and printability remain unqualified.

### Source-face vertical ribs

Select a current native occurrence and source face in **Surface Finish**, then explicitly Apply a definition-wide or occurrence-specific whole-face layer. Enabled definition layers compose first; occurrence layers compose afterward; the last matching layer wins. Smooth overrides ribs; disable/remove reveals the lower layer. These are persistent project transactions with ordinary Undo/Redo, save/open and artifact invalidation, not scene/display mutations.

CLI uses the same typed engine route. Discover admitted face IDs through `project inspect ... --inspect-face OCCURRENCE FACE_ID`; do not infer them from STEP entity numbers or display triangles. For a certified full-height rectangular exterior vertical planar face in an upright bed-anchored native extrusion:

```sh
"$CLI" project edit EXISTING_PRINT_PROJECT --surface-finish 1 ADMITTED_FACE_ID occurrence '{"kind":"vertical_ribs","depth_mm":0.1,"pitch_mm":2,"edge_margin_mm":1,"phase_mm":0}' --preflight --compile --verify --manufacturing-out NEW_RIB_EXPORT --engine "$ENGINE"
"$CLI" project inspect EXISTING_PRINT_PROJECT --verify --replay-out NEW_RIB_MOTION.splr --engine "$ENGINE"
"$CLI" project edit EXISTING_PRINT_PROJECT --surface-finish-enabled 1 false --undo --remove-surface-finish 1 --engine "$ENGINE"
```

For a native complete exterior cylindrical face, choose **Cylindrical vertical ribs**, depth/count/phase fraction, or use the same command route:

```sh
"$CLI" project edit EXISTING_PRINT_PROJECT --surface-finish 1 ADMITTED_CYLINDER_FACE occurrence '{"kind":"cylindrical_ribs","depth_mm":0.1,"rib_count":8,"phase_fraction":0}' --preflight --compile --verify --manufacturing-out NEW_CYLINDER_EXPORT --engine "$ENGINE"
```

Cylinder phase is measured from source-local +X about +Z as a fraction of one rib cycle, independent of the native seam/UV and instance pose. Integer count closes the circumference; manufacturing derives pitch from the certified source radius. A split pair of half-cylinder source faces is not one full-face target. Source BREP, history and scopes are independent of the carrier-specific numeric controls.

Layer IDs come from authoritative project status. Native source material and protected holes remain separate from bounded outward finish additions. Selected trim, pose, process parameters, numerical conditioning, work and nominal exclusions can refuse explicitly; no untextured fallback exists. The changed domain feeds ordinary walls, skins, infill and actual-anchor/support analysis before emission/replay. Project5/surface3/manufacturing6/planner8 is the current clean cutover; emitter/verifier4 and SPLR3 remain unchanged. Old projects/bundles have no compatibility reader. The [rib mini-RFC](../adr/002-cylindrical-grip-ribs.md) and [spatial image mini-RFC](../adr/003-spatial-image-relief.md) separate shared scalar/material foundations from carrier-specific admission.

Run the real production authoring→compile→fresh replay→save→source-deletion→read-only reopen/export scenario against the freshly built native shell and matching engine:

```sh
SPILING_ENGINE_PATH="$PWD/target/debug/spiling-engine" SPILING_CEF_UNSANDBOXED=1 SPILING_CEF_SOFTWARE_GPU=1 \
  xvfb-run -a node tools/smoke-desktop.mjs --executable "$PWD/target/debug/spiling-desktop" --scenario surface-finish --diagnostic-no-gpu --port 9222
```

Use `--scenario cylindrical-finish` for the curved workflow. It admits the unchanged full-face `independent-exporters/occt-cylinder-mm.step` through the same confined native tokens, compares untextured versus finished real paths/program, then exercises history, source-copy deletion and fresh saved read-only inspection/export. These are diagnostic native-control workflows, not GPU, real-dialog or hardware qualification.

Use the resource controls below; do not run CEF beside native compilation. Before a direct desktop Cargo build, stage the fresh target-suffixed engine sidecar and build the current frontend; otherwise shell staging can replace a current engine with an older cached sidecar.

### Source-backed mapped image relief

Select an admitted planar/cylindrical source face and **Image relief**, choose a PNG or canonical PGM, set all seven mapping/error controls, then **Import image and apply**. Admission atomically stores canonical Gray8 samples and appends an undoable face layer. The displayed Canvas is the captured height image, not a displaced mesh or physical printability preview. Compile and fresh emitted-program replay show manufactured paths. **Height image → Captured** reuses stored samples with new mapping parameters or another eligible face through an ordinary metadata edit; deleting the original image must not prevent remapping.

Black means zero added height, white means depth. U repeats/phase wrap pixel nodes; source-Z period/phase use physical millimetres, with image top toward +source Z. Cylinders use source +X/right-handed +Z, independent of native UV/seam; set their margin to0. Planar protected margin must be at least two bead widths and the face wider than four margins. `vertical_error_mm` controls adaptive layer displacement approximation separately from0.001mm XY chord error. Oversized/animated/unsupported images, excessive layer refinement, invalid geometry and unsafe supports fail explicitly. Nominal-bead feature-resolution advisories are bounded sampled diagnostics, not an independent texture/physical certificate.

```sh
"$CLI" project edit EXISTING_PRINT_PROJECT --surface-image 1 ADMITTED_FACE_ID occurrence /path/height.png '{"depth_mm":0.35,"u_repeats":1,"u_phase_fraction":0.125,"z_period_mm":7.3,"z_phase_mm":0,"edge_margin_mm":1,"vertical_error_mm":0.05}' --preflight --compile --verify --manufacturing-out NEW_IMAGE_EXPORT --engine "$ENGINE"
"$CLI" project inspect EXISTING_PRINT_PROJECT --surface-image-out CAPTURED_IMAGE_HASH NEW_HEIGHT.pgm --replay-out NEW_IMAGE_MOTION.splr --engine "$ENGINE"
```

Find `CAPTURED_IMAGE_HASH` in authoritative `surface_overlays.images`; no path or pixels occur in status. A read-only GetSurfaceImage operation publishes the captured canonical asset through checked ByteStream without advancing the project revision. Exported manufacturing provenance includes the complete canonical content table once per image; fresh read-only preview/replay/export needs neither original STEP nor PNG.

The full modified D is upstream of supports: object contact/demand and every relevant lower corridor use final image-modified material and actual deposited strokes. Changing a lower band can invalidate an upper support route, not just its final few millimetres. Current vertical supports are not a fin generator or exact-contact mechanics model.

Use `--scenario image-relief` or `--scenario image-relief-planar` with the CEF command above. The leading images vary in U/Z, include blank rows and non-grid vertical events, and compare native source/realized areas against an independent image integral. The workflow includes stored-image remapping after PNG deletion, history, actual G-code/replay, save, STEP deletion and fresh read-only canonical Canvas preview. These remain explicit no-GPU diagnostic controls, not hardware/picking/physical-print evidence.

On Linux, the CEF runner gives each run an owned temporary `XDG_DATA_HOME`. Restart inside a scenario retains that run's durable operations, but repeated smoke runs neither consume nor clear the operator's retained ledger. Completed runs remove their own app data after desktop/engine closure; uncertain live-writer failures preserve it with a reported path. The ordinary 128-retained-operation admission cap remains unchanged.

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

Spiling runs trusted packaged desktop UI; loading arbitrary websites is not its product model. Normal CEF renderer isolation is defense in depth, not a hostile-web-content qualification project. The approved **local Linux hardware workflow** permits Chromium's existing GPU-process sandbox fallback on the pinned CEF/driver stack without global sandbox-disable flags. Keep upstream warnings visible and describe the actual GPU/renderer isolation accurately; GPU sandbox qualification does not block local manufacturing delivery. The privileged browser/host is outside the renderer sandbox by design. Imported STEP, images and projects still require malformed-data validation and resource bounds. Hardware device admission establishes neither real-dialog, packaged-platform nor physical-machine qualification.

For development-native checks, build current frontend assets and stage the matching engine **before** building the desktop: `pnpm build` implements that order. A direct desktop Cargo build can copy an old staged sidecar over the engine executable; sharing a warm target does not validate sidecar identity.

The viewport panel stays window-height bounded independently of long manufacturing reports. Both WebGPU canvases cap drawing-buffer pixel ratio against the actual device's texture limit and update dimensions/ratio atomically; changing pixel ratio against old dimensions can allocate an oversized intermediate texture.

`--scenario manufacturing` exercises the actual production panel, native intent/history/compile/verification, motion transport, exclusive export, source-independent read-only reopen and child restart/cleanup. It requires working WebGPU by default. On a headless Linux diagnostic host, `--diagnostic-no-gpu` explicitly preserves the startup GPU rejection, then enables a startup-gated native-panel attachment:

```sh
SPILING_ENGINE_PATH="$PWD/target/debug/spiling-engine" SPILING_CEF_UNSANDBOXED=1 \
  xvfb-run -a node tools/smoke-desktop.mjs --executable "$PWD/target/debug/spiling-desktop" \
  --scenario manufacturing --diagnostic-no-gpu --port 9222
```

This mode proves panel/native software behavior, not rendered motion, picking, native-dialog interaction, normal sandboxing, hardware support or packaging certification. Use the resource limits above, with a separately chosen CEF task allowance; do not overlap it with native compilation.

## Release evidence

Use [B0-plan.md](B0-plan.md) for acceptance and [SOURCE.md](SOURCE.md) for source distribution. Record only exercised results in the milestone evidence; missing Windows/macOS/hardware access remains an unmet gate. Preserve [license requirements](../license.md) and do not turn local rules/vision into a chronological release log.
