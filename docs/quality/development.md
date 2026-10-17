<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Development and packaging

## Prerequisites

- Node24.x, pnpm10.32.1, and the Rust toolchain/components in `rust-toolchain.toml`.
- Native C/C++ tools: MSVC on Windows, Xcode command-line tools on macOS, GTK4/CEF dependencies on Linux.
- Matching pinned CEF distribution in `.cache/cef` or an explicit `CEF_PATH`. Cargo/pnpm lockfiles and shell manifests own versions; do not substitute a system webview or arbitrary CEF archive.

Typical Ubuntu native dependencies:

```sh
sudo apt-get install build-essential pkg-config cmake ninja-build libgtk-4-dev \
  libx11-dev libxrandr-dev libxcomposite-dev libxdamage-dev libnss3-dev \
  libasound2-dev libgbm-dev libsecret-1-dev libssl-dev patchelf desktop-file-utils
pnpm bootstrap
pnpm dev
```

macOS development must use the Tauri CLI so CEF's application bundle/helpers
launch correctly; a bare Rust executable is not equivalent.

[`tools/run.mjs`](../../tools/run.mjs) owns root commands:

| Command            | Purpose                                                              |
| ------------------ | -------------------------------------------------------------------- |
| `pnpm bootstrap`   | Locked install, generation and integrated native/frontend build      |
| `pnpm dev`         | Native CEF development workbench                                     |
| `pnpm build`       | Generate contracts → build/stage engine and CLI → frontend → desktop |
| `pnpm contracts`   | Rust-owned TypeScript generation; `--check` rejects drift            |
| `pnpm check`       | Formatting, Clippy, TypeScript and generated-contract checks         |
| `pnpm test`        | Workspace behavior and real CLI/binary interoperability              |
| `pnpm smoke:cli`   | Actual child startup, operations, transfer and reap                  |
| `pnpm bench:smoke` | Diagnostic process/transfer benchmark, not geometry performance      |
| `pnpm package`     | Current-platform package with sidecar, CEF and notices               |

**Build before running real surfaces.** Direct desktop Cargo builds can overwrite
a fresh engine with a stale staged sidecar. Do not build while a desktop smoke
is live. Reuse one disk-backed Cargo target/profile; development/test profiles
already disable debug information and incremental compilation. Selected numerical
crates are optimized independently of the desktop stack.

## Resources and test inputs

Root tools default to one Cargo job; raise `CARGO_BUILD_JOBS` explicitly for the
host. Parallel compiler count is not a memory limit. Use a whole-process cgroup
when bounds matter, and distinguish host RAM/disk allowances from geometry
work/depth/tolerance, protocol and watchdog limits. `/tmp` may be RAM-backed.

```sh
export CARGO_TARGET_DIR="$PWD/target" CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=16
export RAYON_NUM_THREADS=1 RUST_TEST_THREADS=1
systemd-run --wait --pipe --collect --property=WorkingDirectory="$PWD" \
  --property=MemoryMax=16G --property=MemorySwapMax=2G --property=TasksMax=1024 \
  /usr/bin/env PATH="$PATH" CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
  CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS="$CARGO_BUILD_JOBS" RAYON_NUM_THREADS=1 \
  cargo test --locked -p spiling-manufacturing --lib -- --test-threads=1
```

Choose limits that leave host headroom; system scope/user permissions vary.
Actual-child tests use the freshly built `SPILING_TEST_ENGINE` and explicitly
include ignored integration cases. Rayon1 prevents each child creating its own
large worker pool. Under tight Node task limits, `UV_THREADPOOL_SIZE=1` and
`NODE_OPTIONS=--v8-pool-size=1` are available; trusted intent authoring also accepts
an explicit `GOMAXPROCS` override.

Tests consuming generated native parity/operator inputs require the real generators:

```sh
cargo run --locked -p spiling-geometry --example generate_corpus --
cargo run --locked -p spiling-geometry --example generate_operator_fixtures --
```

Generators preserve frozen STEP bytes and refuse changed originals. Review new
fixture/manifest output rather than editing expected geometry by hand. Fixture
manifests own hashes; generated-contract and corpus drift checks are not runtime
acceptance. Missing inputs are not expected geometry refusals.

## Native CLI workflows

```sh
CLI="$PWD/target/debug/spiling-cli"
ENGINE="$PWD/target/debug/spiling-engine"
"$CLI" geometry --scene fixtures/geometry/scenes/realistic-native.scene.json \
  --section 0,0,4:0,0,1 --engine "$ENGINE"
"$CLI" project create NEW_PROJECT --import fixtures/geometry/box-mm.step --save --engine "$ENGINE"
"$CLI" project inspect NEW_PROJECT --engine "$ENGINE"
```

Geometry scenes are import/placement recipes, not saved projects. Native face IDs
come from returned source tables, not guessed STEP ordinals. Project flags execute
left-to-right. Save stores captured source bytes, not dependencies on original
paths. Read-only inspection forbids mutation but permits native queries and fresh
bundle verification. Recovery explicitly selects a previous committed checkpoint;
never blindly retry `save_uncertain`.

