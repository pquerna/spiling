<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Personal inventory and portable project materials

The native engine owns both personal inventory and project authoring. CLI and
workbench use authenticated typed Libraries and Projects services. Neither client
opens the authoritative files. A missing or unavailable personal library does not
prevent project authoring or opening project-owned material copies.

## Location and container identity

The default is `My library.spil` under
`directories::ProjectDirs::from("org", "Spiling", "Spiling").data_local_dir()`.
The workbench remembers a selected library location in its small UI preferences;
projects never depend on that path. The engine accepts an explicit absolute
`--library PATH` for isolated consumers and tests. Reading a missing library does
not create it. The default parent directory is created on the first write.

`spiling.user-library` and `spiling.project` are distinct compiled PTAC profiles.
Opening one as the other fails. The user-library root has its own UUID, logical
revision, independent revision-floor guard and bounded request receipts. One
immutable object owns the reusable material collection; physical stock remains
personal root content. Existing PTAC handle/path validation, exclusive writer
leases and confirmed/uncertain publication apply to both profiles.

Personal mutations attach a short-lived writer, compare the captured library ID
and revision, validate the complete candidate, publish and release. A read/search
never holds a writer lease. Competing windows get a typed conflict or lock error;
there is no automatic merge or new-ID retry. A corrupt existing file is never
replaced by an empty library.

## Material, profile and spool meanings

A filament entry identifies one product/color/diameter variant. Its content
revision is the hash of the complete checked definition, including associated
process profiles and declared appearance provenance. A variant may be entered
without complete manufacturing identity, appearance, profile or quantity. Missing
values remain explicit; stocking a filament does not make it admissible for a job.

A spool is a distinct physical item. Multiple spools reuse a variant rather than
becoming multiple optical ingredients. Its recorded filament revision retains
registration provenance. Availability follows the stable variant ID, so improving
a profile or display name preserves stock. While spools reference a variant,
changing its known material identity/formulation/diameter or declared color is
rejected; create another variant and deliberately reassign physical stock. Adding
previously unknown identity is allowed. Supplier names and color names are not
identity keys.

Stock can be on hand, ordered, empty or archived. Remaining grams are nullable and
carry entered/weighed source; missing quantity is neither zero nor a full spool.
On-hand filtering excludes empty/archived/ordered records and explicitly zero
remaining mass. Current records do not provide gross/tare operands, density,
observation timestamps, usage history, scanning or live machine loadout. Slicing,
verification and export never decrement inventory.

Declared appearance retains its operator/imported source and numeric color. It
does not become measured evidence or a catalog optical model. The workbench uses
shared ColorPresentation with declared-name precedence and an explicit unknown
state. A process profile remains the existing material/printer/nozzle/hotend/
cooling declaration, admitted through the existing manufacturing review/application
service; no second resolver belongs in Libraries.

## Project copies and history

Projects capture exact FilamentRevision data and optional source stamps containing
library ID, filament ID and captured content revision. The source stamp helps
comparison; it is never an external dependency. No spool quantities, shelf paths
or current printer slot state enter the project collection.

PutFilament is a normal persistent-revision-bound Core transaction. It adds or
explicitly replaces the project entry with an expected prior content revision.
Undo/redo preserves complete copies. Adding an unused reusable entry preserves
manufacturing artifacts; applying a different process profile uses the existing
manufacturing transaction and invalidation. Editing My library does not alter a
captured project or job.

The native Project scope supports editing a standalone definition while retaining
its captured origin. A comparison reads the current personal entry under a
captured library identity/revision and shows which fields differ. It distinguishes
same content, a project edit, a personal update and changes on both sides against
the captured origin. Missing or foreign personal origins leave a usable standalone
copy; saving a new personal item remains available.

Replacement is explicit and whole-definition. Accepting the personal version
commits a normal project transaction and updates its origin; Undo/Redo restores
both content and origin. Promoting the project version commits only the personal
file, using the reviewed library generation and expected prior content hash.
Concurrent writes reject that draft for renewed review; they cannot silently
stamp it with newer authority. A promotion does not mark the project saved or
rewrite its origin. Existing stock-variant protections still apply. Neither
replacement applies a process profile to the current job: Manufacturing review
and Apply remain separate. Read-only projects can be compared and deliberately
promoted, while project replacement remains disabled.

