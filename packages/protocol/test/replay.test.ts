/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { decodeReplayDisplay, ReplayDecodeError } from "../src/replay.ts";
import type { ReplayDisplayDescriptor } from "../src/generated.ts";

// Produced by the native independent inspector, not a TypeScript invented verified model.
const fixture: {
  encoding: string;
  descriptor: ReplayDisplayDescriptor;
  data: string;
  actual_positions: number[][];
  program: string;
} = JSON.parse(
  readFileSync(new URL("../../../fixtures/protocol/replay.json", import.meta.url), "utf8"),
);
assert.equal(fixture.encoding, "hex");
const golden = Uint8Array.from(Buffer.from(fixture.data, "hex")).buffer;
const descriptor = fixture.descriptor;
async function rehash(bytes: ArrayBuffer, base = descriptor): Promise<ReplayDisplayDescriptor> {
  const result = structuredClone(base);
  result.resource.sha256 = Buffer.from(await crypto.subtle.digest("SHA-256", bytes)).toString(
    "hex",
  );
  result.resource.name = `artifacts/${result.resource.sha256}`;
  return result;
}

test("native actual replay exposes borrowed strided f64 bytes and unknown first endpoint", async () => {
  const replay = await decodeReplayDisplay(golden, descriptor);
  assert.equal(replay.bytes, golden);
  assert.equal(replay.eventCount, descriptor.info.event_count);
  assert.equal(replay.processCount, descriptor.info.process_count);
  assert.equal(replay.lifecycleCount, descriptor.info.lifecycle_count);
  assert.deepEqual(fixture.actual_positions.slice(0, 4), [
    [1, 1, 0.4],
    [5, 1, 0.4],
    [5, 1, 0.4],
    [9, 1, 0.4],
  ]);
  assert.deepEqual(
    [0, 1, 2, 3].map((event) => replay.position(event)),
    fixture.actual_positions.slice(0, 4),
  );
  assert.equal(replay.position(0)[0], 1); // Actual emitted rounding, not planner's 1.0000004.
  assert.equal(replay.kind(0), 0);
  assert.equal(replay.knownStart(0), false);
  assert.equal(replay.extrusion(0), 0);
  const data = new DataView(golden);
  for (let event = 0; event < replay.eventCount; event++) {
    assert.deepEqual(
      replay.position(event),
      [0, 8, 16].map((axis) => data.getFloat64(96 + 80 * event + axis, true)),
    );
    assert.equal(replay.knownStart(event), event > 0);
  }
  assert.throws(() => replay.position(-1), ReplayDecodeError);
  assert.throws(() => replay.feed(replay.eventCount), ReplayDecodeError);
  assert.throws(() => replay.layer(0.5), ReplayDecodeError);
});

test("native replay preserves actual program absolute E and source lines", async () => {
  const replay = await decodeReplayDisplay(golden, descriptor);
  const motion = fixture.program.split("\n")
    .map((line, index) => ({ line, sourceLine: index + 1 }))
    .filter(({ line }) => line.startsWith("G0 X") || line.startsWith("G1 X"));
  assert.equal(motion.length, replay.eventCount);
  let extrusion = 0;
  for (let event = 0; event < motion.length; event++) {
    const command = motion[event]!;
    const token = command.line.match(/(?:^| )E(-?\d+(?:\.\d+)?)(?= |$)/);
    if (token) extrusion = Number(token[1]);
    assert.equal(replay.extrusion(event), extrusion);
    assert.equal(replay.sourceLine(event), command.sourceLine);
  }
});

test("native coarse sparse golden distinguishes associated stroke height from fine schedule", async () => {
  const fixture: {
    encoding: string; descriptor: ReplayDisplayDescriptor; data: string;
    actual_positions: number[][]; program: string;
  } = JSON.parse(readFileSync(new URL("../../../fixtures/protocol/replay-sparse.json", import.meta.url), "utf8"));
  assert.equal(fixture.encoding, "hex");
  const bytes = Uint8Array.from(Buffer.from(fixture.data, "hex")).buffer;
  const replay = await decodeReplayDisplay(bytes, fixture.descriptor);
  assert.equal(replay.layerCount, 3);
  let coarse = 0, fine = 0;
  for (let event = 0; event < replay.eventCount; event++) {
    assert.deepEqual(replay.position(event), fixture.actual_positions[event]);
    const association = replay.association(event);
    if (!association) {
      assert.ok([0, 6, 7, 11, 12].includes(replay.kind(event)));
      continue;
    }
    assert.equal(association.beadWidthMm, 0.45);
    const height = association.physicalZMm[1] - association.physicalZMm[0];
    if (replay.kind(event) === 5) {
      assert.deepEqual(association.physicalZMm, [0.1, 0.3]);
      assert.equal(replay.layer(event), 2);
      assert.ok(height > 0.1, "coarse association spans fine schedule layers");
      coarse++;
    } else {
      assert.ok(Math.abs(height - 0.1) < 1e-12, "walls/skins retain fine associated height");
      fine++;
    }
  }
  assert.ok(coarse > 0 && fine > 0);
});

