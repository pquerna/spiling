/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { decodeSurfaceImage, SurfaceImageDecodeError } from "../src/surface-image.ts";
import { MAX_SURFACE_IMAGE_BYTES, type ArtifactView, type SurfaceImageRecord } from "../src/generated.ts";

const fixture: {
  encoding: string;
  record: SurfaceImageRecord;
  resource: ArtifactView;
  data: string;
  pixels: number[];
} = JSON.parse(readFileSync(new URL("../../../fixtures/protocol/surface-image.json", import.meta.url), "utf8"));
assert.equal(fixture.encoding, "hex");
const golden = Uint8Array.from(Buffer.from(fixture.data, "hex")).buffer;

async function binding(bytes: ArrayBuffer, width: number, height: number) {
  const image_hash = Buffer.from(await crypto.subtle.digest("SHA-256", bytes)).toString("hex");
  const record: SurfaceImageRecord = {
    ...fixture.record, width, height, image_hash, byte_count: bytes.byteLength,
  };
  const resource: ArtifactView = {
    name: `artifacts/${image_hash}`, sha256: image_hash,
    size_bytes: String(bytes.byteLength), media_type: "image/x-portable-graymap",
  };
  return { record, resource };
}

test("native canonical PGM golden preserves asymmetric top-first row samples in borrowed bytes", async () => {
  const image = await decodeSurfaceImage(golden, fixture.record, fixture.resource);
  assert.equal(image.bytes, golden);
  assert.equal(image.pixels.buffer, golden);
  assert.equal(image.width, fixture.record.width);
  assert.equal(image.height, fixture.record.height);
  assert.deepEqual(Array.from(image.pixels), fixture.pixels);
  assert.notDeepEqual(fixture.pixels.slice(0, image.width), fixture.pixels.slice(-image.width));
});

test("Gray8 raster accepts every byte value including whitespace and PGM syntax bytes without parsing pixels", async () => {
  const header = new TextEncoder().encode("P5\n16 16\n255\n");
  const bytes = new Uint8Array(header.length + 256);
  bytes.set(header);
  for (let i = 0; i < 256; i++) bytes[header.length + i] = i;
  const { record, resource } = await binding(bytes.buffer, 16, 16);
  const image = await decodeSurfaceImage(bytes.buffer, record, resource);
  assert.deepEqual(Array.from(image.pixels), Array.from({ length: 256 }, (_, i) => i));
});

test("canonical dimension bounds include the maximum raster and reject hostile metadata before allocation", async () => {
  const header = new TextEncoder().encode("P5\n256 256\n255\n");
  const bytes = new Uint8Array(header.length + 65536);
  bytes.set(header);
  const { record, resource } = await binding(bytes.buffer, 256, 256);
  assert.equal((await decodeSurfaceImage(bytes.buffer, record, resource)).pixels.length, 65536);
  for (const width of [0, 1, 257, 0xffffffff, 2.5, NaN, Infinity]) {
    await assert.rejects(decodeSurfaceImage(golden, { ...fixture.record, width }, fixture.resource), SurfaceImageDecodeError);
  }
  for (const height of [0, 1, 257, 0xffffffff, 2.5, NaN, Infinity]) {
    await assert.rejects(decodeSurfaceImage(golden, { ...fixture.record, height }, fixture.resource), SurfaceImageDecodeError);
  }
  await assert.rejects(decodeSurfaceImage(new ArrayBuffer(MAX_SURFACE_IMAGE_BYTES + 1), record, resource), SurfaceImageDecodeError);
});

test("general PGM variations are rejected even with internally consistent hashes and descriptor sizes", async () => {
  for (const header of ["P2\n2 2\n255\n", "P5\r2 2\n255\n", "P5\n02 2\n255\n", "P5\n2 2\n256\n", "P5\n2\t2\n255\n", "P5\n#comment\n2 2\n255\n", "P5\n2 2\n255\n\n"]) {
    const encoded = Buffer.concat([Buffer.from(header), Buffer.from([0, 32, 10, 255])]);
    const bytes = Uint8Array.from(encoded).buffer;
    const { record, resource } = await binding(bytes, 2, 2);
    await assert.rejects(decodeSurfaceImage(bytes, record, resource), SurfaceImageDecodeError);
  }
});

test("truncation, trailing data and altered samples cannot enter preview", async () => {
  for (let length = 0; length < golden.byteLength; length++) {
    await assert.rejects(decodeSurfaceImage(golden.slice(0, length), fixture.record, fixture.resource), SurfaceImageDecodeError);
  }
  const trailing = new Uint8Array(golden.byteLength + 1);
  trailing.set(new Uint8Array(golden));
  await assert.rejects(decodeSurfaceImage(trailing.buffer, fixture.record, fixture.resource), SurfaceImageDecodeError);
  const altered = golden.slice(0);
  new Uint8Array(altered)[altered.byteLength - 1]! ^= 1;
  await assert.rejects(decodeSurfaceImage(altered, fixture.record, fixture.resource), /SHA-256 mismatch/);
});

test("metadata and resource identities, exact MIME and complete byte counts are mandatory", async () => {
  for (const change of [
    { name: "artifacts/other" }, { sha256: "0".repeat(64) },
    { size_bytes: `0${fixture.resource.size_bytes}` }, { media_type: "image/png" },
  ]) {
    await assert.rejects(decodeSurfaceImage(golden, fixture.record, { ...fixture.resource, ...change }), SurfaceImageDecodeError);
  }
  for (const change of [
    { image_hash: "F".repeat(64) }, { source_hash: "bad" }, { byte_count: 0 },
    { source_name: "" }, { source_name: "a\u0000b" }, { source_name: "a\u0085b" },
    { source_name: "é".repeat(129) },
  ]) {
    await assert.rejects(decodeSurfaceImage(golden, { ...fixture.record, ...change }, fixture.resource), SurfaceImageDecodeError);
  }
  await assert.rejects(decodeSurfaceImage(golden, { ...fixture.record, extra: true } as SurfaceImageRecord, fixture.resource), SurfaceImageDecodeError);
});
