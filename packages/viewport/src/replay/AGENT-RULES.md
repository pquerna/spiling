<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Replay display rules

- Consume only independently decoded SPLR actual-program display bytes and Rust-generated descriptors. No planner, G-code parser, native shell, freshness decisions or machine execution belongs here.
- Batch schema-3 object/bridge/support/interface/travel/lifecycle roles in packed LineSegments and stationary retract/unretract in packed Points, never event objects or fictional movement. First unknown-start endpoint is explicit. Layer/event/travel controls change draw state only; empty contact-gap schedule selection shows no events. Lifecycle sentinel65535 is not a body layer and appears only in all-layer view. Travel toggle hides travels/unknown marker, not prime or stationary feeder events.
- `replayBudget` charges the immutable packed bytes, CPU projected positions/ordinal arrays and GPU positions before allocation through the owning viewport's combined active/staged 128 MiB admission. Renderer-private uniforms/driver allocations are outside owned-buffer counters, as for geometry.
- Origin-rebase f64 endpoints before f32 projection. Reject projections and overlay translations exceeding 0.001 mm; never silently clamp or invent a fallback. Preserve original bytes for strided draw-state queries.
- The owning viewport serializes admission/upload/render, stages and atomically installs complete motion, and awaits GPU work before cleanup. This module owns and disposes only its own geometries/materials; device loss uses the viewport's fatal surface.
- Physical WebGPU visual acceptance must exercise native actual coordinates, unknown/coincident travel, layer/event/travel controls, large origins, mesh/section coexistence, replacement and device-loss teardown. CPU checks and software GPU diagnostics do not establish that gate.
