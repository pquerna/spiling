<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# B0 acceptance evidence

## Gate status

**B0 acceptance is not yet closed.** Native protocol/CLI, source checks, installed Linux CEF bridge lifecycle, and GPU-unavailable admission checks below are executed results. Engine-delivered WebGPU rendering and physical Windows/macOS/Linux GPU acceptance remain separate unmet gates; source implementation and CI packages are not equivalent to those checks.

Implementation/evidence integration owner: automated coding assistant. Final evidence reviewer: unassigned; assignment remains a milestone acceptance prerequisite.

## Environment and identities

- Linux x86_64 workstation/container; Debian 13 native libraries, GTK4 4.18.6.
- Rust 1.99.0 (`b940084d7`, 2026-09-28), Node 24.15.0, pnpm 10.32.1.
- Tauri 3.0.0-alpha.4, runtime-cef 3.0.0-alpha.5, tauri-build 3.0.0-alpha.3.
- Cargo resolves CEF/cef-dll-sys 152.3.0+152.0.6; committed Cargo.lock and pnpm-lock.yaml are dependency authorities.
- Host Vulkan device: **llvmpipe (LLVM 19.1.7, 256 bits)**, Mesa 25.0.7-2+deb13u1, CPU device. No physical GPU was exposed by this environment.
- Application and engine package version: 0.1.0. The release tooling records commit/worktree state, toolchain versions, lockfile hashes and dependency notices in packaged `notices/provenance.json`.

## Executed checks

### Bootstrap and source checks

Executed `pnpm bootstrap`: locked dependencies, Rust-owned contract generation, staged sidecar, production frontend, native Tauri/CEF compilation, and collected dependency provenance. The actual CEF application compiled successfully; native compilation took 2 minutes 41 seconds on this environment.

Executed `pnpm check`: generated-contract drift, Rust formatting, workspace/all-target Clippy with warnings denied, all package TypeScript checks, and repository formatting passed. Executed the complete root `pnpm test` command, including the actual CLI smoke and 14 decoder tests.

The downloaded native distribution is `cef_binary_152.0.6+g708dc14+chromium-152.0.7977.83_linux64_minimal.tar.bz2`, with upstream archive SHA-1 `9711b86c105fb590da576fe5a829802f1a79d520`, recorded in `archive.json` and generated provenance. The checksum is upstream download integrity metadata, not a claim of signature authentication.

### Native engine and cross-language protocol

Executed `pnpm smoke:cli` against the actual compiled Rust engine and CLI, not fixtures replacing the process:

- Handshake negotiated protocol 1, 64 KiB control and 4 MiB binary limits, real engine PID/build, configured Monstertruck default, and empty geometry capabilities.
- Ping returned pong; CLI completed orderly shutdown before reporting success.
- Engine emitted the 64-byte synthetic triangle and TypeScript decoded its positions and indices.
- Decoder rejected truncated/oversized payloads, unsupported schema, nonzero reserved field, count overflow, out-of-range indices and nonfinite coordinates.
- Protocol-version 2 produced a nonzero CLI exit with an upgrade-required/mismatch diagnostic.
- Valid non-UTF-8 Unix engine/output paths originally panicked during JSON success reporting. After the explicit display-encoding fix, both commands passed, the output matched the native 64-byte payload, and the child PID was gone before reporting success. The root CLI smoke retains this regression on Unix.

Executed native Rust behavior suites: **18 tests passed** across framing, strict handshake, request correlation, real-child lifecycle, timeout/cancellation/drop cleanup and engine errors. Executed TypeScript decoder suite: **14 tests passed** using the native-owned hex fixture and corruption boundaries.

Executed `pnpm bench:smoke`: ten fresh debug-engine process/handshake/64-byte-transfer/shutdown runs. End-to-end wall-clock p50 was **2.731 ms**, p95 **3.330 ms**; sorted samples in milliseconds: 2.676895, 2.677884, 2.715217, 2.717012, 2.730812, 2.780190, 2.784749, 2.833022, 2.849337, 3.329972. This is a bootstrap diagnostic on warmed OS caches, not cold/warm geometry performance or a GPU benchmark.

