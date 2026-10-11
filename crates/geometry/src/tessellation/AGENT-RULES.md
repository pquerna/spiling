<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Strict native display conversion rules

Mesh admitted faces one at a time through the pinned public compressed-shell mesher; preserve source-face order at compression and output boundaries and reject every absent/empty/non-triangle face. Use 0.025 mm mesher tolerance, triangles and two search trials. Audit EVERY triangle: planes use vertex signed distances; cylinders use maximum projected vertex radius and minimum projected triangle distance to the axis, including edges/interior. Mesher tolerance and vertex-only curved checks are not certificates.

Cached FaceInfo.orientation is relative to its canonical carrier (outward radial for cylinders), not the kernel topology flag. Derive analytic normals from that carrier/sign and validate against native surface normals plus native topology orientation; inward hole normals must remain inward. Reject malformed/nonfinite attributes/indices, nonunit normals, degenerate triangles and reversed winding rather than repairing them.

Adapt/remap only the current face, share identical position/normal values, and transfer it to contracts' bounded identity-free chunk builder. Packed reconstruction displacement must be ≤0.025 mm and carrier plus quantization ≤0.05 mm. Engine callers supply remaining mesh capacity; enforce narrowed per-face/aggregate vertex, triangle and byte budgets during audit/packing. Discard partial output on cancellation/error. Never flatten/concatenate the solid or mint session/artifact identities. Check cancellation around upstream calls, between faces and during triangle auditing.

Exercise `cargo test --locked -p spiling-geometry` and the real native corpus smoke; see [geometry evidence](../../../../docs/quality/geometry-evidence.md).
