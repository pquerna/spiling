/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  decodeSection,
  SectionDecodeError,
  validateSectionManifest,
  validateSectionSummary,
} from "../src/section.ts";
import { MAX_GEOMETRY_CHUNK_BYTES, MAX_LOOP_POINTS, MAX_SECTION_BYTES } from "../src/generated.ts";
import type {
  SectionChunkMetadata,
  SectionLoopMetadata,
  SectionSummary,
} from "../src/generated.ts";

type SectionFixture = {
  encoding: string;
  summary: SectionSummary;
  loops: SectionLoopMetadata[];
  chunks: { metadata: SectionChunkMetadata; data: string }[];
};
const fixture: SectionFixture = JSON.parse(
  readFileSync(new URL("../../../fixtures/protocol/section.json", import.meta.url), "utf8"),
);
const multichunk: SectionFixture = JSON.parse(
  readFileSync(
    new URL("../../../fixtures/protocol/section-multichunk.json", import.meta.url),
    "utf8",
  ),
);
assert.equal(fixture.encoding, "hex");
assert.equal(multichunk.encoding, "hex");
const metadata = fixture.chunks[0]!.metadata;
const summary = fixture.summary;
const golden = Uint8Array.from(Buffer.from(fixture.chunks[0]!.data, "hex")).buffer;
const expected = {
  session_id: summary.session_id,
  artifact_id: summary.artifact_id,
  revision: summary.revision,
  chunk_index: 0,
};

async function rehash(
  bytes: ArrayBuffer,
  descriptor: SectionChunkMetadata,
): Promise<SectionChunkMetadata> {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return { ...descriptor, sha256: Buffer.from(digest).toString("hex") };
}

test("Rust-owned algebraic outer/hole section returns borrowed f64 coordinates", async () => {
  validateSectionManifest(
    summary,
    fixture.chunks.map((chunk) => chunk.metadata),
    fixture.loops,
    expected,
  );
  const decoded = await decodeSection(golden, metadata, summary, fixture.loops, expected);
  assert.equal(decoded.schema, 1);
  assert.equal(decoded.byteLength, 320);
  assert.equal(decoded.loopOffsets.buffer, golden);
  assert.equal(decoded.pointsRelativeMm.buffer, golden);
  assert.equal(decoded.loopOffsets.byteOffset, 64);
  assert.equal(decoded.pointsRelativeMm.byteOffset, 80);
  assert.deepEqual(Array.from(decoded.loopOffsets), [0, 5, 10]);
  assert.deepEqual(decoded.planeOriginMm, [0, 0, 4]);
  assert.deepEqual(
    Array.from(decoded.pointsRelativeMm.subarray(0, 9)),
    [0, 0, 0, 20, 0, 0, 20, 10, 0],
  );
});

test("Rust-owned multichunk diagnostic preserves complete contiguous loops", async () => {
  const identity = {
    session_id: multichunk.summary.session_id,
    artifact_id: multichunk.summary.artifact_id,
    revision: multichunk.summary.revision,
  };
  validateSectionManifest(
    multichunk.summary,
    multichunk.chunks.map((chunk) => chunk.metadata),
    multichunk.loops,
    identity,
  );
  for (const chunk of multichunk.chunks) {
    const bytes = Uint8Array.from(Buffer.from(chunk.data, "hex")).buffer;
    const rows = multichunk.loops.slice(
      chunk.metadata.first_loop_ordinal,
      chunk.metadata.first_loop_ordinal + chunk.metadata.loop_count,
    );
    const decoded = await decodeSection(bytes, chunk.metadata, multichunk.summary, rows, {
      ...identity,
      chunk_index: chunk.metadata.chunk_index,
    });
    assert.equal(decoded.firstLoopOrdinal, chunk.metadata.chunk_index);
    assert.deepEqual(Array.from(decoded.loopOffsets), [0, 5]);
    assert.equal(decoded.pointsRelativeMm.byteOffset, 72);
    assert.equal(decoded.pointsRelativeMm.buffer, bytes);
  }
});

