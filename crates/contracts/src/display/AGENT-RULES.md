<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Packed display encoding rules

- Own shared packed-display byte primitives and aggregate policy. [Mesh](mesh/AGENT-RULES.md) and [section](section/AGENT-RULES.md) codecs have distinct owning directories, schemas and local invariants; the [display specification](../../../../docs/protocol/display.md) is canonical.
- Keep version-one little-endian 64-byte layouts independent of kernel, renderer and transport. Never cast arbitrary bytes to Rust memory layouts; checked scalar views require no second decoded mesh or section allocation.
- Validate identities, exact size/schema/reserved bytes, hashes and schema-specific geometry before returning views. Caller bytes remain alive and immutable while borrowed; consumer accounting also covers active/staged artifacts and GPU resources.
- Encoders own final chunks. Transfer or validate one chunk at a time; never concatenate/copy a complete artifact. Native geometry supplies audited data, while engine composition owns publication identity and placement.
- `generate` owns small original algebraic SPLM/SPLS goldens under fixtures/protocol. They certify wire interoperability only. Protocol v1 framing and empty geometry capabilities remain unchanged until coordinated native integration.
- Exercise `cargo test --locked -p spiling-contracts`, generator drift and independent TypeScript decoder tests. Native corpus and packaged GPU acceptance are separate gates.
