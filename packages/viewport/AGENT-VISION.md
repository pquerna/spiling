<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Viewport vision

The viewport is a disposable WebGPU display consumer, independent of React and the native shell. B0 renders only the actual engine-delivered synthetic triangle with a reference grid and orthographic front view. It makes no authoritative geometry, precision, picking, manufacturing, or zero-copy interop claim.

The intended workbench renderer will display revision-aware packed mesh and toolpath batches without taking ownership of BREP authority. Device availability and resource lifecycle must remain explicit; no hidden WebGL fallback or synthetic successful render may bypass a failed GPU gate. The present acceptance condition is a working real WebGPU path with deterministic ownership and useful loss diagnostics.
