<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Durable source-backed project format 2

The executable schema is `spiling-contracts::project`; state and storage authority is `spiling-core`. Engine caches are disposable. CLI, desktop and engine-client do not implement this format themselves. Protocol v4 project jobs operate on captured immutable snapshots rather than frontend recipes or external source paths.

## Directory and records

```text
<project-directory>/
  writer.lock                  stable, regular OS writer-lock inode
  manifest.json                current committed ProjectManifest
  previous.json                optional single RecoveryRecord
  sources/
    <source_sha256>.step        immutable exact imported source bytes
  manufacturing/
    <bundle_sha256>.json        immutable exact compact manufacturing bundle bytes
```

The initial successful save has no previous checkpoint. Root/assets must be regular, not symlinks/reparse points; asset directories must be real directories. Unknown entries are rejected except bounded internal scratch. No external source paths, session IDs, native objects, display artifacts or undo records are serialized. Manufacturing bundles are durable immutable derived assets, not persisted printer execution. `source_name` is a captured display basename, never a filesystem reference; its retained value participates in manufacturing provenance/fingerprints.

`manifest.json` is strict UTF-8 JSON, at most 1,048,576 bytes. Duplicate object fields, unknown fields, trailing JSON, invalid checked IDs, invalid poses and unsupported versions are errors. Its exact fields are:

- `format_version`: `2` (no compatibility reader).
- `project_id`: canonical lowercase, nonnil UUID string.
- `revision`: persistent nonnegative `u32` authoring revision.
- `units`: `"mm"`; `frame`: `"right_handed"`.
- `next_occurrence`: next free nonzero `u32`, strictly greater than every stored occurrence ID.
- `definitions`: at most 32 `StoredDefinition` records, each with `definition_id` and `provenance`.
- `occurrences`: at most 256 `OccurrenceRecord` records, each with nonzero `occurrence_id`, referenced `definition_id`, and `pose` (`translation_mm[3]`, `rotation_xyzw[4]`). Translation is finite and the quaternion is a unit quaternion under the shared pose validator.
- `manufacturing_intent`: explicit `null` or validated `ManufacturingIntent` containing every printer/recipe field.
- `manufacturing_artifact`: explicit `null` or validated `ManufacturingArtifactRecord` containing exact-byte hash, semantic input hash, byte count and software-only summary.

Every definition is referenced by an occurrence; definition and occurrence identities are unique. The writer emits definitions and occurrences in ID order. Readers validate identities/references rather than assuming array order. Provenance contains `source_hash` (lowercase 64-character SHA-256), `source_name`, original `source_unit` (`millimetre`, `metre`, or `inch`) and optional finite nonnegative `uncertainty_mm`. Geometry's shared label/provenance validator is authoritative. Native admission rebuilds geometry from source bytes and independently checks native provenance; a syntactically valid storage snapshot is not proof of native geometry support.

An asset contains 1..16 MiB of exact original imported bytes, not normalized or regenerated STEP. Its filename is the SHA-256 of those bytes. `definition_id` is SHA-256 of the byte sequence `spiling:definition:step-planar-cylindrical-v1\0` followed by the 32 raw source-hash bytes. Hash/identity/reference/provenance mismatches fail before publication. Source bytes enter one `Arc<Vec<u8>>` without copying the captured Vec allocation; every snapshot/history entry shares the same canonical source asset. Reimporting identical bytes from another basename keeps the first retained source label; differing source units/uncertainty are conflicting native provenance, not a new definition.

Persisted immutable assets are limited to 128 files and 256 MiB, including complete unreferenced assets left by earlier saves. There is **no asset garbage collection**: a concurrent reader may still hold a manifest referring to an old immutable asset. Exhaustion is an explicit resource error, not permission to delete old assets. Temporary source scratch has a separate finite limit of eight entries and 16 MiB total; root entries are limited to 32 and each temporary manifest to 1 MiB. A save refuses to exceed scratch capacity, including crash leftovers. These are storage bounds, not a promise that arbitrary externally modified directories are valid. First-save crash staging directories live beside the destination and are not published projects.

Manufacturing records require intent and a semantic input hash matching the current project ID, sorted definitions/provenance, sorted rigid occurrences and explicit intent. Bundles are capped at 16 MiB serialized bytes and allocated Vec capacity; the separate 64 MiB retained/staged/history budget charges allocated capacity, not just length, and deduplicates shared assets. Disk/wire record byte counts remain exact serialized lengths. Load hashes exact bytes, validates bounded strict bundle schema and provenance/reference/summary agreement; engine independently replays the stored program before reporting verified. Persisted `verified` is not trusted. See [manufacturing.md](manufacturing.md) for exact fields, fingerprint and exclusions.

