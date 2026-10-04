<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# B0: packaged bootstrap

B0 delivers a packaged Tauri 3/CEF workbench, a supervised Rust sidecar, a CLI using the same bounded protocol, generated TypeScript contracts, and a WebGPU diagnostic triangle. It does not implement CAD import, project authoring, or manufacturing. Monstertruck is the configured future default kernel, not an available B0 geometry capability.

## Build boundaries

- `crates/contracts`: flat versioned control schemas, bounded framing, explicit diagnostic triangle layout, Rust-to-TypeScript generation.
- `crates/engine-client`: async child-process lifecycle and protocol client shared by CLI and shell; its separate crate is justified by those two consumers.
- `apps/engine`: handshake-gated request dispatch; framed stdout and structured stderr.
- `apps/cli`: real engine diagnostics, synthetic triangle transfer, orderly exit.
- `apps/desktop/src-tauri`: CEF entry point, supervision, bounded binary invoke bridge, packaging.
- `apps/desktop/src`: license assent, GPU gating, status/diagnostics, lifecycle controls.
- `packages/protocol`: generated contracts and validated binary decoder.
- `packages/viewport`: Three.js WebGPU rendering and resource disposal; no Tauri dependency.
- `tools`: reproducible bootstrap, generation/drift checks, verification and packaging.

Each owning area receives current AGENT-RULES.md and AGENT-VISION.md. Source and manifests use OSL-3.0 notices. Private research lives in the separate `/root/spiling-brain` repository, not this checkout. No empty future domain crates.

## Dependency order

1. Verify and pin Tauri 3.0.0-alpha.4, runtime-cef 3.0.0-alpha.5, build 3.0.0-alpha.3 and upstream CEF 152.3.0 packaging APIs. Pin Rust 1.99.0, Node 24 and pnpm 10.32.1.
2. Establish Cargo/pnpm workspaces and area documentation.
3. Implement schemas, generator, framing, engine and shared client; frontend binary decoder can proceed from the frozen wire specification.
4. Integrate CLI, native shell, and frontend. Detect WebGPU before starting the sidecar; no system-webview/WebGL/browser-fixture fallback.
5. Build distributables and exercise the real surfaces, including failure states and shutdown.
6. Commit and push meaningful checkpoints. Record measured acceptance separately from this execution plan.

## Acceptance procedures

- Bootstrap on a documented fresh environment with pinned toolchains and checksummed native downloads.
- Run CLI handshake, ping and binary triangle transfer against the actual child executable; check protocol mismatch, malformed/truncated/oversized frames and shutdown.
- Run generated-contract drift checks and Rust/TypeScript correctness checks.
- Open a packaged CEF application without a separately installed browser. Confirm license display/assent, renderer backend, sidecar PID/build/protocol, diagnostic triangle and measured transfer bytes.
- Interrupt the real engine; observe an actionable interrupted state and restart to a new PID. Exercise mismatch and GPU-unavailable states without beginning an engine session.
- Close the application and confirm its child exits. Account for CEF helpers separately from the engine.
- Repeat packaged capability detection and lifecycle checks on declared Linux, macOS and Windows reference machines. CI builds alone and software rendering do not establish hardware acceptance.

## Evidence and release gate

Record build identity, lockfiles, dependency versions, host/GPU/driver, package, procedures, observed results and failures in `docs/quality/B0-evidence.md`. Physical access to Windows/macOS reference machines is an acceptance prerequisite; do not mark unexecuted checks passed. Build provenance, third-party notices, available source and the OSL assent mechanism must accompany release packaging.

Owner and reviewer assignments are required before final milestone acceptance. B0 is not complete solely because code compiles or Linux software-rendered smoke succeeds.