test("native known coincident travel remains a valid event without invented movement", async () => {
  const replay = await decodeReplayDisplay(golden, descriptor);
  let coincident = false;
  for (let event = 1; event < replay.eventCount; event++) {
    if (
      replay.kind(event) === 0 &&
      replay.position(event).every((value, axis) => value === replay.position(event - 1)[axis])
    ) {
      assert.equal(replay.knownStart(event), true);
      assert.equal(replay.extrusion(event), replay.extrusion(event - 1));
      coincident = true;
    }
  }
  assert.ok(coincident, "native fixture must include known coincident travel");
});

const corruptions: [string, (data: DataView) => void][] = [
  ["magic", (data) => data.setUint8(0, 0)],
  ["version", (data) => data.setUint16(4, 1, true)],
  ["header stride", (data) => data.setUint16(6, 80, true)],
  ["record stride", (data) => data.setUint32(12, 96, true)],
  ["header count", (data) => data.setUint32(8, 0xffffffff, true)],
  ["lifecycle header count", (data) => data.setUint32(28, 1, true)],
  ["reserved tail", (data) => data.setUint8(95, 1)],
  ["unknown kind", (data) => data.setUint8(96 + 46, 0xff)],
  ["unknown flags", (data) => data.setUint8(96 + 47, 2)],
  ["invented first start", (data) => data.setUint8(96 + 47, 1)],
  ["unknown later start", (data) => data.setUint8(176 + 47, 0)],
  ["nonfinite coordinate", (data) => data.setFloat64(96, NaN, true)],
  ["negative E", (data) => data.setFloat64(96 + 24, -1, true)],
  ["nonfinite feed", (data) => data.setFloat64(96 + 32, Infinity, true)],
  ["zero feed", (data) => data.setFloat64(96 + 32, 0, true)],
  ["zero source line", (data) => data.setUint32(96 + 40, 0, true)],
  [
    "reordered source line",
    (data) => data.setUint32(176 + 40, data.getUint32(96 + 40, true), true),
  ],
  ["out of range layer", (data) => data.setUint16(96 + 44, descriptor.info.layer_count, true)],
  ["nonzero initial layer", (data) => data.setUint16(96 + 44, 1, true)],
  ["travel E change", (data) => data.setFloat64(96 + 24, 1, true)],
  ["header bound", (data) => data.setFloat64(32, data.getFloat64(32, true) - 1, true)],
];
test("nominal associations are strict and separate from byte-derived coordinates", async () => {
  const replay = await decodeReplayDisplay(golden, descriptor);
  assert.equal(replay.association(0), null);
  const deposition = Array.from({ length: replay.eventCount }, (_, event) => event)
    .find(event => replay.association(event) !== null)!;
  const association = replay.association(deposition)!;
  assert.ok(association.beadWidthMm > 0);
  assert.ok(Math.abs(association.physicalZMm[1] - replay.position(deposition)[2]) <= 0.000000500001);
  assert.ok(association.physicalZMm[0] < association.physicalZMm[1]);
  for (const [event, offset, value] of [
    [0, 72, 0], [0, 48, 1], [deposition, 48, 0],
    [deposition, 56, Infinity], [deposition, 64, 42],
  ]) {
    const bytes = golden.slice(0), data = new DataView(bytes);
    const start = 96 + event! * 80 + offset!;
    if (offset === 72) data.setUint32(start, value!, true);
    else data.setFloat64(start, value!, true);
    await assert.rejects(decodeReplayDisplay(bytes, await rehash(bytes)), ReplayDecodeError);
  }
});
for (const [name, mutate] of corruptions) {
  test(`replay rejects ${name} even with matching resource hash`, async () => {
    const bytes = golden.slice(0);
    mutate(new DataView(bytes));
    await assert.rejects(decodeReplayDisplay(bytes, await rehash(bytes)), ReplayDecodeError);
  });
}

