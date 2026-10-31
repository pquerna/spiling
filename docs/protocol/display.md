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

Profile `mesh-trimmed-mm-0.05-v2` replaces the old carrier-only profile without a compatibility reader. Native production certifies the complete trimmed-native triangle chain, including regular-chart interpolation, trim displacement, orientation and whole-domain coverage. Metadata reports finite nonnegative achieved `carrier_deviation_mm` and `trim_deviation_mm` separately; their sum must not exceed the frozen 0.025 mm native allowance. Independently measured `quantization_error_mm` must not exceed 0.025 mm, and all three errors together must not exceed 0.05 mm. These scalars live in the typed descriptor, not the unchanged SPLM1 bytes; private native proof payloads are not serialized. Checked JSON, native adapters, manifests and decoders reject absent trim metadata, old profiles or invalid budgets. Accepting a descriptor checks structural admission, not native numerical proof or manufacturing authority.

Chunks split complete triangles with local vertex remapping in deterministic face order. They never drop triangles to fit a byte limit. Shared occurrences do not create additional definition-mesh bytes.

Definition display availability is explicit: `DefinitionRecord.display` is either `mesh` with a nonzero artifact identity or `unavailable` with a bounded typed geometry error. An unavailable display has no synthetic artifact, chunks or GPU picking targets. The definition's native bounds and canonical source-face table remain available independently for scene fitting and list-based native face inspection. Mixed rendered/non-rendered scenes publish atomically; a nonempty native scene may legitimately have zero mesh bytes and must not be labelled empty. Import/open Scene operation results publish no display outputs when their native scene has zero mesh bytes. Display refusal does not authorize or invalidate unrelated native queries; each operation retains its own evidence/error boundary.

Definition bounds conservatively enclose native geometry; they are not required to be tight. In particular, complete carrier-domain enclosures can exceed the trimmed material. These scene-fitting bounds grant no precision, placement or manufacturing authority; those operations use their own native/source-bound queries and declared error budgets.

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

Body: u32 loop offsets `[loop_count+1]`; zero padding to an eight-byte boundary; then f64 XYZ `[3*point_count]` relative to plane origin. Offsets start at zero and end at point count. Each offset difference is at least four and at most 40,000 points, including the closing point. Coordinates are finite; loops are closed and on the normalized metadata plane within 1e-4 mm. The engine summary plane equals one normalization of the original requested plane; query and encoder inputs derive from that same raw request, since repeated f64 normalization can change coefficients. A loop is never split across chunks. Oversized loops are rejected rather than truncated.

Outer/hole winding is measured in the declared canonical frame. Accumulate signed area relative to the first encoded loop vertex, not a potentially distant plane origin. Representable tangential origin shifts preserve loop admission and physical coordinates; reject an origin that collapses the actual plane-relative encoded loop rather than publishing bytes that consumers cannot validate.

Artifact descriptors must cover chunks in ascending contiguous chunk order and loops in contiguous non-overlapping ordinal ranges. Descriptor byte counts, hashes, chunk indices, first loop ordinals and loop counts agree with payloads. Empty sections have no loops or chunks. Paged loop metadata maps each artifact-wide ordinal to occurrence, definition and outer/hole status. These derived loop identities do not remap native source faces.

Native sections use 0.005 mm chordal sampling tolerance and 1e-5 mm Boolean tolerance; byte validation cannot certify the producer's geometric method. Geometry's placed query owns pose/plane covariance and canonical native loop ordering; engine composition owns publication identity. Pack native world coordinates directly, or retain plane-relative coordinates when already supplied, without reapplying placement or reconstructing world coordinates only to subtract the same origin. Algebraic fixtures establish wire interoperability, not native-section correctness.

## Verification boundary

`cargo test --locked -p spiling-contracts` exercises encoders and borrowed validators. `pnpm --filter @spiling/protocol test` independently decodes algebraic fixtures and actual native box/through-hole goldens, validating physical extents, plane residual, area, winding and multi-chunk order. `cargo run --locked -p spiling-geometry --example generate_native_goldens -- --check` reproduces those native bytes through the public facade. Decoder tests do not themselves execute STEP conversion, native booleans, engine transfer or GPU upload; [geometry evidence](../quality/geometry-evidence.md) records those independent surfaces.
