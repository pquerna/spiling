<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Engine operation evidence

Executed on 2026-10-07 in the Linux cloud workspace, against the working-tree implementation based on commit `97f9df1326519e5462dc605b3c46d366c4ef717f`. This is development evidence, not release or physical GPU acceptance.

## Procedures and observed results

- The real-child integration suite passed fourteen tests covering admission, UUID retries and conflicts, queued/running cancellation, completion races, partial snapshots and reconnect, jobs beyond unary deadlines, restart recovery and deduplication, exclusive store ownership, parent-bound pagination, ranged binary retrieval, slow consumers, authentication, ownership EOF, capacity bounds and child reaping.
- CLI smoke passed, including incremental retrieval, Rust-to-TypeScript binary decoding, malformed binary rejection and Linux non-UTF-8 executable paths. The four-chunk diagnostic produced partial updates before completion.
- TypeScript checking, frontend production build, generated contract checking, Rust formatting, warning-denying Clippy and repository formatting passed. The native desktop debug build passed.
- `tools/smoke-desktop.mjs --scenario engine-operations` passed against the real CEF application under Xvfb. Through native invokes it observed a running operation with a complete 64-byte artifact, downloaded the bytes, cancelled with canonical code 1, retained engine availability, restarted and recovered an interrupted operation with canonical code 10. Closing the native window exited both application and engine.

The native smoke deliberately bypasses GPU admission and makes no rendering claim. It uses debug CDP, explicitly unsandboxed CEF and SwiftShader in this container. GTK/D-Bus and Chromium environment warnings occurred without failing the bridge assertions. Normal viewport presentation and physical GPU acceptance were not rerun as part of this operation test.

## Environment preparation

Tool versions are pinned by the repository and activated through the workspace setup. CEF's downloader encountered a certificate trust failure. System-trusted HTTPS downloaded the exact pinned upstream archive; the official extraction helper verified its upstream SHA-1 (`9711b86c105fb590da576fe5a829802f1a79d520`) before extraction. TLS verification was not disabled. Missing local development-library symlink targets were connected to matching already-installed system runtime libraries within the workspace tooling directory.

## Scope and limits

This validates durable diagnostic operations and the native communication path. It does not validate geometry correctness, native-kernel fault isolation, automatic computational resumption or remote engine service deployment. Retention is bounded to 128 operations and a conservative 32 MiB reserved output budget; deletion and garbage collection are not implemented. Exhaustion returns a resource-exhausted error. Unfinished operations become interrupted on engine shutdown or recovery, retaining completed output.
