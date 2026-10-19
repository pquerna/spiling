<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Recoverable storage vision

Store exact source bytes, canonical content-addressed PGM assets, manufacturing bundle JSON with captured content and complete format-8 manifests with bounded ordered surface/process documents, retaining one independently validated previous checkpoint. Readers retain complete snapshots while the single writer advances. Immutable assets synchronize before publication; failed first saves remain retryable without deleting foreign paths. Source-image paths and original PNGs are unnecessary for read-only open/recovery. No eager asset GC, external paths, session data, persisted undo, image processing, program replay or printer execution. OS durability remains platform/evidence-gated.
