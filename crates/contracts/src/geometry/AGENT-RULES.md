<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Geometry identity and provenance rules

- Own kernel/runtime-independent geometry DTOs and frozen support caps. No kernel, shell, renderer or transport-runtime dependencies; hashing and UUID minting are identity utilities only.
- Source/definition hashes are canonical lowercase SHA-256; source faces retain canonical decimal STEP entity strings, including values beyond JavaScript integer precision. Occurrence IDs and their next-free allocator persist in project state; session/artifact handles expire on restart. Native task IDs remain private engine correlation; public observation/cancellation uses durable Google Operations names. Counters/revisions never wrap.
- Geometry and rigid translations are mm/f64. Source unit and declared uncertainty remain independent provenance, not a claimed accuracy guarantee. Reject nonfinite/unordered bounds and nonunit poses; no scale, shear or reflection.
- `FaceInfo.geometry` describes native family, optional checked analytic mm metadata and bounded trim/chart summary independently of source face-use/surface entity kinds. It never authorizes a geometry or manufacturing operation. `orientation` is material-side direction relative to a regular native frame, including parameterization handedness and topology; singular normals require oriented limiting charts/fans.
- Deserialize identities, bounds, poses and provenance through checked constructors. Public native DTO fields require their explicit validation before publication; a FaceRef is valid only against live session/revision/occurrence/definition/source-face records.
- `DefinitionRecord.display` explicitly distinguishes certified Mesh artifacts from Unavailable bounded typed reasons. Native bounds/source-face tables exist independently of mesh availability; zero mesh bytes do not mean an empty native scene. Non-rendered definitions retain native list inspection but no GPU picking/artifact identity. Mixed scenes publish atomically through the same scene authority.
- Export domain DTOs/constant caps centrally through the generator, never handwritten TypeScript. `runtime.rs` owns geometry domain commands/replies and shared JobResult/JobError; native task IDs and job snapshots belong privately to engine execution, not this public API. Native workbench path admission belongs to its host and parented picker; checked NativePath preserves platform bytes/UTF-16 through gRPC. No browser token DTO belongs in this crate.
- `native_geometry.proto` owns typed Geometry Execute/ImportPart/StartSection; `native/geometry.rs` explicitly checks protobuf/domain conversion both ways. Long calls return Operation; domain failures use canonical Google/tonic Status, never a success reply error arm. No JSON tunnel or job/chunk transport commands.
- ArtifactPage resources correspond one-for-one, in order, to packed chunk metadata by size and SHA-256. Bound page lengths/offsets, reject mixed/duplicate/discontinuous records, and reapply finite coordinates, schema, support caps, rigid pose, analytic/domain description, section frame and loop validation on wire decode before publication.
- Exercise consumer-visible typed dispatch, rejection, resource correspondence, identity, unit, frame, stale-reference and native-path fidelity with `cargo test -p spiling-contracts`. Full packed byte/hash/layout validation remains owned by the sibling display module; packed schemas and fixtures do not change with protobuf.
