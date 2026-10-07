<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Protobuf schema rules

- Native API definitions live in spiling/engine.proto; Google schemas under vendor remain unmodified with original licensing.
- Use typed operation metadata/results, standard google.longrunning.Operations and google.bytestream.ByteStream. An optional request_id follows AIP-155.
- Keep binary artifact schemas separate from Protobuf. Generate consumers through contracts/build.rs; do not hand-edit generated native types.
- Engine and shell evolve together; do not add migration shims or version-negotiation infrastructure.
