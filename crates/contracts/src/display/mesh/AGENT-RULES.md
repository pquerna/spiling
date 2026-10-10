<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Packed mesh contract rules

- Own SPLM encoding, identity-free mesh descriptors and borrowed mesh validation. The [display specification](../../../../../docs/protocol/display.md) is canonical; no kernel, renderer or transport dependencies.
- Producers supply audited native carrier deviation in face-table order. Check finite positions/unit normals, every index/face ordinal, profile errors, counts and bytes. Split triangles deterministically with local remapping and f64 origins; never drop geometry to fit a cap. Any push failure poisons the builder.
- `MeshChunkBuilder` takes definition identity, face count and profile, with optional remaining `MeshBudget` that only narrows frozen counts/bytes during packing. Final chunks own bytes plus `MeshChunkDescriptor`; publication moves fields through `into_metadata(session, artifact)` without re-encoding/copying payloads. Native producers never invent engine handles.
- Validate current identities, exact header/size/reserved bytes, SHA-256, reconstructed bounds and all scalar/index values before exposing immutable borrowed views. Manifest validation enforces chunk order and aggregate limits; active/staged consumer budgets remain the consumer's responsibility.
- Exercise `cargo test --locked -p spiling-contracts`, generator drift and `pnpm --filter @spiling/protocol test`. Native carrier/trim correctness requires the separate geometry corpus, not algebraic wire fixtures.
