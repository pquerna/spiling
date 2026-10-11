/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import {
  MAX_GEOMETRY_CHUNK_BYTES,
  MAX_LOOP_POINTS,
  MAX_SECTION_BYTES,
  POSE_NORM_TOLERANCE,
  SECTION_BOOLEAN_TOLERANCE_MM,
  SECTION_HEADER_BYTES,
  SECTION_PLANE_TOLERANCE_MM,
  SECTION_SAMPLING_TOLERANCE_MM,
  SECTION_SCHEMA_VERSION,
} from "./generated.ts";
import type { SectionChunkMetadata, SectionLoopMetadata, SectionSummary } from "./generated.ts";
import {
  checkedInput,
  finite3,
  hash,
  keys,
  session,
  size,
  uint,
  verifyHash,
  zero,
} from "./display.ts";
import type { PackedInput } from "./display.ts";

export type SectionIdentity = Pick<SectionSummary, "session_id" | "artifact_id" | "revision">;
export type SectionChunkIdentity = SectionIdentity & Pick<SectionChunkMetadata, "chunk_index">;

export interface DisplaySection {
  readonly loopOffsets: Uint32Array<ArrayBuffer>;
  readonly pointsRelativeMm: Float64Array<ArrayBuffer>;
  readonly planeOriginMm: readonly [number, number, number];
  readonly chunkIndex: number;
  readonly firstLoopOrdinal: number;
  readonly byteLength: number;
  readonly schema: number;
}

export class SectionDecodeError extends Error {
  constructor(message: string) {
    super(`Invalid SPLS section: ${message}`);
    this.name = "SectionDecodeError";
  }
}
const fail = (message: string): never => {
  throw new SectionDecodeError(message);
};

function pointStart(loops: number): number {
  const end = size(SECTION_HEADER_BYTES, loops + 1, 4, fail);
  const aligned = Math.ceil(end / 8) * 8;
  if (aligned > MAX_GEOMETRY_CHUNK_BYTES) fail("section alignment exceeds chunk budget");
  return aligned;
}

