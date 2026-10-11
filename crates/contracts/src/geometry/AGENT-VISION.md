<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Geometry contract vision

Provide durable content/source identity and bounded, revision-aware scene and display metadata for a native geometry facade and independently validating clients. Geometry definitions are immutable; flat rigid occurrences share them. There are no project persistence, hierarchy import or manufacturing models here.

The contract layer describes normalized mm data, provenance, face carriers and bounded artifacts behind a fully typed protobuf Geometry service. Short actions return checked success DTOs; imports and sections use standard Google Operations, while immutable chunks are retrieved through Artifact descriptors and ByteStream. Rust/TS shell views remain transport-independent; internal task correlation is not a public job API. Durable authoring and software manufacturing contracts live in sibling areas; none implement storage or native algorithms. Support and accuracy require recorded corpus, durability and real-client/display workflows, not schema declarations.
