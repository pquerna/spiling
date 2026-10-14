<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Project authority rules

- Core alone owns persistent project transactions, monotonic revisions/occurrence/surface-layer allocation, semantic dirty comparison and bounded session history. Dependencies are contracts, serialization, hashing and OS storage only; no kernel, runtime or UI.
- Snapshots share immutable captured source/manufacturing Vec allocations, Arc-owned intent and Arc-owned ordered surface document; geometry/history edits never deep-copy unchanged documents or printer/component data. Current/history/staged pins are bounded separately: 32 definitions/64 MiB source bytes, 64 MiB manufacturing allocated Vec capacity and 16 MiB surface allocated capacity. Each manufacturing allocation is at most 16 MiB; each surface document at most 64 layers/32 KiB. History is at most 64 transactions and occurrences at most 256.
- Prepared edits are project/revision-bound and publish atomically. Save attaches a validated immutable checkpoint without clearing history. Read-only forbids mutation and saving.
- Explicit manufacturing intent, whole-face surface metadata and verified-worker artifact publication are prepared transactions. Geometry/intent/surface edits invalidate the active artifact atomically; undo restores coherent inputs/artifact without rewinding allocators. Occurrence deletion cascades its surface layers; final-definition deletion cascades definition layers. Core validates structural target references, bundle framing, exact-byte hash, record and complete semantic-input provenance, never native face identity, plans or independently replayed programs. Region ownership and separate object/lifecycle report totals persist as checked immutable bundle content, not Core-generated metrics or physical qualification.
- JSON parsing preserves finite `f64` values exactly (`serde_json/float_roundtrip`): durable artifact summaries and committed records compare exactly. Do not weaken equality to hide serialization drift or silently re-pin changed totals.
- Storage guarantees and exact format are canonical in [project.md](../../docs/protocol/project.md); exercise `cargo test -p spiling-core` and actual engine crash/concurrency workflows. Tests do not establish power-loss/platform certification.