/** Validate identity, the normalized canonical plane frame and the frozen tolerances. */
export function validateSectionSummary(summary: SectionSummary, expected: SectionIdentity): void {
  keys(
    summary,
    [
      "session_id",
      "artifact_id",
      "revision",
      "schema_version",
      "plane",
      "frame",
      "total_loop_count",
      "chunk_count",
      "total_bytes",
      "sampling_tolerance_mm",
      "boolean_tolerance_mm",
    ],
    fail,
  );
  session(summary.session_id, fail);
  uint(summary.artifact_id, "artifact ID", fail, 1);
  uint(summary.revision, "scene revision", fail);
  if (
    summary.session_id !== expected.session_id ||
    summary.artifact_id !== expected.artifact_id ||
    summary.revision !== expected.revision
  )
    fail("summary identity or revision mismatch");
  if (
    summary.schema_version !== SECTION_SCHEMA_VERSION ||
    summary.sampling_tolerance_mm !== SECTION_SAMPLING_TOLERANCE_MM ||
    summary.boolean_tolerance_mm !== SECTION_BOOLEAN_TOLERANCE_MM
  )
    fail("unsupported schema or tolerances");
  uint(summary.total_bytes, "total byte count", fail, 0, MAX_SECTION_BYTES);
  // Every closed loop needs at least four f64 XYZ points and one offset.
  uint(
    summary.total_loop_count,
    "total loop count",
    fail,
    0,
    Math.floor(MAX_SECTION_BYTES / (4 * 24 + 4)),
  );
  uint(summary.chunk_count, "chunk count", fail, 0, summary.total_loop_count);
  if (
    (summary.total_loop_count === 0) !== (summary.chunk_count === 0) ||
    (summary.chunk_count === 0) !== (summary.total_bytes === 0)
  )
    fail("inconsistent empty section");
  const minimumBytes = summary.total_loop_count * 100 + summary.chunk_count * 68;
  if (summary.total_bytes < minimumBytes)
    fail("summary byte count cannot contain its complete loops");
  keys(summary.plane, ["origin_mm", "normal"], fail);
  finite3(summary.plane.origin_mm, "plane origin", fail);
  finite3(summary.plane.normal, "plane normal", fail);
  const norm = Math.hypot(...summary.plane.normal);
  if (!Number.isFinite(norm) || Math.abs(norm - 1) > POSE_NORM_TOLERANCE)
    fail("plane must be normalized");
  const z = summary.plane.normal.map((value) => value / norm);
  let least = 0;
  for (let axis = 1; axis < 3; axis += 1)
    if (Math.abs(z[axis]!) < Math.abs(z[least]!)) least = axis;
  const axis = [0, 0, 0];
  axis[least] = 1;
  const x = [
    axis[1]! * z[2]! - axis[2]! * z[1]!,
    axis[2]! * z[0]! - axis[0]! * z[2]!,
    axis[0]! * z[1]! - axis[1]! * z[0]!,
  ];
  const xNorm = Math.hypot(...x);
  for (let i = 0; i < 3; i += 1) x[i] = x[i]! / xNorm;
  const y = [
    z[1]! * x[2]! - z[2]! * x[1]!,
    z[2]! * x[0]! - z[0]! * x[2]!,
    z[0]! * x[1]! - z[1]! * x[0]!,
  ];
  keys(summary.frame, ["origin_mm", "x_axis", "y_axis", "z_axis"], fail);
  const actual = [
    summary.frame.origin_mm,
    summary.frame.x_axis,
    summary.frame.y_axis,
    summary.frame.z_axis,
  ];
  const wanted = [summary.plane.origin_mm, x, y, z];
  for (let vector = 0; vector < 4; vector += 1) {
    finite3(actual[vector]!, "frame vector", fail);
    for (let i = 0; i < 3; i += 1)
      if (Math.abs(actual[vector]![i]! - wanted[vector]![i]!) > POSE_NORM_TOLERANCE)
        fail("frame is not the canonical plane frame");
  }
}

function descriptor(metadata: SectionChunkMetadata, summary: SectionSummary): void {
  keys(
    metadata,
    [
      "session_id",
      "artifact_id",
      "schema_version",
      "chunk_index",
      "chunk_count",
      "byte_count",
      "sha256",
      "first_loop_ordinal",
      "loop_count",
    ],
    fail,
  );
  if (metadata.session_id !== summary.session_id || metadata.artifact_id !== summary.artifact_id)
    fail("descriptor identity mismatch");
  if (
    metadata.schema_version !== SECTION_SCHEMA_VERSION ||
    metadata.chunk_count !== summary.chunk_count
  )
    fail("descriptor schema or chunk count mismatch");
  uint(metadata.chunk_index, "chunk index", fail, 0, summary.chunk_count - 1);
  uint(metadata.byte_count, "byte count", fail, SECTION_HEADER_BYTES, MAX_GEOMETRY_CHUNK_BYTES);
  if (metadata.byte_count > summary.total_bytes) fail("chunk bytes exceed artifact total");
  uint(metadata.first_loop_ordinal, "first loop ordinal", fail, 0, summary.total_loop_count - 1);
  uint(
    metadata.loop_count,
    "loop count",
    fail,
    1,
    summary.total_loop_count - metadata.first_loop_ordinal,
  );
  hash(metadata.sha256, fail);
}

function loopRow(row: SectionLoopMetadata): void {
  keys(row, ["ordinal", "occurrence_id", "definition_id", "is_hole", "point_count"], fail);
  uint(row.ordinal, "loop ordinal", fail);
  uint(row.occurrence_id, "occurrence ID", fail, 1);
  hash(row.definition_id, fail);
  if (typeof row.is_hole !== "boolean") fail("invalid hole flag");
  uint(row.point_count, "loop point count", fail, 4, MAX_LOOP_POINTS);
}

