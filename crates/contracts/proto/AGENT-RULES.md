<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Protobuf schema rules

- Native API definitions live under spiling/: common primitives/artifacts/errors, geometry, projects, manufacturing and engine operation envelopes. Keep the import graph one-way; Google schemas under vendor remain unmodified with original licensing.
- Use typed request/reply oneofs and separate native/diagnostic operation metadata/results, standard google.longrunning.Operations and google.bytestream.ByteStream. Optional canonical nonnil request_id UUIDs follow AIP-155; sessions/{UUID} scopes native admission. Short domain errors use canonical Status with typed DomainErrorDetail, never reply error arms or opaque JSON.
- Manufacturing Verify requires an explicit VerificationOutput enum; unspecified/unknown values fail admission. Its typed verified result carries fresh report, captured inspection provenance, original bundle resource and optional SPLR descriptor. Packed motion bytes remain a separate immutable Artifact/ByteStream payload, never repeated protobuf segment messages.
- Preflight carries typed session/request/base-revision and bounded software-only evidence without artifacts; required failures refuse before unresolved obligations. Manufacturing11 captures explicit physical dimensions/policies, surface3 and process3 provenance; SPLR5 preserves actual commands plus nominal owner/dimension associations. Projects.Execute carries revision-bound source-scoped edits with required selector/action/value oneofs. Box-only permanent permission is a message oneof: separator wall or positive-thickness horizontal diaphragm, never an enum/string alias. ProjectInfo returns both metadata documents and truthful whole-volume base values. Missing kinds and unknown/unspecified enums fail. Image operations retain typed paths/mapping and exact metadata/resource bindings. No JSON command tunnels or pixels in inspection/status.
- ManufacturingSummary.native_geometry is required checked native-query coverage, including requested/achieved shape error and certified layer count. Required source-feature, skin and support scopes reject omitted/unspecified/unknown values; only certified coverage is valid for successful artifacts. Source-feature scope distinguishes disjoint placed source bounds from proved world-Z invariant source union, both with full actual occurrence query/slab coverage. Skin covers scheduled D material distance; support covers continuous target outer occupancy. This preserves actual-query provenance, not proof objects, emitted physical union-boundary certification or physical qualification.
- Keep binary artifact schemas separate from Protobuf. Generate consumers through contracts/build.rs; do not hand-edit generated native types.
- Engine and shell evolve together; do not add migration shims or version-negotiation infrastructure.