test("replay rejects zero-distance deposition and unchanged deposition E", async () => {
  const replay = await decodeReplayDisplay(golden, descriptor);
  let deposition = 1;
  while (replay.kind(deposition) === 0) deposition++;
  for (const mutation of ["position", "extrusion"]) {
    const bytes = golden.slice(0),
      data = new DataView(bytes),
      start = 96 + deposition * 80;
    if (mutation === "position") {
      for (let axis = 0; axis < 3; axis++)
        data.setFloat64(start + axis * 8, data.getFloat64(start - 80 + axis * 8, true), true);
    } else data.setFloat64(start + 24, data.getFloat64(start - 80 + 24, true), true);
    await assert.rejects(decodeReplayDisplay(bytes, await rehash(bytes)), ReplayDecodeError);
  }
});

test("replay rejects descriptor tampering, hash mismatch and trailing/truncated bodies", async () => {
  for (const mutate of [
    (value: ReplayDisplayDescriptor) => {
      value.info.event_count = 300005;
    },
    (value: ReplayDisplayDescriptor) => {
      value.info.travel_count = 100001;
    },
    (value: ReplayDisplayDescriptor) => {
      value.info.layer_count = 4097;
    },
    (value: ReplayDisplayDescriptor) => {
      value.info.lifecycle_count = 2;
    },
    (value: ReplayDisplayDescriptor) => {
      value.info.process_count = 100001;
    },
    (value: ReplayDisplayDescriptor) => {
      value.info.program_hash = "not-a-hash";
    },
    (value: ReplayDisplayDescriptor) => {
      value.resource.media_type = "application/octet-stream";
    },
    (value: ReplayDisplayDescriptor) => {
      value.resource.size_bytes = `0${golden.byteLength}`;
    },
    (value: ReplayDisplayDescriptor) => {
      value.resource.sha256 = "0".repeat(64);
      value.resource.name = `artifacts/${value.resource.sha256}`;
    },
    (value: ReplayDisplayDescriptor) => {
      value.info.bounds_mm.max[0] += 1;
    },
  ]) {
    const value = structuredClone(descriptor);
    mutate(value);
    await assert.rejects(decodeReplayDisplay(golden, value), ReplayDecodeError);
  }
  await assert.rejects(decodeReplayDisplay(golden.slice(0, -1), descriptor), ReplayDecodeError);
  await assert.rejects(
    decodeReplayDisplay(new ArrayBuffer(golden.byteLength + 1), descriptor),
    ReplayDecodeError,
  );
});

test("replay rejects loose bounds and attribution without printed layer endpoints", async () => {
  const loose = golden.slice(0),
    data = new DataView(loose);
  data.setFloat64(32, data.getFloat64(32, true) - 1, true);
  const looseDescriptor = await rehash(loose);
  looseDescriptor.info.bounds_mm.min[0] -= 1;
  await assert.rejects(decodeReplayDisplay(loose, looseDescriptor), ReplayDecodeError);

  const bytes = golden.slice(0),
    layers = new DataView(bytes);
  const replay = await decodeReplayDisplay(golden, descriptor);
  for (let event = 1; event < replay.eventCount; event++)
    layers.setUint16(96 + event * 80 + 44, 1, true);
  layers.setUint32(24, 2, true);
  const emptyLayer = await rehash(bytes);
  emptyLayer.info.layer_count = 2;
  await assert.rejects(decodeReplayDisplay(bytes, emptyLayer), ReplayDecodeError);
});
test("packed schedule may skip internal contact gaps without inventing events", async () => {
  // Structural admission only; native plan replay proves actual contact-gap semantics.
  const bytes = golden.slice(0), data = new DataView(bytes);
  data.setUint32(24, 3, true);
  data.setUint16(96 + 2 * 80 + 44, 2, true);
  data.setUint16(96 + 3 * 80 + 44, 2, true);
  const value = structuredClone(descriptor);
  value.info.layer_count = 3;
  const replay = await decodeReplayDisplay(bytes, await rehash(bytes, value));
  assert.equal(replay.layerCount, 3);
  assert.deepEqual([0, 1, 2, 3].map((event) => replay.layer(event)), [0, 0, 2, 2]);
  const trailing = structuredClone(value);
  trailing.info.layer_count = 4;
  data.setUint32(24, 4, true);
  await assert.rejects(decodeReplayDisplay(bytes, await rehash(bytes, trailing)), ReplayDecodeError);
});