/** Validate complete paged descriptors and loop rows, not accumulated payload bytes. */
export function validateSectionManifest(
  summary: SectionSummary,
  chunks: readonly SectionChunkMetadata[],
  loops: readonly SectionLoopMetadata[],
  expected: SectionIdentity,
): void {
  validateSectionSummary(summary, expected);
  if (chunks.length !== summary.chunk_count || loops.length !== summary.total_loop_count)
    fail("manifest page counts do not match summary");
  let previous: SectionLoopMetadata | undefined;
  for (let index = 0; index < loops.length; index += 1) {
    const row = loops[index]!;
    loopRow(row);
    if (row.ordinal !== index) fail("loop rows are reordered");
    if (previous) {
      if (
        row.occurrence_id < previous.occurrence_id ||
        (row.occurrence_id === previous.occurrence_id &&
          (row.definition_id !== previous.definition_id || (previous.is_hole && !row.is_hole))) ||
        (row.occurrence_id !== previous.occurrence_id && row.is_hole)
      )
        fail("loop identity or outer/hole order is invalid");
    } else if (row.is_hole) fail("section cannot start with a hole");
    previous = row;
  }
  let nextLoop = 0;
  let totalBytes = 0;
  for (let index = 0; index < chunks.length; index += 1) {
    const chunk = chunks[index]!;
    descriptor(chunk, summary);
    if (chunk.chunk_index !== index || chunk.first_loop_ordinal !== nextLoop)
      fail("chunk ranges are reordered, overlapping or discontinuous");
    let points = 0;
    for (let loop = nextLoop; loop < nextLoop + chunk.loop_count; loop += 1)
      points += loops[loop]!.point_count;
    if (size(pointStart(chunk.loop_count), points, 24, fail) !== chunk.byte_count)
      fail("descriptor byte count disagrees with complete loop rows");
    nextLoop += chunk.loop_count;
    totalBytes += chunk.byte_count;
    if (!Number.isSafeInteger(totalBytes) || totalBytes > MAX_SECTION_BYTES)
      fail("section byte budget exceeded");
  }
  if (nextLoop !== summary.total_loop_count || totalBytes !== summary.total_bytes)
    fail("manifest totals mismatch");
}

/** Rows are the artifact-wide identity rows for this chunk's complete loop range.
 * Input must stay immutable throughout decoding and while borrowed GPU views live. */
