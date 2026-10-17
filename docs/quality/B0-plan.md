<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Runtime build boundaries

## Build boundaries

| Owner                                                     | Responsibility                                                                                    | Boundary                                               |
| --------------------------------------------------------- | ------------------------------------------------------------------------------------------------- | ------------------------------------------------------ |
| [`crates/contracts`](../../crates/contracts/)             | Domain DTOs, checked protobuf adapters, Rust-derived TypeScript and packed-payload specifications | No desktop, renderer or kernel dependency              |
| [`crates/engine-client`](../../crates/engine-client/)     | `EngineClient` child ownership, authenticated RPCs, deadlines and orderly reap                    | Shared by CLI and shell; no planner or project writer  |
| [`apps/engine`](../../apps/engine/)                       | Compose Core, geometry, manufacturing and durable operations                                      | Sole project writer; native work stays off Tokio       |
| [`apps/cli`](../../apps/cli/)                             | Scriptable operator workflows through the shared client                                           | No alternate import, planner or verifier               |
| [`apps/desktop/src-tauri`](../../apps/desktop/src-tauri/) | CEF entry point, native admission tokens, supervision and binary bridge                           | No renderer geometry or filesystem paths exposed to UI |
| [`apps/desktop/src`](../../apps/desktop/src/)             | Operator review and transient interaction state                                                   | Explicit Apply; native receipts settle durable changes |
| [`packages/protocol`](../../packages/protocol/)           | Generated views and checked packed-byte decoding                                                  | No UI or Tauri dependency                              |
| [`packages/viewport`](../../packages/viewport/)           | WebGPU scene, source-face picking and bounded GPU ownership                                       | Display is approximate; no manufacturing authority     |
| [`tools/run.mjs`](../../tools/run.mjs)                    | Build/generation/staging/package order                                                            | One matching engine, frontend and shell generation     |

## Build and authority flow

`tools/run.mjs build` generates contracts, builds/stages the engine and CLI,
builds frontend assets, then builds the native shell. A direct desktop Cargo
build can copy a stale staged sidecar over the engine binary. Use the canonical
order before real-surface acceptance and do not build while the desktop is live.

The shell admits the local license and a WebGPU device before normal engine
startup. `EngineClient` supervises the child; authenticated gRPC, Google
Operations and ByteStream carry control, asynchronous work and artifact bytes.
Stdout is bounded startup metadata, not a second request protocol. Source and
manufacturing authority stay native; the synthetic triangle is diagnostic only.

## Release boundary

A build or local development smoke does not qualify an installed platform.
Release acceptance requires a clean source identity, matching sidecar/CEF assets,
license/notices/source availability, installation/signing behavior, actual GPU
presentation and picking, lifecycle/failure cleanup and explicit reference-machine
coverage. Physical manufacturing is a separate qualification.

Use [development](development.md) for commands, [runtime failure boundaries](B0-evidence.md)
for diagnosis and [SOURCE](SOURCE.md) for distribution obligations. Record each
release's verification in its commit/PR and artifacts, not a new chapter here.
