<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Source-backed authoring reference

Core owns persistent project state; the engine composes it with disposable native geometry and display caches. CLI and desktop use the same services, not separate project writers. Current storage is project format6 with strict refusal of incompatible formats.

Canonical contracts: [project format, durability and recovery](../protocol/project.md), [control and operations](../protocol/control.md), [display](../protocol/display.md). This page maps their invariants to Rust; it is not a qualification log.

## Read the ownership chain

| Owner                                                      | Entry points and responsibility                                                                                                                                                                                                                            |
| ---------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [Core project](../../crates/core/src/lib.rs)               | `Project::prepare` / `commit`, `undo` / `redo`, `info`, `capture`, `mark_saved`, `retain_high_water`: sole transaction, revision, history and saved-state authority.                                                                                       |
| [Checkpoint storage](../../crates/core/src/storage/mod.rs) | `open_reusing`, `save`, `SaveReceipt`: validated immutable assets, held writer ownership, checkpoint publication and explicit recovery. [OS primitives](../../crates/core/src/storage/os.rs) own anchored IO, locks, synchronization and exclusive rename. |
| [Native worker](../../apps/engine/src/geometry/worker.rs)  | `open`, `validate_provenance`, `validate_surface_targets`, `save`, `inspect_asset`: reconstruct native sources, validate stored targets/bundles and return staged products or publication receipts.                                                        |
| [Native runtime](../../apps/engine/src/geometry/mod.rs)    | `Runtime::event` / `prepare`: revision checks, staged promotion, worker ACK, then Core/scene publication.                                                                                                                                                  |
| [CLI project workflow](../../apps/cli/src/project/mod.rs)  | `parse` and ordered operations use EngineClient; no manifest parsing or direct project storage.                                                                                                                                                            |

Durable operation publication and cancellation ownership are mapped in [engine operations](engine-operations.md). Start there when an operation is admitted but not yet published.

## Transactions, identity and undo

- A prepared edit binds both the originating project instance and persistent `ProjectRevision`. `commit` rejects a foreign/stale edit before replacing the snapshot; admission validates content and retained allocations before publication.
- Successful edits, undo and redo advance persistent revision. Occurrence and surface-layer allocation never rewind with history. `retain_high_water` preserves live counters on same-project reopen; restart can preserve only counters actually checkpointed.
- Dirty is a semantic content comparison with the confirmed saved snapshot, not revision equality. Undo-to-saved can be clean at a newer revision. `save_uncertain` and explicit recovery independently force dirty.
- Save retains session undo/redo; reopen/restart does not restore history. New edits clear redo. Camera, selection and playback are not project transactions.
- Snapshots share immutable source, manufacturing, intent, surface-document and image ownership. Current, history and incoming prepared allocations coexist under separate bounded budgets; replacement cannot free the old project early to admit the new one.
- Geometry/intent/surface edits invalidate the active manufacturing artifact atomically. Undo restores coherent inputs and artifact, not an independently retained preview. Occurrence/final-definition removal also removes its scoped surface layers and prunes unreferenced current images.
- Persistent `ProjectRevision` is distinct from session `SceneRevision`. Geometry publications advance scene identity; metadata/save/artifact-only changes do not remesh unchanged geometry. Native references still bind session, scene, occurrence, definition and canonical source face.

## Open, read-only and source independence

`storage::open_reusing` captures one PTAC saved root, complete immutable object graph and independent guard. A writer retains the stable separate `<name>.spil.lock` inode for the Storage lifetime; a competing writer fails. Same-writer reopen reuses held storage. Actual read-only SQLite captures a committed snapshot without taking the lifetime writer lease; it never secretly reopens writable for recovery. Every normal load consumes guard floors without manufacturing semantic dirtiness or a false saved revision. See [physical storage](../protocol/project-storage.md).

Worker `open` then rebuilds or reuses native definitions from captured project bytes, independently checks provenance, and validates every stored surface target, including disabled layers, against the actual source face table. Original STEP/PNG paths are not dependencies after a successful save.

The old Core project, storage lock, history and native pins survive through staging and ACK. Failed/cancelled/stale open preserves them. Remaining source/proof, manufacturing and image capacities are supplied before incoming allocation; duplicate definitions do not make staged capture free.

Read-only forbids authoring, undo/redo, save/Save As and Compile publication. It permits native queries, Preflight, fresh bundle inspection/replay, image retrieval and export. Switching projects remains possible. Read-only is not permission to trust stored verification metadata.

A display refusal is independent of source admission: a definition can retain native queries with `DefinitionDisplay::Unavailable`. Reopen reconstructs display availability; meshes are neither durable project authority nor a substitute for native face identity.

## Save commit and failure paths

`Project::capture` supplies an immutable checkpoint. `storage::save` validates strict domain closure; PTAC stages bounded content, verifies reuse and publishes current, previous and guard together under one SQLite transaction. First save/Save As builds and checks an owned private database, closes its handles, flushes it and uses atomic no-replace rename: no existing target is overwritten, and no two-link main-file intermediate is created.

