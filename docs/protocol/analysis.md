<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native Analyze case authoring

Analyze currently authors and persists FDM boundary intent through the same
Core, Projects service, engine-client, CLI and native workbench as Prepare and
Review. It does **not** yet construct a mechanical model, transfer boundary
conditions to a discretization, solve equilibrium or publish numerical fields.
Saving a case is not a stress, deformation, strength or physical-qualification
result. Empty load/restraint lists are valid drafts, not an admitted analysis.

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
values preserve intent; neither solver is implemented by this authoring cut.

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
operator reuse must likewise distinguish load-only changes from construction,
restraint, transfer and law changes when the mechanical consumer is implemented;
preserving a manufacturing artifact is not proof of an existing FE cache.

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
an implicit patch center. The future finite-patch distribution must conserve
that requested force and moment together. No lever arm, contact pressure or
force magnitude is invented. A pure moment is permitted; an all-zero wrench
and nonfinite components are refused.

`AnalysisFaceRestraint` contains `face_id` and three explicit
`fixed_translation` flags in the same service axes. At least one direction must
be fixed. Loaded faces are unique, restrained faces are unique, and whole-face
load/restraint overlap refuses. These are authored ideal restraints, not checked
fixtures, constrained rotational DOFs, stability or reaction-balance receipts.

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
The pane identifies missing construction/solve/assessment rather than showing
an invented heatmap or a disabled numerical result as success.

CLI operations execute in order, using returned native face IDs:

```sh
spiling-cli project create hook.spil --import hook.step \
  --analysis-case 'Hook load' 1 basic
spiling-cli project inspect hook.spil
spiling-cli project edit hook.spil \
  --analysis-load 'Hook load' step:LOAD_FACE 0 -12 0 15 0 0 \
  --analysis-fix 'Hook load' step:RESTRAINT_FACE
spiling-cli project inspect hook.spil
```

Replace the face placeholders with actual returned IDs; the force and moment
above are operator-authored example inputs, never inferred defaults. Repeating
`--analysis-case` on the same named subject changes Basic/Enhanced without
changing its identity. `--analysis-load` replaces that face's wrench;
`--analysis-fix` replaces its flags with XYZ fixed. Bounded typed
`--analysis-edit` JSON supports explicit put/remove and directional restraints.
All edits refuse in read-only inspection.

The real-child regression is
`cargo test -p spiling-engine --test project analysis_cases_check_native_patches_and_survive_history_source_absent_reopen -- --nocapture`.
The separate-process CLI regression is
`SPILING_TEST_ENGINE=target/debug/spiling-engine cargo test -p spiling-cli --test project analysis_cases_roundtrip_through_separate_cli_processes_without_the_source_file -- --ignored`.
They exercise native identity, history and source-absent saved return, not a
mechanical solve or packaged desktop/input/GPU qualification.

## Next consuming acceptance

The next useful consumer is one complete printed hook/bracket equilibrium
result with meaningful concentration/stress/displacement fields and explicit
field units, scale, macro versus reconstructed/resolved origin and numerical
assessment. It must bind current explicit print settings, source/service/print
frames, wall/skin/core ownership, directional laws, finite-patch force/moment
transfer, restraints and case/input revisions. A load-only rerun must reuse an
eligible original operator; setup, RHS/solve, assessment and visible-field costs
must be measured separately on the CPU baseline before claiming GPU speedup.
Follow it with actual print-setting/orientation comparison and save/reopen.

Enhanced uses those same cases/captures with detailed infill/interfaces,
higher-order refinement and admitted effective/reduced/offline reuse, not a
separate application or a permanent alternate placeholder engine. None of this
is conferred by case persistence. The [infill lab](../quality/infill-lab.md)
remains numerical research: converged algebraic residuals and analytical beam
controls alone do not qualify thin-road/crossing-road or whole-part fields.
There are no calibrated strength, breaking-load or machine-readiness claims.
