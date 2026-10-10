<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Engine geometry ownership rules

Own project/geometry/manufacturing job dispatch, derived native metadata/artifacts, source capture and the single kernel worker. Compose core Project transactions/storage and the public manufacturing compiler/verifier; never maintain a competing authoritative occurrence/intent/artifact map or allocator. Depend on contracts, core and public geometry/manufacturing facades, never Monstertruck directly. Worker command/result slots have capacity one; only one active job exists. Metadata and packed bytes move out without copying complete artifacts; native objects never cross the boundary.

Import/open definitions are staged until Decide/promote and worker ACK. Reserve private native live pins before promotion, but commit prepared core edits or replace the complete project only after ACK. Keep old project/storage/native/history pins while open stages. Cancellation wins before the decision, not afterward; failed/stale/cancelled stages preserve prior state. Reclaim native objects at safe boundaries against the latest replaceable snapshot, counting old/history and staged resources.

Core retained_sources drives bounded definition and mesh pins, including undo/redo. Current snapshot alone drives scene pages/bounds and face references. Definition pins and consumer leases are independent; final current removal retains history resources until history pruning or project replacement. Released-handle tombstones and terminal jobs are bounded; jobs never retain encoded results. Save/open block authoring, while other pre-promotion native jobs can become stale through transactions.
Manufacturing jobs capture persistent ProjectRevision independently of scene epochs; recipe/artifact publication and their undo/redo preserve geometry caches. Native resident definitions use immutable worker-owned Arc pins for the public compiler. Every open replays the loaded bundle before publication. Separate manufacturing byte budgets count old/current/history and staged assets before allocation/growth; raw FrameKind 4 chunks remain session-associated and immutable. See manufacturing/AGENT-RULES.md.

All placements and native queries are mm/f64. Native section reuse is job-local and requires identical DefinitionId plus exact canonical local plane equation. Compute placed samples directly relative to the world plane origin (rotate local point + translation minus origin), then canonicalize and encode separate occurrence loops with source DefinitionId. Never add a large world offset only to subtract it during ordering or packing. Display artifacts are approximations, never native section input.

Use strict shared control DTOs; structurally valid bad pose/plane values return domain errors without weakening unknown-field/control rejection. See the parent rules and [wire authority](../../../../docs/protocol/control.md). Real child regressions, not source-wiring assertions, govern behavior.

Open/save are asynchronous worker operations using captured immutable core sources/checkpoints. Exact imported Vec bytes transfer into SourceAsset; never re-read external import paths for reopen. Open admission counts retained old/history plus captured incoming source allocations against 64 MiB, including identical IDs, and old plus staged native/artifact budgets; it may reject a replacement that would fit only after dropping the old state. Worker save uses a job-bound shared watchdog commit token before manifest replacement; committed receipts publish even after cancellation. Same-path writer reopen reuses retained Storage safely; read-only takes no new writer lock and cannot author/save. Post-publication durability errors attach the truthful committed checkpoint and return typed failure, not successful durability or cancellation.
