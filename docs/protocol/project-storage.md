<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Spiling PTAC physical storage

`crates/core-ptac` is the independently testable container authority. Its executable
DDL, catalog admission and constants own the representation; [project.md](project.md)
owns strict domain JSON and exact asset-role closure. No SQL connection, arbitrary
SQL mutation, unvalidated JSON value or alternative storage backend is public API.
Only Core uses this crate in production. The Jobs operations database is separate.

## Identity and closed schema

A `.spil` is an ordinary SQLite database, page size 4096,
`application_id=0x50544143` (`PTAC`), `user_version=1`, profile `spiling.project`.
Filename suffix is a dialog convention, not admission evidence. Old project
directories, unrelated SQLite files and altered schemas refuse without repair or
compatibility. All six tables are STRICT with required NOT NULL, length/range/CHECK
constraints and explicit foreign keys:

| Table              | Meaning                                                                                                                          |
| ------------------ | -------------------------------------------------------------------------------------------------------------------------------- |
| `container`        | Exactly one singleton: profile, positive generation, current/optional distinct previous snapshot IDs, guard bytes and their hash |
| `chunks`           | Raw SHA-256 ID, raw length, closed codec and encoded bytes                                                                       |
| `objects`          | Complete raw SHA-256 ID, raw length and exact expected chunk count                                                               |
| `object_chunks`    | Object, contiguous zero-based ordinal, chunk ID                                                                                  |
| `snapshots`        | Snapshot ID and root object ID                                                                                                   |
| `snapshot_objects` | Complete ordered role/object membership; no duplicate pair                                                                       |

Only three explicit reverse indexes support foreign-key deletion/reachability.
No views, triggers, virtual tables, unknown indexes/tables or cascades. Admission
compares the pinned creator's complete expected catalog, not a hash stored in the
input. Bounded structural and row checks precede graph/domain decoding. Full check
also runs SQLite integrity/foreign-key checks and verifies retained and unreachable
logical rows; it never repairs the file.

All IDs/hashes are exactly 32 raw bytes. Object identity is SHA-256 of exact complete
raw bytes; chunk identity is SHA-256 of its raw bytes. Snapshot identity hashes
`spiling:ptac:snapshot:v1\0`, root ID, `member_count:u32_le`, then each
`role:u16_le || object_id`, sorted by role and raw ID. Guard identity hashes exact
guard bytes independently. Hashes detect corruption, not malicious rewriting or
authenticity. Missing/extra/reordered/wrong-length mappings always refuse.

## Chunk profile and bounds

Chunks are fixed 262144 raw bytes except the final chunk; empty objects and holes
are forbidden. Codec 0 is RAW; codec 1 is one standard ZSTD frame. The writer uses
level 3 only when it saves at least 64 bytes. The decoder requires explicit matching
content size, no dictionary/reserved flags, window at most 256 KiB, no trailing,
skippable or concatenated frames, bounded output and both chunk/object digests.
The first verified encoding wins; reused content is verified, never overwritten.

| Resource                         | Bound                                                                                                                                            |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| Root / guard                     | 1 MiB / 4096 bytes                                                                                                                               |
| Object / mappings                | 16 MiB / 64                                                                                                                                      |
| Selected membership / raw graph  | 128 rows / 160 MiB                                                                                                                               |
| Database / rollback journal      | 1 GiB / 2 GiB                                                                                                                                    |
| Retained snapshots / memberships | 512 / 16384                                                                                                                                      |
| Objects / mappings / chunks      | 4096 / 262144 / 4096                                                                                                                             |
| Owned scratch                    | Eight entries / 192 MiB aggregate encoded spool bytes                                                                                            |
| SQLite                           | 1 MiB row limit, 64 KiB SQL, 32 columns, expression depth 64, 128 variables, zero attached databases                                             |
| Working policy                   | 8 MiB cache target, mmap off, memory temp, 250 ms cumulative lock waiting, 1000-instruction progress checks, bounded per-operation work/deadline |

Counts stop at limit+1; integer conversions and lengths are checked before copying
BLOBs or decompressing. Domain retained/history/prepared budgets can be smaller and
are charged separately; page cache is not a process heap cap. No global SQLite heap
limit affects Jobs. Stale/foreign scratch is not recursively removed or treated as
owned by filename; inspect it explicitly before retrying a refused save.

## Recovery and filesystem admission

Every project connection uses the pinned standard platform VFS through a private,
nondefault namespace guard. Admit main header, ancestry, identity, regular-file and
single-link policy before recovery; share in-process ownership before any raw
header descriptor is opened. Recheck through SQLite after consistent recovery.
The VFS delegates page I/O/locking/sync but bounds writes/truncation and permits
only the exact admitted main and its `-journal`. Read-only independently forbids
mutation. No URI interpretation, loadable extensions, on-disk temps, super-journal,
WAL or SHM. Journal type/link/size and super-journal footer are checked before pager
recovery; unknown paths fail rather than appear absent. SQLite's normal absent-WAL
probe succeeds only after both forbidden sidecars are absent and the journal is
admitted. It cannot disguise a super-journal reference as an absent WAL.

