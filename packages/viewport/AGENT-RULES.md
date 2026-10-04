<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Viewport rules

- Own Three.js scene and GPU resources only. Consume decoded display buffers from `@spiling/protocol`; never invoke Tauri, own native geometry, or synthesize an engine artifact.
- `createDiagnosticViewport(canvas, onFailure)` requests a WebGPU adapter/device and fully initializes the renderer before resolving. Pass the probed device to `WebGPURenderer` with `forceWebGL:false`. Three installs an automatic WebGL fallback, so the pinned `_getFallback` hook is disabled before initialization and the resulting backend is checked. Review that integration when updating Three.
- `showTriangle` borrows validated buffers and resolves after the first render. Rendering is single-flight and redraws on payload/size changes, not an idle animation loop. The grid and orthographic XY framing are display context only; diagnostic coordinates have no manufacturing units.
- Device loss and uncaptured GPU errors report a single failure and release resources. `dispose` disconnects resize observation, waits out rendering, disposes geometries/materials/renderer, then destroys the owned device. Every resolved viewport must be disposed by its consumer on teardown or replacement.
- `check` typechecks. Real desktop acceptance must verify WebGPU rendering, resize, loss diagnostics, and stop/restart cleanup on declared hardware; software GPU runs are diagnostics, not hardware support proof.
- Preserve OSL notices and adjacent JSON licensing sidecars.
