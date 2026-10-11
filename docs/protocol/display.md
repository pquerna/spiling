<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Packed geometry display layouts

The [authenticated gRPC control API](control.md) publishes these unchanged version-one packed layouts through immutable artifact descriptors and Google ByteStream.Read. The native engine, shared CLI client and thin workbench consume the same descriptors. Numerical geometry, actual process transfer and physical WebGPU acceptance remain separate from byte interoperability; protocol availability alone establishes none of those gates.

Rust owns DTOs and constants in `crates/contracts/src/geometry/` and encoders/checked validators in `crates/contracts/src/display/`. TypeScript domain types are generated; independent decoders live in `packages/protocol/src/mesh.ts` and `section.ts`. The original SPLT diagnostic is unchanged.

## Shared rules

All numeric fields are little-endian. Headers are exactly 64 bytes. Flags and reserved bytes must be zero. Schema version is 1. A chunk is at most 1,048,576 bytes; counts and arithmetic are checked before allocating or constructing typed views. Hashes are lowercase 64-character hexadecimal SHA-256 of the complete header and body. Descriptor identity must match the expected session, artifact, definition (mesh), and chunk index before admission. Hash success does not replace validation of the layout or numeric invariants.

Decoders validate with byte reads before constructing borrowing typed views. The caller owns the backing buffer and must not mutate it during validation, between validation and upload, or while views remain in use. A successful decode is not permission to exceed combined active/staged renderer budgets.

## SPLM mesh, version 1

| Byte offset | Field                                       |
| ----------- | ------------------------------------------- |
| 0..4        | ASCII `SPLM`                                |
| 4..6        | u16 schema version                          |
| 6..8        | u16 zero flags                              |
| 8..12       | u32 vertex count                            |
| 12..16      | u32 index count                             |
| 16..20      | u32 triangle count                          |
| 20..24      | u32 chunk index                             |
| 24..48      | f64[3] local origin, mm in definition frame |
| 48..64      | zero reserved bytes                         |

Body, without gaps: f32 positions `[3*vertex_count]`, f32 normals `[3*vertex_count]`, u32 indices `[index_count]`, u32 source-face ordinals `[triangle_count]`. Index count equals three times triangle count. Indices are less than vertex count; face ordinals are less than the descriptor's face-table size. Source-face IDs are strings, not JavaScript entity numbers: `step:9007199254740993` must survive unchanged. Face ordinals address a separately supplied definition-scoped canonical face-use table.

Positions reconstruct as `local_origin_mm + f64(position_f32)`. Origins, positions and normals are finite. Normals are definition-frame unit directions with norm error at most 1e-4. Bounds describe the native vertices of this chunk, not the certified bounds of the complete native solid. Reconstructed vertices must be inside those bounds expanded by `quantization_error_mm + 1e-9` mm.

Profile `mesh-mm-0.05-v1` requires finite nonnegative carrier deviation at most 0.025 mm and quantization displacement at most 0.025 mm, with their sum at most 0.05 mm. The encoder measures Euclidean reconstruction displacement. The native producer must establish carrier deviation; accepting a descriptor alone is not proof of source accuracy or trimmed-region Hausdorff error.

Chunks split complete triangles with local vertex remapping in deterministic face order. They never drop triangles to fit a byte limit. Shared occurrences do not create additional definition-mesh bytes.

## SPLS section, version 1

| Byte offset | Field                                |
| ----------- | ------------------------------------ |
| 0..4        | ASCII `SPLS`                         |
| 4..6        | u16 schema version                   |
| 6..8        | u16 zero flags                       |
| 8..12       | u32 loop count in this chunk         |
| 12..16      | u32 point count                      |
| 16..40      | f64[3] plane origin, scene mm        |
| 40..44      | u32 chunk index                      |
| 44..48      | u32 first artifact-wide loop ordinal |
| 48..64      | zero reserved bytes                  |

Body: u32 loop offsets `[loop_count+1]`; zero padding to an eight-byte boundary; then f64 XYZ `[3*point_count]` relative to plane origin. Offsets start at zero and end at point count. Each offset difference is at least four and at most 40,000 points, including the closing point. Coordinates are finite; loops are closed and on the normalized metadata plane within 1e-4 mm. A loop is never split across chunks. Oversized loops are rejected rather than truncated.

Artifact descriptors must cover chunks in ascending contiguous chunk order and loops in contiguous non-overlapping ordinal ranges. Descriptor byte counts, hashes, chunk indices, first loop ordinals and loop counts agree with payloads. Empty sections have no loops or chunks. Paged loop metadata maps each artifact-wide ordinal to occurrence, definition and outer/hole status. These derived loop identities do not remap native source faces.

Section results use native intersections sampled at 0.005 mm chordal tolerance with boolean tolerance 1e-5 mm; byte validation cannot certify the producer's geometric method. Engine composition rotates local samples directly into plane-relative coordinates before ordering/packing, avoiding a large-world-coordinate roundtrip. Algebraic loop fixtures test wire interoperability only and must not be cited as native-section evidence.

## Verification boundary

`cargo test --locked -p spiling-contracts` exercises encoders and borrowed validators. `pnpm --filter @spiling/protocol test` independently decodes algebraic fixtures and actual native box/through-hole goldens, validating physical extents, plane residual, area, winding and multi-chunk order. `cargo run --locked -p spiling-geometry --example generate_native_goldens -- --check` reproduces those native bytes through the public facade. Decoder tests do not themselves execute STEP conversion, native booleans, engine transfer or GPU upload; [geometry evidence](../quality/geometry-evidence.md) records those independent surfaces.
