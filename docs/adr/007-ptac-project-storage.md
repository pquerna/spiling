<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# ADR007: bounded PTAC project containers

## Decision

Save projects as **`.spil`**, a closed Spiling profile of a transactional SQLite
asset container. `spiling-core-ptac` independently owns physical/schema/graph
admission, immutable SHA-256 objects, fixed 256 KiB RAW/ZSTD chunks, current and
optional previous snapshots, bounded maintenance, filesystem ownership and
classified publication. Core retains project schemas, semantic closure,
transactions, undo, allocation budgets and native engine promotion authority.

This is a clean cutover. No previous Spiling directory format is read, converted,
migrated or dual-written. Recreate development projects from original inputs.
The physical container starts at revision 1; existing project/process/surface/
manufacturing and packed output numbers do not change merely for storage.

## Why

A complete root and its immutable membership publish in one SQLite transaction,
rather than independently replacing checkpoint and manifest files. A separate
strict allocator/revision guard preserves high-water information when the current
payload is damaged. Every ordinary open consumes these floors too; they are not
only recovery metadata. Two roots suffice for explicit previous-save recovery;
bounded session undo does not become durable history.

Reusing bundled SQLite isolates a dependency already used by the independent Jobs
ledger. A mandatory nondefault VFS guard confines pager recovery before catalog
admission: SQL authorization alone cannot prevent a hostile rollback journal from
naming external recovery files. SQLite remains responsible for pages, locking and
rollback, not project meaning. Separate physical, graph, domain and native checks
must all succeed.

## Alternatives not selected

No generic KV/backend trait, Turso/redb fallback, commit DAG, persistent map tree,
asset revision registry, CBOR language, second hash family, plugin type registry or
SQL scene normalization. One strict existing JSON project root is the smallest
representation matching current Spiling authority. Unknown required roles and
schemas refuse rather than round-trip as extensions.

## Consequences and limits

Keep a separate stable lifetime writer lease. Never open/close a raw main-file
descriptor alongside live SQLite handles, replace a live database, or publish a
new file using hard-link-plus-unlink. New targets stage privately, close and check
the database, then use atomic no-replace rename. Acquire the irreversible decision
only immediately before COMMIT or namespace publication. Ambiguous errors are not
proof of nonpublication; retain ownership and return explicit uncertainty without
automatic retries or a fabricated saved receipt.

Collection is explicit and atomic; compaction creates a new file, preserving both
roots and exact logical bytes. It is not an in-place live-file swap. A closed
confirmed `.spil` is portable; an active/crashed journal must not be omitted.

The [physical contract](../protocol/project-storage.md) and
[project semantics](../protocol/project.md) are canonical. Deterministic faults and
process kills are software evidence, not power-loss, network/cloud filesystem,
cross-platform packaging or physical manufacturing qualification. Non-Unix paths
currently refuse guarded storage rather than taking an unsafe fallback.
