<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Engine geometry ownership rules

- Compose Core transactions and the public geometry/manufacturing facades; never depend directly on Monstertruck or maintain competing authoritative project state. Native command/result slots have capacity one. Native objects stay on the sole worker; private execution correlations are not another public job service.
- Stage import/open and reserve live pins before Decide/promote. Commit prepared Core state only after worker ACK. Retain the complete old project, storage and history/native pins until replacement succeeds; cancellation or stale/failing work preserves them. Reclaim against the latest replaceable pin snapshot at safe boundaries.
- Core retained sources drive definition/display pins, including undo/redo; only its current snapshot drives scene pages and live face references. Definition pins and consumer leases are independent. Manufacturing publication/history must not invalidate unchanged geometry caches.
- Native bed preparation and manufacturing admission use geometry-owned, source/recipe-bound material support bounds. Check exact definition, pose and query bindings; Core alone applies the prepared pose. Raw source/display bounds and material bounds are distinct authorities; neither establishes closed-material compilation eligibility.
- Section source preparation shares 60 MiB, including retained proofs. Prepared sources, live query results and builder capacities coexist within 64 MiB, independently of the 16 MiB packed-output cap. Subtract retained capacities before queries/growth; release a source after its last occurrence. Geometry owns pose pullback, metric errors and canonical world loops; meshes never supply section authority.
- Import admission separately counts old/history and incoming source allocations, even duplicates, plus pinned proof/index capacity within 64 MiB. Reserve remaining proof capacity before native import growth. Replacement may fail before old state can be released; these admission counters are not whole-kernel RSS bounds.
- Image work captures remaining 4 MiB canonical headroom, including prepared candidates, before normalization. Duplicate content still requires incoming allocation headroom. Core rechecks it at promotion. Finish edits preserve scene/display state; every open validates stored targets against the actual rebuilt/reused face table.

Parent engine rules own RPC, watchdog, save and durable-publication semantics. [Manufacturing ownership](manufacturing/AGENT-RULES.md), [wire authority](../../../../docs/protocol/control.md) and real-child geometry/project/manufacturing checks govern integration; source-wiring assertions do not.
