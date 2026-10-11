<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Protobuf schema rules

- Native API definitions live under spiling/: common primitives/artifacts/errors, geometry, projects, manufacturing and engine operation envelopes. Keep the import graph one-way; Google schemas under vendor remain unmodified with original licensing.
- Use typed request/reply oneofs and separate native/diagnostic operation metadata/results, standard google.longrunning.Operations and google.bytestream.ByteStream. Optional canonical nonnil request_id UUIDs follow AIP-155; sessions/{UUID} scopes native admission. Short domain errors use canonical Status with typed DomainErrorDetail, never reply error arms or opaque JSON.
- Keep binary artifact schemas separate from Protobuf. Generate consumers through contracts/build.rs; do not hand-edit generated native types.
- Engine and shell evolve together; do not add migration shims or version-negotiation infrastructure.
