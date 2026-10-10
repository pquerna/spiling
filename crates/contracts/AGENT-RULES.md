<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Runtime-independent wire authority rules

- Keep strict serde v4 project/geometry/manufacturing/job control types, frame bounds, explicit diagnostic triangle, persistent/source/session identities and packed SPLM/SPLS authority here. No storage, Tokio, Tauri, renderer, CAD or UI dependencies. Software-only profile declarations confer no physical support.
- Check frame magic/version/kind/nonzero request ID and length before allocation; validate exact payload length on writes. EOF between frames is clean, truncation is not.
- Generate TypeScript and small algebraic geometry goldens using `cargo run -p spiling-contracts --bin generate`; `--check` rejects byte drift. Never hand-edit generated.ts or format it independently of its generator.
- Exercise `cargo test -p spiling-contracts`, including truncation boundaries, persisted reference/counter/schema/current-input-hash invariants, bounded manufacturing data and cross-language fixtures. Geometry validators borrow endian-safe byte views before display allocation. See [control](../../docs/protocol/control.md), [project](../../docs/protocol/project.md), [manufacturing](../../docs/protocol/manufacturing.md) and owning module rules.
