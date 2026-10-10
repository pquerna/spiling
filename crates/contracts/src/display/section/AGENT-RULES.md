<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Packed section contract rules

- Own SPLS encoding, summaries/loop metadata and borrowed section validation. The [display specification](../../../../../docs/protocol/display.md) is canonical; no native boolean, placement service or renderer lives here.
- Pack complete closed loops only, at most 40,000 points per loop, retaining occurrence order, outer/hole winding and plane-relative f64 coordinates. The native/composition producer supplies canonical loop rotation/order and real scene/occurrence identity. Never split or truncate a loop to fit a chunk.
- World-coordinate producers use push_loop; placed scene producers use push_loop_relative with coordinates computed directly relative to the plane origin. Both share identical validation/packing and avoid temporary coordinate copies. Never reconstruct a large world coordinate merely to subtract the same origin again.
- Validate the normalized plane/frame, finite coordinates, plane residual, offsets/alignment, identities, exact header/size/reserved bytes and SHA-256 before exposing borrowed views. Manifests enforce contiguous artifact-wide loop ranges and aggregate limits.
- Final chunks own encoded bytes; views borrow immutable caller storage. Consumer accounting covers active/staged artifacts and display resources separately. Empty sections have no chunks or loops, not a fake payload.
- Exercise `cargo test --locked -p spiling-contracts`, generator drift and `pnpm --filter @spiling/protocol test`. Algebraic interoperability fixtures do not prove native section topology, placement or numerical accuracy.
