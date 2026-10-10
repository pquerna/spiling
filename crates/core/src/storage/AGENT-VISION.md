<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Recoverable storage vision

Store exact imported source bytes, content-addressed software manufacturing bundle JSON and complete format-2 manifests, with one independently integrity-validated previous checkpoint. Concurrent readers retain complete snapshots while the single writer advances. Source and bundle assets are synchronized before publishing records; failed first saves are retryable without deleting foreign paths. No eager asset GC, external paths, native/display/session data, undo serialization, program replay or printer execution. OS persistence primitives are implemented explicitly; cross-platform/power-loss guarantees remain evidence-gated.
