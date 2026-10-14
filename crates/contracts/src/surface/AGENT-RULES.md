<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Source-face surface contract rules

Own the single ordered whole-face surface-finish channel, checked nonzero layer IDs, explicit definition/occurrence scopes and Smooth/VerticalRibs/CylindricalRibs/ImageRelief parameters. No material, appearance or arbitrary procedural variants; no planner, codec, native kernel or storage dependencies.

Schema3 admits at most64 layers and32KiB serialized/allocated metadata bytes; bounded deserialization rejects excess before vector growth. The sole images table is canonical-hash sorted, unique and exactly matches every enabled/disabled image layer. Canonical content is exact binary P5 header plus Gray8 nodes, dimensions2..256, per-image65551bytes,16images/1MiB content,4MiB retained canonical allocations. Borrowed validation checks header/dimensions/full-byte SHA-256 without copying pixels. Source metadata is bounded; image content is never repeated in layers/status/inspection.

All mapping values are finite: positive depth/physical source-Z period/vertical error, U repeats1..256, U phase[0,1), source-Z phase[0,period), nonnegative margin. Carrier/process-specific protected-edge and cylinder-zero-margin admission belongs to compilation. Existing rib units/phases and source-local cylinder orientation remain unchanged. Structural targets check source identities and allocator high-water; native face admission remains engine-owned. Old schemas fail without defaults/readers. See [image relief RFC](../../../../docs/adr/003-spatial-image-relief.md) for pixel-node orientation, periodic interpolation and taper.

Enabled definition layers resolve in stored order before enabled occurrence layers; last match wins. effective_layer is the sole precedence authority and preserves layer identity for diagnostic attribution. Smooth overrides lower finishes; removal/disabling reveals lower fills. Disabled layers and the exact canonical/source metadata remain fingerprint/persistence authority. Core owns immutable shared assets, IDs, transactions, history and cascading deletion. Typed protobuf adapters validate every field; TypeScript is centrally generated.

Exercise scope/order/reveal, duplicate/zero/high-water IDs, malformed mappings/schema/targets, borrowed canonical content/hash/dimensions, exact metadata/content references, bounded admission and typed adapter roundtrips through parent-owned contracts/Core verification. See [project protocol](../../../../docs/protocol/project.md).
