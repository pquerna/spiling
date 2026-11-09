<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native Analyze

Analyze currently authors and persists FDM boundary intent through the same
Core, Projects service, engine-client, CLI and native workbench as Prepare and
Review. An initial CPU FP64 mechanical consumer also solves an admitted complete
native orthogonal prism from a saved Basic case under explicitly assumed printed
wall/skin/effective-core laws. This is an **assumed-law numerical study**, not the
qualified Basic release or an actual-deposition reconstruction. Saving a case
alone remains authoring, not a numerical result. Empty load/restraint lists are
valid drafts, not an admitted analysis.

## Persistent and native authority

`ProjectManifest.analysis` and `ProjectInfo.analysis` are required strict
`AnalysisDocument` values. A case record contains its authored `case` and the
owning `ProjectRevision` at its last edit, `edited_revision`; there is no
independent case clock. Missing documents are refused, without changing the
unreleased project format counter or adding a compatibility reader.

Each case has a canonical nonnil UUID `id`, trimmed control-free 1–80-byte
`name`, explicit `layout_id`, `occurrence_id`, immutable `definition_id` and
original `source_hash`, `mode`, `loads` and `restraints`. Names are unique within
a layout. Identity-bearing source/occurrence bindings cannot be replaced under
an existing case ID. Create another case rather than guessing face remapping.
Moving the same occurrence to another layout carries its case and advances its
edit revision. Copies do not invent copied cases. Subject removal cascades its
cases; undo restores the exact document.

`Basic` and `Enhanced` are the only operator modes. Both require FDM-specialized
construction and directional mechanics; neither authorizes an isotropic bulk
solid or bulk modulus multiplied by sparse fraction. Reference belongs to
advanced numerical/validation controls, not a third operator mode. These mode
values preserve intent. The bounded assumed-law macro consumer admits Basic;
Enhanced currently refuses execution rather than silently falling back. Detailed
infill/interfaces, multiscale/higher-order refinement and numerical reference
qualification remain separate obligations, not properties conferred by a mode.

`Projects.Execute(EditAnalysis)` carries session, base project revision and a
typed `put`/`remove` edit, with checked protobuf adapters in both directions.
Core stamps the edit revision and owns atomic mutation/history/dirty state.
The engine checks every patch against the selected layout's current native
source-face table before committing a put. Every project open, including source
reuse, checks all retained patches against rebuilt/reused native faces before
publication. Canonical-looking face strings or display triangles are not native
admission. Read-only, stale, unknown-source/face and excessive-resource edits
refuse without mutating the scene or project.

Case edits do not alter source geometry, scene revision, print settings, process
or surface intent, or the manufacturing input fingerprint/artifact. Mechanical
operator reuse distinguishes load-only changes from construction, restraint,
source, pose, resolution and law changes; preserving a manufacturing artifact is
not proof of an existing FE cache.

## Finite patches, units and frames

The initial patch is one **whole trimmed native source face**, including its
holes, not a vertex, mesh triangle, point load or untrimmed carrier. Arbitrary
sub-face patches and manufactured added-surface selection are not admitted by
this contract. Native face identity does not yet establish the patch's
discretized area, moments, DOF map, load transfer or restraint rank.

All components use the normalized right-handed **definition-local service
frame**, with mm and N. Changing a part's print orientation does not rotate the
authored service load with the printer axes. Each `AnalysisFaceLoad` contains
`face_id`, three `force_n` components and three `moment_n_mm` components. Moment
is the total resultant **about the source origin**, not an additional couple at
an implicit patch center. Every admitted finite-patch distribution must conserve
that requested force and moment together. The mechanical consumer uses the
distribution described below. No lever arm, contact pressure or
force magnitude is invented. A pure moment is permitted; an all-zero wrench
and nonfinite components are refused.

`AnalysisFaceRestraint` contains `face_id` and three explicit
`fixed_translation` flags in the same service axes. At least one direction must
be fixed. Loaded faces are unique, restrained faces are unique, and whole-face
load/restraint overlap refuses. These are authored ideal restraints, not checked
fixtures, constrained rotational DOFs, stability or reaction-balance receipts.

