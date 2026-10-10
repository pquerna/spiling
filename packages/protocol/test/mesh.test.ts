/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { decodeMesh, MeshDecodeError, validateMeshManifest } from "../src/mesh.ts";
import {
  MAX_GEOMETRY_CHUNK_BYTES,
  MAX_NATIVE_FACES,
  MAX_SCENE_MESH_BYTES,
} from "../src/generated.ts";
import type { FaceIndexRow, MeshChunkMetadata } from "../src/generated.ts";

const fixture: {
  encoding: string;
  chunks: { metadata: MeshChunkMetadata; data: string }[];
  face_table: FaceIndexRow[];
} = JSON.parse(
  readFileSync(new URL("../../../fixtures/protocol/mesh.json", import.meta.url), "utf8"),
);
assert.equal(fixture.encoding, "hex");
const metadata = fixture.chunks[0]!.metadata;
const golden = Uint8Array.from(Buffer.from(fixture.chunks[0]!.data, "hex")).buffer;
const expected = {
  session_id: metadata.session_id,
  artifact_id: metadata.artifact_id,
  definition_id: metadata.definition_id,
  chunk_index: 0,
};

async function rehash(
  bytes: ArrayBuffer,
  descriptor: MeshChunkMetadata,
): Promise<MeshChunkMetadata> {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return { ...descriptor, sha256: Buffer.from(digest).toString("hex") };
}

test("Rust-owned algebraic mesh golden returns borrowed face-mapped views", async () => {
  validateMeshManifest(
    fixture.chunks.map((chunk) => chunk.metadata),
    expected,
  );
  const decoded = await decodeMesh(golden, metadata, expected);
  assert.equal(decoded.schema, 1);
  assert.equal(decoded.byteLength, 240);
  assert.equal(decoded.positions.buffer, golden);
  assert.equal(decoded.normals.buffer, golden);
  assert.equal(decoded.indices.buffer, golden);
  assert.equal(decoded.faceOrdinals.buffer, golden);
  assert.equal(decoded.positions.byteOffset, 64);
  assert.equal(decoded.normals.byteOffset, 136);
  assert.equal(decoded.indices.byteOffset, 208);
  assert.equal(decoded.faceOrdinals.byteOffset, 232);
  assert.deepEqual(Array.from(decoded.indices), [0, 1, 2, 3, 4, 5]);
  assert.deepEqual(Array.from(decoded.faceOrdinals), [0, 1]);
  assert.deepEqual(decoded.localOriginMm, [0, 0, 0]);
  assert.equal(fixture.face_table[decoded.faceOrdinals[0]!]!.face_id, "step:9007199254740993");
  assert.equal(fixture.face_table[decoded.faceOrdinals[1]!]!.face_id, "step:42");
  assert.equal(typeof fixture.face_table[0]!.face_id, "string");
});

const corruptions: [string, (view: DataView) => void][] = [
  ["bad magic", (view) => view.setUint8(0, 0)],
  ["bad schema", (view) => view.setUint16(4, 2, true)],
  ["nonzero flags", (view) => view.setUint16(6, 1, true)],
  ["nonzero reserved tail", (view) => view.setUint8(63, 1)],
  ["zero vertices", (view) => view.setUint32(8, 0, true)],
  ["hostile vertices", (view) => view.setUint32(8, 0xffffffff, true)],
  ["incomplete index count", (view) => view.setUint32(12, 5, true)],
  ["zero triangles", (view) => view.setUint32(16, 0, true)],
  ["hostile triangles", (view) => view.setUint32(16, 0xffffffff, true)],
  ["wrong header index", (view) => view.setUint32(20, 1, true)],
  ["NaN origin", (view) => view.setFloat64(24, NaN, true)],
  ["infinite origin", (view) => view.setFloat64(40, Infinity, true)],
  ["finite origin outside bounds", (view) => view.setFloat64(24, 100, true)],
  ["NaN position", (view) => view.setFloat32(64, NaN, true)],
  ["infinite position", (view) => view.setFloat32(68, Infinity, true)],
  ["finite position outside bounds", (view) => view.setFloat32(64, -1, true)],
  ["nonunit normal", (view) => view.setFloat32(144, 2, true)],
  ["zero normal", (view) => view.setFloat32(144, 0, true)],
  ["NaN normal", (view) => view.setFloat32(136, NaN, true)],
  ["infinite normal", (view) => view.setFloat32(140, Infinity, true)],
  ["first index out of range", (view) => view.setUint32(208, 6, true)],
  ["last index out of range", (view) => view.setUint32(228, 0xffffffff, true)],
  ["face ordinal out of range", (view) => view.setUint32(232, 2, true)],
];
for (const [description, mutate] of corruptions) {
  test(`mesh rejects ${description} even with an authentic hash`, async () => {
    const bytes = golden.slice(0);
    mutate(new DataView(bytes));
    await assert.rejects(
      decodeMesh(bytes, await rehash(bytes, metadata), expected),
      MeshDecodeError,
    );
  });
}

