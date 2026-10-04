/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { decodeTriangle, TriangleDecodeError } from "../src/triangle.ts";
import { MAX_BINARY_BYTES } from "../src/generated.ts";

const fixture: unknown = JSON.parse(
  readFileSync(new URL("../../../fixtures/protocol/triangle.json", import.meta.url), "utf8"),
);
if (
  !fixture ||
  typeof fixture !== "object" ||
  !("encoding" in fixture) ||
  fixture.encoding !== "hex" ||
  !("data" in fixture) ||
  typeof fixture.data !== "string"
) {
  throw new Error(
    'The native-owned triangle golden fixture must contain {encoding:"hex",data:string}.',
  );
}
const golden = Uint8Array.from(Buffer.from(fixture.data, "hex")).buffer;

test("native golden SPLT produces aligned borrowed typed views", () => {
  const decoded = decodeTriangle(golden);
  assert.equal(decoded.byteLength, 64);
  assert.equal(decoded.schema, 1);
  assert.equal(decoded.positions.buffer, golden);
  assert.equal(decoded.indices.buffer, golden);
  assert.equal(decoded.positions.byteOffset, 16);
  assert.equal(decoded.indices.byteOffset, 52);
  assert.deepEqual(Array.from(decoded.indices), [0, 1, 2]);
  const expected = [-0.75, -0.6, 0, 0.75, -0.6, 0, 0, 0.75, 0];
  expected.forEach((value, index) => assert.ok(Math.abs(decoded.positions[index]! - value) < 1e-6));
});

const corruptions: [string, (view: DataView) => void][] = [
  ["wrong magic", (view) => view.setUint8(0, 0)],
  ["unknown schema", (view) => view.setUint16(4, 2, true)],
  ["nonzero reserved", (view) => view.setUint16(6, 1, true)],
  ["wrong vertex count", (view) => view.setUint32(8, 4, true)],
  ["incomplete triangle", (view) => view.setUint32(12, 2, true)],
  ["hostile vertex count", (view) => view.setUint32(8, 0xffffffff, true)],
  ["hostile index count", (view) => view.setUint32(12, 0xffffffff, true)],
  ["NaN position", (view) => view.setFloat32(16, NaN, true)],
  ["infinite position", (view) => view.setFloat32(48, Infinity, true)],
  ["out-of-range first index", (view) => view.setUint32(52, 3, true)],
  ["out-of-range last index", (view) => view.setUint32(60, 0xffffffff, true)],
];
for (const [description, mutate] of corruptions) {
  test(`rejects ${description} before returning display views`, () => {
    const bytes = golden.slice(0);
    mutate(new DataView(bytes));
    assert.throws(() => decodeTriangle(bytes), TriangleDecodeError);
  });
}

test("rejects truncation, trailing bytes, and oversized input", () => {
  for (const length of [0, 15, 16, 63])
    assert.throws(() => decodeTriangle(golden.slice(0, length)), TriangleDecodeError);
  const trailing = new Uint8Array(65);
  trailing.set(new Uint8Array(golden));
  assert.throws(() => decodeTriangle(trailing), TriangleDecodeError);
  assert.throws(() => decodeTriangle(new ArrayBuffer(MAX_BINARY_BYTES + 1)), TriangleDecodeError);
});

test("rejects misaligned ranges and honors aligned range boundaries", () => {
  const misaligned = new Uint8Array(65);
  misaligned.set(new Uint8Array(golden), 1);
  assert.throws(() => decodeTriangle(misaligned.subarray(1)), TriangleDecodeError);
  const aligned = new Uint8Array(72);
  aligned.set(new Uint8Array(golden), 4);
  const decoded = decodeTriangle(aligned.subarray(4, 68));
  assert.equal(decoded.positions.buffer, aligned.buffer);
  assert.equal(decoded.positions.byteOffset, 20);
  assert.equal(decoded.indices.byteOffset, 56);
  assert.equal(decoded.byteLength, 64);
});