## Identity, transactions and saved state

Core snapshots contain active sources, occurrences, manufacturing intent and optional shared manufacturing asset. Prepared edits validate before mutating anything; commit checks originating project instance and captured revision. Intent changes and artifact publication are ordinary transactions; geometry/intent mutation invalidates active artifacts atomically, undo restores coherent inputs/artifact. Every edit/undo/redo advances persistent revision; allocator never rewinds. At most 64 transactions retain 32 definitions/64 MiB source bytes and a separate 64 MiB manufacturing budget. Assets are shared allocations, not copied per snapshot. Save preserves history and revision.

Dirty compares source records, occurrences, intent and artifact record with confirmed saved content, not revision/allocator numbers. Undoing to saved content may be clean at a later revision. Saved comparisons do not pin extra asset allocations. A new empty project is clean but unattached; its first save needs an explicit new target. Read-only projects reject edit/compile publication/undo/redo/save (including Save As), but permit independent inspection/replay.

Persistent revisions are distinct from the engine's ephemeral `SceneRevision`. The latter advances on every scene publication, including open/new replacements, so stale session references cannot revive. Same-project live reopen preserves the old in-process persistent revision/allocator high-water, including discarded unsaved allocations; it does not replay unsaved edits. Ordinary process restart guarantees saved counters only, not every unsaved allocation in a lost process.

## Writer ownership and reader snapshots

A writer holds an exclusive OS `File` lock on `writer.lock` for the entire retained `Storage` lifetime. It never unlinks/recreates that lock. Concurrent writers fail with `ProjectLocked`; process exit releases OS ownership. The storage directory, source directory and lock inode are checked for substitution. A same-writer staged reopen uses `storage::open_reusing` and retains the existing lock until successful engine publication. Failure/cancellation cannot destroy the old live project or lock.

A read-only open never acquires the writer lock. It captures one complete manifest then loads every immutable source and manufacturing asset referenced by it. Writer replacements cannot mix newer records with older assets. The loaded project owns its bytes without reopening external sources. No eager deletion may invalidate concurrently captured snapshots.

The engine passes the remaining old/current/history byte capacity to `storage::open_reusing` before source capture. The loader bounds each incoming file by the remaining caller budget before allocating its buffer. Old and staged captured allocations coexist through native ACK, even for identical definition IDs; their combined envelope is 64 MiB. Insufficient capacity fails with `ResourceLimit` without first dropping the old state or temporarily capturing a second full budget.

Manufacturing open likewise receives the remaining retained/staged manufacturing budget before allocation, so the old complete project and incoming bundles coexist within 64 MiB through ACK, not old64+incoming64 transient overshoot.

## Save publication, cancellation and errors

Ordinary save to attached storage:

1. Validate captured manifest/snapshot and previous committed content/references. Write missing immutable source and manufacturing assets to exclusively created temporary files; synchronize bytes; publish final hash filenames without replacing existing assets. Validate reused assets against exact captured bytes. Synchronize asset directories before any manifest publication.
2. Emit `AssetsSynced`. Write and synchronize a temporary current manifest.
3. If a previous checkpoint already exists, validate its schema, project identity and source references rather than silently repairing/replacing corrupt or foreign checkpoint data. Write and synchronize the one recovery record; atomically replace `previous.json`; synchronize the project directory. This checkpoint publication precedes current replacement. Saving a recovered project uses valid current content as its previous checkpoint when available; only explicit recovery intent permits using the independently validated recovery record in place of corrupt current content.
4. Emit `BeforeManifestReplace`. Recheck cancellation **after** the observer returns and before starting replacement. Cancellation wins here and earlier; current manifest remains complete and unchanged. A failed save can leave complete unreferenced assets and can refresh the recovery checkpoint, but never publishes partial source references.
5. Make the commit decision and atomically replace `manifest.json`. Ignore subsequent cancellation. Emit `AfterManifestReplace`, then synchronize the project directory and return a truthful receipt.

