/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import test from "node:test";
import { decodeMesh, validateMeshManifest } from "../src/mesh.ts";
import { decodeSection, validateSectionManifest } from "../src/section.ts";
import type {
  FaceIndexRow,
  MeshChunkMetadata,
  SectionChunkMetadata,
  SectionLoopMetadata,
  SectionSummary,
} from "../src/generated.ts";

type MeshFixture = {
  source_hash: string;
  face_table: FaceIndexRow[];
  chunks: { metadata: MeshChunkMetadata; data: string }[];
};
type SectionFixture = {
  source_hash: string;
  summary: SectionSummary;
  loops: SectionLoopMetadata[];
  chunks: { metadata: SectionChunkMetadata; data: string }[];
};
function fixture<T>(name: string): T {
  return JSON.parse(
    readFileSync(new URL(`../../../fixtures/protocol/${name}`, import.meta.url), "utf8"),
  );
}

test("native box wire retains all source faces and physical mm extents", async () => {
  const native = fixture<MeshFixture>("native-mesh.json");
  assert.equal(
    native.source_hash,
    createHash("sha256")
      .update(readFileSync(new URL("../../../fixtures/geometry/box-mm.step", import.meta.url)))
      .digest("hex"),
  );
  const first = native.chunks[0]!.metadata;
  const identity = {
    session_id: first.session_id,
    artifact_id: first.artifact_id,
    definition_id: first.definition_id,
  };
  validateMeshManifest(
    native.chunks.map((chunk) => chunk.metadata),
    identity,
  );
  const seen = new Set<number>();
  const min = [Infinity, Infinity, Infinity];
  const max = [-Infinity, -Infinity, -Infinity];
  for (const chunk of native.chunks) {
    const mesh = await decodeMesh(
      Uint8Array.from(Buffer.from(chunk.data, "hex")).buffer,
      chunk.metadata,
      { ...identity, chunk_index: chunk.metadata.chunk_index },
    );
    for (const ordinal of mesh.faceOrdinals) seen.add(ordinal);
    for (let index = 0; index < mesh.positions.length; index += 3) {
      for (let axis = 0; axis < 3; axis++) {
        const value = mesh.localOriginMm[axis]! + mesh.positions[index + axis]!;
        min[axis] = Math.min(min[axis]!, value);
        max[axis] = Math.max(max[axis]!, value);
      }
    }
  }
  assert.deepEqual(
    Array.from(seen).sort((a, b) => a - b),
    [0, 1, 2, 3, 4, 5],
  );
  assert.equal(new Set(native.face_table.map((row) => row.face_id)).size, 6);
  for (let axis = 0; axis < 3; axis++) {
    assert.ok(Math.abs(min[axis]!) <= 1e-6);
    assert.ok(Math.abs(max[axis]! - [20, 10, 8][axis]!) <= 1e-6);
  }
});

for (const name of ["native-section.json", "native-section-multichunk.json"]) {
  test(`native through-hole ${name} preserves planar material and circular hole`, async () => {
    const native = fixture<SectionFixture>(name);
    assert.equal(
      native.source_hash,
      createHash("sha256")
        .update(
          readFileSync(new URL("../../../fixtures/geometry/through-hole.step", import.meta.url)),
        )
        .digest("hex"),
    );
    const summary = native.summary;
    const identity = {
      session_id: summary.session_id,
      artifact_id: summary.artifact_id,
      revision: summary.revision,
    };
    validateSectionManifest(
      summary,
      native.chunks.map((chunk) => chunk.metadata),
      native.loops,
      identity,
    );
    const areas: number[] = [];
    for (const chunk of native.chunks) {
      const rows = native.loops.slice(
        chunk.metadata.first_loop_ordinal,
        chunk.metadata.first_loop_ordinal + chunk.metadata.loop_count,
      );
      const decoded = await decodeSection(
        Uint8Array.from(Buffer.from(chunk.data, "hex")).buffer,
        chunk.metadata,
        summary,
        rows,
        { ...identity, chunk_index: chunk.metadata.chunk_index },
      );
      for (let loop = 0; loop < rows.length; loop++) {
        const start = decoded.loopOffsets[loop]!;
        const end = decoded.loopOffsets[loop + 1]!;
        const row = rows[loop]!;
        let area = 0;
        let perimeter = 0;
        const dot = (point: number, axis: readonly number[]) =>
          axis.reduce(
            (sum, value, index) => sum + value * decoded.pointsRelativeMm[point * 3 + index]!,
            0,
          );
        for (let point = start; point < end; point++) {
          assert.ok(Math.abs(dot(point, summary.plane.normal)) <= 1e-4);
          if (row.is_hole) {
            const x = decoded.pointsRelativeMm[point * 3]! + decoded.planeOriginMm[0] - 10;
            const y = decoded.pointsRelativeMm[point * 3 + 1]! + decoded.planeOriginMm[1] - 10;
            assert.ok(Math.abs(Math.hypot(x, y) - 3) <= 0.005);
          }
          if (point + 1 < end) {
            area +=
              dot(point, summary.frame.x_axis) * dot(point + 1, summary.frame.y_axis) -
              dot(point + 1, summary.frame.x_axis) * dot(point, summary.frame.y_axis);
            perimeter += Math.hypot(
              ...[0, 1, 2].map(
                (axis) =>
                  decoded.pointsRelativeMm[(point + 1) * 3 + axis]! -
                  decoded.pointsRelativeMm[point * 3 + axis]!,
              ),
            );
          }
        }
        area *= 0.5;
        assert.equal(area < 0, row.is_hole);
        if (row.is_hole) {
          assert.ok(Math.abs(perimeter - 6 * Math.PI) <= 0.1);
          assert.ok(Math.abs(area + 9 * Math.PI) <= 6 * Math.PI * 0.005 + Math.PI * 0.005 ** 2);
        } else assert.ok(Math.abs(area - 400) <= 1e-6);
        areas[row.ordinal] = area;
      }
    }
    assert.equal(areas.length, 2);
    assert.ok(
      Math.abs(areas[0]! + areas[1]! - (400 - 9 * Math.PI)) <=
        6 * Math.PI * 0.005 + Math.PI * 0.005 ** 2,
    );
  });
}

test("native section manifest rejects reordered and overlapping complete-loop ranges", () => {
  const native = fixture<SectionFixture>("native-section-multichunk.json");
  const identity = {
    session_id: native.summary.session_id,
    artifact_id: native.summary.artifact_id,
    revision: native.summary.revision,
  };
  const chunks = native.chunks.map((chunk) => chunk.metadata);
  assert.throws(() =>
    validateSectionManifest(native.summary, [...chunks].reverse(), native.loops, identity),
  );
  assert.throws(() =>
    validateSectionManifest(
      native.summary,
      [chunks[0]!, { ...chunks[1]!, first_loop_ordinal: 0 }],
      native.loops,
      identity,
    ),
  );
});
