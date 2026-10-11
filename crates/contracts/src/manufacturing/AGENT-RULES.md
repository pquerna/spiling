<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Software manufacturing contract rules

- Own explicit printer/recipe/intent, normalized plan, provenance, bundle/report/artifact and transport-independent manufacturing DTOs and admission validators. No geometry kernel, planner, storage, replay parser or runtime dependencies.
- Typed Manufacturing gRPC Execute owns Get/SetIntent; Compile/Verify return standard Google Operations with native metadata/results. Domain Inspect maps Verify with an explicit captured project base revision; immutable bundles are read through artifact descriptors and ByteStream, never chunk commands. Short reply failures use canonical Google Status, not success message arms.
- Protobuf adapters explicitly check every domain conversion in both directions, including enum values, finite measurements, profile/component budgets, hashes and descriptor/record correspondence. Bundle/profile authoring and persistence retain their existing bounded inert JSON schema; no JSON payload tunnels through protobuf. Generated TypeScript describes shell authoring data, never engine-executed code. No implicit profile/recipe defaults, old-format decoding, aliases or physical capability claims.
- Frame is explicit right-handed Cartesian XYZ millimetres; bed is envelope minimum Z. Current software supports one extruder, solid infill, fixed-height layers each containing deposition and no generated supports or empty aggregate layers. Component Box/Cylinder shapes are metadata, not collision certification.
- Validate finite positive process parameters, capabilities, reference integrity, semantic hashes and resource budgets before use. Nested plan deserialization explicitly borrows one owned cumulative segment budget across every layer/path and admits at most 100000 actual segments before point-vector growth; empty/one-point paths are rejected during admission. Preserve strict fields, per-array limits and serialized Rust/TypeScript shape. Semantic input fingerprints sort identities and exclude project revision/allocator/artifacts/timing; include captured source provenance and explicit intent.
- Bundle schema validation never establishes replay success: independent emitted-program replay is mandatory on compile and reopen. See [manufacturing protocol](../../../../docs/protocol/manufacturing.md).
- Exercise negative schema/resource/reference/hash and wire tests with `cargo test -p spiling-contracts`; generate all TypeScript centrally through the existing generator.
