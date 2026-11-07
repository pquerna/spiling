/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Compare complete actual-window samples under the same recorded resource envelope.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

const metrics = [
  "first_frame_readback_us",
  "open_to_published_us",
  "open_to_source_readback_us",
  "launch_to_source_readback_us",
];
const median = (values) => {
  const sorted = [...values].sort((a, b) => a - b),
    mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
};
const identity = (r) => [
  r.project_sha256,
  r.preferences_sha256,
  r.engine_build,
  r.kernel,
  r.warmups,
  r.software_only,
  r.cache_state,
  r.graphics,
  r.environment,
];
function check(rows) {
  assert.ok(rows.length > 0, "empty sample set");
  assert.equal(rows.length, rows[0].samples, "incomplete sample set");
  assert.deepEqual(
    rows.map((r) => r.sample).sort((a, b) => a - b),
    Array.from({ length: rows.length }, (_, i) => i),
    "duplicate or missing samples",
  );
  for (const row of rows) {
    assert.equal(row.samples, rows.length, "inconsistent sample count");
    assert.equal(row.benchmark, "native-open-v1");
    assert.equal(row.software_only, true);
    assert.ok(
      row.environment?.resource_limits &&
        row.graphics &&
        row.engine_build &&
        row.kernel &&
        row.project_sha256 &&
        row.preferences_sha256,
      "missing workload/environment identity",
    );
    assert.deepEqual(identity(row), identity(rows[0]), "mixed workload or configuration");
    assert.equal(row.workbench_sha256, rows[0].workbench_sha256, "binary changed during samples");
    assert.equal(row.engine_sha256, rows[0].engine_sha256, "binary changed during samples");
    for (const key of metrics)
      assert.ok(Number.isFinite(row[key]) && row[key] > 0, `invalid ${key}`);
  }
}
export function compareOpen(before, after) {
  check(before);
  check(after);
  assert.deepEqual(
    identity(after[0]),
    identity(before[0]),
    "input/producer/graphics/resource configuration differs; no like-for-like speedup",
  );
  return metrics.map((metric) => {
    const a = median(before.map((r) => r[metric])),
      b = median(after.map((r) => r[metric]));
    return {
      metric,
      baseline_samples: before.length,
      candidate_samples: after.length,
      baseline_median_us: a,
      candidate_median_us: b,
      speedup: a / b,
      baseline_mad_us: median(before.map((r) => Math.abs(r[metric] - a))),
      candidate_mad_us: median(after.map((r) => Math.abs(r[metric] - b))),
    };
  });
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [beforePath, afterPath, ...extra] = process.argv.slice(2);
  assert.ok(
    beforePath && afterPath && !extra.length,
    "usage: node tools/compare-open-bench.mjs BEFORE.ndjson AFTER.ndjson",
  );
  const read = async (path) =>
    (await readFile(path, "utf8"))
      .trim()
      .split(/\r?\n/)
      .map((line) => JSON.parse(line));
  console.log(
    JSON.stringify(
      {
        comparisons: compareOpen(await read(beforePath), await read(afterPath)),
        timing_is_not_a_correctness_or_hardware_qualification: true,
      },
      null,
      2,
    ),
  );
}
