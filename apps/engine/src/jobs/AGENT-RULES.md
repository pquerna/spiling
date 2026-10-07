<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Durable diagnostic operations rules

- SQLite with WAL/FULL synchronization owns atomic acceptance/deduplication and output/state publication. One engine holds the store file lock. Persist before acknowledgement or observation.
- Admission belongs to the engine even when the RPC is dropped. A supplied nonzero UUID deduplicates identical requests within the parent while retained; conflicting parameters fail ALREADY_EXISTS.
- Worker reads immutable inputs. Terminal states never change. Cancel intent races with completion under the store owner; interrupted persisted jobs recover as ABORTED, never replayed.
- Bound retained operations, pending work, reserved storage and chunk size. A coalescing watcher carries latest complete state, not durable event history. All outputs reference complete SQLite blobs committed with the snapshot.
- Keep synthetic diagnostic limits explicit. No authoring mutation or current-plan publication exists; revision labels do not imply a project implementation.
- Acceptance tests exercise actual child cancellation, retry, restart, partial reads, capacity and terminal races.