const corruptions: [string, (view: DataView) => void][] = [
  ["bad magic", (view) => view.setUint8(0, 0)],
  ["unsupported schema", (view) => view.setUint16(4, 2, true)],
  ["nonzero flags", (view) => view.setUint16(6, 1, true)],
  ["nonzero reserved tail", (view) => view.setUint8(63, 1)],
  ["wrong loop count", (view) => view.setUint32(8, 1, true)],
  ["hostile loop count", (view) => view.setUint32(8, 0xffffffff, true)],
  ["incomplete point count", (view) => view.setUint32(12, 7, true)],
  ["hostile point count", (view) => view.setUint32(12, 0xffffffff, true)],
  ["NaN origin", (view) => view.setFloat64(16, NaN, true)],
  ["infinite origin", (view) => view.setFloat64(32, Infinity, true)],
  ["wrong finite origin", (view) => view.setFloat64(16, 1, true)],
  ["wrong chunk index", (view) => view.setUint32(40, 1, true)],
  ["wrong first loop ordinal", (view) => view.setUint32(44, 1, true)],
  ["nonzero first offset", (view) => view.setUint32(64, 1, true)],
  ["offsets out of range", (view) => view.setUint32(68, 11, true)],
  ["incomplete first loop", (view) => view.setUint32(68, 3, true)],
  ["nonincreasing offsets", (view) => view.setUint32(68, 0, true)],
  ["wrong final offset", (view) => view.setUint32(72, 9, true)],
  ["nonzero alignment padding", (view) => view.setUint8(76, 1)],
  ["NaN coordinate", (view) => view.setFloat64(80, NaN, true)],
  ["infinite coordinate", (view) => view.setFloat64(88, Infinity, true)],
  ["off-plane coordinate", (view) => view.setFloat64(96, 0.001, true)],
  ["unclosed loop", (view) => view.setFloat64(176, 0.1, true)],
  [
    "zero-area outer loop",
    (view) => {
      for (let point = 0; point < 5; point += 1)
        for (let axis = 0; axis < 3; axis += 1)
          view.setFloat64(80 + point * 24 + axis * 8, 0, true);
    },
  ],
];
for (const [description, mutate] of corruptions) {
  test(`section rejects ${description} even with an authentic hash`, async () => {
    const bytes = golden.slice(0);
    mutate(new DataView(bytes));
    await assert.rejects(
      decodeSection(bytes, await rehash(bytes, metadata), summary, fixture.loops, expected),
      SectionDecodeError,
    );
  });
}

const descriptorCorruptions: [string, (value: SectionChunkMetadata) => void][] = [
  [
    "wrong session",
    (value) => {
      value.session_id = "550e8400-e29b-41d4-a716-446655440001";
    },
  ],
  [
    "wrong artifact",
    (value) => {
      value.artifact_id += 1;
    },
  ],
  [
    "bad schema",
    (value) => {
      value.schema_version = 2;
    },
  ],
  [
    "wrong chunk index",
    (value) => {
      value.chunk_index = 1;
    },
  ],
  [
    "wrong chunk count",
    (value) => {
      value.chunk_count = 2;
    },
  ],
  [
    "wrong byte count",
    (value) => {
      value.byte_count += 1;
    },
  ],
  [
    "bad hash",
    (value) => {
      value.sha256 = "0".repeat(64);
    },
  ],
  [
    "noncanonical hash",
    (value) => {
      value.sha256 = value.sha256.toUpperCase();
    },
  ],
  [
    "wrong first ordinal",
    (value) => {
      value.first_loop_ordinal = 1;
    },
  ],
  [
    "zero loops",
    (value) => {
      value.loop_count = 0;
    },
  ],
  [
    "out-of-range loops",
    (value) => {
      value.loop_count = 3;
    },
  ],
  [
    "fractional loops",
    (value) => {
      value.loop_count = 1.5;
    },
  ],
  [
    "unknown field",
    (value) => {
      Object.assign(value, { extra: true });
    },
  ],
];
for (const [description, mutate] of descriptorCorruptions) {
  test(`section rejects descriptor ${description}`, async () => {
    const changed = structuredClone(metadata);
    mutate(changed);
    await assert.rejects(
      decodeSection(golden, changed, summary, fixture.loops, expected),
      SectionDecodeError,
    );
  });
}