- Before publication, cancellation or proven nonpublication leaves current content intact and does not attach the proposed destination. Owned staging remnants can remain; cleanup never recursively deletes substituted or foreign entries.
- Worker `save` calls `CommitToken::begin_commit` at `SaveStage::BeforeCommit`, immediately before existing-file COMMIT or new-target namespace rename, after cancellable validation/insertion. Later cancellation cannot claim rollback. The runtime delivers the actual receipt through ACK and `Project::mark_saved`.
- A final directory-sync failure yields published storage plus `SaveReceipt.durability_error`. Core attaches that storage, sets `save_uncertain=true` and stays dirty. An existing target retains its prior confirmed baseline; a newly attached unconfirmed target has no confirmed saved revision.
- The operation still fails with typed project IO. Its checked failed-only `ProjectSaveUncertain` receipt travels in Status details through [native `terminal_error`](../../apps/engine/src/native.rs), never successful `ProjectSaved`. Unknown COMMIT outcomes preserve the attempted lease/journal and freeze writes pending explicit reopen. Sticky uncertainty observed by a later Get is not evidence that a different failed Save As published its destination.
- Undo and same-session reopen cannot clear uncertainty. A fully confirmed save clears it. Clients reconcile authoritative state without silently retrying a failed save.
- Explicit idle collection verifies both retained roots and atomically removes only unreachable container rows; compact-to-new preserves those roots/guard/bytes without replacing the live file. Captured Core Arcs remain valid. Source/history/staging exhaustion is a typed refusal, never permission to drop history pins, truncate output or silently run maintenance.

## Recovery and fresh replay

Normal open never falls back from corrupt current content. Explicit recovery independently validates the previous PTAC root and intact allocator guard even when current payload is corrupt. It restores previous semantic content with a fresh revision above every recorded/live high-water and all allocator maxima. `recovered_previous=true` remains dirty until a confirmed save. Its bound recovery Save retains the validated previous source, never the damaged current. Recovery is not autosave, old-format conversion or replay of unsaved edits; corrupt guard/physical structure refuses.

Saved manufacturing bundles are exact immutable assets, but stored `verified` reports are descriptive. Worker `open` calls `inspect_asset` before project promotion. [Manufacturing `inspect_bundle` / `validate_inspection_bundle`](../../crates/manufacturing/src/lib.rs) require current producer/kernel identities and independently replay the actual program. A former producer cannot acquire current verification authority merely because its bundle JSON validates. Incompatible producers refuse; no compatibility reader or automatic recompilation repairs them.

Report-only inspection and optional actual-program motion share that inspection path. Export/replay after source removal uses captured project assets, not original paths or planner-preview geometry. See [manufacturing support](manufacturing-evidence.md) for process limits; successful authoring/import does not establish whole-part manufacturability or machine readiness.

## Minimal operator reproduction

Use built matching CLI/engine binaries from [development](development.md); substitute the configured target directory if overridden. Commands below create only an owned temporary source and a new project target:

```sh
CLI="$PWD/target/debug/spiling-cli"
ENGINE="$PWD/target/debug/spiling-engine"
scratch=$(mktemp -d)
cp fixtures/geometry/box-mm.step "$scratch/source.step"
"$CLI" project create "$scratch/project.spil" --import "$scratch/source.step" --engine "$ENGINE"
"$CLI" project edit "$scratch/project.spil" --pose 1 '{"translation_mm":[40,0,0],"rotation_xyzw":[0,0,0,1]}' --save --undo --redo --engine "$ENGINE"
rm -- "$scratch/source.step"
mv -- "$scratch/project.spil" "$scratch/moved.spil"
"$CLI" project inspect "$scratch/moved.spil" --section 0,0,4:0,0,1 --engine "$ENGINE"
```

Flags execute left-to-right; create/edit checkpoint final content, while inspect is read-only. Compare authoritative project/occurrence/provenance output across processes, not session handles. The edit's intermediate save must retain history; undo/redo after it must remain coherent. `project recover PROJECT --engine "$ENGINE"` explicitly inspects previous content; add ordered `--save` only when publishing that recovered state is intended.

[Container conformance](../../crates/core-ptac/src/adversarial_tests.rs), [Core storage regressions](../../crates/core/src/storage/tests.rs), [Core transaction regressions](../../crates/core/src/tests.rs), [engine project integration](../../apps/engine/tests/project.rs) and [CLI project integration](../../apps/cli/tests/project.rs) own corruption, locks, source removal, recovery, uncertainty and process cleanup. Run `cargo test -p spiling-core-ptac` independently of Core/kernel for fixed identity/catalog vectors, bounded mutations/model sequences, indexed real VFS write/sync/truncate/delete/lock faults and subprocess COMMIT kills. Engine fault names are `staged`, `rows_inserted`, `before_commit`, `committed`, `before_publish`, `published` (plus explicit postpublication error hooks); use only disposable projects. Process-crash/fault evidence does not certify power loss, macOS/Windows durability, network/cloud filesystems or physical manufacturing.

## Support boundaries

No restart-persistent undo, automatic unsaved-edit recovery, background GC, general CAD/topology editing or automatic operation resumption is promised. Explicit idle collection/compact-to-new are storage operations, not project edits. Network/cloud-synchronized filesystems and hostile external mutation are not certified. Platform storage primitives and their qualification limits are specified in the project contract. CLI/native bridge behavior alone does not establish packaged desktop, real-dialog, physical GPU or printer acceptance.
