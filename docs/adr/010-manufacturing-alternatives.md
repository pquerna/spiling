<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# ADR010: explicit manufacturing alternatives

## Decision

Keep one applied manufacturing configuration, called **Current**. Add an explicit
alternative draft that can be evaluated without applying it, compared with a
fixed reference, retained as a named option and deliberately adopted. Preserve
the ordinary Settings → Apply → Review path for operators who do not need an
experiment.

These concepts have separate meanings:

- A **draft** is unapplied editor state. Ordinary navigation retains it.
- An **option** is immutable named manufacturing intent with an exact context
  binding. Keeping or removing one changes project content, not Current inputs.
- A **reference** is a captured configuration/result used for comparison. It is
  not a second active project.
- A **result** belongs to exact captured inputs and a producer. It is not editable
  intent or manufacturing authorization for another configuration.
- **Undo/Redo** records bounded session transactions, not a version browser.
- **Save** makes project content durable. It never implicitly applies drafts.

## Authority and boundaries

Core owns validated options, context binding, history, dirty state and adoption.
An option's content-derived identity binds its name, context and complete intent.
The context covers source definitions/provenance, occurrence poses, surface and
process construction, canonical image content and captured materials/hardware.
It excludes active intent, compiled artifacts, other options and unrelated
appearance studies. Old-context options remain inspectable; they cannot silently
rebase onto new geometry or setup.

Evaluation uses a detached checked snapshot sharing immutable assets with Current
and clearing the active artifact. It does not create another project attachment,
engine, writer lease or undo stack. The existing Jobs/worker performs compilation
and independent actual-program inspection. A result returns exact intent and
context, fresh verification, checked bundle/replay descriptors and a bounded
descriptive input diff. A failed/canceled trial cannot replace a valid result or
publish into Current.

Project status carries small option summaries. Full option retrieval is explicit
and revision-bound; complete intents must not multiply beyond the bounded RPC
envelope. Retained options, manifests, prepared/history ownership, terminal
receipts, artifacts and display memory all retain their separate limits.

Options are ordinary required domain content in the PTAC root. PTAC's current and
previous saved roots remain crash recovery; they are not user versions or a
persistent branch DAG. Numerical unreleased formats remain stable and older
development documents require recreation, without readers or defaults.

## Interaction contract

Prepare owns authoring; Review owns read-only comparison. A trial explicitly says
that Current is unchanged. Evaluation and adoption are different actions. One
active trial editor is enough; inspecting another option does not implicitly apply
it or replace a changed draft. The candidate retains its original project,
revision and context binding. State changes require explicit reevaluation rather
than automatic adoption or guessed rebase.

Use one canvas with explicit reference/candidate switching initially. Preserve
camera and compatible inspection filters. Do not equate layer indices between
different physical schedules. Input differences, actual starts, physical
intervals and available material/command summaries explain the result; truncation
or unavailable metrics remain visible. A nominal measurement is not a physical
strength, seal, collision or machine-readiness claim.

Adoption is one revision-bound intent transaction and remains undoable. Keeping
an option is separately explicit. Export remains bound to Current's fresh checked
inspection: browsing a candidate must never redirect ordinary export. Read-only
projects can inspect/evaluate, not retain, remove or adopt.

## Alternatives and consequences

Do not implement experiments by temporarily applying and undoing Current, or by
opening hidden project copies. Those approaches conflate history, publication,
stale results and export authority. A generic branch/merge system is unnecessary
for bounded recipe alternatives. Save As remains a file operation, not an
experiment manager; whole-project milestones and independent duplicates are
separate possible products.

No new algorithm crate is justified by this workflow. Core, existing native
services, manufacturing and the checked renderer retain their existing ownership.
New regional construction and deposition mathematics need their own algorithm and
qualification decisions.

## Acceptance boundary

Exercise Current → trial → fresh comparison → keep → adopt → Undo/Redo → save →
source-independent reopen → fresh reevaluation through native and CLI consumers.
Protect stale/foreign context, failed/canceled evaluation, bounded ownership,
read-only inspection, navigation/draft retention and Current-only export. Actual
window interaction must distinguish what is applied, what is inspected and what
Save preserves. Headless host tests alone do not establish that usability,
packaged-platform behavior or physical manufacturing performance.