Use `DELETE`, `synchronous=EXTRA`, `auto_vacuum=NONE`, defensive mode,
`trusted_schema=OFF`, foreign keys, recursive triggers off and an allowlisted
authorizer. Existing open never creates/normalizes a schema or changes unsupported
journal mode. Actual read-only recovery-required/busy failures do not secretly
reopen writable. Never delete a crashed journal for cosmetic single-file copying.

The stable separate `.spil.lock` lease prevents a second lifetime writer. SQLite
still owns transaction locks. Metadata identity checks do not open/close raw main
descriptors; that can invalidate POSIX locks. New-target publication uses a closed,
flushed, independently reopened checker and atomic no-replace rename, never a
two-link intermediate database. Cleanup touches only recorded owned identities.

## Publication and maintenance

Generation/current/previous form a compare-and-swap base. Stage outside the write
transaction, verify reuse, insert complete graphs and guard, then acquire the
irreversible decision immediately before COMMIT. New files acquire it immediately
before namespace rename instead. Before-decision cancellation rolls back. Later
cancellation cannot pretend publication was undone.

Ordinary errors represent proven nonpublication; receipts distinguish confirmed,
published-unconfirmed and unknown outcomes. Failed commit cleanup permits at most
one bounded consistent SQLite reinspection. Unknown outcomes freeze writes and keep
the lease/journal until explicit reopen/inspection; no raw page inference or retry.
Post-rename sync/reopen/cleanup failures are post-publication diagnostics, never an
ordinary error claiming destination absence. Core/engine retain failed uncertainty
before ACK without publishing successful `ProjectSaved` data.

Explicit idle collection validates both retained graphs and deletes unreachable
memberships/snapshots, mappings/objects, then chunks in one transaction. Cancellation
rolls back; captures are not invalidated. Compaction copies only both verified roots,
guard, objects and exact encoded chunks into a fresh checked new file; no live
replacement or in-place VACUUM. It does not change project revision, dirty or undo.
Stats distinguish current raw, previous-only raw/encoded, reachable/unreachable
encoded, physical database/free pages/journal/owned scratch, exact counts and fixed
bounds. Free pages and unreachable payload are not additive reclaim promises.

`Projects.StorageMaintenance` is one typed authenticated Projects operation with a
captured project revision and closed `Stats`, `CollectUnreachable`, `CompactToNew`
actions. Only compact accepts a target `NativePath`; it must name a new file.
The sole worker and Jobs serialize these operations with project save/open and
manufacturing publication. Stats is read-only; mutation requires writable idle
ownership. `ProjectStorageMaintained { action, stats }` is inert checked output,
never a new scene, attachment, revision, saved baseline or undo entry. Uncertain
compaction fails without successful stats or attachment switching and retains its
target ownership in one bounded runtime slot until explicit reconciliation.

```sh
spiling-cli project inspect PROJECT.spil --storage-stats --engine ENGINE
spiling-cli project open PROJECT.spil --collect-unreachable --engine ENGINE
spiling-cli project open PROJECT.spil --compact-to NEW.spil --engine ENGINE
```

Run `cargo test -p spiling-core-ptac` without the engine/kernel. Its indexed test-only
VFS faults and subprocess kills exercise actual SQLite, separate from production
namespace restriction. See [authoring reproduction](../quality/authoring-evidence.md)
for Core/native journeys. Linux software tests do not qualify power loss, macOS/
Windows durability, network/cloud filesystems, packaging or physical manufacturing.

The bounded mutation-fuzz targets cover database/header/catalog/row bytes, object
mappings, snapshot preimages and RAW/ZSTD decoding. Default tests retain fixed
small corpora; longer deterministic campaigns use explicit seeds and at most 4096
additional cases per target, with the same storage/work/decompression limits:

```sh
SPILING_PTAC_FUZZ_CASES=1024 SPILING_PTAC_FUZZ_SEED=81239 \
  cargo test -p spiling-core-ptac --lib
```

These mutation campaigns are not an exhaustive fuzzer or electrical-power-loss
simulation. Retain seeds, minimized failures and campaign logs outside this runbook.
Run the ignored `tests::bounded_storage_workload_measurements` explicitly with
`cargo test -p spiling-core-ptac --lib tests::bounded_storage_workload_measurements -- --ignored --exact --nocapture`
for phase timings and exact space accounting of shared small members, a maximum
object and a large combined synthetic capture. It exercises container capacity,
not domain-valid maximum STEP/image/bundle admission or physical disk latency.
