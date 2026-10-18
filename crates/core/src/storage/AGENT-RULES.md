<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Storage rules

- Validate bounded strict format-7 manifests, ordered surface/process documents, source scopes and all allocators, every enabled/disabled image reference and sorted sole metadata table, exact canonical PGM dimensions/header/byte count/SHA, source SHA/definition/provenance, exact manufacturing bundle hash/size/schema/current-input provenance and complete canonical content table before publication. Old project formats are explicitly refused, even when their image directory is absent. Persisted verification metadata is not replay proof; engine/manufacturing owns mandatory independent replay and native face-table admission. Ordinary open never falls back; recovery explicitly reads the previous checkpoint and retains revision, occurrence, surface-layer, process-layer and process-operation allocator high-water.
- Hold one stable OS writer lock in retained Storage; reuse that Storage for same-writer reopen. Read-only captures immutable assets without locking. Never unlink the lock or garbage-collect assets.
- Sync source, manufacturing and images/<hash>.pgm immutable assets before checkpoint/manifest publication; honor cancellation only before replacement starts. Protect image directory attachment identity like source/bundle directories. Staging directory publication must never replace an existing destination. Cleanup is confined to exclusively owned temporary entries.
- Maintain finite source, manufacturing, image and manifest budgets. Capture each incoming file only within the caller's remaining retained-allocation capacity, checking metadata before allocation; do not discard old state to make room. Each immutable asset directory separately permits 128 complete files/256 MiB and eight scratch files, whose combined bytes cannot exceed one maximum asset (16 MiB for source/bundle; 65551 bytes for image). Existing image validation uses bounded stack storage rather than a duplicate heap allocation. Return typed failures without repairing storage or weakening durability. See [format](../../../../docs/protocol/project.md) and [image decision](../../../../docs/adr/003-spatial-image-relief.md). Deterministic tests cover observable failure/cancel/recovery/concurrency behavior; platform certification is separate.
