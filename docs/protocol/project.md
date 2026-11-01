<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Durable source-backed project format

The executable storage schema is `spiling-contracts::project`; state and storage authority is `spiling-core`. Engine caches are disposable. CLI, desktop and engine-client do not implement this format themselves. Generated protobuf Projects services operate on captured immutable snapshots rather than frontend recipes or external source paths. Long opens/saves are standard Google Operations scoped to the native session; RPC and operation metadata are not persisted project manifest schemas.

Format evolution and the initial release compatibility boundary follow the [canonical pre-release format policy](../../AGENTS.md#pre-release-format-evolution).

Project status and manifests carry a strict required-nullable `hardware` capture,
including before a recipe exists. Its complete portable definitions travel with
history and manufacturing input; private fleet/route/credential data remains in
the personal library. [Hardware contracts](hardware.md) own selection and
invalidation semantics.

## Container and records

```text
<name>.spil                    closed Spiling PTAC SQLite profile
<name>.spil.lock               stable lifetime writer lease, not project content
<name>.spil-journal            SQLite rollback state; may be required after a crash
```

The project is one ordinary SQLite file with a closed, six-table schema, immutable exact-byte objects and exactly current plus optional previous saved snapshots. The independently testable `spiling-core-ptac` owns SQLite, filesystem admission, chunks and publication; Core owns project meaning. See [the physical storage contract](project-storage.md) and [ADR007](../adr/007-ptac-project-storage.md). Existing project directories are refused without readers, converters, migrations or compatibility defaults. Recreate development projects through original source imports.

The root object is strict UTF-8 `ProjectManifest` JSON, at most 1,048,576 bytes. Duplicate object fields, unknown fields, trailing JSON, invalid checked IDs, invalid poses and unsupported versions are errors. No external lookup paths, native objects, session IDs, display artifacts or undo records are persisted. `source_name` remains a captured display basename and manufacturing provenance, never a lookup path. Its exact fields are:

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

Core requires exact equality between the root's referenced assets and snapshot membership: roles 1 source STEP, 2 manufacturing bundle, 3 canonical image, 4 original palette and 5 canonical palette. Unknown, missing and extra roles/members refuse. Shared content retains one exact-byte identity across roles. Domain budgets remain independent of the container's row/disk bounds; storage capacity is not permission to capture more live/history/prepared data. Explicit reachability collection and compact-to-new never collect Core's session history or reinterpret a corrupt project.

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
target/debug/spiling-cli --engine "$PWD/target/debug/spiling-engine" project create "$work/study.spil" \
  --appearance-palette "$work/allowed-palette.json" \
  --appearance-target fixtures/appearance/target-orange.json --appearance-compare \
  --appearance-accept 261ae7e7-20d9-5969-9ed5-dd82eea29bcf/catalog-rgb \
  --save --appearance-target fixtures/appearance/target-red.json --undo --redo --undo --save
rm "$work/allowed-palette.json"  # This newly created owned input only.
target/debug/spiling-cli --engine "$PWD/target/debug/spiling-engine" project inspect "$work/study.spil" \
  --appearance-report "$work/reopened-report.json" --appearance-compare
```

`--appearance-accept-report` explicitly accepts a complete report without selecting a candidate; it is not a reserved candidate-key sentinel. `--appearance-limits FILE` supplies strict `max_visited`/`max_alternatives` JSON after a target. Exports use the shared exclusive output transaction and commit only after observed child reap. Inspect opens fresh read-only and uses captured assets after source deletion. Storage keeps project10/manufacturing11/process3/surface3 and the current packed-format/producer freshness checks unchanged.

## Optical reference construction studies

`spiling-contracts::optical` owns strict typed requests, physical cycles and reports; `spiling-appearance::optical` evaluates them without Core, geometry, threads, filesystem or printer dependencies. The named model `prusa_v7_007bb922` is a source-attributed reference estimate, ported from the MIT TypeScript v7 functions at `007bb922994cdf985c324cb734f6504e805f6790`. Its own matrix, byte rounding, fitted corrections, display clipping and pure-component guard are reference semantics, not a change to C1a's unclipped CSS-D65 conversion or target ranking. The original notice is retained in the port and release notices.

An `OpticalRequest` identifies two/three distinct palette material keys, a target/tolerances, observation, an upright finite build-frame-mm patch, an ordered requested cycle and a finite search grid. Patch Z runs along build Z and its unit outward normal lies in build XY; only normal-to-patch viewing is represented. Cycles retain each material index and positive physical slot height, phase slot and physical origin Z. At that origin the selected phase slot begins. Resolution analytically clips the periodic construction to the patch's actual height; it does not iterate all layers or equate slot counts with exposed height. A finite partial cycle can expose unequal fractions from an equal-slot recipe. Requests outside the stated f64 representability envelope refuse rather than clamp coordinates or phase.

Search explicitly enumerates pattern × uniform slot height × phase, in that order, with at most1024 declared combinations and at most256 retained alternatives. Requested recipes may use unequal slot heights. A phase outside a particular pattern is a visited filtered candidate, not modulo-normalized into another recipe. Reports retain the requested assessment, every visited grid index, exposed heights/fractions, predicted Lab/display bytes, target errors, ranked assessed alternatives and traversal/completion state. Alternative truncation does not stop enumeration. Incomplete work/cancelled reports cannot be accepted, even after finding an estimate within tolerance.

The predictor uses only positive-exposure components. Catalog base RGB is explicitly quantized to the reference's byte input; supported declared measured base colors are converted through existing unclipped colorimetry and remain unassessed outside the model's RGB domain. Unknown evidence/conditions are not assigned zero error. D50, ten-degree or unknown observation is unassessed, not silently adapted to D65/2°. Reference-model corrections are not Spiling calibration: no finite backing transport, viewing-distance/stripe-visibility model, arbitrary orientation, physical uncertainty bound, material/process admission or routed printing is supplied. `reference_estimate` and `study_only_not_routed` remain explicit.

`AppearanceStudy.optical` is a required nullable field. Its request and accepted report are immutable Arcs, deduplicated across current/history/prepared/saved pins under the existing64MiB appearance allowance. The typed authoring request is stored with the project; its external CLI JSON path is not a persistent input. Base data remains the original/canonical palette asset. C1a target/limits and optical requests are independent. Palette replacement revokes optical report/selection authority and cannot retain an incompatible query. Complete-report acceptance binds the exact current project/revision/hash, palette/evidence and request; it may select the requested estimate or a retained assessed grid candidate. Study edits preserve manufacturing bytes/input identity and SceneRevision.

`Projects.Execute` carries typed `SetOpticalStudy`/`AcceptOpticalReport`; `CompareOptical`/`GetOpticalReport` use the existing worker/ACK/native Operations and checked ByteStream path. `OpticalCompared` binds captured project/revision, a producer/input-bound record and `application/x-spiling-optical-report+json` bytes (<=1MiB). ProjectInfo contains request/selection/report metadata, never the full report. Compare/Get permit read-only; stale queued work and historical operations cannot authorize current acceptance. Existing development projects missing the strict field must be recreated; no format-counter change or compatibility default is introduced.

`Projects.ApplyOpticalConstruction` applies a retained complete selection to an existing source-bound surface construction. Exactly two ordered bindings name each palette evidence key/hash and the loaded material ID/declaration hash. Core captures the selected physical recipe, original target/patch/observation, complete evidence rows, loaded declarations and occurrence pose in the manufacturing intent; no external palette path is needed to recover it. Preparation/cancellation/stale checks and the promotion ACK precede one atomic revision increment, manufacturing invalidation and ordinary undo/redo. Application preserves SceneRevision and publishes no scene/artifact outputs. Independent recipe edits must explicitly clear the captured origin. The CLI exposes `--apply-optical CONSTRUCTION_INDEX BINDINGS_JSON_FILE`; the supplied file contains two typed `OpticalMaterialBinding` values, not a copied recipe.

The CLI consumes real engine computation, not a local predictor:

```sh
work="$(mktemp -d)"
cp fixtures/appearance/palette.json "$work/palette.json"
cp fixtures/appearance/optical-layer-cycle.json "$work/request.json"
target/debug/spiling-cli --engine "$PWD/target/debug/spiling-engine" project create "$work/study.spil" \
  --appearance-palette "$work/palette.json" --optical-study "$work/request.json" \
  --optical-compare --optical-accept requested --save --undo --redo
rm "$work/palette.json" "$work/request.json"  # Only these newly owned external inputs.
target/debug/spiling-cli --engine "$PWD/target/debug/spiling-engine" project inspect "$work/study.spil" \
  --optical-report "$work/reopened-report.json" --optical-compare
```

The example's target is the pinned model's synthetic reference result, not a measured mixed coupon or a manufacturing-quality promise. Its finite0.15mm window over two0.10mm slots exercises unequal exposed fractions. `--optical-accept best` chooses a retained ranked candidate; an integer denotes the original grid index, not its rank. `--optical-accept-report` accepts a complete report without selecting an estimate. Export destinations must be new and use the shared owned-handle/reap/rollback transaction. Study authoring remains headless. The explicit application operation connects a retained selection to the bounded fixed-dual manufacturing route; fresh inspection reassesses its realized surface and the legacy viewport can tint actual tool strokes using captured catalog metadata. Backed-patch optics and physical color accuracy remain unqualified.

## Writer ownership and reader snapshots

The stable separate `<name>.spil.lock` lease lasts for the retained writable Storage lifetime. Competing writers fail with `ProjectLocked`; a read-only attachment uses an actual read-only SQLite connection and never secretly recovers writable. Same-writer staged reopen reuses its ownership. Main file, sidecars, lock and ancestry are checked for substitutions and hard links. Raw main-file descriptors are never opened/closed while any in-process SQLite connection owns that identity.

A short read transaction captures one selected snapshot and its guard; all source/image/palette/bundle bytes are verified before they become Core assets. Old and incoming prepared allocations coexist through native ACK, charged against the same 64 MiB source, 64 MiB manufacturing, 4 MiB image and existing appearance budgets. Identical IDs do not authorize temporary double-budget capture. No original external input is reopened. SQLite reader locks and lifetime-owned Core Arcs prevent mixed snapshots and collection from invalidating captured assets.

## Save publication, cancellation and errors

Core captures and validates an immutable snapshot without advancing authoring revision or clearing undo. PTAC stages bounded encoded bytes outside the write transaction, verifies reused content, compares generation/root identities and inserts the complete candidate under `BEGIN IMMEDIATE`. Roots, membership and independent guard publish together. Prior rows remain unreachable until explicit maintenance, not implicit cancelled-save cleanup.

Existing-file cancellation remains effective through insertion and immediately before COMMIT. First save/Save As builds and independently checks a complete private database, closes every connection and flushes it before the decision immediately before atomic exclusive/no-replace rename. Never use hard-link-plus-unlink publication or replace a live database. Save As retains the independently admitted last saved snapshot as previous when distinct. Parent/staging directory sync failures after rename are post-publication outcomes.

`SaveReceipt` distinguishes confirmed publication, observed but durability-unconfirmed publication, and unknown commit outcomes. Pre-publication errors leave the prior saved baseline. An unconfirmed/unknown receipt retains target ownership, sets sticky `save_uncertain` and dirty, and never fabricates a confirmed saved revision. Unknown outcomes stop writes pending explicit reopen/inspection. Undo, Get and same-session reopen do not erase uncertainty; only a confirmed Save clears it. Failed operations carry the checked failed-only `ProjectSaveUncertain` result, not a successful `ProjectSaved`. The native host keeps the caller-authorized attempted route but never automatically retries or restart-reopens an uncertain save.

`SaveStage::BeforeCommit` is the engine's actual irreversible decision; the other hooks expose staging, inserted rows, COMMIT returned, and target namespace publication. `SPILING_PROJECT_FAULT` terminates only disposable actual-child workflows at the named transitions described by the owning engine tests. Process termination tests are not electrical power-loss certification.

## Explicit previous-save recovery and guard

`container.guard` contains independently hashed, strict JSON `ProjectRecoveryGuard`: project ID, revision high-water and the four occurrence/surface-layer/process-layer/process-operation allocator high-waters. It must dominate every admitted retained root and agree on project identity. It detects accidental corruption, not maliciously recomputed hashes or authenticity.

Every ordinary open also initializes live revision/allocators from the maximum of selected root, guard and same-project live high-water. The root's actual saved revision/content remain separate, so raising floors alone does not make content dirty. Revisions/IDs lost with an unsaved process are outside restart guarantees.

Explicit previous recovery validates that root's complete graph and guard independently of current payload validity, allocates a fresh revision above all admitted floors, and forces dirty. Missing/bad guard, unreadable physical structure, unavailable previous or exhaustion refuse; no automatic fallback occurs. Confirmed recovery Save retains its bound validated previous source rather than the damaged current. The two roots share one database and are not independent media backups.

## Platform and evidence boundaries

The filesystem adapter currently admits Unix paths and uses anchored no-follow ancestry, separate OS leases, SQLite's pinned standard VFS with the mandatory recovery namespace guard, and atomic no-replace rename. Linux is the local test surface. macOS code is not exercised durability evidence; non-Unix admission explicitly refuses rather than silently weakening safety. Network/cloud-synchronized filesystems, hostile concurrent namespace changes, cross-platform packaging and power-loss behavior remain unqualified.

See [storage conformance and reproduction](../quality/authoring-evidence.md), `cargo test -p spiling-core-ptac` and `cargo test -p spiling-core`. Native geometry reconstruction, independently decoded actual-program replay and physical printability have separate owning gates.

## Portable material collections

A project captures reusable filament/profile definitions as immutable collection
content, with a small explicit nullable descriptor in its root. Role 6 owns the
exact collection bytes. This is separate from personal physical stock and from
the material profile actually applied to manufacturing intent. The
[personal/project material contract](library.md) owns identities, limits,
transactions, publication and consumer workflows.
