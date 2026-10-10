<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Protocol package vision

This dependency-free package gives TypeScript consumers Rust-generated control contracts and validated packed display buffers. The engine remains authoritative; the package owns neither planning nor rendering.

The package exposes generated bounded control ABI v4 project/geometry/manufacturing/job DTOs, data-only printer-authoring types and independently checked schema-v1 SPLT diagnostics and SPLM/SPLS geometry buffers. Original algebraic vectors isolate interoperability; native box/through-hole vectors connect real facade output to independent decoding. Core owns durable authoring; Rust owns manufacturing validation/planning/replay; actual storage/runtime/desktop acceptance remains separate.

The package's acceptance boundary is cross-language byte compatibility, explicit expected identities/revisions, immutable borrowed buffers, and rejection of malformed hashes, sizes, alignment, coordinates, normals, source-face ordinals, and section loop ranges before GPU upload. Algebraic display/section goldens do not establish native import, tessellation, section correctness, or desktop acceptance. Native geometry remains authoritative; the package neither computes BREP geometry nor supplies a mesh-derived section fallback.
