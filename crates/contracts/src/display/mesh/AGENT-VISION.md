<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Packed mesh contract vision

Provide one deterministic bounded representation of definition-local display meshes, reusable by native geometry, engine publication and independently checked clients. Packed ownership transfers one chunk at a time; repeated occurrences reuse definition geometry rather than creating another native or wire model.

Producer description and ephemeral publication identity are separate. Complete trimmed-native triangle-chain coverage and separate interpolation/trim accuracy remain the native producer's obligation, while packing certifies reconstruction displacement and checked metadata retains the achieved combined display budget. The trimmed display profile changes descriptor semantics, not SPLM1 byte layouts. Descriptor admission never establishes manufacturing authority or GPU/desktop support.
