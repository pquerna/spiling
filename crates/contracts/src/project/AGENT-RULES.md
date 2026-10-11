<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Durable project contract rules

- Own checked persistent project identity/revisions, bounded manifest/source references and typed project commands/errors. No storage, kernel, shell, UI or transaction implementation.
- ProjectRevision is persistent authoring history order and never rewinds on undo. SceneRevision is session-scoped publication order; save changes neither. Session/handle identities and packed artifacts are never manifest authority.
- Format 2 stores exact-source provenance, rigid occurrences, explicit optional manufacturing intent/artifact records in normalized mm/right-handed coordinates. Validate schema, duplicate/reference/counter/identity/pose/provenance, resource bounds and current semantic input hash. Artifact records require intent; optional fields must appear explicitly as values or null.
- Project commands use checked typed Projects protobuf adapters and generated TypeScript shell views. No framed control or older-format shims. Project errors remain typed recoverable domain errors in Status details; asynchronous Open/Save use Google Operations with separate native metadata and tagged domain JobError. ProjectInfo dirty state is semantic content equality, not revision equality.
- Core owns filesystem integrity, writer locks, transactions and recovery; engine owns native admission and atomic scene publication. A manifest schema does not establish durable storage or geometry support.
