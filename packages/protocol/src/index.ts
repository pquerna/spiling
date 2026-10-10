/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
export * from "./generated.ts";
export { decodeTriangle, TriangleDecodeError } from "./triangle.ts";
export type { DiagnosticTriangle } from "./triangle.ts";
export { decodeMesh, MeshDecodeError, validateMeshManifest } from "./mesh.ts";
export type { DisplayMesh, MeshIdentity, MeshChunkIdentity } from "./mesh.ts";
export {
  decodeSection,
  SectionDecodeError,
  validateSectionSummary,
  validateSectionManifest,
} from "./section.ts";
export type { DisplaySection, SectionIdentity, SectionChunkIdentity } from "./section.ts";
