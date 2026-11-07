/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Compare qualified same-input samples, never unlike jobs or compile profiles.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const [beforePath, afterPath, ...extra] = process.argv.slice(2);
assert.ok(
  beforePath && afterPath && extra.length === 0,
  "usage: node tools/compare-slice-bench.mjs BEFORE.ndjson AFTER.ndjson",
);
async function groups(path) {
  const rows = (await readFile(path, "utf8"))
    .trim()
    .split(/\r?\n/)
    .map((line) => JSON.parse(line));
  assert.ok(rows.length > 0);
  const result = new Map();
  for (const row of rows) {
    assert.equal(row.benchmark, "native-manufacturing-v1");
    assert.equal(row.software_only, true);
    assert.ok(Number.isFinite(row.elapsed_us) && row.elapsed_us > 0);
    const key = JSON.stringify([row.phase, row.input_hash]);
    const group = result.get(key) ?? [];
    group.push(row);
    result.set(key, group);
  }
  for (const group of result.values()) {
    assert.equal(group.length, group[0].samples, "incomplete sample set");
    assert.deepEqual(
      group.map((r) => r.sample).sort((a, b) => a - b),
      Array.from({ length: group.length }, (_, i) => i),
      "duplicate or missing samples",
    );
  }
  return result;
}
const before = await groups(beforePath),
  after = await groups(afterPath);
assert.deepEqual(
  [...before.keys()].sort(),
  [...after.keys()].sort(),
  "workload/phase coverage differs",
);
const median = (values) => {
  const sorted = [...values].sort((a, b) => a - b),
    mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
};
const identity = (row) =>
  JSON.stringify([
    row.engine_build,
    row.kernel,
    row.sources,
    row.intent_source,
    row.program_hash,
    row.occurrences,
    row.definitions,
    row.layers,
    row.deposition_segments,
    row.selection,
    row.debug_assertions,
    row.available_parallelism,
    row.rayon_threads,
    row.warmup,
  ]);
const comparisons = [];
for (const [key, baseline] of before) {
  const candidate = after.get(key),
    expected = identity(baseline[0]);
  for (const row of [...baseline, ...candidate])
    assert.equal(
      identity(row),
      expected,
      "producer/output/CPU configuration differs; do not report a like-for-like speedup",
    );
  const a = median(baseline.map((r) => r.elapsed_us)),
    b = median(candidate.map((r) => r.elapsed_us));
  comparisons.push({
    phase: baseline[0].phase,
    input_hash: baseline[0].input_hash,
    parts: baseline[0].occurrences,
    baseline_samples: baseline.length,
    candidate_samples: candidate.length,
    baseline_median_us: a,
    candidate_median_us: b,
    speedup: a / b,
    baseline_mad_us: median(baseline.map((r) => Math.abs(r.elapsed_us - a))),
    candidate_mad_us: median(candidate.map((r) => Math.abs(r.elapsed_us - b))),
    program_hash: baseline[0].program_hash,
  });
}
console.log(
  JSON.stringify(
    { comparisons, timing_is_not_a_correctness_or_hardware_qualification: true },
    null,
    2,
  ),
);
