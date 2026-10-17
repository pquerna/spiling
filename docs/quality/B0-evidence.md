<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# B0 acceptance evidence

## Applicability of these results

The historical bootstrap and packaging results below describe the B0 pipe transport. The current engine/CLI/shell use gRPC operations and ByteStream without cross-version compatibility; old runs do not confer current protocol/lifecycle acceptance. See [engine operation evidence](engine-operations.md) and the independently exercised local hardware workflow below. Packaged/platform gates remain distinct.

## Gate status

**B0 acceptance is not yet closed.** Historical native/CLI, installed Linux CEF bridge lifecycle and GPU-unavailable checks are executed results. The local Linux hardware workflow below also exercises current engine-delivered rendering and manufacturing. Full packaged Windows/macOS/Linux reference-machine acceptance remains separate; a development run or CI package does not close those gates.

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

Rebuilt from clean source `4642e0e3bd66618e3701bc68a89abcc0eccb4f55`; the packaged provenance records that commit with `dirty: false`. The Debian artifact was **139.81 MiB**. Verified native legal bytes directly inside the Debian archive against the upstream distribution: CEF LICENSE.txt SHA-256 `058c3827ffb827ff3edda471ae7e1bb1d1aa5931985f0126043ccd33409e792f`; Chromium CREDITS.html SHA-256 `c8563b78af9d71ccd5b9ecf0f56e789d3e3fb426e1389f82771a05c75c0188ea`. Repeated the packaged GPU-unavailable/native-close smoke successfully on this artifact.

CEF's Debian bundle places the real desktop in `usr/share/Spiling` and its launcher/sidecar in `usr/bin`. The initial native start failed by looking for the sidecar in `share/Spiling`; after correcting installation-prefix resolution, actual packaged invokes negotiated protocol 1 and returned the engine's raw 64-byte triangle, decoded successfully by TypeScript.

Direct opt-in CDP/native bridge diagnostics exercised actual start/status, external SIGKILL detection, fresh-PID restart, intentional shutdown, diagnostic interruption, and native window close while an engine was live. All stopped/replaced engine PIDs were reaped; the desktop exited with code 0 after native close. A separate packaged run with protocol 2 rejected startup with an upgrade-required mismatch and no negotiated session. These bridge checks deliberately bypassed the unavailable-GPU admission gate; they do not prove the full UI/rendering path.

Executed the packaged `gpu-unavailable` smoke under Xvfb with the CEF sandbox explicitly disabled. The injected missing-GPU branch displayed the unsupported diagnostic, native status remained `stopped` with no handshake, screenshots were captured, and native window close exited successfully. Injection and unsandboxed execution are diagnostic qualifications, not production sandbox/hardware acceptance.

The normal SwiftShader smoke did **not** pass: CEF GPU subprocesses exited with code 11, followed by a timeout waiting for the engine-running UI. Corrected the diagnostic switches to include `--`; upstream treats valueless arguments without that prefix as positional arguments. A separate Chromium 150 software-WebGPU attempt obtained an adapter and exercised shared repeated-disposal completion, but produced a black capture, `OperationError: Instance dropped in popErrorScope`, and no adapter on replacement. No visible triangle or successful GPU replacement is claimed.

A further native development-mode comparison used the installed llvmpipe Vulkan ICD, explicit Vulkan/unsafe-WebGPU flags, and the unstripped CEF distribution. The real UI reached `Running` and reported the engine's 64-byte transfer, but its diagnostic triangle canvas remained white in both CDP and independent X11 root-framebuffer captures (`artifacts/cef-mesa-view.png` and `artifacts/cef-mesa-x11.png`). A standalone WebGPU clear, independent of Three and triangle data, read back the expected RGBA `[255, 64, 0, 255]`. That establishes GPU execution, not successful presentation of the diagnostic triangle. SwiftShader comparisons with alternate argument combinations also produced GPU-process exit code 11. These additional CEF comparison windows were closed through the native path; the Vulkan comparison's live engine child was reaped.

### Platform CI