The project root and ProjectInfo contain an explicit nullable
MaterialCollectionRecord (hash, bytes and counts), never the full collection.
Project PTAC role 6 contains exact canonical collection bytes. Core validates
hash, counts, content revisions and dependency closure on load/save. Current,
history and prepared snapshots share immutable Arc-owned bytes, with a separate
64 MiB retained allowance. Open captures charge incoming data before replacing
old state. Project-only material browsing works read-only and without a personal
file. GetFilament returns an exact captured revision; ListFilaments is bounded.

## Bounds and publication

Initial hard bounds are 256 variants, 1,024 retained revisions, 512 physical
spools, 128 recipe records, 16 associated profiles per variant and 16 MiB for a
material collection. One filament definition is at most 32 KiB. Typed control
messages remain at most 64 KiB, and list pages contain at most 20 entries.
Recipe storage records do not by themselves expose recipe transfer or matching;
those consuming workflows require their own bounded service admission.

LibraryWrite carries a canonical nonzero request UUID and complete request digest.
The latest 64 receipts are retained. Repeating an identical retained request
resolves publication without reapplying its mutation or advancing the logical
revision. Reusing its UUID with other content fails. The engine re-publishes the
unchanged current snapshot to obtain fresh publication confirmation; merely
observing a receipt cannot resolve an earlier directory-sync failure. PTAC retains
the distinct previous snapshot when the current candidate is unchanged.

Confirmed, unconfirmed and unknown outcomes are distinct. The latter two retain
the original request and draft. A candidate identity in an uncertain receipt is
not proof that those bytes committed. A typed rejected write can be refreshed and
reviewed as a new request. Transport failure never authorizes silently generating
a new spool ID. Receipt eviction is bounded: a very old retry may require explicit
inspection and cannot claim automatic deduplication.

## Consumer surfaces and reproduction

The Materials window is reachable from Project, Prepare and the command palette.
My library supports entry/search, on-hand filtering, exact inspection, additional
spools, archive and explicit project capture. Project selection and profile review retain the exact project identity/revision;
a project change requires fresh inspection before review. Read-only inspection
remains available while Apply requires a writable project. Project inspection offers captured
profiles to the existing Manufacturing editor for review and Apply. Saving a
current job material to My library creates a separate personal item; it never
pretends two files committed atomically. Location selection chooses another file;
it is not a move, privacy export or backup operation.

CLI examples (paths are explicit and no target is overwritten):

```sh
spiling-cli library status --library /absolute/My-library.spil
spiling-cli library add-filament "My PLA" --hex CF524E --library /absolute/My-library.spil
spiling-cli library add-filament "Captured profile" --profile material-selection.json --library /absolute/My-library.spil
spiling-cli library filaments --on-hand --library /absolute/My-library.spil
spiling-cli library add-spool FILAMENT_UUID --grams 530 --library /absolute/My-library.spil
spiling-cli project open Job.spil --library /absolute/My-library.spil --library-use FILAMENT_UUID --save
spiling-cli project inspect Job.spil --project-materials
```

`--project-material-profile FILAMENT_UUID PROFILE_ID REVISION SETUP_JSON` applies
an exact project-owned profile against an explicit installed setup. Rich inventory
edits use a bounded inert LibraryWrite JSON file with `library write REQUEST_JSON`;
the client maps it into typed protobuf, never a JSON RPC tunnel. Retain its request
UUID when resolving uncertainty. No stock operation confers machine readiness.

Exercise storage/domain regressions with `cargo test -p spiling-core-ptac` and
`cargo test -p spiling-core`. Actual child coverage is
`cargo test -p spiling-engine --test library`. Native host coverage is
`SPILING_TEST_ENGINE=PATH cargo test -p spiling-workbench library_ -- --ignored` (inventory/copy plus project editing, comparison,
stale promotion, Undo/Redo and read-only return).
Use the [development resource controls](../quality/development.md) for native
builds/checks. These tests establish scoped software behavior, not power-loss,
physical color, printer or cross-platform packaging qualification.
