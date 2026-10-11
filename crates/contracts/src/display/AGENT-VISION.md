<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Packed display layout vision

Supply deterministic, bounded, face-mapped mesh chunks and whole-loop section chunks without kernel or renderer coupling. Mesh production is definition-scoped and identity-free; engine publication attaches ephemeral session/artifact identities without changing encoded bytes or the wire DTO. Clients independently validate exact wire bytes and borrow their scalar views; encoded ownership transfers by chunk, not by an additional complete model copy.

SPLM/SPLS bytes are shared by typed native geometry services, immutable artifact retrieval and checked clients without a transport-dependent layout change. Original algebraic interoperability fixtures exercise multiple source faces, loops and chunks; native-derived goldens and numerical native/desktop gates establish separate evidence. Codec availability or an RPC declaration alone does not establish native correctness or GPU support.