## Mechanical service and model admission

`Projects.RunAnalysis` captures the exact saved case record, project revision,
native definition and occurrence pose, plus explicit `AnalysisRunSettings`.
Both adapters validate typed recipes and three symmetric strictly SPD 6×6
engineering tensors; no JSON RPC tunnel, default material, stiffness floor or
bulk-modulus/density substitution exists. Tensor order is `[xx, yy, zz, yz, xz,
xy]` for stress and `[xx, yy, zz, 2yz, 2xz, 2xy]` for strain. Tensors are in
global print axes, not reconstructed individual road tangents. Wall and skin are
assumed homogenized printed regions. The core is an **assumed effective core
law**, not a qualified constituent/RVE receipt. Its exact `compatible_recipe`
and explicit model/compatibility `justification` must accompany the laws. A
changed density/pattern/recipe refuses unless an explicitly compatible assumption
is supplied; rebinding a hash does not calculate a changed law.

The initial source-only admission requires actual complete placed native
CLOSED-slab and source-leaf receipts over the entire source height, with closed
shape allowance ≤0.001 mm. Descriptive planes only propose/filter query levels.
Each orthogonal boundary chord must match one complete native graph interval
enclosure and native span face ordinal. Canonical polygon start order is not
native span order. Display triangles never supply material or face authority.
Native holes and all disconnected paths remain in the model; no cropped source,
soft void material or removed cells can rescue admission. General curved/nonprism
parts, cap patches, non-axis-aligned print rotations, unequal top/bottom skins
and projects with regional process/surface additions currently refuse. These are
explicit coverage limits, not replacements for the full native product scope.

A conforming affine Hex27/Q2 whole-part mesh uses full 3×3×3 Gauss integration.
Physical skin thickness takes precedence; the remaining perimeter-width strips
are walls; the residual is core. Regions partition volume without overlap.
Interfaces are explicitly assumed tied continuum hypotheses, not witnessed
fine-road attachment or weld quality. Face-connected components are retained;
zero-area/local point/edge contacts refuse rather than weld. Distinct cut planes
remain distinct; near-coincident cuts (≤1e-8 mm) refuse rather than coalesce a
thin region or gap. Each component
requires six independent restrained rigid modes. The original sparse stiffness
is factored without regularization by the exact-pinned CPU numerical library.
There is no hidden reduced integration or artificial pinning.

For each admitted full side-face patch, 3×3 facet quadrature integrates area,
centroid and rotational inertia. Affine traction is `F/A + b × (x-centroid)`;
the residual couple is `M_source_origin - centroid × F`, not a second lever-arm
moment. Consistent Q2 nodal loads and retained patch quadrature are independently
summed for force and **source-origin** moment. Original-coefficient FP64
quadrature separately reconstructs strain, stress, internal forces, reactions
and energy for free residual, global force/moment balance and work/energy
assessment. A residual is not a discretization/model accuracy receipt.

Print orientation rotates construction and tensors; the intended service wrench
stays in source axes. Native/source/pose, exact compatible laws/recipe, restraints,
resolution, producer and kernel identities own the single worker's operator
cache. Load-only edits reuse the original factor. Model work occurs outside the
writer on the existing bounded worker; revision/cancellation and promotion ACK
rules also govern fields. Cancelled, failed or stale work cannot synthesize a
successful result or replace a retained success.

Admission is bounded at 768 elements, 16,000 DOFs, 4 MiB packed fields, conservative
2 GiB symbolic admission and 512 MiB numeric factor/scratch. Requests select
1–60 seconds; the native watchdog shortens its normal deadline to this limit
and retains its cancellation grace/exit policy for non-preemptible work. Safe
boundary budget checks do not promise mid-factor cancellation. The existing
session compute runtime remains authoritative; this first solver actually runs
on CPU FP64, not CubeCL or a backend-name-only GPU implementation.

## Fields, evidence and saved return