const metadataCorruptions: [string, (value: MeshChunkMetadata) => void][] = [
  [
    "stale session",
    (value) => {
      value.session_id = "550e8400-e29b-41d4-a716-446655440001";
    },
  ],
  [
    "noncanonical session",
    (value) => {
      value.session_id = value.session_id.toUpperCase();
    },
  ],
  [
    "nil session",
    (value) => {
      value.session_id = "00000000-0000-0000-0000-000000000000";
    },
  ],
  [
    "wrong artifact",
    (value) => {
      value.artifact_id += 1;
    },
  ],
  [
    "zero artifact",
    (value) => {
      value.artifact_id = 0;
    },
  ],
  [
    "wrong definition",
    (value) => {
      value.definition_id = "0".repeat(64);
    },
  ],
  [
    "noncanonical definition",
    (value) => {
      value.definition_id = value.definition_id.toUpperCase();
    },
  ],
  [
    "wrong schema",
    (value) => {
      value.schema_version = 2;
    },
  ],
  [
    "wrong profile",
    (value) => {
      Object.assign(value, { mesh_profile: "invented-profile" });
    },
  ],
  [
    "wrong index",
    (value) => {
      value.chunk_index = 1;
      value.chunk_count = 2;
    },
  ],
  [
    "zero chunks",
    (value) => {
      value.chunk_count = 0;
    },
  ],
  [
    "fractional chunks",
    (value) => {
      value.chunk_count = 1.5;
    },
  ],
  [
    "wrong byte count",
    (value) => {
      value.byte_count -= 1;
    },
  ],
  [
    "zero faces",
    (value) => {
      value.face_count = 0;
    },
  ],
  [
    "excessive faces",
    (value) => {
      value.face_count = MAX_NATIVE_FACES + 1;
    },
  ],
  [
    "bad hash",
    (value) => {
      value.sha256 = "0".repeat(64);
    },
  ],
  [
    "noncanonical hash",
    (value) => {
      value.sha256 = value.sha256.toUpperCase();
    },
  ],
  [
    "nonfinite bounds",
    (value) => {
      value.bounds_mm.max[0] = Infinity;
    },
  ],
  [
    "inverted bounds",
    (value) => {
      value.bounds_mm.min[0] = 3;
    },
  ],
  [
    "negative carrier error",
    (value) => {
      value.carrier_deviation_mm = -0.001;
    },
  ],
  [
    "excessive carrier error",
    (value) => {
      value.carrier_deviation_mm = 0.025001;
    },
  ],
  [
    "NaN quantization",
    (value) => {
      value.quantization_error_mm = NaN;
    },
  ],
  [
    "excessive quantization",
    (value) => {
      value.quantization_error_mm = 0.025001;
    },
  ],
  [
    "unknown field",
    (value) => {
      Object.assign(value, { extra: true });
    },
  ],
];
for (const [description, mutate] of metadataCorruptions) {
  test(`mesh rejects descriptor ${description}`, async () => {
    const changed = structuredClone(metadata);
    mutate(changed);
    await assert.rejects(decodeMesh(golden, changed, expected), MeshDecodeError);
  });
}

test("mesh rejects hash corruption with otherwise valid scalar data", async () => {
  const bytes = golden.slice(0);
  new DataView(bytes).setFloat32(68, 0.5, true);
  await assert.rejects(decodeMesh(bytes, metadata, expected), /SHA-256 mismatch/);
});

