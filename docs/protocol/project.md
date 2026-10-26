<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Durable source-backed project format

The executable storage schema is `spiling-contracts::project`; state and storage authority is `spiling-core`. Engine caches are disposable. CLI, desktop and engine-client do not implement this format themselves. Generated protobuf Projects services operate on captured immutable snapshots rather than frontend recipes or external source paths. Long opens/saves are standard Google Operations scoped to the native session; RPC and operation metadata are not persisted project manifest schemas.

Format evolution and the initial release compatibility boundary follow the [canonical pre-release format policy](../../AGENTS.md#pre-release-format-evolution).

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
  images/
    <image_sha256>.pgm          immutable canonical Gray8 height images
  palette-originals/
    <original_sha256>.json      immutable exact imported palette JSON
  palette-canonical/
    <canonical_sha256>.json     immutable canonical palette JSON
```

The initial successful save has no previous checkpoint. Root/assets must be regular, not symlinks/reparse points; asset directories must be real directories. Unknown entries are rejected except bounded internal scratch. No external source paths, session IDs, native objects, display artifacts or undo records are serialized. Manufacturing bundles are durable immutable derived assets, not persisted printer execution. `source_name` is a captured display basename, never a filesystem reference; its retained value participates in manufacturing provenance/fingerprints.

`manifest.json` is strict UTF-8 JSON, at most 1,048,576 bytes. Duplicate object fields, unknown fields, trailing JSON, invalid checked IDs, invalid poses and unsupported versions are errors. Its exact fields are:

- `format_version`: `10` (strict cutover; no compatibility reader).
- `project_id`: canonical lowercase, nonnil UUID string.
- `revision`: persistent nonnegative `u32` authoring revision.
- `units`: `"mm"`; `frame`: `"right_handed"`.
- `next_occurrence`: next free nonzero `u32`, strictly greater than every stored occurrence ID.
- `next_surface_layer`: next free nonzero `u32`, strictly greater than every stored surface-layer ID; allocation never wraps or rewinds.
- `surface_overlays`: required inline `SurfaceOverlays` schema3 document: ordered `layers` and hash-sorted unique `images`, at most64 layers and32KiB serialized/allocated metadata bytes. The image table contains exactly the referenced assets, including disabled layers; pixels never occur inline in ProjectInfo.
- `next_process_layer`, `next_process_operation`: next free nonzero monotonic `u32` identities; undo does not rewind either allocator.
- `process_document`: required strict typed schema3 process document, at most64 layers and256 total operations. Core allocates layer/operation IDs. Whole-owned and finite source-box selectors use checked definition/occurrence scope, independent of face finish.
- `appearance_study`: required strict `AppearanceStudy` with nullable palette/target/accepted report/cached report descriptor/selection and explicit search limits. The accepted report is inline; ProjectInfo exposes only its descriptor. No missing-field compatibility default is supplied.
- `definitions`: at most 32 `StoredDefinition` records, each with `definition_id` and `provenance`.
- `occurrences`: at most 256 `OccurrenceRecord` records, each with nonzero `occurrence_id`, referenced `definition_id`, and `pose` (`translation_mm[3]`, `rotation_xyzw[4]`). Translation is finite and the quaternion is a unit quaternion under the shared pose validator.
- `manufacturing_intent`: explicit `null` or validated `ManufacturingIntent` containing every printer/recipe field.
- `manufacturing_artifact`: explicit `null` or validated `ManufacturingArtifactRecord` containing exact-byte hash, semantic input hash, byte count and software-only summary.

Every definition is referenced by an occurrence; definition and occurrence identities are unique. The writer emits definitions and occurrences in ID order. Readers validate identities/references rather than assuming array order. Provenance contains `source_hash` (lowercase 64-character SHA-256), `source_name`, original `source_unit` (`millimetre`, `metre`, or `inch`) and optional finite nonnegative `uncertainty_mm`. Geometry's shared label/provenance validator is authoritative. Native admission rebuilds geometry from source bytes and independently checks native provenance; a syntactically valid storage snapshot is not proof of native geometry support.

Each surface layer has checked nonzero `layer_id`, explicit `enabled`, a canonical `face_id` STEP string, tagged `scope` and tagged `finish`. Scope is `{"kind":"definition","definition_id":...}` or `{"kind":"occurrence","occurrence_id":...,"expected_definition_id":...}` and must match stored identities. Finish is Smooth, VerticalRibs, CylindricalRibs or `{"kind":"image_relief","image_hash":...,"parameters":...}`. The executable contracts own parameter validation; native/process eligibility remains a separate manufacturing gate. Image mapping and error bounds are specified in [ADR003](../adr/003-spatial-image-relief.md). IDs are unique and below the allocator. Disabled layers remain authored content and retain their images. Enabled definition layers apply in stored order, then enabled occurrence layers in stored order; the last matching whole-face fill wins. Smooth explicitly overrides lower finishes; removal/disabling reveals lower fills. A syntactically canonical face string is insufficient: engine open validates every stored target against the exact rebuilt/reused source-face table before publication.

`ProcessDocument` resolves SparseFraction independently from surface finishes. Selectors are `{"kind":"whole_owned_volume"}` and `{"kind":"source_box_mm","min":[x,y,z],"max":[x,y,z]}`; every physical-mm bound is finite and strictly ordered. Values are `{"kind":"sparse_fraction","fraction":f64}` with `0 < fraction <= 1`. Operations require nullable tagged `interface`: `{"kind":"separator_wall"}` or `{"kind":"horizontal_diaphragm","thickness_mm":positive_f64}` explicitly permits permanent construction on a Box Replace only, never Erase. Plate thickness is centered on the selected source-Z boundary, not a layer count; native/pose/material/support admission remains a separate Compile obligation. Enabled definition layers compose before occurrence layers. Each layer begins transparent; Replace writes its selected field and Erase clears only that layer there. Reset removes ALL exact-scope contributions, including disabled/erased ones. `ProjectInfo.resolved_process` describes the authoritative whole-owned BASE, definition inheritance and recipe values, excluding boxes; it is not a uniform effective value for spatial output. Compiled region/interface/diaphragm/resolution records describe realized spatial output.

`SurfaceImageRecord` contains canonical `image_hash`, `width`, `height`, `byte_count`, original `source_hash` and captured display `source_name`. Dimensions are2..256, at most65,536 pixels; canonical bytes are exactly ASCII `P5\n{width} {height}\n255\n` followed by top-first row-major Gray8 pixels, with no trailing bytes. SHA-256 covers the complete canonical header and pixels, not only the raster. Each asset is at most65,551 bytes. Up to16 current images and1MiB current canonical bytes are admitted; unique current/history/live-prepared images share a separate4MiB byte budget. Core owns one shared canonical allocation per retained content hash; equivalent reimports retain the first admitted record and allocation without rewriting history. Removing the final current reference prunes the current table, not history or durable files.

Import accepts bounded PNG or canonical PGM source bytes, at most1MiB. PNG decoding is engine-owned: palette/transparency expand,16-bit channels use their high byte, RGB luminance is `(299R+587G+114B+500)/1000`, then alpha is applied with `(luma*alpha+127)/255`; no gamma/ICC correction. APNG and oversized/unsupported images fail explicitly. Neither the renderer nor Core normalizes images. Original PNG files are not project dependencies after admission; `images/<image_hash>.pgm` is authoritative.

An asset contains 1..16 MiB of exact original imported bytes, not normalized or regenerated STEP. Its filename is the SHA-256 of those bytes. `definition_id` is SHA-256 of the byte sequence `spiling:definition:step-native-trimmed-v2\0` followed by the 32 raw source-hash bytes. Hash/identity/reference/provenance mismatches fail before publication. Source bytes enter one `Arc<Vec<u8>>` without copying the captured Vec allocation; every snapshot/history entry shares the same canonical source asset. Reimporting identical bytes from another basename keeps the first retained source label; differing source units/uncertainty are conflicting native provenance, not a new definition.

Each immutable asset directory is limited to128 files and256MiB, including complete unreferenced assets left by earlier saves; image files additionally remain at most65,551 bytes each. There is **no asset garbage collection**: a concurrent reader may still hold a manifest referring to an old immutable asset. Exhaustion is an explicit resource error, not permission to delete old assets. Scratch permits at most eight entries, with aggregate bytes capped at16MiB for source/manufacturing directories and65,551 bytes for the image directory. Root entries are limited to32 and each temporary manifest to1MiB. A save refuses to exceed scratch capacity, including crash leftovers. These are storage bounds, not certification of arbitrary externally modified directories. First-save crash staging directories live beside the destination and are not published projects.

Manufacturing records bind project identity, source/occurrence records, explicit intent and complete ordered finish/process documents, including disabled/shadowed operations. Manufacturing schema11 captures these inputs, required deposition limits/sparse/top-skin policies, typed permanent construction and bounded canonical image content. Format10 refuses older manifests/checkpoints without missing-field defaults. Bundles remain capped at16MiB under Core's separate64MiB retained/staged/history allowance. Load checks exact bytes/hash, strict schema, full input/provenance/content agreement and summary. Engine independently replays stored program bytes; persisted `verified` is descriptive. See [manufacturing.md](manufacturing.md).

## Identity, transactions and saved state

Core snapshots share active sources, occurrences, manufacturing intent, `Arc<SurfaceOverlays>`, `Arc<ProcessDocument>`, canonical image assets and optional manufacturing asset. Prepared edits validate before mutation and commit checks originating project/revision. Process, finish, geometry and intent changes invalidate the complete active artifact atomically; undo restores coherent input/artifact without allocator rewind. Occurrence deletion removes occurrence-scoped finish/process layers; final-definition deletion also removes definition layers. Metadata transactions preserve scene/FaceRef epochs while advancing persistent revision. History is at most64 transactions with separate source/manufacturing/surface/image/process allocation budgets; process documents have64KiB current and16MiB retained caps. Unchanged documents/assets share allocations. Save preserves history/revision.

Dirty compares source records, occurrences, intent, complete finish/process documents and artifact record with saved content, not revision/allocator numbers. Undoing to saved content may be clean at a later revision. A new project is clean but unattached; its first save requires a new target. Read-only rejects mutation, Compile publication, history and Save, while allowing effective-intent inspection and independent stored-program replay/export.

Persistent revisions are distinct from the engine's ephemeral `SceneRevision`. The latter advances on every scene publication, including open/new replacements, so stale session references cannot revive. Same-project live reopen preserves the old in-process persistent revision/allocator high-water, including discarded unsaved allocations; it does not replay unsaved edits. Ordinary process restart guarantees saved counters only, not every unsaved allocation in a lost process.

`Projects.Execute` surface commands are typed protobuf requests: AddSurfaceFinish carries session parent, persistent project base revision, live FaceRef, explicit Definition/Occurrence scope and checked finish; an image finish must reference an already admitted current image. ImportSurfaceImageFinish additionally carries the selected NativePath and mapping parameters; image+layer admission is atomic. Stored images can be remapped/reused with AddSurfaceFinish without the original file or another raster transfer. RemoveSurfaceFinish carries revision/layer ID; SetSurfaceFinishEnabled adds enabled. No caller-supplied IDs or JSON tunnels. Engine rejects read-only/stale/session/scene/source-reference failures before converting to persistent Core targets. Unknown layers/scopes are invalid-project errors; bounded-document/allocator exhaustion is ResourceLimit. Every authoritative ProjectInfo receipt contains the complete typed surface metadata document. Native carrier/process/pose unsupported compilation is an explicit failure, never untextured success.

`Projects.Execute.EditProcess` carries session, checked persistent base revision and typed `ProcessEdit`. AddLayer/AddOperation allocate identities atomically; update/remove/reorder/enable/reset use checked IDs/source bindings. No client insertion IDs or JSON RPC tunnel exists. CLI supports whole-owned set, `--process-set-box OCC SCOPE MIN_X MIN_Y MIN_Z MAX_X MAX_Y MAX_Z FRACTION none|separator-wall|horizontal-diaphragm:MM`, reset, typed edit and read-only resolve. Resolve labels `base_whole_owned_resolution` separately from `finite_box_intent`; compiled output is spatial authority. Workbench bounds, explicit permanent-construction kind/thickness/approval and Apply share the same services/history/read-only guards.

`SetSparseFraction` updates only the exact-scope whole-owned winner; it never edits a box winner. `SetBoxSparseFraction` reuses the unique intended active box operation across boundary/fraction changes, preserving IDs/order; ambiguous targets diagnose. Ordinary adjustments do not consume a new layer each time. Both edits work before recipe selection without persisting a fallback intent. Explicit AddLayer/AddOperation retain their ordered meaning.

GetSurfaceImage is a revision-bound read-only native operation, valid in read-only projects. It captures the canonical asset Arc and reserves at most65,551 ledger bytes. After native promotion ACK the engine publishes actual immutable PGM bytes through Jobs/ByteStream and returns `SurfaceImagePrepared` with project ID, captured revision, authoritative record and bound `ArtifactView`. Media type is `image/x-portable-graymap`; descriptor hash/size/resource name must bind the record. A guessed descriptor is not a byte resource. Preview cannot advance the project revision or publish a scene. Clients verify bounded bytes/hash/header/dimensions before constructing borrowed raster views.

## Appearance palette studies

`spiling-contracts::appearance` owns the inert records, `spiling-appearance` owns scalar evaluation, and Core owns study transactions and immutable assets. The library depends only on contracts; an empty project needs no geometry import, Compile, kernel query, manufacturing intent, UI or GPU to compare colors.

Palette import captures bounded regular-file JSON (16MiB, 4096 rows), preserving exact original bytes and a separately hashed canonical document. Stable keys bind material/evidence IDs; evidence hashes exclude display labels. Core retains a shared `PreparedPalette`: sorted validated rows, canonical hash and per-row evidence hashes. Comparisons borrow this proof rather than reparsing, serializing, hashing or sorting unchanged palettes. Original/canonical buffers, parsed records, identity caches, accepted reports and retained history share a separate64MiB allowance. Open reserves the remaining allowance before capture/decode, parses original JSON once and verifies canonical correspondence. Palette assets use their own directories, not image retention or GC.

The current real example is the six-row MIT OpenPrintTag Prusament subset in `fixtures/appearance/`, pinned to commit `2451ae7adc115cfc40cf1892bddbb0b4a2f1b1a3` with original upstream license and exact conversion provenance. Its RGB values are **catalog GUI estimates only**: not owned spools, measured printed colors, optical properties or inferred translucency. Vendor Lab claims without acquisition conditions are not promoted to measurements.

Colors declare encoded sRGB, XYZ Y=1, XYZ Y=100 or Lab under the fixed D65 reference white/2-degree convention. The matching CSS Color4 rational sRGB matrix uses D65 xy=(0.3127,0.3290), Y=1. Lab and XYZ admit finite out-of-display-gamut values without clipping; scalar numerical overflow is an explicit error. CIEDE2000 uses kL=kC=kH=1. A match requires both requested ΔE00 and optional absolute lightness tolerances. D50/10-degree or otherwise incompatible readings are not silently adapted.

Measured rows are supplied **declared readings**, identified by exact material/evidence/lot and evidence hashes. Their observation, instrument/calibration, aperture, geometry/specular/orientation, backing, thickness, cohort and process/construction references must be declared and match the requested lookup context; absent references remain unassessed. Lot belongs to each reading, never the target: different candidate lots do not filter one another. Matching strings do not certify lot provenance, printed coupons, calibration quality, optical validity or any manufacturing process. Unknown evidence is never assigned a fabricated zero error.

Enumeration visits stable key order and charges every row, including explicitly filtered or unassessed rows. `visits.len()` is the visited ordinal/count; typed assessments partition filtered, assessed and unknown/incompatible rows. Complete traversal is distinct from its outcome: `assessed_match` establishes at least one assessed tolerance match; `assessed_mismatch` requires every relevant row assessed with no match; `no_assessed_match` also covers assessed mismatches alongside relevant unknown/incompatible evidence. Evidence explicitly excluded by the requested scope is filtered, not silently treated as relevant unknown. Alternatives rank nearest **assessed** candidates by ΔE00 then key and retain their filament names. `alternatives_truncated` limits display only, not search completeness or global optimality.

`work_limit` and `cancelled` are incomplete best-found reports, even when a visited row matched. Their operation can deliver checked transient evidence but they cannot replace the durable report/selection. Typed report JSON is at most1MiB; its compact visit keys are `k`=candidate key, `e`=evidence hash, `a`=assessment. Assessment tags are `a`=assessed, `f`=explicitly filtered, `i`=incompatible conditions and `u`=unknown; assessed fields `e`=catalog(`c`)/declared measured(`m`), `d`=ΔE00, `l`=absolute lightness difference, `m`=within both tolerances. Generated Rust/TypeScript own the executable shape.

Import and target edits clear the active report/selection; explicit acceptance binds the latest complete comparison's exact current project/revision/hash and optional retained candidate key. Undo/redo restores coherent study state. These study-only edits preserve unrelated manufactured artifact bytes/input hashes and native SceneRevision. Core counts the **whole** candidate manifest and recovery envelope before mutation, so a report fitting its artifact cap cannot silently create an unsaveable study. Accepted report descriptors are cached, not re-encoded for ProjectInfo.

`Projects.ImportAppearancePalette`, `CompareAppearance` and `GetAppearanceReport` are standard authenticated native Operations through the existing dispatcher, worker, promotion ACK, Jobs and ByteStream. `Projects.Execute` carries typed target/accept requests; no alternate transport or JSON RPC tunnel exists. Compare/Get are read-only-capable. Returned `AppearanceCompared` binds captured project/revision, report descriptor and `application/x-spiling-appearance-report+json` artifact; engine-client checks exact bytes/hash/size/producer/input identity. Stale queued work cannot publish, and historical operation evidence cannot authorize current acceptance.

The CLI is the current consumed workflow; no appearance workbench form is implied:

```sh
work="$(mktemp -d)"
cp fixtures/appearance/palette.json "$work/allowed-palette.json"
target/debug/spiling-cli --engine "$PWD/target/debug/spiling-engine" project create "$work/study" \
  --appearance-palette "$work/allowed-palette.json" \
  --appearance-target fixtures/appearance/target-orange.json --appearance-compare \
  --appearance-accept 261ae7e7-20d9-5969-9ed5-dd82eea29bcf/catalog-rgb \
  --save --appearance-target fixtures/appearance/target-red.json --undo --redo --undo --save
rm "$work/allowed-palette.json"  # This newly created owned input only.
target/debug/spiling-cli --engine "$PWD/target/debug/spiling-engine" project inspect "$work/study" \
  --appearance-report "$work/reopened-report.json" --appearance-compare
```

`--appearance-accept-report` explicitly accepts a complete report without selecting a candidate; it is not a reserved candidate-key sentinel. `--appearance-limits FILE` supplies strict `max_visited`/`max_alternatives` JSON after a target. Exports use the shared exclusive output transaction and commit only after observed child reap. Inspect opens fresh read-only and uses captured assets after source deletion. Project10/manufacturing11/process3/surface3/SPLR5 remain unchanged; current producer freshness uses planner22/emitter7/verifier9.

## Writer ownership and reader snapshots

A writer holds an exclusive OS `File` lock on `writer.lock` for the entire retained `Storage` lifetime. It never unlinks/recreates that lock. Concurrent writers fail with `ProjectLocked`; process exit releases OS ownership. The storage directory, source directory and lock inode are checked for substitution. A same-writer staged reopen uses `storage::open_reusing` and retains the existing lock until successful engine publication. Failure/cancellation cannot destroy the old live project or lock.

A read-only open never acquires the writer lock. It captures one complete manifest then loads every referenced immutable source, canonical image (including disabled layers), manufacturing asset and original/canonical palette pair. Writer replacements cannot mix newer records with older assets. The loaded project owns its bytes without reopening external source/image/palette files. No eager deletion may invalidate concurrently captured snapshots.

The engine passes the remaining old/current/history byte capacity to `storage::open_reusing` before source capture. The loader bounds each incoming file by the remaining caller budget before allocating its buffer. Old and staged captured allocations coexist through native ACK, even for identical definition IDs; their combined envelope is 64 MiB. Insufficient capacity fails with `ResourceLimit` without first dropping the old state or temporarily capturing a second full budget.

Manufacturing open likewise receives the remaining retained/staged manufacturing budget before allocation, so the old complete project and incoming bundles coexist within 64 MiB through ACK, not old64+incoming64 transient overshoot.

Image open likewise receives the remaining4MiB retained/staged image budget before capture, including live prepared candidates. Hash/header/dimensions/record mismatches, missing or substituted image files fail before publication. Assets are written and synchronized before atomic manifest replacement, and current/previous recovery candidates each validate their complete image graph.

## Save publication, cancellation and errors

Ordinary save to attached storage:

1. Validate captured manifest/snapshot and previous committed content/references. Write missing immutable source and manufacturing assets to exclusively created temporary files; synchronize bytes; publish final hash filenames without replacing existing assets. Validate reused assets against exact captured bytes. Synchronize asset directories before any manifest publication.
2. Emit `AssetsSynced`. Write and synchronize a temporary current manifest.
3. If a previous checkpoint already exists, validate its schema, project identity and source references rather than silently repairing/replacing corrupt or foreign checkpoint data. Write and synchronize the one recovery record; atomically replace `previous.json`; synchronize the project directory. This checkpoint publication precedes current replacement. Saving a recovered project uses valid current content as its previous checkpoint when available; only explicit recovery intent permits using the independently validated recovery record in place of corrupt current content.
4. Emit `BeforeManifestReplace`. Recheck cancellation **after** the observer returns and before starting replacement. Cancellation wins here and earlier; current manifest remains complete and unchanged. A failed save can leave complete unreferenced assets and can refresh the recovery checkpoint, but never publishes partial source references.
5. Make the commit decision and atomically replace `manifest.json`. Ignore subsequent cancellation. Emit `AfterManifestReplace`, then synchronize the project directory and return a truthful receipt.

A first save or different-target save must not replace any existing destination, even an empty directory. It builds an exclusively named sibling `.spiling-stage-<uuid>` directory with its own held writer lock and complete synchronized assets/manifest. At the same cancellation boundary it verifies ownership, synchronizes the staged directory, and publishes the entire directory using an exclusive/no-replace OS rename. For first save, `AfterManifestReplace` means the complete directory has reached the final destination; it is not emitted for the earlier private manifest rename. Parent-directory synchronization completes durability. Before-publication failure leaves the destination absent and safely retryable. Rollback only unlinks known owned file identities and removes matching empty directory identities; it never recursively deletes substituted/foreign paths. Process exit may leave an unpublished sibling stage; no automatic recursive orphan cleanup is performed.

Errors before current publication return `Err(ProjectError)` and do not attach storage/change live content. Once publication succeeds, a failure of its final directory synchronization cannot be described as cancellation or as though nothing was saved. `SaveReceipt.durability_error` carries typed `Io` alongside the actual published storage/manifest. The engine must attach the receipt before reporting the failed job. Core keeps the prior confirmed saved baseline/revision for the same attached target, sets `save_uncertain=true`, and forces `dirty=true`; a newly attached unconfirmed target has no confirmed saved revision. Undo does not clear uncertainty, and a same-session reopen does not erase it. A later fully successful save clears uncertainty. No successful `ProjectSaved` result may be emitted for an unconfirmed durability receipt. Inspect status and the explicit error before deciding whether to retry.

The failed Google operation preserves this particular save's attachment receipt in a typed Status detail: one NativeOperationResult containing checked ProjectSaved info with `save_uncertain=true`, alongside the project Io error. Its status remains failed; it is not a successful response. The shell may attach the actual published Save As route using that receipt. A later Get's sticky uncertainty is not proof that the observed operation published a target. Duplicate, malformed, wrong-domain or certain receipts are rejected.

`SaveStage` observers are real transition hooks, not simulated crash success. Engine `SPILING_PROJECT_FAULT=after_assets|before_manifest_replace|after_manifest_replace` exits the actual child at the corresponding hook. A process crash before publication leaves the prior complete current (or no first-save target); a crash after publication leaves a complete new current with synchronized sources. Power-loss guarantees depend on the filesystem/device successfully honoring synchronization, and are **not** proved by process-crash tests.

## Explicit recovery and high-water checkpoint

`previous.json` is strict JSON, at most 1 MiB, containing exactly:

```text
RecoveryRecord {
  manifest: ProjectManifest,             // previous committed semantic content
  revision_high_water: ProjectRevision, // at least candidate/current revision
  next_occurrence_high_water: u32,      // at least candidate/current next counter
  next_surface_layer_high_water: u32,   // at least candidate/current surface counter
  next_process_layer_high_water: u32,
  next_process_operation_high_water: u32
}
```

All high-water fields must be at least their embedded manifest values. Recovery independently validates the complete saved graph, restores semantic content, takes each allocator's maximum recorded counter and allocates a fresh revision above the recorded high-water. It cannot reuse IDs published by an interrupted candidate. Exhaustion refuses. Saved revision describes the confirmed embedded checkpoint; recovered/dirty state remains until confirmed Save.

## Platform and evidence boundaries

- Linux: anchored `openat`/`O_NOFOLLOW` regular-file IO, stable OS file locking, file/directory `fsync`, atomic `renameat` replacement, and `renameat2(RENAME_NOREPLACE)` initial publication. Unsupported exclusive rename/filesystem synchronization returns typed error; no check-then-overwrite fallback.
- macOS: the corresponding anchored IO, atomic replacement, `renameatx_np(RENAME_EXCL)` publication, directory `fsync`, and file `F_FULLFSYNC` after `sync_all`. These primitives are implemented but unverified on macOS in this wave.
- Windows: checked non-reparse file/directory handles, OS file locking, `MoveFileExW` with write-through and replacement only where required, exclusive initial move, and explicit directory flushing. Directory flush/access and atomic/durability behavior remain unverified on Windows in this wave. A filesystem/permission configuration refusing directory flushing yields typed `Io` (before publication, or an uncertain receipt after publication), **not** silently weakened durable success.

No cross-platform power-loss certification is claimed. Network/cloud-synchronized filesystems and hostile concurrent mutation outside the cooperative writer protocol are not certified. `cargo test -p spiling-core` exercises deterministic public behavior including revision/dirty/history/source pins, lock/reuse/read-only snapshots, corruption, cancellation boundaries, recovery high-water, foreign-target ownership and postpublication durability uncertainty. Actual-engine crash/concurrency/native reconstruction evidence and platform evidence are separate acceptance surfaces owned by engine/integration.