const summaryCorruptions: [string, (value: SectionSummary) => void][] = [
  [
    "stale session",
    (value) => {
      value.session_id = "550e8400-e29b-41d4-a716-446655440001";
    },
  ],
  [
    "nil session",
    (value) => {
      value.session_id = "00000000-0000-0000-0000-000000000000";
    },
  ],
  [
    "wrong artifact",
    (value) => {
      value.artifact_id += 1;
    },
  ],
  [
    "stale revision",
    (value) => {
      value.revision -= 1;
    },
  ],
  [
    "fractional revision",
    (value) => {
      value.revision = 0.5;
    },
  ],
  [
    "wrong schema",
    (value) => {
      value.schema_version = 2;
    },
  ],
  [
    "wrong sampling tolerance",
    (value) => {
      value.sampling_tolerance_mm = 0.006;
    },
  ],
  [
    "wrong boolean tolerance",
    (value) => {
      value.boolean_tolerance_mm = 0;
    },
  ],
  [
    "nonfinite plane origin",
    (value) => {
      value.plane.origin_mm[0] = NaN;
    },
  ],
  [
    "zero plane normal",
    (value) => {
      value.plane.normal = [0, 0, 0];
    },
  ],
  [
    "nonunit plane normal",
    (value) => {
      value.plane.normal = [0, 0, 2];
    },
  ],
  [
    "infinite plane normal",
    (value) => {
      value.plane.normal[0] = Infinity;
    },
  ],
  [
    "wrong frame origin",
    (value) => {
      value.frame.origin_mm[0] = 1;
    },
  ],
  [
    "noncanonical frame",
    (value) => {
      value.frame.x_axis = [1, 0, 0];
      value.frame.y_axis = [0, 1, 0];
    },
  ],
  [
    "nonfinite frame",
    (value) => {
      value.frame.z_axis[0] = NaN;
    },
  ],
  [
    "negative total bytes",
    (value) => {
      value.total_bytes = -1;
    },
  ],
  [
    "over-budget total bytes",
    (value) => {
      value.total_bytes = MAX_SECTION_BYTES + 1;
    },
  ],
  [
    "inconsistent empty count",
    (value) => {
      value.total_loop_count = 0;
    },
  ],
  [
    "unknown field",
    (value) => {
      Object.assign(value, { extra: true });
    },
  ],
];
for (const [description, mutate] of summaryCorruptions) {
  test(`section rejects summary ${description}`, async () => {
    const changed = structuredClone(summary);
    mutate(changed);
    assert.throws(() => validateSectionSummary(changed, expected), SectionDecodeError);
    await assert.rejects(
      decodeSection(golden, metadata, changed, fixture.loops, expected),
      SectionDecodeError,
    );
  });
}

test("section enforces loop metadata point count, ordinal, winding and completeness", async () => {
  for (const mutate of [
    (rows: SectionLoopMetadata[]) => {
      rows[0]!.point_count = 4;
    },
    (rows: SectionLoopMetadata[]) => {
      rows[0]!.ordinal = 1;
    },
    (rows: SectionLoopMetadata[]) => {
      rows[0]!.occurrence_id = 0;
    },
    (rows: SectionLoopMetadata[]) => {
      rows[1]!.definition_id = "invalid";
    },
    (rows: SectionLoopMetadata[]) => {
      rows[1]!.is_hole = false;
    },
    (rows: SectionLoopMetadata[]) => {
      rows.pop();
    },
  ]) {
    const rows = structuredClone(fixture.loops);
    mutate(rows);
    await assert.rejects(
      decodeSection(golden, metadata, summary, rows, expected),
      SectionDecodeError,
    );
  }
});