[Run 37175500953](https://github.com/pquerna/spiling/actions/runs/37175500953), at `70cdd18`, completed bootstrap, source/protocol checks, and distributable builds on Ubuntu 24.04 and macOS 14. Windows 2022 completed native bootstrap and Clippy/TypeScript checks but failed repository formatting after CRLF checkout. Added `.gitattributes` to require LF text checkout; attribute resolution was checked locally. The corrected Windows run must pass before claiming three-platform CI success. No CI runner result establishes physical GPU acceptance.

[Run 37177432853](https://github.com/pquerna/spiling/actions/runs/37177432853), at `4642e0e`, passed the complete Ubuntu build and package path. macOS passed bootstrap and source checks, then rejected the smoke fixture's invalid-UTF-8 filename with `EILSEQ` before invoking the CLI. Restricted that fixture to Linux, where byte filenames are valid; the cross-platform handshake, transfer, mismatch, and cleanup checks remain unchanged. The corrected Linux CLI smoke passed locally. Windows was cancelled during bootstrap by the superseding push, so it supplies no corrected acceptance result.

The completed Windows log identifies the cancelled bootstrap's actual stall: native debug compilation finished, then selective CEF `LICENSE.txt` extraction remained in host `tar` for more than 38 minutes. Added early-stop extraction (`--fast-read` on BSD tar, `--occurrence=1` on GNU tar). A local smoke with both tar implementations recovered the identical 1,662-byte license and SHA-256 `058c3827ffb827ff3edda471ae7e1bb1d1aa5931985f0126043ccd33409e792f`; BSD tar completed in approximately 0.4 seconds on this Linux host. This is not a Windows rerun.

Cancelled the superseding matrix run at the user's cost-policy request. Automatic CI now checks material source/configuration changes in one Linux job without CEF compilation or Clippy; selected-platform native packaging is explicitly dispatched. Full checks remain local. The complete lean command sequence passed locally: frozen install, contract drift, TypeScript, 18 selected Rust behavior tests, real CLI interoperability, and 14 TypeScript decoder tests. Actionlint 1.7.12 accepted the workflow. No further native GitHub matrix was dispatched.

Exercised the revised `pnpm package` path locally: Tauri compiled release with `--no-bundle`, notices were collected after native CEF availability, and `tauri bundle` produced the 139.81 MiB Debian artifact without another compilation. Extracted CEF license/credits hashes matched the values above. The extracted package passed the injected GPU-unavailable/native-close smoke under unsandboxed Xvfb. This diagnostic artifact records base commit `37a4ba7` with `dirty: true`; it is not a release candidate. Full `pnpm check && pnpm test` also passed locally after the cost-focused implementation.

## Local Linux hardware development workflow

Exercised from main base `0c402ba` with the Linux runtime, bounded-canvas and replay-ownership changes in this commit. This is a debug-development result, not a packaged release. Native identity remains Monstertruck `5a4ddb7582c3d7f75a39e800a9fef345cdfe435e`, project4/manufacturing5/surface3/planner7, emitter4/verifier4; it does not include the unfinished native-geometry parity cutover.

- Ubuntu 26.04.1 LTS, Linux 7.0.0-38-generic; Rust 1.99.0, Node 24.15.0, pnpm 10.32.1 and the committed lockfiles.
- Physical Radeon 8060S / RDNA 3, Mesa 26.0.8, Xorg/amdgpu and a 3440×1440 LG display. CEF/Chromium 152.0.7977.83 admitted a nonfallback AMD WebGPU adapter with native Vulkan compositing and normal ANGLE/OpenGL. `VK_DRIVER_FILES=/usr/share/vulkan/icd.d/radeon_icd.json`; no software renderer, driver-block bypass, global sandbox-disable flag or loader-preload override.
- The exact debug executable had an AppArmor user-namespace allowance; global host restrictions were retained. Renderer processes reported seccomp mode 2. Chromium reported GPU sandbox false under the approved local desktop fallback; this is not a fully GPU-sandboxed claim.
- `fixtures/geometry/box-mm.step` SHA-256 `1b23042e12183748e12bc4e0195ae546c80296ce4170709eb3f836dce2f4ac98`; `fixtures/manufacturing/complete-sparse.intent.json` SHA-256 `b91e7054907216e4ea6ecaf472cadeea950fb20e642586d4be15815893918c77`.

Built with `node tools/run.mjs build` using the shared `CARGO_TARGET_DIR` and matching staged engine. Ran `node tools/smoke-desktop.mjs --executable /home/pquerna/src/spiling/target/debug/spiling-desktop --scenario manufacturing --port 9222` on physical Xorg display `:90`, without `--diagnostic-no-gpu`.

The production panel passed intent validation/Apply/history, native bed preparation and Preflight, Compile, fresh report-only and actual-program motion replay, a native cancellation request and terminal-result invariants, exclusive four-file export/no-overwrite rejection, source-deleted read-only reopen with fresh replay/export, and a new engine session requiring fresh verification. Native window close reaped the engine and exited successfully. The emitted bundle had 41 layers, 589 paths and 835 deposition segments; independent replay decoded 1,952 events / 93,792 packed bytes and reported `verified: true`, with GPU display admission ready. Inspected native-box and fresh-report screenshots.

A separate controlled native-window smoke enlarged the actual canvas to 9,741.828125 CSS pixels high. Its drawing buffer changed from 527×439 to 443×8,191 and back to 527×439 under the device limit; the workbench remained Running, no JavaScript exceptions were captured, and the restored native box was visually inspected. Ignored local evidence includes `artifacts/main-runtime-resize.json`, `main-runtime-resize.png`, `main-runtime-gpu-security.json` and `manufacturing-production-panel.png`.

Affected desktop all-target Clippy, engine-client tests (15 passed, 7 ignored), package TypeScript checks, Rust formatting, changed formatted-file checks and the canonical build passed. Full `pnpm check` did not pass: the existing Core `storage::open_reusing` eight-argument Clippy failure remains outside this patch. Repository-wide Prettier also reported 31 pre-existing files; no formatting sweep was applied.

These results establish this local hardware/software workflow only, not real native-dialog interaction, physical face-picking acceptance, packaged installation, other GPU/platform coverage, physical printability or machine readiness.

## Remaining acceptance prerequisites

- Full packaged engine-delivered WebGPU presentation, UI interruption/restart/stop and mismatch presentation, successful GPU replacement/loss cleanup, and normal desktop isolation defaults on the declared reference machines.
- Corrected Windows/macOS native distribution evidence, requested explicitly rather than inferred from automatic Linux checks.
- Windows/macOS packaged installation and actual GPU/lifecycle runs on declared reference machines.
- Packaged physical Linux GPU/reference-machine acceptance; the local development run above is not distribution qualification. Software/CPU rendering remains diagnostic-only.
- Fresh-machine reproduction, release signing/distribution/licensing review, source access and recipient-assent review appropriate to release channels.

Use [B0-plan.md](B0-plan.md) for acceptance procedures and [development.md](development.md) for actual commands. Do not present any unchecked prerequisite as passed or silently narrow the support matrix.
