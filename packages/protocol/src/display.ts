/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { MAX_GEOMETRY_CHUNK_BYTES } from "./generated.ts";
import type { AabbMm, SessionId, SourceHash } from "./generated.ts";

export type Fail = (message: string) => never;
export type PackedInput = ArrayBuffer | Uint8Array<ArrayBuffer>;
const hostLittleEndian = new Uint8Array(new Uint32Array([1]).buffer)[0] === 1;

export function checkedInput(input: PackedInput, header: number, alignment: number, fail: Fail) {
  const buffer = input instanceof ArrayBuffer ? input : input.buffer;
  const offset = input instanceof ArrayBuffer ? 0 : input.byteOffset;
  const length = input.byteLength;
  if (!(buffer instanceof ArrayBuffer)) fail("payload must use an ArrayBuffer");
  if (length < header || length > MAX_GEOMETRY_CHUNK_BYTES)
    fail("payload size outside chunk limits");
  if (offset % alignment !== 0) fail("payload alignment is invalid");
  if (!hostLittleEndian) fail("host byte order is unsupported");
  return { buffer, offset, length, data: new DataView(buffer, offset, length) };
}

export function uint(value: number, name: string, fail: Fail, min = 0, max = 0xffffffff): void {
  if (!Number.isInteger(value) || value < min || value > max) fail(`invalid ${name}`);
}

export function session(value: SessionId, fail: Fail): void {
  if (
    typeof value !== "string" ||
    !/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(value) ||
    value === "00000000-0000-0000-0000-000000000000"
  )
    fail("invalid session UUID");
}

export function hash(value: SourceHash, fail: Fail): void {
  if (typeof value !== "string" || !/^[0-9a-f]{64}$/.test(value))
    fail("invalid lowercase SHA-256 identity");
}

export function finite3(value: readonly number[], name: string, fail: Fail): void {
  if (!Array.isArray(value) || value.length !== 3 || !value.every(Number.isFinite))
    fail(`invalid ${name}`);
}

export function bounds(value: AabbMm, fail: Fail): void {
  keys(value, ["min", "max"], fail);
  finite3(value.min, "bounds minimum", fail);
  finite3(value.max, "bounds maximum", fail);
  for (let axis = 0; axis < 3; axis += 1) {
    if (value.min[axis]! > value.max[axis]!) fail("bounds are inverted");
  }
}

export function keys(value: object, expected: readonly string[], fail: Fail): void {
  if (!value || typeof value !== "object" || Array.isArray(value))
    fail("metadata must be an object");
  const actual = Object.keys(value);
  if (actual.length !== expected.length || actual.some((key) => !expected.includes(key)))
    fail("metadata fields do not match the contract");
}

export function size(base: number, count: number, stride: number, fail: Fail): number {
  const result = base + count * stride;
  if (!Number.isSafeInteger(result) || result > MAX_GEOMETRY_CHUNK_BYTES)
    fail("invalid layout size");
  return result;
}

export function zero(data: DataView, start: number, end: number, fail: Fail): void {
  for (let i = start; i < end; i += 1)
    if (data.getUint8(i) !== 0) fail("reserved bytes must be zero");
}

export async function verifyHash(
  buffer: ArrayBuffer,
  offset: number,
  length: number,
  expected: SourceHash,
  fail: Fail,
): Promise<void> {
  hash(expected, fail);
  if (!globalThis.crypto?.subtle) fail("Web Crypto SHA-256 is unavailable");
  let digest: ArrayBuffer;
  try {
    digest = await globalThis.crypto.subtle.digest(
      "SHA-256",
      new Uint8Array(buffer, offset, length),
    );
  } catch {
    fail("Web Crypto SHA-256 failed");
  }
  const actual = Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
  if (actual !== expected) fail("SHA-256 mismatch");
}
