/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import {
  MAX_REPLAY_DISPLAY_BYTES,
  MAX_REPLAY_EVENTS,
  REPLAY_HEADER_BYTES,
  REPLAY_RECORD_BYTES,
  MAX_PRINT_LAYERS,
  MAX_DEPOSITION_SEGMENTS,
  MAX_MANUFACTURING_BUNDLE_BYTES,
  MAX_MANUFACTURING_LABEL_BYTES,
} from "./generated.ts";
import type { ReplayDisplayDescriptor } from "./generated.ts";
import { bounds, hash, keys, uint, verifyHash, zero } from "./display.ts";

export type ReplayKind = 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12;
export const REPLAY_KIND_LABELS = [
  "travel",
  "perimeter",
  "solid_fill",
  "bottom_skin",
  "top_skin",
  "sparse_fill",
  "lifecycle_travel",
  "prime",
  "bridge",
  "support",
  "support_interface",
  "retract",
  "unretract",
] as const;
export const REPLAY_LIFECYCLE_LAYER = 65535;

export interface DecodedReplayDisplay {
  readonly bytes: ArrayBuffer;
  readonly eventCount: number;
  readonly depositionCount: number;
  readonly travelCount: number;
  readonly layerCount: number;
  readonly lifecycleCount: number;
  readonly processCount: number;
  readonly boundsMm: { min: [number, number, number]; max: [number, number, number] };
  position(index: number): [number, number, number];
  extrusion(index: number): number;
  feed(index: number): number;
  sourceLine(index: number): number;
  layer(index: number): number;
  kind(index: number): ReplayKind;
  knownStart(index: number): boolean;
}
export class ReplayDecodeError extends Error {
  constructor(message: string) {
    super(`Invalid SPLR replay: ${message}`);
    this.name = "ReplayDecodeError";
  }
}
const fail = (message: string): never => {
  throw new ReplayDecodeError(message);
};

