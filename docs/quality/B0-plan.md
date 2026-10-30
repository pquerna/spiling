<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Runtime build boundaries

## Build boundaries

| Owner                                                 | Responsibility                                                                                    | Boundary                                              |
| ----------------------------------------------------- | ------------------------------------------------------------------------------------------------- | ----------------------------------------------------- |
| [`crates/contracts`](../../crates/contracts/)         | Domain DTOs, checked protobuf adapters, Rust-derived TypeScript and packed-payload specifications | No desktop, renderer or kernel dependency             |
| [`crates/engine-client`](../../crates/engine-client/) | `EngineClient` child ownership, authenticated RPCs, deadlines and orderly reap                    | Shared by CLI and shell; no planner or project writer |
| [`apps/engine`](../../apps/engine/)                   | Compose Core, geometry, manufacturing and durable operations                                      | Sole project writer; native work stays off Tokio      |
| [`apps/cli`](../../apps/cli/)                         | Scriptable operator workflows through the shared client                                           | No alternate import, planner or verifier              |
| [`apps/workbench`](../../apps/workbench/)             | Native eframe window, bounded asynchronous host, transient forms and checked wgpu display         | Protocol client; no Core/kernel/planner ownership     |
| [`packages/protocol`](../../packages/protocol/)       | Rust-derived data types and independent checked packed-byte decoders                              | No UI/runtime dependency                              |
| [`tools/run.mjs`](../../tools/run.mjs)                | Matching native builds, contract checks, notices and package assembly                             | One coherent engine/client generation                 |

## Build and authority flow

`tools/run.mjs build` checks generated contracts and builds the engine, CLI and
workbench together. No browser asset build or sidecar staging copy is involved.
Build before real-surface acceptance; never replace binaries while a window is live.

The workbench admits the offline license and native wgpu device before engine
startup. EngineClient supervises the child; authenticated gRPC, Google Operations
and ByteStream carry control, asynchronous work and checked artifact bytes. Stdout
is bounded startup metadata, never a second protocol. Core remains sole writer;
the synthetic triangle is an explicitly separate diagnostic.

## Release boundary

A build or local development smoke does not qualify an installed platform.
Release acceptance requires a clean source identity, matching adjacent binaries,
license/notices/source availability, installation/signing behavior, actual GPU
presentation and picking, lifecycle/failure cleanup and explicit reference-machine
coverage. Physical manufacturing is a separate qualification.

Use [development](development.md) for commands, [runtime failure boundaries](B0-evidence.md)
for diagnosis and [SOURCE](SOURCE.md) for distribution obligations. Record each
release's verification in its commit/PR and artifacts, not a new chapter here.
