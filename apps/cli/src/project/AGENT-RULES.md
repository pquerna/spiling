<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Project command workflow rules

- Own ordered operator argument validation and orchestration through EngineClient only. Core/engine owns identities, transactions, history, storage and native geometry; never read/write project manifests or assets here.
- Validate poses, IDs, flag ordering, bounded inert intent JSON and read-only workflow restrictions before child spawn. Preserve OsString project/source/intent/output paths; labels are not filesystem references. Ordered --intent/--compile use the manufacturing service, while --verify and --manufacturing-out allow read-only replay/export. Rust engine validation owns manufacturing acceptance.
- Observe authoritative ProjectInfo after each operation. Create/edit checkpoint their final state; open does not implicitly save, inspect is read-only, recovery is explicit. Save never clears session history or implies recovery of arbitrary unsaved edits.
- Use shared geometry inspection/transfer helpers and checked project/manufacturing/operation/error contracts. Observe native work through Google Operations names and the existing client polling lifecycle, not private native task IDs. Fetch immutable descriptor-bound, bounded/hash/input/schema/summary-validated bundles through EngineClient/ByteStream after independent engine replay; no alternate transport, decoder, planner, store or watchdog. Export plan/program/the exact fresh verification/provenance through the exclusive output transaction, identify the exported content hash, and label software_only/not_machine_ready without implying physical readiness.
- On save failure reconcile project status without retry, preserve dirty/save_uncertain evidence and let the owning CLI reap the child before terminal stderr output. Recoverable manufacturing failures also reconcile authoritative project/intent/artifact status before cleanup. No success report or output completion manifest for uncertain/failed publication.
- Exercise actual separate CLI processes, source deletion/reopen, ordered undo/redo/checkpoints, native sections/faces and failure cleanup. Canonical commands: [development](../../../../docs/quality/development.md#real-engine-and-cli); storage semantics: [project format](../../../../docs/protocol/project.md).
