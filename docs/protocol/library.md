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

Hardware inventory, public project captures and private machine/plate associations
share this same container and mutation protocol. [Hardware contracts](hardware.md)
own their identity, compatibility, credential and connection boundaries.

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
Complete RecipeTransfer values are limited to 48 KiB of canonical domain JSON;
actual typed protobuf controls still have the independent 64 KiB bound. A closure
that cannot fit refuses explicitly; it never drops profiles or evidence.

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

## Reusable recipes

A target specifies the desired numeric color. A recipe captures two or three
exact, distinct filament variants, an exact palette evidence row per ingredient,
the source/license/conversion metadata, the named optical model, observation,
physical layer cycle and bounded search parameters. A cycle has ordered material
slots with millimetre heights, phase and Z origin; it is not a percentage blend.
Recipe identity is a UUID plus the hash of its complete definition. A transfer
contains exactly its ingredient revisions, including associated profiles and
appearance declarations; missing, duplicated or unrelated dependencies refuse.

Libraries.ListRecipes/GetRecipe and Projects.ListRecipes/GetRecipe return bounded
summaries and complete typed transfers. PutRecipe atomically merges the closure.
Existing variants keep their current revision, archive state, origins and stock;
a required earlier revision is retained independently. Recipe replacement requires
the expected prior recipe hash plus the library CAS or persistent project base
revision. Name/color matching never substitutes another ingredient. Explicit
library removal is revision-bound. Project mutations use normal Undo/Redo,
recovery and save; a copied recipe remains usable with no personal file.

CaptureRecipe requires a complete assessed report and explicitly retained
selection. It binds each evidence key to an exact project ingredient, retains
only those evidence rows and preserves their source metadata. For a retained
candidate it captures that candidate's actual physical cycle. The template stores
no previous success claim, native face, occurrence, loaded tool assignment or
machine program. Explicit ingredient/evidence mapping is operator attribution,
not proof of material calibration.

LoadRecipeStudy requires a project-owned exact recipe and explicit new upright
build-frame patch and cycle-origin context. It installs the captured palette,
rebases both requested and searched cycle origins, and clears comparison reports
and selections. Importing, capturing or loading an unused recipe preserves
manufacturing output. Recompare uses the existing optical service; acceptance and
ApplyOpticalConstruction remain distinct explicit operations. Only the existing
bounded fixed-dual construction can be applied, with two loaded declarations and
source-bound construction independently admitted by manufacturing. Three-material
recipes are reference studies, not executable three-tool support.

The native Library page separates Filaments, Recipes and Hardware within My library
and Project. Add variants from either scope to a retained ingredient draft. Create
an unassessed declaration recipe directly in either scope; a new declaration
palette uses explicit sRGB as catalog estimates and leaves absent/non-sRGB optical
evidence unknown. Its source points here to identify the declaration conversion;
complete ingredient definitions retain the original operator/imported source.
No RGB averaging or measured evidence is invented. Inspecting a Project recipe
exposes its exact ingredients, target, cycle, provenance and editable study
context. Fresh comparison results are inspectable before acceptance and remain
bound to exact study inputs and the captured revision. Accepted results may survive
the acceptance revision; unrelated stale comparisons do not become current.

Save a copy to My library creates a new recipe ID in one personal transaction.
It leaves the project unchanged. Copy complete recipe to Project imports the
reviewed immutable closure in one project transaction; it does not automatically
load, reassess or apply it. Duplicate recipe IDs refuse rather than silently
replace. Updating a template through the typed PutRecipe operation requires its
expected revision. Personal edits never silently update earlier projects. Stock-
wide target search, reference catalog ingestion, print calibration and privacy
export are separate future consumers.

## Consumer surfaces and reproduction

The Library page is reachable from Project, Prepare and the command palette.
Its explicit Back action returns to the workbench and retains ordinary drafts.
Focused editors preserve their subject/scope; replacing a draft requires deliberate
discard. Private credential input clears on leaving its view.
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
spiling-cli library recipes --library /absolute/My-library.spil
spiling-cli library recipe RECIPE_UUID --library /absolute/My-library.spil
spiling-cli project open Job.spil --library /absolute/My-library.spil --recipe-use RECIPE_UUID --save
spiling-cli project open Job.spil --recipe-study RECIPE_UUID context.json --optical-compare --optical-accept requested --save
spiling-cli project inspect Job.spil --project-recipes --recipe-inspect RECIPE_UUID
spiling-cli project open Job.spil --library /absolute/My-library.spil --recipe-save RECIPE_UUID
```

`--project-material-profile FILAMENT_UUID PROFILE_ID REVISION SETUP_JSON` applies
an exact project-owned profile against an explicit installed setup. Rich inventory
edits use a bounded inert LibraryWrite JSON file with `library write REQUEST_JSON`;
the client maps it into typed protobuf, never a JSON RPC tunnel. Retain its request
UUID when resolving uncertainty. Recipe import accepts a bounded complete
RecipeTransfer JSON file through `--recipe-import`. After explicit report
acceptance, `--recipe-capture NAME ingredients.json` captures the chosen cycle;
the file contains RecipeIngredient values in evidence order. `--recipe-study`
uses a RecipeStudyContext JSON file with `patch` and `cycle_origin_z_mm`.
No stock operation confers machine readiness.

Exercise storage/domain regressions with `cargo test -p spiling-core-ptac` and
`cargo test -p spiling-core`. Actual child coverage is
`cargo test -p spiling-engine --test library`. Native host coverage is
`SPILING_TEST_ENGINE=PATH cargo test -p spiling-workbench library_ -- --ignored` (inventory/copy plus project editing, comparison,
stale promotion, Undo/Redo and read-only return).
Use the [development resource controls](../quality/development.md) for native
builds/checks. These tests establish scoped software behavior, not power-loss,
physical color, printer or cross-platform packaging qualification.
