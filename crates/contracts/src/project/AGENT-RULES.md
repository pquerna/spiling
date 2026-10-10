<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Durable project contract rules

- Own checked persistent project identity/revisions, bounded manifest/source references and typed project commands/errors. No storage, kernel, shell, UI or transaction implementation.
- ProjectRevision is persistent authoring history order and never rewinds on undo. SceneRevision is session-scoped publication order; save changes neither. Session/handle identities and packed artifacts are never manifest authority.
- Format 2 stores exact-source provenance, rigid occurrences, explicit optional manufacturing intent/artifact records in normalized mm/right-handed coordinates. Validate schema, duplicate/reference/counter/identity/pose/provenance, resource bounds and current semantic input hash. Artifact records require intent; optional fields must appear explicitly as values or null.
- Project commands use protocol v4 with generated TypeScript. No older-format/protocol adapters. Project errors remain typed recoverable domain errors; asynchronous work shares EngineJob and tagged geometry/project/manufacturing JobError.
- Core owns filesystem integrity, writer locks, transactions and recovery; engine owns native admission and atomic scene publication. A manifest schema does not establish durable storage or geometry support.