test("stationary extrusion and new deposition roles are independently admitted", async () => {
  const native = await decodeReplayDisplay(golden, descriptor);
  const bytes = new ArrayBuffer(golden.byteLength + 160);
  const raw = new Uint8Array(bytes), data = new DataView(bytes);
  raw.set(new Uint8Array(golden, 0, 176));
  raw.set(new Uint8Array(golden, 176), 336);
  const value = structuredClone(descriptor);
  value.info.event_count += 2;
  value.info.process_count += 2;
  value.resource.size_bytes = String(bytes.byteLength);
  data.setUint32(8, value.info.event_count, true);
  data.setUint32(88, value.info.process_count, true);
  for (let event = 1; event <= 2; event++) {
    const start = 96 + event * 80;
    raw.set(new Uint8Array(golden, 96, 80), start);
    data.setFloat64(start + 24, event === 1 ? native.extrusion(0) - 0.8 : native.extrusion(0), true);
    data.setUint32(start + 40, native.sourceLine(0) + event, true);
    data.setUint8(start + 46, event === 1 ? 11 : 12);
    data.setUint8(start + 47, 1);
    data.setUint32(start + 72, 0xffffffff, true);
  }
  for (let event = 3; event < value.info.event_count; event++) {
    const start = 96 + event * 80;
    data.setUint32(start + 40, data.getUint32(start + 40, true) + 2, true);
  }
  const admitted = await decodeReplayDisplay(bytes, await rehash(bytes, value));
  assert.equal(admitted.processCount, 2);
  assert.deepEqual(admitted.position(1), admitted.position(0));
  assert.deepEqual(admitted.position(2), admitted.position(1));
  for (const [offset, mutation] of [
    [176, 1.1], // retract must not move
    [256 + 24, 0.1], // recovery cannot add deposited filament
    [176 + 24, 0], // retract must decrease E
  ]) {
    const corrupted = bytes.slice(0);
    new DataView(corrupted).setFloat64(offset!, mutation!, true);
    await assert.rejects(decodeReplayDisplay(corrupted, await rehash(corrupted, value)), ReplayDecodeError);
  }
  for (const role of [8, 9, 10, 13]) {
    const roleBytes = bytes.slice(0);
    new DataView(roleBytes).setUint8(336 + 46, role);
    const decoded = await decodeReplayDisplay(roleBytes, await rehash(roleBytes, value));
    assert.equal(decoded.kind(3), role);
  }
  const countBytes = bytes.slice(0);
  new DataView(countBytes).setUint32(88, 1, true);
  await assert.rejects(decodeReplayDisplay(countBytes, await rehash(countBytes, value)), ReplayDecodeError);
});

