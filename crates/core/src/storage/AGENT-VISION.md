<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Recoverable storage vision

Store exact imported source bytes, canonical content-addressed PGM image assets, software manufacturing bundle JSON with complete captured image content and complete format-6 manifests with inline bounded metadata-only surface overlays, with one independently integrity-validated previous checkpoint. Concurrent readers retain complete snapshots while the single writer advances. All immutable assets are synchronized before publishing records; failed first saves are retryable without deleting foreign paths. Source image paths and original PNG files are unnecessary for read-only open and explicit recovery. No eager asset GC, external paths, native/display/session data, undo serialization, image processing, program replay or printer execution. OS persistence primitives are implemented explicitly; cross-platform/power-loss guarantees remain evidence-gated.