For manufacturing, author trusted TypeScript only outside the engine, or use the
checked-in inert JSON. Desktop/engine load bounded JSON; they do not execute profiles:

```sh
pnpm manufacturing:intent fixtures/manufacturing/complete-sparse.intent.ts NEW_INTENT.json
"$CLI" project create NEW_PRINT_PROJECT --import fixtures/geometry/box-mm.step \
  --intent NEW_INTENT.json --center-on-bed 1 --preflight --compile --verify \
  --manufacturing-out NEW_EXPORT --replay-out NEW_MOTION.splr --save --engine "$ENGINE"
"$CLI" project inspect NEW_PRINT_PROJECT --preflight --verify \
  --manufacturing-out NEW_REOPENED_EXPORT --replay-out NEW_REOPENED_MOTION.splr --engine "$ENGINE"
```

All output destinations must be new. Bed preparation precedes explicit pose Apply;
Preflight is not manufacturing authority. Inspect/replay independently decode saved
program bytes, never trust a stored `verified` flag. Export uses the common exclusive
writer. [Manufacturing](manufacturing-evidence.md) maps the implementation and current
refusals; [authoring](authoring-evidence.md) maps transactions and recovery.

Source-face finishes are ordinary revision-bound project edits. Use current native
occurrence/face IDs, choose scope and explicitly Apply; Undo/Redo/Save remain Core
operations. Whole-face ribs and image relief retain narrow certified carrier/pose
eligibility despite generic base geometry. Image preview shows captured Gray8 data,
not displacement. Compile and fresh replay show manufactured paths. Parameter/wire
contracts live in [manufacturing protocol](../protocol/manufacturing.md),
[rib ADR](../adr/002-cylindrical-grip-ribs.md) and [image ADR](../adr/003-spatial-image-relief.md).

## Actual desktop surface

Use a real graphical session with its display, authorization and session bus.
Normal startup requires offline license assent and WebGPU before spawning the
engine. For a fresh debug build:

```sh
SPILING_ENGINE_PATH="$ENGINE" node tools/smoke-desktop.mjs \
  --executable "$PWD/target/debug/spiling-desktop" --scenario manufacturing \
  --require-hardware-gpu --port 9222
SPILING_ENGINE_PATH="$ENGINE" node tools/smoke-desktop.mjs \
  --executable "$PWD/target/debug/spiling-desktop" --scenario geometry \
  --scene fixtures/geometry/scenes/realistic-native.scene.json --port 9222
```

The manufacturing runner exercises intent/placement/history, Compile, actual-byte
report/motion, cancellation, export, source-independent reopen and child cleanup.
`--manufacturing-source RELATIVE_STEP` keeps the entire unchanged complete-sparse
intent for a real mechanical input; it does not make refused geometry successful.
Geometry exercises shared definitions, native picking/sections and transfer lifetime.
`surface-finish`, `cylindrical-finish`, `image-relief` and `image-relief-planar` exercise
their corresponding authoring/Compile/replay flows. `project-bridge` bypasses GPU
admission to exercise native storage/bridge behavior, not visible/pickable acceptance.

Headless-only diagnostic attachment is explicit, not hardware qualification:

```sh
SPILING_ENGINE_PATH="$ENGINE" SPILING_CEF_UNSANDBOXED=1 xvfb-run -a \
  node tools/smoke-desktop.mjs --executable "$PWD/target/debug/spiling-desktop" \
  --scenario manufacturing --diagnostic-no-gpu --port 9222
```

Never ship diagnostic overrides as defaults. `SPILING_CEF_DEBUG_PORT` enables a
privileged local debugging endpoint; it is normally off. Debug fixture admission
uses confined native tokens, not renderer filesystem access. [Runtime boundaries](B0-evidence.md)
cover CEF loader, trusted desktop isolation and GPU fallback. Linux native dialogs
require a working xdg-desktop-portal backend; bridge tests do not exercise dialogs.

## Packaging, CI and recording results

`pnpm package` selects Debian, macOS `.app` or Windows NSIS. It builds release once,
collects native notices, then bundles existing binaries under `target/release/bundle/`.
Installed CEF Debian layout resolves the sidecar from the installation-prefix `bin`;
other packages resolve it adjacent to the actual executable. A debug engine override
is not an installed-path fallback.

Automatic CI is a lean Linux source/contract/client check; native platform packaging
is explicitly dispatched. Markdown-only changes do not launch builds. Neither CI
compilation nor a local smoke establishes installed-platform or physical-machine
qualification. Full checks remain available; report actual failures, not a pass
inferred from selected filters.

Commit/PR verification notes contain changed behavior, exercised commands, outcomes
and remaining risks. Detailed logs, screenshots, identities and measurements belong
in CI or ignored `artifacts/`. Keep this runbook and code maps current; do not append
checkpoint histories. [SOURCE](SOURCE.md) and [license](../license.md) govern source,
notices, assent and release obligations.
