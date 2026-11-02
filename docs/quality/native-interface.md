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

Seam settings apply to the entire print job's closed constructed walls, not the
selected face. Nearest, Preferred direction and Spread starts share the existing
complete-intent draft and Apply settings action. Review's Seam starts controls
inspect fresh actual closed-wall starts; visibility and navigation never edit
the project. Counts follow selected layers, role/tool, playback and clipping,
excluding surrounding context. A start marker is not a cosmetic-face assignment,
minimum-separation guarantee or measured seal.

Try and Compare preserves the ordinary Settings → Apply → Review path. A separate
trial editor identifies its captured subject and says Current is unchanged.
Evaluate alternative computes a result; Use these settings deliberately adopts
the reviewed intent. Keep option is a separate project write, not activation.
Only one alternative draft is edited at a time. Ordinary Back retains it;
replacement/discard and project-close resolution are explicit. Saving does not
silently apply it. Read-only projects can inspect/evaluate, not keep or adopt.

Review's reference/candidate switch changes inspection only. Keep the canvas,
camera and compatible filters; distinguish physical height from layer index.
Input differences and actual result summaries have separate meanings. A stale
candidate stays visibly bound to its original revision/context and requires
reevaluation; cancellation/failure cannot relabel older output as the latest
trial. Options are named settings captures, not project versions or a second
Current. Ordinary Export remains Current-only regardless of the visible side.
Actual-motion controls scroll within half the available central height so expanded
inspection and playback retain a usable canvas. Start coordinates and command
details use progressive disclosure; counts and navigation remain above them.

Layer review keeps selected layers in color and shows other layers as dim neutral
actual-program deposition paths by default. Other layers can be hidden for isolated
inspection. Context respects role/tool visibility and section clipping, excludes
travel/pressure/lifecycle events, and does not enter selected stroke statistics or
section caps. Command playback advances only the selected layers; dim context shows
complete surrounding deposition, including later layers. It is display context,
not evidence that those commands have executed.

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
Reference discovery keeps Review, Next results and Enter manually outside the
scrolling result body. Up/Down moves a visible selection; Enter reviews only the
selected result for the current query and does not interrupt IME composition.
Catalog update checks are read-only; source differences open an explicit draft.
Back to project selection retains the acquisition destination, and requires saved
or discarded edits. Declared products can lack printable process profiles; show
that requirement at the action that needs it.
Match target separates closest individual filaments from layered reference recipes.
Selection opens a focused review with exact ingredients, goal difference, physical
cycle and missing process requirements. Keep captures a project result; Open study
then offers fresh comparison and explicit construction application. Search limits,
cancellation and unassessed evidence stay visible. A color goal miss is inspectable
and selectable best effort; process/geometry admission remains independent.
Private credentials clear on leaving their view; hiding details never implies a
connection was used. Unconfirmed library publications retain their original request
and require reconciliation before the application closes.

Activity details are dismissible. Closing the drawer never cancels work; cancellation
and resumed observation remain separately named actions. The status strip retains
problem counts and uncertain-save state. Commands show a selected result; Enter
executes that result. Closing/restarting the application requires deliberate selection
rather than becoming an incidental search default.

## Exercising a change

Use the production gallery's Panels, Fields, Color, Acquisition and Try / Compare categories in each theme.
The complete panel specimen includes ready, changed, blocked, pending, uncertain,
invalid, read-only and stale states; its close/reopen path is interactive. Inspect narrow/normal panes,
long names and scaled text. Test keyboard help, primary-action reasons, Escape,
focus restoration and uninterrupted numeric typing. Gallery values are illustrative
and cannot mutate a project.

Then exercise the real affected native flow: source selection/section, draft/apply,
receipt, stale-output invalidation, save/reopen/fresh verification/export and awaited
child cleanup as relevant. A snapshot or enabled accessibility feature is not
keyboard, screen-reader, packaged-platform or universal GPU qualification.

For Try and Compare, freshly verify Current in Program review, then choose
Try an alternative in Review or Prepare's Settings. Change the complete trial
settings, Evaluate alternative and switch between the fixed reference and checked
candidate. Exercise Keep option separately from Use these settings, Undo/Redo,
Current-only export while inspecting the candidate, and explicit trial discard.
After save and source-independent reopen, inspect the kept settings and evaluate
again; a stored option is not a freshly verified result.

[Development](development.md#native-egui-workbench) owns run commands and hardware
acceptance; [ADR006](../adr/006-native-egui-workbench.md) owns runtime architecture.
