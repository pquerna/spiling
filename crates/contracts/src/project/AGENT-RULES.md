<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Durable project contract rules

- Own checked persistent project identity/revisions, bounded manifest/source references and typed project commands/errors. No storage, kernel, shell, UI or transaction implementation.
- ProjectRevision is persistent authoring history order and never rewinds on undo. SceneRevision is session-scoped publication order; save changes neither. Session/handle identities and packed artifacts are never manifest authority.
- Format8 stores exact-source provenance, rigid occurrences, ordered whole-face surface overlays, a bounded process document and allocator high-water, plus explicit optional manufacturing intent/artifact records in normalized mm/right-handed coordinates. Process IDs never rewind. The sole sorted images table references checked canonical images/<hash>.pgm assets, including disabled layers. Validate schemas, identities/references/counters/poses/provenance, budgets and current semantic input hash. Artifact records require intent; optional fields appear as values or null. No older-format reader.
- Project commands use checked typed Projects protobuf adapters and generated TypeScript shell views. No framed control or older-format shims. Project errors remain typed recoverable domain errors in Status details; Open/Save/ImportSurfaceImageFinish/GetSurfaceImage use Google Operations with separate native metadata and tagged domain JobError. Image import returns atomic status/layer/metadata; read-only canonical image retrieval returns captured identity/revision plus exact published descriptor without revision advancement. ProjectInfo dirty state is semantic content equality, not revision equality.
- Surface mutations use persistent ProjectRevision and native FaceRef; engine validates live native identity before Core receives structural persistent targets. ProjectInfo always returns the full bounded surface document. Metadata mutation invalidates manufacturing without republishing the scene.
- Process edits carry persistent ProjectRevision and checked SurfaceScope identity, never FaceRef or SurfaceFinish. ProjectInfo returns the authored document plus authoritative whole-volume base, inherited definition and global recipe values when intent exists; these rows do not claim uniform spatial density. Clients read authored boxes, not a second spatial resolver. Reset removes every exact-scope layer, including disabled/erased boxes; history restores the complete document.
- SetSparseFraction updates only an exact-scope WholeOwnedVolume winner. SetBoxSparseFraction updates the exact-scope winning box, including changed bounds and explicit nullable permission, or allocates one contribution. Repeated adjustments preserve layer/operation IDs. Explicit AddLayer remains ordered document authoring, not the normal Apply action.
- Core owns filesystem integrity, writer locks, transactions and recovery; engine owns native admission and atomic scene publication. A manifest schema does not establish durable storage or geometry support.
