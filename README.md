<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Spiling

Native BREP manufacturing workbench with a Rust engine and a cross-platform desktop interface. [Monstertruck](https://github.com/virtualritz/monstertruck) is the default CAD kernel.

## Current scope

B0 implements the runtime bootstrap: a Tauri 3/CEF desktop shell, a supervised Rust engine, a CLI, generated control contracts, and a WebGPU **synthetic diagnostic triangle** delivered by the engine. It does not yet import CAD, edit projects, slice, verify machine programs, or export printable jobs. Monstertruck is the configured future kernel; B0 advertises no geometry capabilities.

Windows, macOS, and Linux are packaging targets, not automatically verified support claims. See [B0 evidence](docs/quality/B0-evidence.md) for executed checks and remaining acceptance prerequisites. Software-rendered/container runs do not certify physical GPU support.

## Development

Install Node 24.x, pnpm 10.32.1, Rustup, and the platform-native prerequisites in [development.md](docs/quality/development.md). The repository pins Rust 1.99.0 and commits both dependency lockfiles.

```sh
pnpm bootstrap
pnpm dev
```

The first native build downloads the pinned CEF distribution. The desktop displays the complete OSL license offline and requires assent before entering the workbench. WebGPU initialization gates engine startup; there is no WebGL or system-webview fallback.

| Command                                | Behavior                                                       |
| -------------------------------------- | -------------------------------------------------------------- |
| `pnpm contracts`                       | Generate Rust-owned TypeScript contracts                       |
| `pnpm check`                           | Contract drift, Rust formatting/clippy, TypeScript, formatting |
| `pnpm test`                            | Rust behavior checks and real CLI/binary interoperability      |
| `pnpm smoke:cli`                       | Actual engine handshake, triangle decode, mismatch rejection   |
| `pnpm bench:smoke`                     | Ten fresh-process diagnostic transfers                         |
| `pnpm package`                         | Current-platform native distributable and dependency notices   |
| `pnpm smoke:desktop --executable PATH` | Exercise a packaged CEF application and engine lifecycle       |

Packages are emitted under `target/release/bundle/`: Debian on Linux, `.app` on macOS, NSIS on Windows. See the development guide for diagnostics, packaging limits, and sandbox requirements.

## Architecture and contributing

- `apps/engine`, `apps/cli`, `apps/desktop`: native engine, protocol client, and CEF workbench.
- `crates/contracts`: control schemas, bounded framing, diagnostic binary layout, TypeScript generator.
- `crates/engine-client`: shared native protocol and child-process lifecycle client.
- `packages/protocol`, `packages/viewport`: generated/decoded display contracts and WebGPU rendering.
- `tools`: bootstrap, checks, packaging, provenance, and real-surface smoke runners.

Read [AGENTS.md](AGENTS.md) before changing an area, then its `AGENT-RULES.md` and `AGENT-VISION.md`. Those files describe current rules and direction, not development history. See [B0-plan.md](docs/quality/B0-plan.md) for milestone scope and [control.md](docs/protocol/control.md) for the wire contract.

Private research and planning live separately in `/root/spiling-brain`. This repository retains self-contained canonical implementation rules, public protocol specifications, and release evidence; private research does not silently override them.

## License

Copyright (c) 2026 Spiling contributors.
Licensed under the Open Software License version 3.0.

**SPDX-License-Identifier: `OSL-3.0`**

The canonical license text is in [LICENSE.md](LICENSE.md). See [docs/license.md](docs/license.md) for file headers, Cargo/npm metadata, attribution, and distribution guidance. Third-party materials retain their own licenses.
