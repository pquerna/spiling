<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native workbench: admission and failure boundaries

## Read the implementation

| Question                                                            | Owner                                                                                                |
| ------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| Who creates the native window and wgpu device?                      | [workbench](../../apps/workbench/src/lib.rs), [main](../../apps/workbench/src/main.rs)               |
| Who owns the child and rejects mismatched builds?                   | [engine-client](../../crates/engine-client/src/), [host](../../apps/workbench/src/host.rs)           |
| How do accepted operations settle and stale results lose authority? | [host actor](../../apps/workbench/src/host/), [manufacture](../../apps/workbench/src/manufacture.rs) |
| Who owns display buffers and picking?                               | [scene](../../apps/workbench/src/scene.rs), [viewport](../../apps/workbench/src/viewport/)           |
| How are native packages assembled?                                  | [tools/run.mjs](../../tools/run.mjs)                                                                 |

Startup requires offline license assent, a working native wgpu device and a
matching supervised engine. Imported geometry never falls back to the separate
unitless diagnostic triangle. The application has no embedded browser runtime.

## Failure map

| Symptom                        | Inspect first                                                        | Invariant                                                          |
| ------------------------------ | -------------------------------------------------------------------- | ------------------------------------------------------------------ |
| Build/instance mismatch        | Shared client handshake and adjacent binary                          | Exact identity/capabilities; no arbitrary executable search        |
| Engine interruption            | Host phase, child owner and operation ledger                         | Old session handles/results never regain authority after restart   |
| Native geometry without mesh   | DefinitionDisplay::Unavailable                                       | Retain native queries/face list; no invented picking geometry      |
| Stale source/review after edit | Captured revision/session, scene publication and manufacture capture | No stale picking, mutation or export authority                     |
| Slow/paused transfer           | Bounded scene/review workers and explicit cancellation               | Keep prior labelled display; release staging before close          |
| Native picker unavailable      | OS portal/backend and parent handle                                  | Cancellation preserves state; no browser or shell-command fallback |
| Close leaves a child           | Dirty confirmation, staged cleanup and EngineClient reap             | Await orderly shutdown/exit before releasing ownership             |

## Trust and qualification

Files, RPCs and packed display data retain bounded malformed-input validation.
The engine is isolated in a supervised process; the Rust UI owns transient drafts
and disposable GPU resources, never project/kernel/planner authority.

Linux X11/Vulkan local window and extracted-package workflows are separate from
Wayland, Windows/macOS installation, signing, assistive input, arbitrary GPUs and
physical-machine acceptance. Inspection is development-only and off in packaged
binaries. Default fonts do not cover every script. Do not infer platform support
from compilation, AccessKit enablement or software rendering.

[Development](development.md) owns actual-child/window/package procedures;
[build boundaries](B0-plan.md#build-boundaries), [control](../protocol/control.md)
and [source distribution](SOURCE.md) own corresponding contracts. Detailed run
identities, screenshots and timings stay in artifacts and commit/PR notes.