export async function decodeSection(
  input: PackedInput,
  metadata: SectionChunkMetadata,
  summary: SectionSummary,
  rows: readonly SectionLoopMetadata[],
  expected: SectionChunkIdentity,
): Promise<DisplaySection> {
  validateSectionSummary(summary, expected);
  descriptor(metadata, summary);
  uint(expected.chunk_index, "expected chunk index", fail);
  if (metadata.chunk_index !== expected.chunk_index) fail("descriptor chunk index mismatch");
  if (rows.length !== metadata.loop_count) fail("chunk loop metadata is incomplete");
  for (let i = 0; i < rows.length; i += 1) {
    loopRow(rows[i]!);
    if (rows[i]!.ordinal !== metadata.first_loop_ordinal + i) fail("loop row ordinal mismatch");
    if (i > 0) {
      const previous = rows[i - 1]!;
      const row = rows[i]!;
      if (
        row.occurrence_id < previous.occurrence_id ||
        (row.occurrence_id === previous.occurrence_id &&
          (row.definition_id !== previous.definition_id || (previous.is_hole && !row.is_hole))) ||
        (row.occurrence_id !== previous.occurrence_id && row.is_hole)
      )
        fail("chunk loop identity or outer/hole order is invalid");
    } else if (metadata.first_loop_ordinal === 0 && rows[i]!.is_hole) {
      fail("section cannot start with a hole");
    }
  }
  const { buffer, offset, length, data } = checkedInput(input, SECTION_HEADER_BYTES, 8, fail);
  if (length !== metadata.byte_count) fail("descriptor byte count mismatch");
  if (data.getUint32(0, true) !== 0x534c5053 || data.getUint16(4, true) !== SECTION_SCHEMA_VERSION)
    fail("magic or schema mismatch");
  zero(data, 6, 8, fail);
  zero(data, 48, 64, fail);
  const loops = data.getUint32(8, true);
  const points = data.getUint32(12, true);
  if (
    loops !== metadata.loop_count ||
    points < loops * 4 ||
    data.getUint32(40, true) !== expected.chunk_index ||
    data.getUint32(44, true) !== metadata.first_loop_ordinal
  )
    fail("header and descriptor disagree");
  const pointsStart = pointStart(loops);
  if (size(pointsStart, points, 24, fail) !== length)
    fail("payload size does not exactly match its counts");
  const offsetsEnd = SECTION_HEADER_BYTES + (loops + 1) * 4;
  zero(data, offsetsEnd, pointsStart, fail);
  await verifyHash(buffer, offset, length, metadata.sha256, fail);
  const origin: [number, number, number] = [
    data.getFloat64(16, true),
    data.getFloat64(24, true),
    data.getFloat64(32, true),
  ];
  if (
    !origin.every(Number.isFinite) ||
    origin.some((value, axis) => value !== summary.plane.origin_mm[axis])
  )
    fail("nonfinite or mismatched plane origin");
  const offsetAt = (loop: number) => data.getUint32(SECTION_HEADER_BYTES + loop * 4, true);
  if (offsetAt(0) !== 0 || offsetAt(loops) !== points) fail("offsets do not span all points");
  const normal = summary.plane.normal;
  const frameX = summary.frame.x_axis;
  const frameY = summary.frame.y_axis;
  for (let loop = 0; loop < loops; loop += 1) {
    const start = offsetAt(loop);
    const end = offsetAt(loop + 1);
    const count = end - start;
    if (end > points || count < 4 || count > MAX_LOOP_POINTS || count !== rows[loop]!.point_count)
      fail("invalid complete loop offsets or metadata point count");
    let firstX = 0,
      firstY = 0,
      firstZ = 0;
    let lastX = 0,
      lastY = 0,
      lastZ = 0;
    let previousU = 0,
      previousV = 0,
      area = 0;
    for (let point = start; point < end; point += 1) {
      const base = pointsStart + point * 24;
      const x = data.getFloat64(base, true);
      const y = data.getFloat64(base + 8, true);
      const z = data.getFloat64(base + 16, true);
      if (
        !Number.isFinite(x) ||
        !Number.isFinite(y) ||
        !Number.isFinite(z) ||
        !Number.isFinite(origin[0] + x) ||
        !Number.isFinite(origin[1] + y) ||
        !Number.isFinite(origin[2] + z) ||
        Math.abs(x * normal[0] + y * normal[1] + z * normal[2]) > SECTION_PLANE_TOLERANCE_MM
      )
        fail("nonfinite or off-plane coordinate");
      const u = x * frameX[0] + y * frameX[1] + z * frameX[2];
      const v = x * frameY[0] + y * frameY[1] + z * frameY[2];
      if (point === start) {
        firstX = x;
        firstY = y;
        firstZ = z;
      } else area += previousU * v - u * previousV;
      previousU = u;
      previousV = v;
      lastX = x;
      lastY = y;
      lastZ = z;
    }
    if (Math.hypot(firstX - lastX, firstY - lastY, firstZ - lastZ) > SECTION_PLANE_TOLERANCE_MM)
      fail("loop is not closed");
    if (!Number.isFinite(area) || area === 0 || area < 0 !== rows[loop]!.is_hole)
      fail("loop area or outer/hole winding is invalid");
  }
  return {
    loopOffsets: new Uint32Array(buffer, offset + SECTION_HEADER_BYTES, loops + 1),
    pointsRelativeMm: new Float64Array(buffer, offset + pointsStart, points * 3),
    planeOriginMm: origin,
    chunkIndex: expected.chunk_index,
    firstLoopOrdinal: metadata.first_loop_ordinal,
    byteLength: length,
    schema: SECTION_SCHEMA_VERSION,
  };
}
