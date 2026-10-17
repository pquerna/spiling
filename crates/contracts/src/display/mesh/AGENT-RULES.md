<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Packed mesh contract rules

- Own SPLM encoding, identity-free mesh descriptors and borrowed mesh validation. The [display specification](../../../../../docs/protocol/display.md) is canonical; no kernel, renderer or transport dependencies.
- Producers supply independently audited native interpolation and whole trimmed-domain deviation in face-table order under `mesh-trimmed-mm-0.05-v2`. Their finite nonnegative sum fits the frozen 0.025 mm native allowance; independently measured f32 displacement fits 0.025 mm. Check positions/unit normals, every index/face ordinal, errors, counts and bytes. Split triangles deterministically with local remapping and f64 origins; never drop geometry to fit a cap. Any push failure poisons the builder.
- `MeshChunkBuilder` takes definition identity, face count and profile, with optional remaining `MeshBudget` that only narrows frozen counts/bytes during packing. Final chunks own bytes plus `MeshChunkDescriptor`; publication moves fields through `into_metadata(session, artifact)` without re-encoding/copying payloads. Native producers never invent engine handles.
- Shared checked metadata validation admits only the current profile and separate achieved native/trim/f32 budgets through JSON, native adapters, chunks and manifests. Validate identities, exact header/size/reserved bytes, SHA-256, reconstructed bounds and all scalar/index values before exposing immutable borrowed views. Manifest validation enforces chunk order and aggregate limits; active/staged consumer budgets remain the consumer's responsibility. Descriptor admission is not a native coverage proof or manufacturing authority.
- Exercise `cargo test --locked -p spiling-contracts`, generator drift and `pnpm --filter @spiling/protocol test`. Native carrier/trim correctness requires the separate geometry corpus, not algebraic wire fixtures.
