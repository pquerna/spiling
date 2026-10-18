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

`storage::open_reusing` captures one committed manifest and its complete immutable asset graph. A writer retains the stable `writer.lock` inode for the Storage lifetime; a competing writer fails. Same-writer reopen reuses held storage. Read-only captures a committed snapshot without taking the writer lock.

Worker `open` then rebuilds or reuses native definitions from captured project bytes, independently checks provenance, and validates every stored surface target, including disabled layers, against the actual source face table. Original STEP/PNG paths are not dependencies after a successful save.

The old Core project, storage lock, history and native pins survive through staging and ACK. Failed/cancelled/stale open preserves them. Remaining source/proof, manufacturing and image capacities are supplied before incoming allocation; duplicate definitions do not make staged capture free.

Read-only forbids authoring, undo/redo, save/Save As and Compile publication. It permits native queries, Preflight, fresh bundle inspection/replay, image retrieval and export. Switching projects remains possible. Read-only is not permission to trust stored verification metadata.

A display refusal is independent of source admission: a definition can retain native queries with `DefinitionDisplay::Unavailable`. Reopen reconstructs display availability; meshes are neither durable project authority nor a substitute for native face identity.

## Save commit and failure paths

`Project::capture` supplies an immutable checkpoint. `storage::save` validates it, synchronizes immutable source/manufacturing/image assets, publishes the validated previous checkpoint, then atomically publishes current content. First save/Save As builds an owned sibling staging directory and uses exclusive publication: even an existing empty target must not be overwritten.

- Before publication, cancellation or IO failure leaves current content intact and does not attach the proposed destination. Complete unreferenced assets or owned staging remnants can remain; cleanup never deletes substituted or foreign entries.
- Worker `save` calls `CommitToken::begin_commit` at `SaveStage::BeforeManifestReplace`. Once replacement wins, cancellation cannot claim rollback. The runtime must deliver the actual receipt through ACK and `Project::mark_saved`.
- A final directory-sync failure yields published storage plus `SaveReceipt.durability_error`. Core attaches that storage, sets `save_uncertain=true` and stays dirty. An existing target retains its prior confirmed baseline; a newly attached unconfirmed target has no confirmed saved revision.
- The operation still fails with typed project IO. Its own checked `ProjectSaved` receipt travels in Status details through [native `terminal_error`](../../apps/engine/src/native.rs). Sticky uncertainty observed by a later Get is not evidence that a different failed Save As published its destination.
- Undo and same-session reopen cannot clear uncertainty. A fully confirmed save clears it. Clients reconcile authoritative state without silently retrying a failed save.
- Immutable assets are not garbage-collected while readers can hold old manifests. Storage/source/history/scratch exhaustion is a typed refusal, never permission to drop history pins early, truncate output or delete old assets.

## Recovery and fresh replay

Normal open never falls back from corrupt current content. Explicit recovery reads and independently validates `previous.json` and its assets. It restores previous semantic content with a fresh revision above recorded high-water and preserves both allocators. `recovered_previous=true` remains dirty until a confirmed save. Recovery is not autosave or replay of arbitrary unsaved edits.

Saved manufacturing bundles are exact immutable assets, but stored `verified` reports are descriptive. Worker `open` calls `inspect_asset` before project promotion. [Manufacturing `inspect_bundle` / `validate_inspection_bundle`](../../crates/manufacturing/src/lib.rs) require current producer/kernel identities and independently replay the actual program. A former producer cannot acquire current verification authority merely because its bundle JSON validates. Incompatible producers refuse; no compatibility reader or automatic recompilation repairs them.

Report-only inspection and optional actual-program motion share that inspection path. Export/replay after source removal uses captured project assets, not original paths or planner-preview geometry. See [manufacturing support](manufacturing-evidence.md) for process limits; successful authoring/import does not establish whole-part manufacturability or machine readiness.

## Minimal operator reproduction

Use built matching CLI/engine binaries from [development](development.md); substitute the configured target directory if overridden. Commands below create only an owned temporary source and a new project target:

```sh
CLI="$PWD/target/debug/spiling-cli"
ENGINE="$PWD/target/debug/spiling-engine"
scratch=$(mktemp -d)
cp fixtures/geometry/box-mm.step "$scratch/source.step"
"$CLI" project create "$scratch/project" --import "$scratch/source.step" --engine "$ENGINE"
"$CLI" project edit "$scratch/project" --pose 1 '{"translation_mm":[40,0,0],"rotation_xyzw":[0,0,0,1]}' --save --undo --redo --engine "$ENGINE"
rm -- "$scratch/source.step"
"$CLI" project inspect "$scratch/project" --section 0,0,4:0,0,1 --engine "$ENGINE"
```

Flags execute left-to-right; create/edit checkpoint final content, while inspect is read-only. Compare authoritative project/occurrence/provenance output across processes, not session handles. The edit's intermediate save must retain history; undo/redo after it must remain coherent. `project recover PROJECT --engine "$ENGINE"` explicitly inspects previous content; add ordered `--save` only when publishing that recovered state is intended.

[Core storage regressions](../../crates/core/src/storage/tests.rs), [Core transaction regressions](../../crates/core/src/tests.rs), [engine project integration](../../apps/engine/tests/project.rs) and [CLI project integration](../../apps/cli/tests/project.rs) own corruption, locks, source removal, recovery, uncertainty and process cleanup scenarios. `SPILING_PROJECT_FAULT=after_assets|before_manifest_replace|after_manifest_replace` terminates an actual engine at storage transitions; use only owned disposable projects. Process-crash behavior does not certify filesystem/device power loss.

## Support boundaries

No restart-persistent undo, automatic unsaved-edit recovery, asset GC, general CAD/topology editing or automatic operation resumption is promised. Network/cloud-synchronized filesystems and hostile external mutation are not certified. Platform storage primitives and their qualification limits are specified in the project contract. CLI/native bridge behavior alone does not establish packaged desktop, real-dialog, physical GPU or printer acceptance.
