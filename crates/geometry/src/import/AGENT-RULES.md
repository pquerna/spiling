<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Declared STEP admission rules

Parse UTF-8 once through maintained step-p21 after source/entity lexical caps; require one DATA section and declared AP203/AP214 profile. Qualified schema OIDs must match the named ISO/AP family and contain positive edition/conformance arcs; do not assume a fixed three-arc suffix. Corresponding pcurve representation contexts are metadata, not additional carrier support. Resolve referenced length/radian units and uncertainty in that AST. Refuse assemblies, unsupported reachable geometry, duplicate/missing canonical face uses and reachable conversion loss. Check remaining native-face capacity before conversion, then normalize through public modeling transforms and require valid closed solid plus certified bounds.

`mod.rs` owns native admission and positional source/native face assertions; `ast.rs` owns bounded lexical/AST traversal; `units.rs` resolves only solid-referenced representation contexts. Every normalized definition includes exact bounded line/circle witnesses plus actual native vertices for sections. Circular witnesses precede modeling's rational-curve conversion and are not sampled. `FaceInfo.orientation` is the effective native direction relative to the declared plane normal or outward cylinder radial normal, including processor handedness; inward hole faces are false. No source paths or session/artifact identities belong here.

Exercise `cargo test --locked -p spiling-geometry` and the real native corpus smoke; see [geometry evidence](../../../../docs/quality/geometry-evidence.md).
