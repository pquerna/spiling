<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Protocol package rules

- Rust contracts own control types and protocol constants. `src/generated.ts` is generator output, never hand-maintained. The handwritten decoder and exports are owned here; there are no UI or native-shell dependencies.
- `decodeTriangle` accepts an ArrayBuffer or an ArrayBuffer-backed byte range. It enforces the generated binary limit, B0 counts, SPLT magic, schema, reserved zero, exact size, four-byte alignment, little-endian host order, finite positions, and index bounds before either typed view is constructed. Output views borrow the caller's bytes; callers must preserve buffer lifetime and not mutate them during GPU upload.
- B0's packed payload is a synthetic diagnostic triangle, not a generalized geometry mesh. The canonical wire description is `../../docs/protocol/control.md`; do not broaden the accepted format without a contract change.
- `check` runs TypeScript validation. `test` runs decoder invariants against the native-owned `../../fixtures/protocol/triangle.json` cross-language golden payload. Root CLI smoke additionally exercises bytes transferred from the real Rust engine. Neither replaces packaged end-to-end evidence.
- Maintain OSL source headers and JSON `.license` sidecars. Generated headers are emitted by the Rust generator.