Successful Operations return `AnalysisCompleted` with captured project revision,
input hash and an immutable ByteStream `application/x-spiling-analysis-fields`
artifact. Producer `assumed-printed-hex27-1`, model scope
`planned_assumed_tied_macro` and `accuracy_evidence: unqualified` are explicit.
An explicitly retained local Operations store retains successful fields across
engine restart (CLI `--store DIRECTORY`; the default CLI store is temporary).
A portable `.spil` result attachment/automatic workbench result restore
is **not implemented**. Saved cases reopen without original STEP paths and can
be solved again. Read-only numerical studies do not change dirty state, history,
project or scene revisions. Failed attempts preserve old immutable artifacts.

SPLA1 is explicit little-endian packed data, never Rust memory layout. Its
32-byte header is magic `SPLA`, u32 version 1, u32 JSON-metadata length (≤48 KiB),
node/element/patch counts, and two zero reserved u32s. Strict bounded metadata
precedes fixed records: 72-byte nodes (source position mm, displacement mm,
applied force N; each three f64s); 232-byte elements (27 u32 nodes, center and size
in mm as six f64s, u32 role wall=0/skin=1/core=2, f64 envelope volume mm³, six f64
centroid macro stresses MPa, energy N·mm, dimensionless concentration); 64-byte
patch samples (three f64 source coordinates mm, f64 area mm², three f64 tractions
MPa, u32 authored load index, zero reserved u32). Checked decode validates exact
length/counts, finite values, connectivity, units-bearing metadata and producer
before bounded index-based partial inspection. Client retrieval binds hash,
size, MIME, input identity and captured revision.
SPLA currently has Rust consumers only; no TypeScript packed decoder or
cross-language binary interoperability qualification is claimed.

Macro stresses are effective continuum/centroid samples, **not resolved road
maxima or strength**. Signed layer-normal stress and nodal displacement remain
available. Regional energy concentration uses an explicitly reported radius of
three bead widths and nominal deposited-volume normalization: core envelope
volume times declared fraction, with corresponding energy density conversion,
not stiffness scaling. This nominal volume does not witness actual deposition
or welds. The native pane displays actual computed cell samples on a source-Z
slice with field name, units and actual min/max scale; it retains a stale last
success visibly and compares consecutive compatible construction/orientation
captures under the same service wrench. Desktop rendering/hardware qualification
is distinct from headless host/service checks.

Costs separate native capture, construction, original operator setup, warm RHS,
solve, original-coefficient assessment and field computation. Field packing,
transport, display and authoring metadata costs are not included in those phase
numbers. First-operator and warm measurements are not cold/p50/p95 CPU/GPU cohort
qualification. Basic's proposed 2s warm load/5s moved patch/30s first-result
engineering targets and Enhanced's declared warm-hybrid target remain proposed.
Independent analytical bending/rigid/shear/assembly checks do not qualify fine
roads, general hook discretization accuracy, N1, physical calibration, breaking
load or machine readiness.

The complete original hook fixture and explicit uncalibrated assumptions are
`fixtures/geometry/analysis-hook.step` and
`fixtures/analysis/hook-assumptions.json`. Bounded reproduction:

```sh
cargo test --locked -p spiling-engine -p spiling-analysis --lib -j 2 -- tests::
cargo test --locked -p spiling-engine --test project whole_hook_native -j 2 -- --nocapture
```

On Linux, run native workloads sequentially in an external cgroup with declared
memory/task and wall-time limits; the numerical allocation/deadline limits are
not whole-process RSS guarantees. Verification notes retain actual commands and
measurements, not implied universal hardware acceptance.

## Case edit budgets

The document permits eight cases and sixteen load plus sixteen restraint patches
per case, with 64 KiB allocated metadata per document and 4 MiB distinct
current/history/saved/prepared allocations in Core. Parsing bounds arrays before
unbounded collection growth. The independent project manifest/control limits
still apply. These are short bounded metadata edits, not new long-running jobs;
they use existing revision, busy and single-writer admission. Native open/save
retain their existing Operations cancellation/publication lifecycle.

## Operator surfaces and reproduction

