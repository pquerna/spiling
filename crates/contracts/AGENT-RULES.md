<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Runtime-independent wire authority rules

- Keep serde control types, frame bounds, and triangle schema constants authoritative here. No Tokio, Tauri, renderer, CAD, or UI dependencies.
- Check frame magic/version/kind/nonzero request ID and length before allocation; validate exact payload length on writes. EOF between frames is clean, truncation is not.
- Generate TypeScript using `cargo run -p spiling-contracts --bin generate`; `--check` rejects byte drift. Never hand-edit generated.ts or format it independently of its generator.
- Exercise `cargo test -p spiling-contracts`, including every truncation boundary and the cross-language fixture. See [control protocol](../../docs/protocol/control.md).