A first save or different-target save must not replace any existing destination, even an empty directory. It builds an exclusively named sibling `.spiling-stage-<uuid>` directory with its own held writer lock and complete synchronized assets/manifest. At the same cancellation boundary it verifies ownership, synchronizes the staged directory, and publishes the entire directory using an exclusive/no-replace OS rename. For first save, `AfterManifestReplace` means the complete directory has reached the final destination; it is not emitted for the earlier private manifest rename. Parent-directory synchronization completes durability. Before-publication failure leaves the destination absent and safely retryable. Rollback only unlinks known owned file identities and removes matching empty directory identities; it never recursively deletes substituted/foreign paths. Process exit may leave an unpublished sibling stage; no automatic recursive orphan cleanup is performed.

Errors before current publication return `Err(ProjectError)` and do not attach storage/change live content. Once publication succeeds, a failure of its final directory synchronization cannot be described as cancellation or as though nothing was saved. `SaveReceipt.durability_error` carries typed `Io` alongside the actual published storage/manifest. The engine must attach the receipt before reporting the failed job. Core keeps the prior confirmed saved baseline/revision for the same attached target, sets `save_uncertain=true`, and forces `dirty=true`; a newly attached unconfirmed target has no confirmed saved revision. Undo does not clear uncertainty, and a same-session reopen does not erase it. A later fully successful save clears uncertainty. No successful `ProjectSaved` result may be emitted for an unconfirmed durability receipt. Inspect status and the explicit error before deciding whether to retry.

`SaveStage` observers are real transition hooks, not simulated crash success. Engine `SPILING_PROJECT_FAULT=after_assets|before_manifest_replace|after_manifest_replace` exits the actual child at the corresponding hook. A process crash before publication leaves the prior complete current (or no first-save target); a crash after publication leaves a complete new current with synchronized sources. Power-loss guarantees depend on the filesystem/device successfully honoring synchronization, and are **not** proved by process-crash tests.

## Explicit recovery and high-water checkpoint

`previous.json` is strict JSON, at most 1 MiB, containing exactly:

```text
RecoveryRecord {
  manifest: ProjectManifest,             // previous committed semantic content
  revision_high_water: ProjectRevision, // at least candidate/current revision
  next_occurrence_high_water: u32        // at least candidate/current next counter
}
```

Both high-water fields must be at least their embedded manifest values. The record is published before replacement, so it includes IDs/revision from the candidate that may subsequently become current. Explicit recovery validates this record and all **its** sources independently, without silently trusting or falling back from current JSON. It restores the embedded semantic snapshot, sets the allocator to the maximum recorded next counter, and gives it a fresh revision strictly above the recorded revision high-water. Revision exhaustion is an explicit resource error. `saved_revision` reports the embedded checkpoint's confirmed revision; `recovered_previous=true` and `dirty=true` remain until a fully confirmed save. This prevents recovered authoring from reusing IDs allocated by a newer committed snapshot. Recovery is inspection/authoring of a complete checkpoint, never replay of unsaved transactions or printer operations. Ordinary open never falls back; invalid/missing previous checkpoints also fail explicitly.

## Platform and evidence boundaries

- Linux: anchored `openat`/`O_NOFOLLOW` regular-file IO, stable OS file locking, file/directory `fsync`, atomic `renameat` replacement, and `renameat2(RENAME_NOREPLACE)` initial publication. Unsupported exclusive rename/filesystem synchronization returns typed error; no check-then-overwrite fallback.
- macOS: the corresponding anchored IO, atomic replacement, `renameatx_np(RENAME_EXCL)` publication, directory `fsync`, and file `F_FULLFSYNC` after `sync_all`. These primitives are implemented but unverified on macOS in this wave.
- Windows: checked non-reparse file/directory handles, OS file locking, `MoveFileExW` with write-through and replacement only where required, exclusive initial move, and explicit directory flushing. Directory flush/access and atomic/durability behavior remain unverified on Windows in this wave. A filesystem/permission configuration refusing directory flushing yields typed `Io` (before publication, or an uncertain receipt after publication), **not** silently weakened durable success.

No cross-platform power-loss certification is claimed. Network/cloud-synchronized filesystems and hostile concurrent mutation outside the cooperative writer protocol are not certified. `cargo test -p spiling-core` exercises deterministic public behavior including revision/dirty/history/source pins, lock/reuse/read-only snapshots, corruption, cancellation boundaries, recovery high-water, foreign-target ownership and postpublication durability uncertainty. Actual-engine crash/concurrency/native reconstruction evidence and platform evidence are separate acceptance surfaces owned by engine/integration.