test("section rejects reordered, overlapping, missing and discontinuous page ranges", () => {
  const chunks = multichunk.chunks.map((chunk) => chunk.metadata);
  const identity = multichunk.summary;
  assert.throws(
    () => validateSectionManifest(identity, [...chunks].reverse(), multichunk.loops, identity),
    SectionDecodeError,
  );
  assert.throws(
    () =>
      validateSectionManifest(
        identity,
        [chunks[0]!, { ...chunks[1]!, first_loop_ordinal: 0 }],
        multichunk.loops,
        identity,
      ),
    SectionDecodeError,
  );
  assert.throws(
    () =>
      validateSectionManifest(
        identity,
        [chunks[0]!, { ...chunks[1]!, first_loop_ordinal: 2 }],
        multichunk.loops,
        identity,
      ),
    SectionDecodeError,
  );
  assert.throws(
    () => validateSectionManifest(identity, chunks.slice(0, 1), multichunk.loops, identity),
    SectionDecodeError,
  );
  assert.throws(
    () => validateSectionManifest(identity, chunks, [...multichunk.loops].reverse(), identity),
    SectionDecodeError,
  );
  assert.throws(
    () =>
      validateSectionManifest(
        { ...identity, total_bytes: identity.total_bytes - 1 },
        chunks,
        multichunk.loops,
        identity,
      ),
    SectionDecodeError,
  );
  assert.throws(
    () =>
      validateSectionManifest(
        identity,
        [{ ...chunks[0]!, byte_count: 191 }, chunks[1]!],
        multichunk.loops,
        identity,
      ),
    SectionDecodeError,
  );
  assert.throws(
    () =>
      validateSectionManifest(
        identity,
        chunks,
        [{ ...multichunk.loops[0]!, is_hole: true }, multichunk.loops[1]!],
        identity,
      ),
    SectionDecodeError,
  );
  assert.throws(
    () =>
      validateSectionManifest(
        identity,
        chunks,
        [multichunk.loops[0]!, { ...multichunk.loops[1]!, definition_id: "0".repeat(64) }],
        identity,
      ),
    SectionDecodeError,
  );
  assert.throws(
    () =>
      validateSectionManifest(
        identity,
        chunks,
        [multichunk.loops[0]!, { ...multichunk.loops[1]!, occurrence_id: 2 }],
        identity,
      ),
    SectionDecodeError,
  );
});

test("section accepts a valid empty artifact, but no fabricated empty chunk", async () => {
  const empty = { ...summary, total_loop_count: 0, chunk_count: 0, total_bytes: 0 };
  validateSectionManifest(empty, [], [], expected);
  await assert.rejects(decodeSection(golden, metadata, empty, [], expected), SectionDecodeError);
});

test("section rejects hash changes, exact-length failures and oversized input", async () => {
  const bytes = golden.slice(0);
  new DataView(bytes).setFloat64(224, 2.5, true);
  await assert.rejects(
    decodeSection(bytes, metadata, summary, fixture.loops, expected),
    /SHA-256 mismatch/,
  );
  for (const length of [0, 63, 64, 319])
    await assert.rejects(
      decodeSection(
        golden.slice(0, length),
        { ...metadata, byte_count: length },
        summary,
        fixture.loops,
        expected,
      ),
      SectionDecodeError,
    );
  const trailing = new Uint8Array(328);
  trailing.set(new Uint8Array(golden));
  await assert.rejects(
    decodeSection(trailing, { ...metadata, byte_count: 328 }, summary, fixture.loops, expected),
    SectionDecodeError,
  );
  await assert.rejects(
    decodeSection(
      new ArrayBuffer(MAX_GEOMETRY_CHUNK_BYTES + 1),
      metadata,
      summary,
      fixture.loops,
      expected,
    ),
    SectionDecodeError,
  );
});

test("section hashes only aligned caller range and rejects f64 misalignment", async () => {
  const aligned = new Uint8Array(golden.byteLength + 24);
  aligned.fill(0xa5);
  aligned.set(new Uint8Array(golden), 8);
  const decoded = await decodeSection(
    aligned.subarray(8, 328),
    metadata,
    summary,
    fixture.loops,
    expected,
  );
  assert.equal(decoded.pointsRelativeMm.buffer, aligned.buffer);
  assert.equal(decoded.pointsRelativeMm.byteOffset, 88);
  const misaligned = new Uint8Array(324);
  misaligned.set(new Uint8Array(golden), 4);
  await assert.rejects(
    decodeSection(misaligned.subarray(4), metadata, summary, fixture.loops, expected),
    SectionDecodeError,
  );
});

test("section validates non-Z canonical frames and rebased large origins", async () => {
  const changed = structuredClone(summary);
  changed.plane = { origin_mm: [1e9, 1e9, 1e9], normal: [1, 0, 0] };
  changed.frame = {
    origin_mm: [...changed.plane.origin_mm],
    x_axis: [0, 0, -1],
    y_axis: [0, 1, 0],
    z_axis: [1, 0, 0],
  };
  const bytes = golden.slice(0);
  const data = new DataView(bytes);
  for (let axis = 0; axis < 3; axis += 1) data.setFloat64(16 + axis * 8, 1e9, true);
  for (let point = 0; point < 10; point += 1) {
    const base = 80 + point * 24;
    const x = data.getFloat64(base, true),
      y = data.getFloat64(base + 8, true);
    data.setFloat64(base, 0, true);
    data.setFloat64(base + 8, x, true);
    data.setFloat64(base + 16, y, true);
  }
  const decoded = await decodeSection(
    bytes,
    await rehash(bytes, metadata),
    changed,
    fixture.loops,
    expected,
  );
  assert.deepEqual(decoded.planeOriginMm, [1e9, 1e9, 1e9]);
  assert.deepEqual(Array.from(decoded.pointsRelativeMm.subarray(3, 6)), [0, 20, 0]);
});