test("mesh accepts quantified bound expansion, but not points beyond it", async () => {
  const bytes = golden.slice(0);
  new DataView(bytes).setFloat32(64, -0.02, true);
  const changed = await rehash(bytes, { ...metadata, quantization_error_mm: 0.025 });
  await decodeMesh(bytes, changed, expected);
  new DataView(bytes).setFloat32(64, -0.026, true);
  await assert.rejects(decodeMesh(bytes, await rehash(bytes, changed), expected), MeshDecodeError);
});

test("mesh rejects exact-length failures and oversized input", async () => {
  for (const length of [0, 63, 64, golden.byteLength - 1])
    await assert.rejects(
      decodeMesh(golden.slice(0, length), { ...metadata, byte_count: length }, expected),
      MeshDecodeError,
    );
  const trailing = new Uint8Array(golden.byteLength + 4);
  trailing.set(new Uint8Array(golden));
  await assert.rejects(
    decodeMesh(trailing, { ...metadata, byte_count: trailing.byteLength }, expected),
    MeshDecodeError,
  );
  await assert.rejects(
    decodeMesh(new ArrayBuffer(MAX_GEOMETRY_CHUNK_BYTES + 1), metadata, expected),
    MeshDecodeError,
  );
});

test("mesh hashes only an aligned caller range and rejects misalignment", async () => {
  const aligned = new Uint8Array(golden.byteLength + 16);
  aligned.fill(0xa5);
  aligned.set(new Uint8Array(golden), 4);
  const decoded = await decodeMesh(aligned.subarray(4, 4 + golden.byteLength), metadata, expected);
  assert.equal(decoded.positions.buffer, aligned.buffer);
  assert.equal(decoded.positions.byteOffset, 68);
  const misaligned = new Uint8Array(golden.byteLength + 1);
  misaligned.set(new Uint8Array(golden), 1);
  await assert.rejects(decodeMesh(misaligned.subarray(1), metadata, expected), MeshDecodeError);
});

test("mesh manifest rejects missing/reordered/inconsistent pages and aggregate overflow", () => {
  assert.throws(() => validateMeshManifest([], expected), MeshDecodeError);
  assert.throws(
    () => validateMeshManifest([{ ...metadata, chunk_count: 2 }], expected),
    MeshDecodeError,
  );
  assert.throws(
    () =>
      validateMeshManifest(
        [
          { ...metadata, chunk_index: 1, chunk_count: 2 },
          { ...metadata, chunk_count: 2 },
        ],
        expected,
      ),
    MeshDecodeError,
  );
  assert.throws(
    () =>
      validateMeshManifest(
        [
          { ...metadata, chunk_count: 2 },
          { ...metadata, chunk_index: 1, chunk_count: 2, face_count: 3 },
        ],
        expected,
      ),
    MeshDecodeError,
  );
  const count = Math.floor(MAX_SCENE_MESH_BYTES / MAX_GEOMETRY_CHUNK_BYTES) + 1;
  const overBudget = Array.from({ length: count }, (_, chunk_index) => ({
    ...metadata,
    chunk_index,
    chunk_count: count,
    byte_count: MAX_GEOMETRY_CHUNK_BYTES,
  }));
  assert.throws(() => validateMeshManifest(overBudget, expected), MeshDecodeError);
});

test("mesh enforces the frozen normal tolerance and permits profile maximum errors", async () => {
  const bytes = golden.slice(0);
  new DataView(bytes).setFloat32(144, 1.00005, true);
  const allowed = await rehash(bytes, {
    ...metadata,
    carrier_deviation_mm: 0.025,
    quantization_error_mm: 0.025,
  });
  await decodeMesh(bytes, allowed, expected);
  new DataView(bytes).setFloat32(144, 1.0002, true);
  await assert.rejects(decodeMesh(bytes, await rehash(bytes, allowed), expected), MeshDecodeError);
});

test("mesh rejects incomplete descriptor fields and nonnumeric counts", async () => {
  const missing = structuredClone(metadata);
  Reflect.deleteProperty(missing, "sha256");
  await assert.rejects(decodeMesh(golden, missing, expected), MeshDecodeError);
  const stringCount = structuredClone(metadata);
  Object.assign(stringCount, { byte_count: "240" });
  await assert.rejects(decodeMesh(golden, stringCount, expected), MeshDecodeError);
});
