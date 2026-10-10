<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Project authority rules

- Core alone owns persistent project transactions, monotonic revisions/occurrence allocation, semantic dirty comparison and bounded session history. Dependencies are contracts, serialization, hashing and OS storage only; no kernel, runtime or UI.
- Snapshots share immutable captured source/manufacturing Vec allocations and Arc-owned intent; geometry/history edits never deep-copy printer/component data. Current/history pins are bounded separately: 32 definitions/64 MiB source bytes and 64 MiB manufacturing allocated Vec capacity, not merely serialized length; each manufacturing allocation is at most 16 MiB. History is at most 64 transactions and occurrences at most 256.
- Prepared edits are project/revision-bound and publish atomically. Save attaches a validated immutable checkpoint without clearing history. Read-only forbids mutation and saving.
- Explicit manufacturing intent and verified-worker artifact publication are prepared transactions. Geometry/intent edits invalidate the active artifact atomically; undo restores coherent intent/geometry/artifact. Core validates bundle framing, exact-byte hash, record and semantic-input provenance, never plans or independently replays programs.
- Storage guarantees and exact format are canonical in [project.md](../../docs/protocol/project.md); exercise `cargo test -p spiling-core` and actual engine crash/concurrency workflows. Tests do not establish power-loss/platform certification.