In the native Analyze workspace select a part, create a case, pick source faces
in the source tree/viewport, enter an explicit wrench, set a face load and fix a
different face. Apply commits once; Save writes the normal `.spil` checkpoint.
Read-only cases remain inspectable. Unapplied or rejected drafts are retained;
stale drafts require explicit discard/reopen. Prepare still owns print settings.
Numerical studies require a separately explicit planned recipe and compatible
assumed laws. These detached study inputs do not silently adopt or replace
Prepare's manufacturing intent. The pane identifies unresolved coverage and
accuracy rather than presenting the study as a qualified Basic result.

CLI operations execute in order, using returned native face IDs:

```sh
spiling-cli project create hook.spil --import hook.step \
  --analysis-case 'Hook load' 1 basic
spiling-cli project inspect hook.spil
spiling-cli project edit hook.spil \
  --analysis-load 'Hook load' step:LOAD_FACE 0 -12 0 15 0 0 \
  --analysis-fix 'Hook load' step:RESTRAINT_FACE
spiling-cli project inspect hook.spil
spiling-cli --store analysis-store project inspect hook.spil \
  --analysis-run 'Hook load' fixtures/analysis/hook-assumptions.json
```

Replace the face placeholders with actual returned IDs; the force and moment
above are operator-authored example inputs, never inferred defaults. Repeating
`--analysis-case` on the same named subject changes Basic/Enhanced without
changing its identity. `--analysis-load` replaces that face's wrench;
`--analysis-fix` replaces its flags with XYZ fixed. Bounded typed
`--analysis-edit` JSON supports explicit put/remove and directional restraints.
All edits refuse in read-only inspection.
`--analysis-run NAME SETTINGS_JSON_FILE` also permits read-only inspection. It
prints the actual engine summary, bounded centroid field samples and durable
operation/artifact identity; it does not calculate mechanics in the CLI.
With the same retained `--store DIRECTORY`, `--analysis-return OPERATION_NAME`
retrieves a completed historical result and checks its project/input/artifact
identity without rerunning. `capture_matches_project_revision` distinguishes a
matching saved revision from historical evidence; returned costs belong to the
original calculation, not the retrieval.
The CLI's default store is temporary: without an explicit `--store DIRECTORY`,
artifacts do not survive CLI exit. Local ledger retention is not a portable
result attachment inside the saved project.

The real-child regression is
`cargo test -p spiling-engine --test project analysis_cases_check_native_patches_and_survive_history_source_absent_reopen -- --nocapture`.
The separate-process CLI regression is
`SPILING_TEST_ENGINE=target/debug/spiling-engine cargo test -p spiling-cli --test project analysis_cases_roundtrip_through_separate_cli_processes_without_the_source_file -- --ignored`.
They exercise native identity, history and source-absent saved return, not a
mechanical solve or packaged desktop/input/GPU qualification.

## Next consuming acceptance

The qualified release still requires a complete printed hook/bracket equilibrium
result with meaningful concentration/stress/displacement fields and explicit
field units, scale, macro versus reconstructed/resolved origin and numerical
assessment. It must bind current explicit print settings, source/service/print
frames, wall/skin/core ownership, directional laws, finite-patch force/moment
transfer, restraints and case/input revisions. A load-only rerun must reuse an
eligible original operator; setup, RHS/solve, assessment and visible-field costs
must be measured separately on the CPU baseline before claiming GPU speedup.
The initial consumer exercises those numerical phases, orientation comparison
and local-ledger saved return under assumed laws. It does not close portable
project result attachment, general native coverage, substantial application
refinement/accuracy, detailed infill/interfaces or physical/model evidence.
Those obligations cannot be replaced by additional case metadata or a coupon.

Enhanced uses those same cases/captures with detailed infill/interfaces,
higher-order refinement and admitted effective/reduced/offline reuse, not a
separate application or a permanent alternate placeholder engine. None of this
is conferred by case persistence. The [infill lab](../quality/infill-lab.md)
remains numerical research: converged algebraic residuals and analytical beam
controls alone do not qualify thin-road/crossing-road or whole-part fields.
There are no calibrated strength, breaking-load or machine-readiness claims.
