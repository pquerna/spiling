<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Protocol package rules

- Rust contracts own all domain metadata and protocol/schema constants. `src/generated.ts` is generator output, never hand-maintained. Handwritten checked decoders and additive exports are owned here; there are no UI, renderer, kernel, or native-shell dependencies.
- `decodeTriangle` remains the synchronous, synthetic B0 SPLT diagnostic decoder. Do not broaden it into CAD geometry or change its existing exports.
- Async `decodeMesh` (SPLM) and `decodeSection` (SPLS) validate generated descriptor fields, expected identity, SHA-256 through Web Crypto, bounded exact layouts, alignment, reserved zero, finite coordinates, and element/range invariants before creating borrowing output views. Mesh validation checks normals, indices, face ordinals, reconstructed bounds, and profile errors; section validation checks complete closed planar loops, winding, and matching loop metadata. Full manifest validation checks ordered page completeness and aggregate byte budgets; section ranges must be contiguous and nonoverlapping.
- Callers must validate complete descriptor/loop pages against the expected session/artifact/definition or scene revision before consuming chunks, keep each input immutable throughout asynchronous decoding, and preserve its lifetime/immutability while borrowed views are used. Fetch/hash/upload at most one bounded chunk at a time; these decoders do not account for combined active/staged GPU memory or establish native geometry accuracy.
- `check` runs TypeScript validation. `test` exercises borrowing/rejection against Rust-owned algebraic and native-derived box/through-hole goldens under `../../fixtures/protocol/`. Native goldens are generated through the public geometry facade, separately from the cheap contract generator. Control ABI v4, project format v2 and packed schema v1 are distinct; generated project DTOs do not implement storage/undo. Canonical specifications live under `../../docs/protocol/`; cross-language decoding alone proves neither durability nor GPU support.
- Generated printer, recipe, component and manufacturing DTOs are the TypeScript data-authoring contract. Type checking is not Rust runtime validation, native planning or emitted-program replay. The engine receives inert JSON, never authoring code; packed geometry decoders do not decode manufacturing bundles.
- Maintain OSL source headers and JSON `.license` sidecars. Generated headers are emitted by the Rust generator.
