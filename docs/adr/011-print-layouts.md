<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# ADR011: Independent print layouts in one project

Status: accepted.

A project may contain several print jobs using the same imported geometry. Each job
needs independent membership, setup, settings and output. A renderer-only collection
of beds cannot supply that authority, and separate project files duplicate sources
and lose transactional transfers.

Core therefore owns an ordered, bounded collection of named print layouts. Each has
a stable UUID and owns its occurrences, optional portable hardware capture,
manufacturing intent, named options and current artifact. Source definitions and
canonical images remain shared project assets. Moving preserves an occurrence's ID
and local pose; copying allocates new occurrence and occurrence-owned contribution
IDs. Both operations are single undoable transactions. There is no second top-level
manufacturing state.

The interface calls these jobs **plates**. Physical sheets remain personal hardware;
adding a project plate does not create physical inventory. Focus is transient and
read-only navigation does not change project revision or saved content. Manufacturing,
hardware and option commands explicitly address the selected layout. Focus cannot
switch during an admitted engine job. Scene epochs expire on focus/membership
changes; old picks, sections and arrangement proposals cannot authorize another job.

Compilation captures only the addressed layout's occurrences, referenced definitions
and applicable shared/occurrence contributions. Core preserves another layout's
artifact only when its complete scoped semantic fingerprint still matches. Shared
definition edits can invalidate several jobs. This is input freshness, not fresh
verification: independent actual-program replay remains required before delivery.
Every retained job's program is independently inspected during native reopen.

The native navigator and explicit move/copy sheets use the same Projects service as
the CLI. Draft navigation requires deliberate resolution, and plate edits close on
matching engine receipts. Per-plate cameras/section settings are bounded transient
state; one current replay is decoded at a time. Overview positions must never become
machine coordinates.

Quantity includes the selected original. The explicit **Use additional plates**
choice extends the same bounded placement service to new single-tool jobs using the
current setup; Arrange and Fill remain current-plate operations. Every proposed
job passes ordinary compilation, independent replay and construction containment.
One aggregate candidate budget and the project's membership/plate limits bound the
search. The proposal accounts for the complete requested quantity and any shortfall;
it does not promise optimal packing. One Core transaction applies all placements
and creates the additional jobs, with fresh occurrence/contribution IDs and shared
immutable assets. Undo reverses the whole distribution. Trial programs are not
adopted: each resulting job still requires ordinary preparation and fresh inspection
before export.

Limits and schemas live in [project contracts](../protocol/project.md). Fixed-dual
occurrence-bound routes refuse transfer/duplication until assignment remapping is
qualified. No automatic printer dispatch, swept-tool clearance or physical print
qualification follows from a successful layout or software replay.
