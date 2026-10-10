<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Geometry identity and provenance rules

- Own kernel/runtime-independent geometry DTOs and frozen support caps. No kernel, shell, renderer or transport-runtime dependencies; hashing and UUID minting are identity utilities only.
- Source/definition hashes are canonical lowercase SHA-256; source faces retain canonical decimal STEP entity strings, including values beyond JavaScript integer precision. Occurrence IDs and their next-free allocator persist in project state; session/job/artifact handles expire on restart. Counters/revisions never wrap.
- Geometry and rigid translations are mm/f64. Source unit and declared uncertainty remain independent provenance, not a claimed accuracy guarantee. Reject nonfinite/unordered bounds and nonunit poses; no scale, shear or reflection.
- `FaceInfo.orientation` is the effective native face direction relative to its carrier: plane normal or outward cylinder radial. It includes kernel parameterization handedness and topological orientation; an inward hole wall is false. It is not necessarily the raw kernel face flag.
- Deserialize identities, bounds, poses and provenance through checked constructors. Public native DTO fields require their explicit validation before publication; a FaceRef is valid only against live session/revision/occurrence/definition/source-face records.
- Export every DTO/constant centrally through the generator, never handwritten TypeScript. `runtime.rs` owns strict v4 geometry/artifact controls and common EngineJob/tagged geometry/project/manufacturing JobError metadata; sibling project/manufacturing areas own their DTOs. Production shell paths remain token-only; NativePath is private local-pipe input.
- Exercise consumer-visible identity, JSON rejection, unit, frame, stale-reference and native-path fidelity tests with `cargo test -p spiling-contracts`. Layout validation is owned by the sibling display module.