test("section admits exactly 40000 points in one complete loop and rejects 40001", async () => {
  for (const points of [MAX_LOOP_POINTS, MAX_LOOP_POINTS + 1]) {
    const bytes = new ArrayBuffer(72 + points * 24);
    const data = new DataView(bytes);
    data.setUint32(0, 0x534c5053, true);
    data.setUint16(4, 1, true);
    data.setUint32(8, 1, true);
    data.setUint32(12, points, true);
    data.setFloat64(32, 4, true);
    data.setUint32(68, points, true);
    // A closed algebraic rectangle with repeated closing samples exercises only
    // the decoder's point cap, not native curve sampling or geometry acceptance.
    data.setFloat64(72 + 24, 20, true);
    data.setFloat64(72 + 48, 20, true);
    data.setFloat64(72 + 48 + 8, 10, true);
    data.setFloat64(72 + 72 + 8, 10, true);
    const changedSummary = { ...summary, total_loop_count: 1, total_bytes: bytes.byteLength };
    const changedMetadata = await rehash(bytes, {
      ...metadata,
      loop_count: 1,
      byte_count: bytes.byteLength,
    });
    const rows = [{ ...fixture.loops[0]!, point_count: points }];
    if (points === MAX_LOOP_POINTS) {
      validateSectionManifest(changedSummary, [changedMetadata], rows, expected);
      const decoded = await decodeSection(bytes, changedMetadata, changedSummary, rows, expected);
      assert.equal(decoded.pointsRelativeMm.length, MAX_LOOP_POINTS * 3);
    } else {
      assert.throws(
        () => validateSectionManifest(changedSummary, [changedMetadata], rows, expected),
        SectionDecodeError,
      );
      await assert.rejects(
        decodeSection(bytes, changedMetadata, changedSummary, rows, expected),
        SectionDecodeError,
      );
    }
  }
});

test("section rejects impossible summary sizes and malformed numeric counts", async () => {
  for (const changed of [
    { ...summary, total_bytes: 1 },
    { ...summary, total_bytes: 200 },
    { ...summary, total_loop_count: 0xffffffff },
    { ...summary, total_loop_count: NaN },
    { ...summary, chunk_count: 0.5 },
  ]) {
    assert.throws(() => validateSectionSummary(changed, expected), SectionDecodeError);
    await assert.rejects(
      decodeSection(golden, metadata, changed, fixture.loops, expected),
      SectionDecodeError,
    );
  }
  const missing = structuredClone(metadata);
  Reflect.deleteProperty(missing, "sha256");
  await assert.rejects(
    decodeSection(golden, missing, summary, fixture.loops, expected),
    SectionDecodeError,
  );
});

test("section decoder rejects inconsistent identities within its complete loop range", async () => {
  for (const rows of [
    [fixture.loops[0]!, { ...fixture.loops[1]!, definition_id: "0".repeat(64) }],
    [{ ...fixture.loops[0]!, occurrence_id: 2 }, fixture.loops[1]!],
    [fixture.loops[0]!, { ...fixture.loops[1]!, occurrence_id: 2 }],
  ])
    await assert.rejects(
      decodeSection(golden, metadata, summary, rows, expected),
      SectionDecodeError,
    );
});

test("section applies the declared closure and plane-residual tolerances", async () => {
  const bytes = golden.slice(0);
  const data = new DataView(bytes);
  data.setFloat64(96, 0.00005, true);
  data.setFloat64(176, 0.00005, true);
  data.setFloat64(192, 0.00005, true);
  await decodeSection(bytes, await rehash(bytes, metadata), summary, fixture.loops, expected);
  data.setFloat64(192, 0.000101, true);
  await assert.rejects(
    decodeSection(bytes, await rehash(bytes, metadata), summary, fixture.loops, expected),
    SectionDecodeError,
  );
});