// Compose structural packed-replay cases around native body bytes. This does not assert
// that this composition is a native-verified executable machine program.
async function lifecycleCase(): Promise<{
  bytes: ArrayBuffer;
  descriptor: ReplayDisplayDescriptor;
}> {
  const body = await decodeReplayDisplay(golden, descriptor);
  assert.equal(body.lifecycleCount, 0);
  const bytes = new ArrayBuffer(golden.byteLength + 4 * 80);
  const raw = new Uint8Array(bytes),
    data = new DataView(bytes);
  raw.set(new Uint8Array(golden, 0, 96));
  raw.set(new Uint8Array(golden, 96), 96 + 2 * 80);
  const events = body.eventCount + 4;
  data.setUint32(8, events, true);
  data.setUint32(28, 4, true);
  const record = (event: number, point: number[], e: number, line: number, kind: number) => {
    const start = 96 + event * 80;
    point.forEach((coordinate, axis) => data.setFloat64(start + axis * 8, coordinate, true));
    data.setFloat64(start + 24, e, true);
    data.setFloat64(start + 32, 600, true);
    data.setUint32(start + 40, line, true);
    data.setUint16(start + 44, 65535, true);
    data.setUint8(start + 46, kind);
    data.setUint8(start + 47, event === 0 ? 0 : 1);
    data.setUint32(start + 72, 0xffffffff, true);
  };
  record(0, [0, 0, 0.2], 0, 1, 6);
  record(1, [0, 1, 0.2], 1, 2, 7);
  for (let event = 2; event < events - 2; event++) {
    const start = 96 + event * 80;
    data.setFloat64(start + 24, data.getFloat64(start + 24, true) + 1, true);
    data.setUint32(start + 40, data.getUint32(start + 40, true) + 2, true);
    data.setUint8(start + 47, data.getUint8(start + 47) | 1);
  }
  const last = body.position(body.eventCount - 1),
    clearance = body.boundsMm.max[2] + 1;
  const lastE = body.extrusion(body.eventCount - 1) + 1;
  const lastLine = body.sourceLine(body.eventCount - 1) + 2;
  record(events - 2, [last[0], last[1], clearance], lastE, lastLine + 1, 6);
  record(events - 1, [0, 0, clearance], lastE, lastLine + 2, 6);
  const value = structuredClone(descriptor);
  value.info.event_count = events;
  value.info.lifecycle_count = 4;
  value.resource.size_bytes = String(bytes.byteLength);
  for (let axis = 0; axis < 3; axis++) {
    let min = Infinity,
      max = -Infinity;
    for (let event = 0; event < events; event++) {
      const coordinate = data.getFloat64(96 + event * 80 + axis * 8, true);
      min = Math.min(min, coordinate);
      max = Math.max(max, coordinate);
    }
    value.info.bounds_mm.min[axis] = min;
    value.info.bounds_mm.max[axis] = max;
    data.setFloat64(32 + axis * 8, min, true);
    data.setFloat64(56 + axis * 8, max, true);
  }
  return { bytes, descriptor: await rehash(bytes, value) };
}
test("complete body admits actual stationary startup retraction after lifecycle prime", async () => {
  const complete = await lifecycleCase();
  const before = await decodeReplayDisplay(complete.bytes, complete.descriptor);
  const bytes = new ArrayBuffer(complete.bytes.byteLength + 160);
  const raw = new Uint8Array(bytes), data = new DataView(bytes);
  raw.set(new Uint8Array(complete.bytes, 0, 256));
  // Lifecycle prime supplies the known XYZ for the first body feeder-only event.
  raw.set(new Uint8Array(complete.bytes, 176, 80), 256);
  data.setFloat64(256 + 24, before.extrusion(1) - 0.8, true);
  data.setUint16(256 + 44, 0, true);
  data.setUint8(256 + 46, 11);
  raw.set(new Uint8Array(complete.bytes, 256, 80), 336);
  data.setFloat64(336 + 24, before.extrusion(1) - 0.8, true);
  raw.set(new Uint8Array(complete.bytes, 256, 80), 416);
  data.setUint8(416 + 46, 12);
  raw.set(new Uint8Array(complete.bytes, 336), 496);
  const value = structuredClone(complete.descriptor);
  value.info.event_count += 2;
  value.info.process_count += 2;
  value.resource.size_bytes = String(bytes.byteLength);
  data.setUint32(8, value.info.event_count, true);
  data.setUint32(88, value.info.process_count, true);
  for (let event = 0; event < value.info.event_count; event++)
    data.setUint32(96 + event * 80 + 40, event + 1, true);
  const admitted = await decodeReplayDisplay(bytes, await rehash(bytes, value));
  assert.deepEqual([2, 3, 4].map((event) => admitted.kind(event)), [11, 0, 12]);
  assert.deepEqual(admitted.position(2), admitted.position(1));
  assert.deepEqual(admitted.position(4), admitted.position(3));
  assert.equal(admitted.processCount, 2);
  assert.equal(admitted.extrusion(4), admitted.extrusion(1));
  for (const [offset, byte] of [[256 + 47, 0], [256 + 44, 1], [256 + 46, 12]]) {
    const corrupted = bytes.slice(0);
    new DataView(corrupted).setUint8(offset!, byte!);
    await assert.rejects(decodeReplayDisplay(corrupted, await rehash(corrupted, value)), ReplayDecodeError);
  }
  const moved = bytes.slice(0);
  new DataView(moved).setFloat64(256, before.position(1)[0] + 0.1, true);
  await assert.rejects(decodeReplayDisplay(moved, await rehash(moved, value)), ReplayDecodeError);
  const extraRecovery = bytes.slice(0);
  new DataView(extraRecovery).setFloat64(416 + 24, before.extrusion(1) + 0.1, true);
  await assert.rejects(decodeReplayDisplay(extraRecovery, await rehash(extraRecovery, value)), ReplayDecodeError);
});


