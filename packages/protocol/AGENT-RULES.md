<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Protocol package rules

- Protobuf owns the native API; Rust checked domain adapters derive the sole shell view and packed artifact constants. `src/generated.ts` is generator output, never hand-maintained. This package contains no UI, kernel, renderer, native-shell or gRPC-Web transport dependencies.
- `decodeTriangle` remains the bounded synthetic SPLT diagnostic decoder; do not broaden it into geometry. `decodeMesh` and `decodeSection` independently validate unchanged SPLM/SPLS bytes, hashes, identities, revisions, exact layout/alignment, finite values, bounds and face/loop ranges before returning borrowed views.
- Validate complete descriptor/loop pages and aggregate budgets against expected identity/revision before consuming chunks. Resource descriptors correspond one-to-one to chunk pages; retrieve bytes through standard ByteStream names, not numeric native tasks or a framed command tunnel. Preserve caller buffer lifetime and immutability through hashing and GPU upload; fetch/hash/upload one bounded chunk at a time.
- Generated printer/recipe/component types are inert data-authoring contracts. Rust validates intent and owns planning and emitted-program replay; persisted manufacturing bundles remain distinct from protobuf. Software-only artifacts do not establish machine support.
- `check` validates TypeScript; `test` exercises Rust-owned algebraic and native-derived frozen fixtures. Run the central generator and drift check after contract changes. Cross-language bytes alone do not establish native correctness, durability or GPU support; canonical schemas live under `../../docs/protocol/`.
- Maintain OSL source headers and JSON `.license` sidecars. Generated headers are emitted by the Rust generator.
