<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Viewport vision

The viewport is a disposable WebGPU geometry display consumer, independent of React and the native shell. It displays engine-admitted definition meshes through shared occurrence batches, revision-aware source-face picking, native section overlays, and an orthographic inspection camera. Native BREP, source units, f64 carrier parameters, and section computation remain authoritative outside this package; GPU meshes and lines are approximate display only.

The supported display contract is derived multi-instance geometry: bounded decoded SPLM/SPLS chunks, atomic staged scene/section publication, origin-rebased placements, and explicit interrupted synchronization with resume. Project persistence and authoring remain outside the viewport. There is no diagnostic-triangle UI fallback, manufacturing planner, mesh-derived section, WebGL fallback, or per-occurrence duplicate geometry ownership.

Acceptance requires visible, correctly placed and pickable native objects on declared physical WebGPU reference machines, including repeated instances, large origins, section overlays, upload cancellation/resume, and awaited stop/restart/device-loss cleanup. Implementation alone does not establish that hardware gate or native geometry accuracy. Resource counters describe owned display buffers rather than unavailable total-driver-memory measurements.
