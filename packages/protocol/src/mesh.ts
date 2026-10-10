/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import {
  MAX_DISPLAY_TRIANGLES,
  MAX_DISPLAY_VERTICES,
  MAX_GEOMETRY_CHUNK_BYTES,
  MAX_NATIVE_FACES,
  MAX_SCENE_MESH_BYTES,
  MESH_CARRIER_TOLERANCE_MM,
  MESH_HEADER_BYTES,
  MESH_PROFILE,
  MESH_QUANTIZATION_TOLERANCE_MM,
  MESH_SCHEMA_VERSION,
  MESH_SURFACE_TOLERANCE_MM,
  NORMAL_NORM_TOLERANCE,
} from "./generated.ts";
import type { MeshChunkMetadata } from "./generated.ts";
import {
  bounds,
  checkedInput,
  hash,
  keys,
  session,
  size,
  uint,
  verifyHash,
  zero,
} from "./display.ts";
import type { PackedInput } from "./display.ts";

export type MeshIdentity = Pick<MeshChunkMetadata, "session_id" | "artifact_id" | "definition_id">;
export type MeshChunkIdentity = MeshIdentity & Pick<MeshChunkMetadata, "chunk_index">;

export interface DisplayMesh {
  readonly positions: Float32Array<ArrayBuffer>;
  readonly normals: Float32Array<ArrayBuffer>;
  readonly indices: Uint32Array<ArrayBuffer>;
  readonly faceOrdinals: Uint32Array<ArrayBuffer>;
  readonly localOriginMm: readonly [number, number, number];
  readonly chunkIndex: number;
  readonly byteLength: number;
  readonly schema: number;
}

export class MeshDecodeError extends Error {
  constructor(message: string) {
    super(`Invalid SPLM mesh: ${message}`);
    this.name = "MeshDecodeError";
  }
}
const fail = (message: string): never => {
  throw new MeshDecodeError(message);
};

function descriptor(metadata: MeshChunkMetadata, expected: MeshIdentity): void {
  keys(
    metadata,
    [
      "session_id",
      "artifact_id",
      "definition_id",
      "schema_version",
      "mesh_profile",
      "chunk_index",
      "chunk_count",
      "byte_count",
      "sha256",
      "face_count",
      "bounds_mm",
      "carrier_deviation_mm",
      "quantization_error_mm",
    ],
    fail,
  );
  session(metadata.session_id, fail);
  uint(metadata.artifact_id, "artifact ID", fail, 1);
  hash(metadata.definition_id, fail);
  if (
    metadata.session_id !== expected.session_id ||
    metadata.artifact_id !== expected.artifact_id ||
    metadata.definition_id !== expected.definition_id
  )
    fail("descriptor identity mismatch");
  if (metadata.schema_version !== MESH_SCHEMA_VERSION || metadata.mesh_profile !== MESH_PROFILE)
    fail("unsupported schema or mesh profile");
  uint(
    metadata.chunk_count,
    "chunk count",
    fail,
    1,
    Math.floor(MAX_SCENE_MESH_BYTES / MESH_HEADER_BYTES),
  );
  uint(metadata.chunk_index, "chunk index", fail, 0, metadata.chunk_count - 1);
  uint(metadata.byte_count, "byte count", fail, MESH_HEADER_BYTES, MAX_GEOMETRY_CHUNK_BYTES);
  uint(metadata.face_count, "face count", fail, 1, MAX_NATIVE_FACES);
  hash(metadata.sha256, fail);
  bounds(metadata.bounds_mm, fail);
  const carrier = metadata.carrier_deviation_mm;
  const quantization = metadata.quantization_error_mm;
  if (
    !Number.isFinite(carrier) ||
    carrier < 0 ||
    carrier > MESH_CARRIER_TOLERANCE_MM ||
    !Number.isFinite(quantization) ||
    quantization < 0 ||
    quantization > MESH_QUANTIZATION_TOLERANCE_MM ||
    carrier + quantization > MESH_SURFACE_TOLERANCE_MM
  )
    fail("invalid profile error budget");
}

/** Validate complete ordered descriptor pages without concatenating chunk bodies. */
export function validateMeshManifest(
  metadata: readonly MeshChunkMetadata[],
  expected: MeshIdentity,
): void {
  if (
    metadata.length === 0 ||
    metadata.length > Math.floor(MAX_SCENE_MESH_BYTES / MESH_HEADER_BYTES)
  )
    fail("mesh must have a bounded nonempty chunk manifest");
  const first = metadata[0]!;
  let bytes = 0;
  for (let index = 0; index < metadata.length; index += 1) {
    const chunk = metadata[index]!;
    descriptor(chunk, expected);
    if (
      chunk.chunk_index !== index ||
      chunk.chunk_count !== metadata.length ||
      chunk.face_count !== first.face_count
    )
      fail("manifest is inconsistent or reordered");
    bytes += chunk.byte_count;
    if (!Number.isSafeInteger(bytes) || bytes > MAX_SCENE_MESH_BYTES)
      fail("manifest exceeds mesh byte budget");
  }
}