/** Borrow immutable bytes; independent actual-program verification remains native-owned. */
export async function decodeReplayDisplay(
  bytes: ArrayBuffer,
  descriptor: ReplayDisplayDescriptor,
): Promise<DecodedReplayDisplay> {
  keys(descriptor, ["info", "resource"], fail);
  const { info, resource } = descriptor;
  keys(
    info,
    [
      "schema_version",
      "bundle_hash",
      "input_hash",
      "program_hash",
      "verifier_version",
      "frame",
      "event_count",
      "deposition_count",
      "travel_count",
      "layer_count",
      "lifecycle_count",
      "process_count",
      "bounds_mm",
    ],
    fail,
  );
  keys(resource, ["name", "size_bytes", "sha256", "media_type"], fail);
  hash(info.bundle_hash, fail);
  hash(info.input_hash, fail);
  hash(info.program_hash, fail);
  hash(resource.sha256, fail);
  if (info.schema_version !== 3 || info.frame !== "right_handed_millimetres")
    fail("unsupported schema or coordinate frame");
  if (
    typeof info.verifier_version !== "string" ||
    info.verifier_version.length === 0 ||
    new TextEncoder().encode(info.verifier_version).byteLength > MAX_MANUFACTURING_LABEL_BYTES ||
    /[\u0000-\u001f\u007f]/.test(info.verifier_version)
  )
    fail("invalid verifier version");
  uint(info.event_count, "event count", fail, 2, MAX_REPLAY_EVENTS);
  uint(info.deposition_count, "deposition count", fail, 1, MAX_DEPOSITION_SEGMENTS);
  uint(info.travel_count, "travel count", fail, 1, MAX_DEPOSITION_SEGMENTS);
  uint(info.layer_count, "layer count", fail, 1, MAX_PRINT_LAYERS);
  uint(info.lifecycle_count, "lifecycle count", fail, 0, 4);
  uint(info.process_count, "process count", fail, 0, MAX_DEPOSITION_SEGMENTS);
  if (info.lifecycle_count !== 0 && info.lifecycle_count !== 4) fail("unsupported lifecycle count");
  if (
    info.event_count !== info.deposition_count + info.travel_count + info.lifecycle_count + info.process_count
  )
    fail("count mismatch");
  bounds(info.bounds_mm, fail);
  const length = REPLAY_HEADER_BYTES + REPLAY_RECORD_BYTES * info.event_count;
  if (
    !(bytes instanceof ArrayBuffer) ||
    bytes.byteLength !== length ||
    length > MAX_REPLAY_DISPLAY_BYTES
  )
    fail("payload size does not exactly match bounded counts");
  if (
    resource.size_bytes !== String(length) ||
    resource.media_type !== "application/x-spiling-replay-motion" ||
    resource.name !== `artifacts/${resource.sha256}`
  )
    fail("resource descriptor mismatch");
  const data = new DataView(bytes, 0, length);
  if (
    data.getUint32(0, true) !== 0x524c5053 ||
    data.getUint16(4, true) !== 3 ||
    data.getUint16(6, true) !== REPLAY_HEADER_BYTES ||
    data.getUint32(12, true) !== REPLAY_RECORD_BYTES
  )
    fail("magic, schema or stride mismatch");
  for (const [offset, value] of [
    [8, info.event_count],
    [16, info.deposition_count],
    [20, info.travel_count],
    [24, info.layer_count],
    [28, info.lifecycle_count],
    [88, info.process_count],
  ])
    if (data.getUint32(offset!, true) !== value) fail("header/descriptor count mismatch");
  zero(data, 80, 88, fail);
  zero(data, 92, REPLAY_HEADER_BYTES, fail);
  const min: [number, number, number] = [Infinity, Infinity, Infinity];
  const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];
  for (let axis = 0; axis < 3; axis++) {
    if (
      data.getFloat64(32 + axis * 8, true) !== info.bounds_mm.min[axis] ||
      data.getFloat64(56 + axis * 8, true) !== info.bounds_mm.max[axis]
    )
      fail("header bounds mismatch");
  }
  let deposition = 0,
    travel = 0,
    lifecycle = 0,
    lifecyclePhase = 0,
    lastLayer = 0,
    layerDepositions = 0,
    lastLine = 0,
    lastE = 0;
  let process = 0;
  let retractedE: number | undefined;
  for (let index = 0; index < info.event_count; index++) {
    const start = REPLAY_HEADER_BYTES + index * REPLAY_RECORD_BYTES;
    let moved = false;
    for (let axis = 0; axis < 3; axis++) {
      const coordinate = data.getFloat64(start + axis * 8, true);
      if (!Number.isFinite(coordinate)) fail("nonfinite position");
      min[axis] = Math.min(min[axis]!, coordinate);
      max[axis] = Math.max(max[axis]!, coordinate);
      if (index > 0 && coordinate !== data.getFloat64(start - REPLAY_RECORD_BYTES + axis * 8, true))
        moved = true;
    }
    const e = data.getFloat64(start + 24, true),
      f = data.getFloat64(start + 32, true);
    const line = data.getUint32(start + 40, true),
      layer = data.getUint16(start + 44, true);
    const kind = data.getUint8(start + 46),
      flags = data.getUint8(start + 47);
    if (!Number.isFinite(e) || (e < 0 && kind !== 11 && retractedE === undefined) || !Number.isFinite(f) || f <= 0)
      fail("invalid extrusion or feed");
    uint(line, "source line", fail, 1, MAX_MANUFACTURING_BUNDLE_BYTES);
    if (line <= lastLine) fail("source line sequence is not increasing");
    if (kind > 12 || flags !== (index === 0 ? 0 : 1)) fail("unknown kind or invalid start flags");
    if (index === 0 && kind !== 0 && kind !== 6)
      fail("first endpoint must be unknown-start travel");
    if (kind === 6 || kind === 7) {
      if (info.lifecycle_count !== 4 || layer !== REPLAY_LIFECYCLE_LAYER)
        fail("invalid lifecycle layer attribution");
      if (
        kind !== (lifecyclePhase === 1 ? 7 : 6) ||
        lifecyclePhase >= 4 ||
        retractedE !== undefined ||
        (lifecyclePhase < 2 && deposition !== 0) ||
        (lifecyclePhase >= 2 &&
          (deposition !== info.deposition_count || travel !== info.travel_count))
      )
        fail("invalid lifecycle sequence");
      if (
        lifecyclePhase === 2 &&
        (data.getFloat64(start, true) !== data.getFloat64(start - 48, true) ||
          data.getFloat64(start + 8, true) !== data.getFloat64(start - 40, true) ||
          data.getFloat64(start + 16, true) <= data.getFloat64(start - 32, true))
      )
        fail("final lift must be vertical with increasing Z");
      if (
        lifecyclePhase === 3 &&
        data.getFloat64(start + 16, true) !== data.getFloat64(start - 32, true)
      )
        fail("park must retain clearance Z");
      if (kind === 7) {
        if (index === 0 || !moved || e <= lastE)
          fail("prime needs known nonzero movement and increasing extrusion");
      } else if (e !== lastE) fail("lifecycle travel changes extrusion");
      lifecycle++;
      lifecyclePhase++;
    } else {
      if (info.lifecycle_count === 4 && lifecyclePhase !== 2)
        fail("body outside lifecycle boundaries");
      const startupRetraction = kind === 11 && process === 0 &&
        info.lifecycle_count === 4 && lifecyclePhase === 2 && index > 0;
      if (
        layer >= info.layer_count ||
        (deposition === 0 && travel === 0 && ((!startupRetraction && kind !== 0) || layer !== 0))
      )
        fail("body must start with layer-zero travel or complete-job retraction");
      if (layer !== lastLayer) {
        if (layer <= lastLayer || (travel !== 0 && layerDepositions === 0) || (kind !== 0 && kind !== 11))
          fail("invalid layer sequence");
        layerDepositions = 0;
        lastLayer = layer;
      }
      if (kind === 0) {
        travel++;
        if (e !== lastE) fail("travel changes extrusion");
      } else if (kind === 11 || kind === 12) {
        if (index === 0 || moved) fail("process extrusion must be known and stationary");
        if (kind === 11) {
          if (retractedE !== undefined || e >= lastE) fail("invalid retraction");
          retractedE = lastE;
        } else {
          if (e <= lastE || retractedE !== e) fail("invalid recovery");
          retractedE = undefined;
        }
        process++;
      } else {
        if (index === 0 || !moved || e <= lastE || retractedE !== undefined)
          fail("deposition needs known nonzero movement and recovered increasing extrusion");
        deposition++;
        layerDepositions++;
      }
    }
    lastE = e;
    lastLine = line;
  }
  if (
    deposition !== info.deposition_count ||
    travel !== info.travel_count ||
    lifecycle !== info.lifecycle_count ||
    process !== info.process_count ||
    retractedE !== undefined ||
    lastLayer + 1 !== info.layer_count ||
    layerDepositions === 0
  )
    fail("actual counts or layers mismatch");
  for (let axis = 0; axis < 3; axis++)
    if (min[axis] !== info.bounds_mm.min[axis] || max[axis] !== info.bounds_mm.max[axis])
      fail("bounds do not exactly cover actual endpoints");
  await verifyHash(bytes, 0, length, resource.sha256, fail);
  const offset = (index: number): number => {
    uint(index, "event index", fail, 0, info.event_count - 1);
    return REPLAY_HEADER_BYTES + index * REPLAY_RECORD_BYTES;
  };
  return Object.freeze({
    bytes,
    eventCount: info.event_count,
    depositionCount: deposition,
    travelCount: travel,
    lifecycleCount: lifecycle,
    processCount: process,
    layerCount: info.layer_count,
    boundsMm: { min, max },
    position(index: number): [number, number, number] {
      const start = offset(index);
      return [
        data.getFloat64(start, true),
        data.getFloat64(start + 8, true),
        data.getFloat64(start + 16, true),
      ];
    },
    extrusion: (index: number) => data.getFloat64(offset(index) + 24, true),
    feed: (index: number) => data.getFloat64(offset(index) + 32, true),
    sourceLine: (index: number) => data.getUint32(offset(index) + 40, true),
    layer: (index: number) => data.getUint16(offset(index) + 44, true),
    kind: (index: number) => data.getUint8(offset(index) + 46) as ReplayKind,
    knownStart: (index: number) => data.getUint8(offset(index) + 47) === 1,
  });
}
