/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import {
  MAX_SURFACE_IMAGE_BYTES,
  MAX_SURFACE_IMAGE_SOURCE_NAME_BYTES,
  type ArtifactView,
  type SurfaceImageRecord,
} from "./generated.ts";
import { hash, keys, uint, verifyHash } from "./display.ts";

export interface DecodedSurfaceImage {
  readonly bytes: ArrayBuffer;
  readonly width: number;
  readonly height: number;
  /** Borrowed Gray8 pixels in original source row order, top row first. */
  readonly pixels: Uint8Array<ArrayBuffer>;
}
export class SurfaceImageDecodeError extends Error {
  constructor(message: string) {
    super(`Invalid canonical height image: ${message}`);
    this.name = "SurfaceImageDecodeError";
  }
}
const fail = (message: string): never => {
  throw new SurfaceImageDecodeError(message);
};

/** Strict frozen P5 only; never normalizes a source PNG or accepts general PGM syntax. */
export async function decodeSurfaceImage(
  bytes: ArrayBuffer,
  record: SurfaceImageRecord,
  resource: ArtifactView,
): Promise<DecodedSurfaceImage> {
  keys(record, ["image_hash", "width", "height", "byte_count", "source_hash", "source_name"], fail);
  keys(resource, ["name", "size_bytes", "sha256", "media_type"], fail);
  hash(record.image_hash, fail);
  hash(record.source_hash, fail);
  uint(record.width, "image width", fail, 2, 256);
  uint(record.height, "image height", fail, 2, 256);
  if (
    typeof record.source_name !== "string" ||
    record.source_name.length === 0 ||
    record.source_name.length > MAX_SURFACE_IMAGE_SOURCE_NAME_BYTES ||
    new TextEncoder().encode(record.source_name).byteLength > MAX_SURFACE_IMAGE_SOURCE_NAME_BYTES ||
    /[\u0000-\u001f\u007f-\u009f]/.test(record.source_name)
  )
    fail("invalid source name");
  const header = new TextEncoder().encode(`P5\n${record.width} ${record.height}\n255\n`);
  const length = header.byteLength + record.width * record.height;
  if (
    !(bytes instanceof ArrayBuffer) ||
    bytes.byteLength !== length ||
    length > MAX_SURFACE_IMAGE_BYTES
  )
    fail("payload size does not exactly match bounded dimensions");
  if (
    record.byte_count !== length ||
    resource.size_bytes !== String(length) ||
    resource.sha256 !== record.image_hash ||
    resource.name !== `artifacts/${record.image_hash}` ||
    resource.media_type !== "image/x-portable-graymap"
  )
    fail("image record/resource descriptor mismatch");
  const content = new Uint8Array(bytes);
  for (let index = 0; index < header.byteLength; index += 1) {
    if (content[index] !== header[index]) fail("noncanonical P5 header");
  }
  await verifyHash(bytes, 0, length, record.image_hash, fail);
  return {
    bytes,
    width: record.width,
    height: record.height,
    pixels: new Uint8Array(bytes, header.byteLength, record.width * record.height),
  };
}