test("lifecycle events retain unknown start and no object-layer attribution or object counts", async () => {
  const value = await lifecycleCase();
  const replay = await decodeReplayDisplay(value.bytes, value.descriptor);
  assert.equal(replay.lifecycleCount, 4);
  assert.equal(replay.depositionCount, descriptor.info.deposition_count);
  assert.equal(replay.travelCount, descriptor.info.travel_count);
  assert.equal(replay.layerCount, descriptor.info.layer_count);
  assert.equal(replay.eventCount, replay.depositionCount + replay.travelCount + 4);
  assert.equal(replay.knownStart(0), false);
  assert.equal(replay.knownStart(2), true);
  assert.deepEqual(
    [0, 1, replay.eventCount - 2, replay.eventCount - 1].map((event) => replay.kind(event)),
    [6, 7, 6, 6],
  );
  assert.deepEqual(
    [0, 1, replay.eventCount - 2, replay.eventCount - 1].map((event) => replay.layer(event)),
    [65535, 65535, 65535, 65535],
  );
  assert.deepEqual(replay.boundsMm, value.descriptor.info.bounds_mm);
});

test("lifecycle rejects false layers, misplaced prime, invented homing start, lift/park and count corruption", async () => {
  const value = await lifecycleCase();
  const count = value.descriptor.info.event_count;
  const mutations: ((data: DataView) => void)[] = [
    (data) => data.setUint16(96 + 44, 0, true),
    (data) => data.setUint16(96 + 2 * 80 + 44, 65535, true),
    (data) => data.setUint8(96 + 47, 1),
    (data) => data.setUint8(96 + 80 + 47, 0),
    (data) => data.setUint8(96 + 80 + 46, 6),
    (data) => data.setUint8(96 + (count - 1) * 80 + 46, 7),
    (data) => data.setFloat64(96 + 80 + 24, 0, true),
    (data) => {
      for (let axis = 0; axis < 3; axis++)
        data.setFloat64(96 + 80 + axis * 8, data.getFloat64(96 + axis * 8, true), true);
    },
    (data) => data.setFloat64(96 + (count - 2) * 80 + 24, 100, true),
    (data) => data.setFloat64(96 + (count - 2) * 80, 100, true),
    (data) => data.setFloat64(96 + (count - 1) * 80 + 16, 0, true),
    (data) => data.setUint32(28, 0, true),
  ];
  for (const mutate of mutations) {
    const bytes = value.bytes.slice(0);
    mutate(new DataView(bytes));
    await assert.rejects(
      decodeReplayDisplay(bytes, await rehash(bytes, value.descriptor)),
      ReplayDecodeError,
    );
  }
  const loose = value.bytes.slice(0);
  const data = new DataView(loose);
  const looseDescriptor = structuredClone(value.descriptor);
  looseDescriptor.info.bounds_mm.min[0] -= 1;
  data.setFloat64(32, looseDescriptor.info.bounds_mm.min[0], true);
  await assert.rejects(
    decodeReplayDisplay(loose, await rehash(loose, looseDescriptor)),
    ReplayDecodeError,
  );
});

test("skin and sparse roles preserve protected object layer and deposition semantics", async () => {
  const body = await decodeReplayDisplay(golden, descriptor);
  let deposition = 1;
  while (body.kind(deposition) === 0) deposition++;
  for (const kind of [3, 4, 5]) {
    const bytes = golden.slice(0),
      data = new DataView(bytes),
      start = 96 + deposition * 80;
    data.setUint8(start + 46, kind);
    const replay = await decodeReplayDisplay(bytes, await rehash(bytes));
    assert.equal(replay.kind(deposition), kind);
    assert.equal(replay.depositionCount, body.depositionCount);
    assert.equal(replay.layer(deposition), body.layer(deposition));
    data.setUint16(start + 44, 65535, true);
    await assert.rejects(decodeReplayDisplay(bytes, await rehash(bytes)), ReplayDecodeError);
  }
});

test("a new object layer must begin with travel after the prior layer deposited", async () => {
  const bytes = golden.slice(0),
    data = new DataView(bytes);
  const body = await decodeReplayDisplay(golden, descriptor);
  let deposition = 1;
  while (body.kind(deposition) === 0) deposition++;
  deposition++;
  while (body.kind(deposition) === 0) deposition++;
  for (let event = deposition; event < body.eventCount; event++)
    data.setUint16(96 + event * 80 + 44, 1, true);
  data.setUint32(24, 2, true);
  const value = await rehash(bytes);
  value.info.layer_count = 2;
  await assert.rejects(decodeReplayDisplay(bytes, value), ReplayDecodeError);
});
