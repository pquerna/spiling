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
async function rehash(bytes: ArrayBuffer): Promise<ReplayDisplayDescriptor> {
  const result = structuredClone(descriptor);
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
  assert.deepEqual(fixture.actual_positions.slice(0, 4), [
    [1, 1, 0.4],
    [5, 1, 0.4],
    [5, 1, 0.4],
    [5, 5, 0.4],
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
      [0, 8, 16].map((axis) => data.getFloat64(96 + 48 * event + axis, true)),
    );
    assert.equal(replay.knownStart(event), event > 0);
  }
  assert.throws(() => replay.position(-1), ReplayDecodeError);
  assert.throws(() => replay.feed(replay.eventCount), ReplayDecodeError);
  assert.throws(() => replay.layer(0.5), ReplayDecodeError);
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
  ["version", (data) => data.setUint16(4, 2, true)],
  ["header stride", (data) => data.setUint16(6, 48, true)],
  ["record stride", (data) => data.setUint32(12, 96, true)],
  ["header count", (data) => data.setUint32(8, 0xffffffff, true)],
  ["reserved header", (data) => data.setUint8(28, 1)],
  ["reserved tail", (data) => data.setUint8(95, 1)],
  ["unknown kind", (data) => data.setUint8(96 + 46, 3)],
  ["unknown flags", (data) => data.setUint8(96 + 47, 2)],
  ["invented first start", (data) => data.setUint8(96 + 47, 1)],
  ["unknown later start", (data) => data.setUint8(144 + 47, 0)],
  ["nonfinite coordinate", (data) => data.setFloat64(96, NaN, true)],
  ["negative E", (data) => data.setFloat64(96 + 24, -1, true)],
  ["nonfinite feed", (data) => data.setFloat64(96 + 32, Infinity, true)],
  ["zero feed", (data) => data.setFloat64(96 + 32, 0, true)],
  ["zero source line", (data) => data.setUint32(96 + 40, 0, true)],
  [
    "reordered source line",
    (data) => data.setUint32(144 + 40, data.getUint32(96 + 40, true), true),
  ],
  ["out of range layer", (data) => data.setUint16(96 + 44, descriptor.info.layer_count, true)],
  ["nonzero initial layer", (data) => data.setUint16(96 + 44, 1, true)],
  ["travel E change", (data) => data.setFloat64(96 + 24, 1, true)],
  ["header bound", (data) => data.setFloat64(32, data.getFloat64(32, true) - 1, true)],
];
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
      start = 96 + deposition * 48;
    if (mutation === "position") {
      for (let axis = 0; axis < 3; axis++)
        data.setFloat64(start + axis * 8, data.getFloat64(start - 48 + axis * 8, true), true);
    } else data.setFloat64(start + 24, data.getFloat64(start - 48 + 24, true), true);
    await assert.rejects(decodeReplayDisplay(bytes, await rehash(bytes)), ReplayDecodeError);
  }
});

test("replay rejects descriptor tampering, hash mismatch and trailing/truncated bodies", async () => {
  for (const mutate of [
    (value: ReplayDisplayDescriptor) => {
      value.info.event_count = 200001;
    },
    (value: ReplayDisplayDescriptor) => {
      value.info.travel_count = 100001;
    },
    (value: ReplayDisplayDescriptor) => {
      value.info.layer_count = 4097;
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

test("replay rejects loose advertised bounds and empty/skipped layers", async () => {
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
    layers.setUint16(96 + event * 48 + 44, 1, true);
  layers.setUint32(24, 2, true);
  const emptyLayer = await rehash(bytes);
  emptyLayer.info.layer_count = 2;
  await assert.rejects(decodeReplayDisplay(bytes, emptyLayer), ReplayDecodeError);
});
