<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Storage rules

- Validate strict format-8 manifests, ordered surface/process documents, source scopes and allocators, enabled/disabled image references and sorted metadata, exact canonical PGM dimensions/bytes/SHA, source provenance and complete manufacturing bundle schema/hash/input/content before publication. Old formats are refused. Persisted verification metadata is not replay proof; engine/manufacturing owns independent replay and native admission. Ordinary open never falls back; explicit recovery reads the previous checkpoint and retains revision and every allocator high-water.
- Hold one stable OS writer lock in retained Storage; reuse that Storage for same-writer reopen. Read-only captures immutable assets without locking. Never unlink the lock or garbage-collect assets.
- Sync source, manufacturing and images/<hash>.pgm immutable assets before checkpoint/manifest publication; honor cancellation only before replacement starts. Protect image directory attachment identity like source/bundle directories. Staging directory publication must never replace an existing destination. Cleanup is confined to exclusively owned temporary entries.
- Maintain finite source, manufacturing, image and manifest budgets. Capture each incoming file only within the caller's remaining retained-allocation capacity, checking metadata before allocation; do not discard old state to make room. Each immutable asset directory separately permits 128 complete files/256 MiB and eight scratch files, whose combined bytes cannot exceed one maximum asset (16 MiB for source/bundle; 65551 bytes for image). Existing image validation uses bounded stack storage rather than a duplicate heap allocation. Return typed failures without repairing storage or weakening durability. See [format](../../../../docs/protocol/project.md) and [image decision](../../../../docs/adr/003-spatial-image-relief.md). Deterministic tests cover observable failure/cancel/recovery/concurrency behavior; platform certification is separate.
