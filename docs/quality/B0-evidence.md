<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Desktop runtime: admission and failure boundaries

## Read the implementation

| Question                                                      | Owner                                                                                                                                          |
| ------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| How is CEF started and configured?                            | [`runtime.rs`](../../apps/desktop/src-tauri/src/runtime.rs), [`build.rs`](../../apps/desktop/src-tauri/build.rs)                               |
| Who owns the child and rejects mismatched instances/builds?   | [`engine-client`](../../crates/engine-client/src/), shell [`engine`](../../apps/desktop/src-tauri/src/engine/)                                 |
| How do UI operations settle and stale results lose authority? | [`useWorkbench.ts`](../../apps/desktop/src/useWorkbench.ts), [`useManufacturing.ts`](../../apps/desktop/src/manufacturing/useManufacturing.ts) |
| Who owns render-device buffers and picking?                   | [`viewport`](../../packages/viewport/src/)                                                                                                     |
| How are native assets staged and packages assembled?          | [`tools/run.mjs`](../../tools/run.mjs), shell [`build.rs`](../../apps/desktop/src-tauri/build.rs)                                              |

Normal startup requires offline license assent, a working WebGPU device and a
matching supervised engine. Browser-only preview must report desktop-required;
GPU refusal must not silently start a native session or substitute WebGL.
Imported native geometry never falls back to the synthetic diagnostic triangle.

## Failure map

| Symptom                               | Inspect first                                           | Invariant                                                                                          |
| ------------------------------------- | ------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| Child build/instance mismatch         | Shared client handshake and staged sidecar              | No cross-generation compatibility or arbitrary executable search                                   |
| Engine interruption                   | Shared child owner, operations ledger, UI epoch         | Old session handles and active results cannot regain authority after restart                       |
| Empty/oversized canvas                | Viewport drawing-buffer resize and CSS panel bounds     | Atomically bound physical size by the actual device limit; page/report height is not canvas height |
| Native geometry but no mesh           | `DefinitionDisplay::Unavailable`, not transport retry   | Preserve source/native queries; no phantom artifact or GPU picking claim                           |
| Replay replaced during project switch | Workbench project-replacement cleanup                   | One awaited GPU owner; stale inspection cannot race a second clear                                 |
| Linux Vulkan initialization mismatch  | Executable loader link and shipped CEF resources        | GTK's transitive dependency does not inherit executable RUNPATH                                    |
| Close leaves a child                  | Shell close admission, discard decision and client reap | Native shutdown/owner EOF completes before process ownership is released                           |

Linux hardware startup enables native Vulkan compositing while retaining normal
ANGLE selection and driver/adapter safety blocks. The executable directly links
the shipped Vulkan loader so its RUNPATH selects one loader for GTK and Dawn;
do not solve this with replacement libraries or a development preload.

## Desktop trust and qualification

The packaged UI is trusted local content, not arbitrary websites. Normal CEF
renderer isolation is defense in depth; imported files still require malformed
input validation and bounded work. Chromium's existing local GPU sandbox fallback
is accepted without a global sandbox-disable flag. Do not present that as full
GPU sandbox or packaged-platform qualification.

Debugging is opt-in through `SPILING_CEF_DEBUG_PORT`. Unsandboxed/software-GPU
switches are explicit diagnostics, never production hardware evidence. Local
hardware workflows exercise visible geometry/picking and operator manufacturing;
Windows/macOS/Linux installed-reference coverage, native dialogs, GPU replacement
and physical-machine behavior require their own acceptance.

[Development commands](development.md) exercise the actual surface.
[Build boundaries](B0-plan.md#build-boundaries), [control protocol](../protocol/control.md)
and [source distribution](SOURCE.md) own the corresponding contracts. Detailed
run identities, screenshots and timings stay in artifacts and commit/PR notes.