### Frontend and browser admission

Executed desktop production bundling and viewport/protocol TypeScript checks. Vite 8.3.2 produced the production assets.

Opened the actual React frontend in a real headless browser through Vite at 127.0.0.1:1420. Observed the offline full license, disabled entry button before checkbox assent, then clicked assent and entry. The browser-only surface showed **desktop-required**, disabled native lifecycle actions, and no mock engine or fake triangle. Captured visual screenshots during the run.

This browser check proves the admission/rejection UI only; it does not prove packaged CEF, a native GPU, or manufacturing functionality.

### Packaged Linux CEF and native lifecycle

Executed `pnpm package` and extracted `target/release/bundle/deb/Spiling_0.1.0_amd64.deb`. Inspected the native CEF library, graphics libraries, sandbox helper, locales/resources, sidecar, offline OSL/NOTICE, source information, and generated dependency notices. The extracted package used existing host libraries; this is not fresh-machine installation proof.

CEF's Debian bundle places the real desktop in `usr/share/Spiling` and its launcher/sidecar in `usr/bin`. The initial native start failed by looking for the sidecar in `share/Spiling`; after correcting installation-prefix resolution, actual packaged invokes negotiated protocol 1 and returned the engine's raw 64-byte triangle, decoded successfully by TypeScript.

Direct opt-in CDP/native bridge diagnostics exercised actual start/status, external SIGKILL detection, fresh-PID restart, intentional shutdown, diagnostic interruption, and native window close while an engine was live. All stopped/replaced engine PIDs were reaped; the desktop exited with code 0 after native close. A separate packaged run with protocol 2 rejected startup with an upgrade-required mismatch and no negotiated session. These bridge checks deliberately bypassed the unavailable-GPU admission gate; they do not prove the full UI/rendering path.

Executed the packaged `gpu-unavailable` smoke under Xvfb with the CEF sandbox explicitly disabled. The injected missing-GPU branch displayed the unsupported diagnostic, native status remained `stopped` with no handshake, screenshots were captured, and native window close exited successfully. Injection and unsandboxed execution are diagnostic qualifications, not production sandbox/hardware acceptance.

The normal SwiftShader smoke did **not** pass: CEF GPU subprocesses exited with code 11, followed by a timeout waiting for the engine-running UI. Corrected the diagnostic switches to include `--`; upstream treats valueless arguments without that prefix as positional arguments. A separate Chromium 150 software-WebGPU attempt obtained an adapter and exercised shared repeated-disposal completion, but produced a black capture, `OperationError: Instance dropped in popErrorScope`, and no adapter on replacement. No visible triangle or successful GPU replacement is claimed.

### Platform CI

[Run 37175500953](https://github.com/pquerna/spiling/actions/runs/37175500953), at `70cdd18`, completed bootstrap, source/protocol checks, and distributable builds on Ubuntu 24.04 and macOS 14. Windows 2022 completed native bootstrap and Clippy/TypeScript checks but failed repository formatting after CRLF checkout. Added `.gitattributes` to require LF text checkout; attribute resolution was checked locally. The corrected Windows run must pass before claiming three-platform CI success. No CI runner result establishes physical GPU acceptance.

## Remaining acceptance prerequisites

- Full packaged engine-delivered WebGPU rendering, UI interruption/restart/stop and mismatch presentation, successful GPU replacement/loss cleanup, and production sandbox behavior on a working driver.
- Final native CEF/Chromium legal-notice inspection and corrected three-platform CI completion.
- Windows/macOS packaged installation and actual GPU/lifecycle runs on declared reference machines.
- Physical Linux GPU/reference-machine run; software/CPU rendering is diagnostic-only.
- Fresh-machine reproduction, release signing/distribution/licensing review, source access and recipient-assent review appropriate to release channels.

Use [B0-plan.md](B0-plan.md) for acceptance procedures and [development.md](development.md) for actual commands. Do not present any unchecked prerequisite as passed or silently narrow the support matrix.