/** Input must stay immutable throughout decoding and while its borrowed GPU views live. */
export async function decodeMesh(
  input: PackedInput,
  metadata: MeshChunkMetadata,
  expected: MeshChunkIdentity,
): Promise<DisplayMesh> {
  descriptor(metadata, expected);
  uint(expected.chunk_index, "expected chunk index", fail);
  if (metadata.chunk_index !== expected.chunk_index) fail("descriptor chunk index mismatch");
  const { buffer, offset, length, data } = checkedInput(input, MESH_HEADER_BYTES, 4, fail);
  if (length !== metadata.byte_count) fail("descriptor byte count mismatch");
  if (data.getUint32(0, true) !== 0x4d4c5053 || data.getUint16(4, true) !== MESH_SCHEMA_VERSION)
    fail("magic or schema mismatch");
  zero(data, 6, 8, fail);
  zero(data, 48, 64, fail);
  const vertices = data.getUint32(8, true);
  const indexCount = data.getUint32(12, true);
  const triangles = data.getUint32(16, true);
  uint(vertices, "vertex count", fail, 1, MAX_DISPLAY_VERTICES);
  uint(triangles, "triangle count", fail, 1, MAX_DISPLAY_TRIANGLES);
  if (indexCount !== triangles * 3 || data.getUint32(20, true) !== expected.chunk_index)
    fail("header counts or chunk index mismatch");
  const normalsStart = size(MESH_HEADER_BYTES, vertices, 12, fail);
  const indicesStart = size(normalsStart, vertices, 12, fail);
  const facesStart = size(indicesStart, indexCount, 4, fail);
  const end = size(facesStart, triangles, 4, fail);
  if (length !== end) fail("payload size does not exactly match its counts");
  await verifyHash(buffer, offset, length, metadata.sha256, fail);
  const origin: [number, number, number] = [
    data.getFloat64(24, true),
    data.getFloat64(32, true),
    data.getFloat64(40, true),
  ];
  if (!origin.every(Number.isFinite)) fail("nonfinite local origin");
  const expansion = metadata.quantization_error_mm + 1e-9;
  for (let vertex = 0; vertex < vertices; vertex += 1) {
    for (let axis = 0; axis < 3; axis += 1) {
      const component = vertex * 3 + axis;
      const position = data.getFloat32(MESH_HEADER_BYTES + component * 4, true);
      const reconstructed = origin[axis]! + position;
      if (
        !Number.isFinite(position) ||
        !Number.isFinite(reconstructed) ||
        reconstructed < metadata.bounds_mm.min[axis]! - expansion ||
        reconstructed > metadata.bounds_mm.max[axis]! + expansion
      )
        fail("position outside finite advertised bounds");
    }
    const normalStart = normalsStart + vertex * 12;
    const nx = data.getFloat32(normalStart, true);
    const ny = data.getFloat32(normalStart + 4, true);
    const nz = data.getFloat32(normalStart + 8, true);
    if (
      !Number.isFinite(nx) ||
      !Number.isFinite(ny) ||
      !Number.isFinite(nz) ||
      Math.abs(Math.hypot(nx, ny, nz) - 1) > NORMAL_NORM_TOLERANCE
    )
      fail("invalid unit normal");
  }
  for (let index = 0; index < indexCount; index += 1)
    if (data.getUint32(indicesStart + index * 4, true) >= vertices) fail("index out of bounds");
  for (let triangle = 0; triangle < triangles; triangle += 1)
    if (data.getUint32(facesStart + triangle * 4, true) >= metadata.face_count)
      fail("face ordinal out of bounds");
  return {
    positions: new Float32Array(buffer, offset + MESH_HEADER_BYTES, vertices * 3),
    normals: new Float32Array(buffer, offset + normalsStart, vertices * 3),
    indices: new Uint32Array(buffer, offset + indicesStart, indexCount),
    faceOrdinals: new Uint32Array(buffer, offset + facesStart, triangles),
    localOriginMm: origin,
    chunkIndex: expected.chunk_index,
    byteLength: length,
    schema: MESH_SCHEMA_VERSION,
  };
}
