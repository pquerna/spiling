<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Storage rules

- Validate bounded strict format-2 manifests, source SHA/definition/provenance, exact manufacturing bundle hash/size/schema/current-input provenance and every reference before publication. Persisted verification metadata is not replay proof; engine/manufacturing owns mandatory independent replay. Ordinary open never falls back; recovery explicitly reads the previous checkpoint.
- Hold one stable OS writer lock in retained Storage; reuse that Storage for same-writer reopen. Read-only captures immutable assets without locking. Never unlink the lock or garbage-collect assets.
- Sync assets before checkpoint/manifest publication; honor cancellation only before replacement starts. Staging directory publication must never replace an existing destination. Cleanup is confined to exclusively owned temporary entries.
- Maintain finite source, manufacturing and manifest budgets. Capture each incoming file only within the caller's remaining source/manufacturing retained-allocation capacity, checking metadata before allocation; do not discard the old state to make room. Each immutable asset directory separately permits 128 complete files/256 MiB and eight scratch files/16 MiB. Return typed failures without repairing storage or weakening durability. See [format](../../../../docs/protocol/project.md). Deterministic tests cover observable failure/cancel/recovery/concurrency behavior; platform certification is separate.
