<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native interface conventions

The workbench presents source geometry, authored settings and independently decoded
programs through one native UI. `apps/workbench/src/ui` owns presentation mechanics;
callers own scope, enablement and transient drafts. Existing commands, engine
transactions and observed receipts retain mutation authority.

## Surface anatomy

Every surface has a title, subject/scope, grouped body, one clear next action and a
visible close/back path. The persistent workbench home is the return destination,
not another temporary dialog. Clear selection ends contextual part inspection. Pane titles are subordinate to page/dialog titles; group
labels are subordinate to pane titles. Preserve Graphite/Paper, readable secondary
text and the canvas. Use aligned fields and whitespace before adding containers. Cap a form to the
lesser of its reading width and available space; setting a fixed maximum must
never expand a narrow parent. Preserve field identities across reflow and state
changes.

Use shared `pane_header`, `group`, `text`, `number`, `percent`, `primary`, `help` and `problem`
components with their real consumers. Primary actions use the common blue/white
treatment; unavailable primary actions show their reason without requiring hover.
Do not infer readiness or duplicate engine validation to choose a visual action.

Labels use sentence case and concise operator terms. Aim for one to four words;
meaning, units and localization take priority over word counts. Optional short
hints explain one control. Longer explanations use a deliberately opened help or
named details surface. Essential scope, errors, save uncertainty, incompleteness,
current verification and consequential qualification remain visible.

## Actions and exits

Close ends a view; Discard draft abandons an edit; Cancel operation requests native
cancellation and awaits its outcome. These actions are never interchangeable.
Collapsing a property group does not discard its draft. Mutations remain pending
until the matching receipt, rather than closing on request acknowledgement.

Section analysis is a nonmodal side pane. Switching workflow tabs retains its
active cut. Close analysis ends clipping/following while retaining plane parameters
and project-keyed saved views; it does not edit the project or close the engine.
Native source contours remain distinct from approximate display caps and nominal
program beads. Native query refusals do not grant alternative geometry authority.

Help opens by pointer or keyboard activation of its named About control, with F1
also available when that control has focus. Escape closes help before its parent;
explicit Close help restores focus. View exits do not consume editing shortcuts or
Escape belonging to a higher modal/popup. Keep labels associated with controls and
widget identities stable when errors or optional rows appear.

Problem summaries remain short; exact diagnostic text stays selectable in Details.
Preserve bounds, codes and witnesses instead of truncating the underlying evidence.
Never invent a native cause or remedy. Software verification remains distinct from
physical qualification; **NOT MACHINE READY** belongs in Review/export context.

Prepare separates setup, material, settings, placement and infill into named tasks.
Applying a settings draft still commits the complete intent. Infill edits display
percentages while retaining the engine fraction contract. Check/Compile describe
committed inputs; unapplied drafts stay labelled. Review promotes Verify program,
then Export after current passing inspection. Qualification and stale state remain
visible beside the action.

Inspectors keep task navigation and their action strip outside the body scroll.
Back to Inspect hides an ordinary draft; replacing its source face/part or infill
scope asks whether to keep editing or discard. Project replacement still uses the
engine's independent dirty-state confirmation after resolving local drafts.

Library, Color study and long reference content (gallery/license) use focused pages with a stable Back to workbench action.
Filament/spool/recipe and hardware editors have explicit return routes. Ordinary
Back retains draft values and their destination; selecting a different edited
subject cannot silently replace them. Copy to project, Load study, Compare, Accept
selection and Apply construction remain distinct native operations. Short names
are the normal reading layer; exact identities and declarations remain inspectable.
Material catalog review has its own Library page: Use profile stages a draft,
then returns to Material for Apply profile. A catalogue selection is not a write. Its staged draft captures project/session
identity even when Material has not been visited. Apply or explicitly discard a
changed draft before selecting another profile or loading another intent.
Private credentials clear on leaving their view; hiding details never implies a
connection was used. Unconfirmed library publications retain their original request
and require reconciliation before the application closes.

Activity details are dismissible. Closing the drawer never cancels work; cancellation
and resumed observation remain separately named actions. The status strip retains
problem counts and uncertain-save state. Commands show a selected result; Enter
executes that result. Closing/restarting the application requires deliberate selection
rather than becoming an incidental search default.

## Exercising a change

Use the production gallery's Panels, Fields and Color categories in each theme.
The complete panel specimen includes ready, changed, blocked, pending, uncertain,
invalid, read-only and stale states; its close/reopen path is interactive. Inspect narrow/normal panes,
long names and scaled text. Test keyboard help, primary-action reasons, Escape,
focus restoration and uninterrupted numeric typing. Gallery values are illustrative
and cannot mutate a project.

Then exercise the real affected native flow: source selection/section, draft/apply,
receipt, stale-output invalidation, save/reopen/fresh verification/export and awaited
child cleanup as relevant. A snapshot or enabled accessibility feature is not
keyboard, screen-reader, packaged-platform or universal GPU qualification.

[Development](development.md#native-egui-workbench) owns run commands and hardware
acceptance; [ADR006](../adr/006-native-egui-workbench.md) owns runtime architecture.
