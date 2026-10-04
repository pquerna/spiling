<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# B0 acceptance evidence

## Gate status

**B0 acceptance is not yet closed.** Native protocol/CLI and frontend checks below are executed results. Packaged CEF checks and physical Windows/macOS/Linux GPU acceptance must be recorded separately; source implementation and CI targets are not equivalent to those checks.

Implementation/evidence integration owner: automated coding assistant. Final evidence reviewer: unassigned; assignment remains a milestone acceptance prerequisite.

## Environment and identities

- Linux x86_64 workstation/container; Debian 13 native libraries, GTK4 4.18.6.
- Rust 1.99.0 (`b940084d7`, 2026-09-28), Node 24.15.0, pnpm 10.32.1.
- Tauri 3.0.0-alpha.4, runtime-cef 3.0.0-alpha.5, tauri-build 3.0.0-alpha.3.
- Cargo resolves CEF/cef-dll-sys 152.3.0+152.0.6; committed Cargo.lock and pnpm-lock.yaml are dependency authorities.
- Host Vulkan device: **llvmpipe (LLVM 19.1.7, 256 bits)**, Mesa 25.0.7-2+deb13u1, CPU device. No physical GPU was exposed by this environment.
- Application and engine package version: 0.1.0. The release tooling records commit/worktree state, toolchain versions, lockfile hashes and dependency notices in packaged `notices/provenance.json`.

## Executed checks

### Native engine and cross-language protocol

Executed `pnpm smoke:cli` against the actual compiled Rust engine and CLI, not fixtures replacing the process:

- Handshake negotiated protocol 1, 64 KiB control and 4 MiB binary limits, real engine PID/build, configured Monstertruck default, and empty geometry capabilities.
- Ping returned pong; CLI completed orderly shutdown before reporting success.
- Engine emitted the 64-byte synthetic triangle and TypeScript decoded its positions and indices.
- Decoder rejected truncated/oversized payloads, unsupported schema, nonzero reserved field, count overflow, out-of-range indices and nonfinite coordinates.
- Protocol-version 2 produced a nonzero CLI exit with an upgrade-required/mismatch diagnostic.

Executed native Rust behavior suites: **18 tests passed** across framing, strict handshake, request correlation, real-child lifecycle, timeout/cancellation/drop cleanup and engine errors. Executed TypeScript decoder suite: **14 tests passed** using the native-owned hex fixture and corruption boundaries.

### Frontend and browser admission

Executed desktop production bundling and viewport/protocol TypeScript checks. Vite 8.3.2 produced the production assets.

Opened the actual React frontend in a real headless browser through Vite at 127.0.0.1:1420. Observed the offline full license, disabled entry button before checkbox assent, then clicked assent and entry. The browser-only surface showed **desktop-required**, disabled native lifecycle actions, and no mock engine or fake triangle. Captured visual screenshots during the run.

This browser check proves the admission/rejection UI only; it does not prove packaged CEF, a native GPU, or manufacturing functionality.

## Remaining acceptance prerequisites

- Actual packaged CEF application startup, engine-delivered WebGPU rendering, real interruption/restart/stop, protocol mismatch, GPU-unavailable gating and native-close cleanup.
- Package inspection for CEF native libraries/helpers, sidecar, OSL text, source information and dependency notices.
- Windows/macOS packaged installation and actual GPU/lifecycle runs on declared reference machines.
- Physical Linux GPU/reference-machine run; software/CPU rendering is diagnostic-only.
- Fresh-machine reproduction, release signing/distribution/licensing review, source access and recipient-assent review appropriate to release channels.

Use [B0-plan.md](B0-plan.md) for acceptance procedures and [development.md](development.md) for actual commands. Do not present any unchecked prerequisite as passed or silently narrow the support matrix.
