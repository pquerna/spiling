/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import {
  MAX_BINARY_BYTES,
  TRIANGLE_SCHEMA_VERSION,
  TRIANGLE_HEADER_BYTES,
  TRIANGLE_VERTEX_COUNT,
  TRIANGLE_INDEX_COUNT,
} from "./generated.ts";

export interface DiagnosticTriangle {
  readonly positions: Float32Array<ArrayBuffer>;
  readonly indices: Uint32Array<ArrayBuffer>;
  readonly byteLength: number;
  readonly schema: number;
}

export class TriangleDecodeError extends Error {
  constructor(message: string) {
    super(`Invalid SPLT diagnostic: ${message}`);
    this.name = "TriangleDecodeError";
  }
}

// SPLT is little-endian. Reject incompatible hosts rather than silently
// reinterpreting bytes, or copying into an apparently zero-copy display view.
const hostLittleEndian = new Uint8Array(new Uint32Array([1]).buffer)[0] === 1;

export function decodeTriangle(input: ArrayBuffer | Uint8Array<ArrayBuffer>): DiagnosticTriangle {
  const buffer = input instanceof ArrayBuffer ? input : input.buffer;
  const offset = input instanceof ArrayBuffer ? 0 : input.byteOffset;
  const length = input.byteLength;
  const fail = (message: string): never => {
    throw new TriangleDecodeError(message);
  };

  if (length > MAX_BINARY_BYTES) fail("payload exceeds negotiated binary limit");
  if (length < TRIANGLE_HEADER_BYTES) fail("truncated header");
  if (offset % 4 !== 0) fail("payload is not four-byte aligned");
  if (!hostLittleEndian) fail("host byte order is unsupported");
  const data = new DataView(buffer, offset, length);
  if (data.getUint32(0, true) !== 0x544c5053) fail("magic must be SPLT");
  const schema = data.getUint16(4, true);
  if (schema !== TRIANGLE_SCHEMA_VERSION) fail("unsupported schema");
  if (data.getUint16(6, true) !== 0) fail("reserved field must be zero");
  const vertices = data.getUint32(8, true);
  const indices = data.getUint32(12, true);
  if (vertices !== TRIANGLE_VERTEX_COUNT || indices !== TRIANGLE_INDEX_COUNT) {
    fail("B0 requires exactly three vertices and three indices");
  }
  if (indices % 3 !== 0) fail("index count must contain complete triangles");
  const positionBytes = vertices * 3 * Float32Array.BYTES_PER_ELEMENT;
  const indexOffset = TRIANGLE_HEADER_BYTES + positionBytes;
  const expectedBytes = indexOffset + indices * Uint32Array.BYTES_PER_ELEMENT;
  if (!Number.isSafeInteger(expectedBytes) || expectedBytes > MAX_BINARY_BYTES)
    fail("invalid counts or size");
  if (length !== expectedBytes) fail("payload size does not exactly match its counts");
  if ((offset + TRIANGLE_HEADER_BYTES) % 4 !== 0 || (offset + indexOffset) % 4 !== 0) {
    fail("typed array ranges are not aligned");
  }
  // Complete validation using DataView precedes creation of either typed view.
  for (let i = 0; i < vertices * 3; i += 1) {
    if (!Number.isFinite(data.getFloat32(TRIANGLE_HEADER_BYTES + i * 4, true)))
      fail("nonfinite position");
  }
  for (let i = 0; i < indices; i += 1) {
    if (data.getUint32(indexOffset + i * 4, true) >= vertices) fail("index is out of bounds");
  }
  return {
    positions: new Float32Array(buffer, offset + TRIANGLE_HEADER_BYTES, vertices * 3),
    indices: new Uint32Array(buffer, offset + indexOffset, indices),
    byteLength: length,
    schema,
  };
}
