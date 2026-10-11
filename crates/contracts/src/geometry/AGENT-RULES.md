<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Geometry identity and provenance rules

- Own kernel/runtime-independent geometry DTOs and frozen support caps. No kernel, shell, renderer or transport-runtime dependencies; hashing and UUID minting are identity utilities only.
- Source/definition hashes are canonical lowercase SHA-256; source faces retain canonical decimal STEP entity strings, including values beyond JavaScript integer precision. Occurrence IDs and their next-free allocator persist in project state; session/artifact handles expire on restart. Native task IDs remain private engine correlation; public observation/cancellation uses durable Google Operations names. Counters/revisions never wrap.
- Geometry and rigid translations are mm/f64. Source unit and declared uncertainty remain independent provenance, not a claimed accuracy guarantee. Reject nonfinite/unordered bounds and nonunit poses; no scale, shear or reflection.
- `FaceInfo.orientation` is the effective native face direction relative to its carrier: plane normal or outward cylinder radial. It includes kernel parameterization handedness and topological orientation; an inward hole wall is false. It is not necessarily the raw kernel face flag.
- Deserialize identities, bounds, poses and provenance through checked constructors. Public native DTO fields require their explicit validation before publication; a FaceRef is valid only against live session/revision/occurrence/definition/source-face records.
- Export shell DTOs/constant caps centrally through the generator, never handwritten TypeScript. `runtime.rs` owns geometry domain commands/replies and shared JobResult/JobError; native task IDs and job snapshots belong privately to engine execution, not this public API. Production shell selection remains token-only; checked NativePath is native gRPC input preserving platform bytes/UTF-16.
- `native_geometry.proto` owns typed Geometry Execute/ImportPart/StartSection; `native/geometry.rs` explicitly checks protobuf/domain conversion both ways. Long calls return Operation; domain failures use canonical Google/tonic Status, never a success reply error arm. No JSON tunnel or job/chunk transport commands.
- ArtifactPage resources correspond one-for-one, in order, to packed chunk metadata by size and SHA-256. Bound page lengths/offsets, reject mixed/duplicate/discontinuous records, and reapply finite coordinates, schema, support caps, rigid pose, carrier, section frame and loop validation on wire decode before publication.
- Exercise consumer-visible typed dispatch, rejection, resource correspondence, identity, unit, frame, stale-reference and native-path fidelity with `cargo test -p spiling-contracts`. Full packed byte/hash/layout validation remains owned by the sibling display module; packed schemas and fixtures do not change with protobuf.
