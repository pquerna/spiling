<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Protocol package vision

This dependency-free package gives TypeScript consumers Rust-generated control contracts and validated packed display buffers. The engine remains authoritative; the package owns neither planning nor rendering.

B0 supports exactly the version-one SPLT diagnostic payload and generated bounded control metadata. Its acceptance boundary is cross-language byte compatibility plus rejection of malformed, oversized, misaligned, nonfinite, and out-of-range inputs before GPU upload. Future mesh schemas must declare identity, revision, bounds, units, and provenance through canonical native contracts rather than silently expanding this diagnostic decoder.
