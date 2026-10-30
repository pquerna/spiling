<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native egui workbench

Status: adopted. Native project, source display and recoverable manufacturing
through fresh actual-program replay/export are implemented in `apps/workbench`.
The native development command is the default; legacy retirement follows parity.

## Decision

Replace the Tauri/CEF, React and Three.js desktop frontend with one Rust
`eframe`/`egui`/`wgpu` workbench. `apps/workbench` is its permanent application
home, with binary `spiling-workbench`. Use one exact upstream egui family and
wgpu generation, pinned in Cargo manifests and lockfile. Do not import another
application's engine, UI dependency closure or unreleased widget framework.

The supervised engine remains a separate process. The workbench uses the same
`EngineClient`/`EngineRpc`, authenticated loopback gRPC, Google Operations and
ByteStream as the CLI. Core remains the sole project writer; geometry,
manufacturing, persistence, history and emitted-program verification retain their
current owners and contracts. This changes neither file versions nor supported
geometry/process scope. Disposable meshes never become material authority.

The workbench host owns the child, checked project snapshots, native file dialogs,
accepted-operation observation and saved checkpoint route. A bounded command queue
and coalesced snapshots separate native asynchronous work from immediate-mode UI.
A drawing pass cannot admit a mutation. Failed observations expose explicit Resume
and Cancel without retrying mutations. Dirty replacement/close is revision-bound;
closing awaits child cleanup. Only a matching native open/save receipt can attach
a route, including the receipt's uncertain-save outcome. Restart reopens that
checkpoint; it does not recover unsaved edits.

Use native OS decorations and parented dialogs. Graphite and Paper share typography,
spacing and semantic controls in an application-owned component layer. The gallery
uses that production layer. Add complex controls only with a production consumer.
Source/replay rendering belongs in a dedicated wgpu renderer with checked
binary input, revision-aware selection and explicit GPU resource lifetimes. Layout,
camera and drafts are UI state; project requirements are engine transactions.

## Migration and alternatives

Retain `apps/desktop` as a usable temporary baseline while porting complete operator
flows. Its correctness fixes continue; this is not a second product roadmap.

1. N1: real native project window, engine identity, Open/Save/history, bounded
   jobs/problems, dirty confirmation and child lifecycle. The central area explicitly
   states that 3D display is not implemented.
2. N2: source geometry, shared occurrences, native face inspection/picking and
   sections with atomic transfer/cancellation.
3. N3: one complete recoverable manufacturing journey through fresh actual motion,
   verification and exclusive export. Switch `pnpm dev` to native here; retain an
   explicitly named temporary legacy command and enumerate remaining parity gaps.
4. N4: existing workbench authoring, finish/image, process/replay, recovery and
   diagnostic parity through unchanged supported operator workflows.
5. N5: usable native packaging on the exercised host, legal resources and input/
   accessibility review; remove Tauri, CEF, React, Three and their build/run paths.

Keeping CEF indefinitely avoids a port but retains the browser/bridge/runtime
stack. An in-process engine would simplify calls but weaken process isolation
and duplicate lifecycle/authority boundaries. A large external design system would
bring unrelated dependencies and incompatible widget generations. Choose the
small upstream runtime and a production-driven component layer instead.

## Acceptance limits

Actual native-window project selection, state inspection, dirty cancellation,
saving and observed child exit are required for N1. Host tests and gallery renders
supplement that evidence. N2 requires visible native source and actual face picks;
N3 requires the complete manufacturing flow, not an attractive preview. Native
inspection is opt-in development tooling, disabled in distributions.

A local hardware-backed window does not establish Windows/macOS/Linux packaging,
universal GPU support, IME/screen-reader coverage, physical printability or machine
readiness. [Development](../quality/development.md#native-egui-workbench) owns the
reproduction procedure; existing geometry/manufacturing evidence remains binding.
