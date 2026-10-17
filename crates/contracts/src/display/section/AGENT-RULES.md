<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Packed section contract rules

- Own SPLS encoding, summaries/loop metadata and borrowed section validation. The [display specification](../../../../../docs/protocol/display.md) is canonical; no native boolean, placement service or renderer lives here.
- Pack complete closed loops only, at most 40,000 points per loop, retaining occurrence order, outer/hole winding and plane-relative f64 coordinates. The native/composition producer supplies canonical loop rotation/order and real scene/occurrence identity. Never split or truncate a loop to fit a chunk.
- Native world-coordinate producers use `push_loop`; producers already holding plane-relative data use `push_loop_relative` without reconstructing world coordinates. Both lanes share validation/packing without temporary coordinate copies. Validate the coordinates actually encoded and anchor winding at their first loop vertex: representable tangential origin shifts preserve admission; an origin that erases a loop must refuse before publication.
- Validate the normalized plane/frame, finite coordinates, plane residual, offsets/alignment, identities, exact header/size/reserved bytes and SHA-256 before exposing borrowed views. Manifests enforce contiguous artifact-wide loop ranges and aggregate limits.
- Final chunks own encoded bytes; views borrow immutable caller storage. `SectionChunkBuilder::retained_bytes` counts pending and encoded capacities together. Shared-pool callers set remaining owned capacity before each push/finish; admit vector/string growth and the encoded copy while pending f64 points remain live. This does not change the packed wire cap. Other consumers account active/staged artifacts and display resources separately. Empty sections have no chunks or loops, not a fake payload.
- Exercise `cargo test --locked -p spiling-contracts`, generator drift and `pnpm --filter @spiling/protocol test`. Algebraic interoperability fixtures do not prove native section topology, placement or numerical accuracy.
