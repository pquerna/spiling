<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Protobuf schema rules

- Native API definitions live under spiling/: common primitives/artifacts/errors, geometry, projects, manufacturing and engine operation envelopes. Keep the import graph one-way; Google schemas under vendor remain unmodified with original licensing.
- Use typed request/reply oneofs and separate native/diagnostic operation metadata/results, standard google.longrunning.Operations and google.bytestream.ByteStream. Optional canonical nonnil request_id UUIDs follow AIP-155; sessions/{UUID} scopes native admission. Short domain errors use canonical Status with typed DomainErrorDetail, never reply error arms or opaque JSON.
- Manufacturing Verify requires an explicit VerificationOutput enum; unspecified/unknown values fail admission. Its typed verified result carries fresh report, captured inspection provenance, original bundle resource and optional SPLR descriptor. Packed motion bytes remain a separate immutable Artifact/ByteStream payload, never repeated protobuf segment messages.
- Manufacturing Preflight uses a typed session/request/base-revision request and native operation result with bounded software-only checks, optional checked occurrence/STEP face attribution and no artifact. Manufacturing schema4 requires process/support policies and captured ordered surface provenance; SPLR stays3 with process_count. Projects.Execute adds typed revision-bound source-face finish commands and always returns the full typed document in ProjectInfo; missing/unknown kinds fail rather than invent defaults or tunnel JSON.
- Keep binary artifact schemas separate from Protobuf. Generate consumers through contracts/build.rs; do not hand-edit generated native types.
- Engine and shell evolve together; do not add migration shims or version-negotiation infrastructure.
