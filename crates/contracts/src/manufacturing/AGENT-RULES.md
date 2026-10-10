<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Software manufacturing contract rules

- Own explicit printer/recipe/intent, normalized plan, provenance, bundle/report/artifact and protocol-v4 manufacturing DTOs and admission validators. No geometry kernel, planner, storage, replay parser or runtime dependencies.
- Rust consumes inert bounded JSON; generated TypeScript describes authoring data, never engine-executed code. No implicit profile/recipe defaults, old-format decoding, aliases or physical capability claims.
- Frame is explicit right-handed Cartesian XYZ millimetres; bed is envelope minimum Z. Current software supports one extruder, solid infill, fixed-height layers each containing deposition and no generated supports or empty aggregate layers. Component Box/Cylinder shapes are metadata, not collision certification.
- Validate finite positive process parameters, capabilities, reference integrity, semantic hashes and resource budgets before use. Nested plan deserialization explicitly borrows one owned cumulative segment budget across every layer/path and admits at most 100000 actual segments before point-vector growth; empty/one-point paths are rejected during admission. Preserve strict fields, per-array limits and serialized Rust/TypeScript shape. Semantic input fingerprints sort identities and exclude project revision/allocator/artifacts/timing; include captured source provenance and explicit intent.
- Bundle schema validation never establishes replay success: independent emitted-program replay is mandatory on compile and reopen. See [manufacturing protocol](../../../../docs/protocol/manufacturing.md).
- Exercise negative schema/resource/reference/hash and wire tests with `cargo test -p spiling-contracts`; generate all TypeScript centrally through the existing generator.
