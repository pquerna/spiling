<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Source-face surface contract rules

Own the single ordered whole-face surface-finish channel, checked nonzero layer IDs, explicit definition/occurrence scopes and Smooth/VerticalRibs parameters. No material, appearance, image or arbitrary procedural variants; no planner, native kernel or storage dependencies.

Schema1 admits at most64 layers and32KiB serialized/allocated document bytes; bounded deserialization rejects excess before vector growth. Depth, pitch and edge margin are finite positive millimetres; phase is finite in[0,pitch). Structural target validation checks stored source identities and allocator high-water. Canonical STEP face syntax is not proof of a real native face: engine must validate the exact rebuilt source face table.

Enabled definition layers resolve in stored order before enabled occurrence layers; last match wins. Smooth overrides lower ribs, while removal/disabling reveals lower fills. Disabled layers remain fingerprint/persistence authority. Core owns immutable Arc snapshots, IDs, transactions,16MiB retained allocations, history and cascading deletion. Projects protobuf adapters carry explicit typed fields; TypeScript is centrally generated.

Exercise scope/order/reveal, duplicate/zero/high-water IDs, malformed parameters/schema/targets, bounded admission and typed adapter roundtrips through parent-owned contracts/Core verification. See [project protocol](../../../../docs/protocol/project.md).
