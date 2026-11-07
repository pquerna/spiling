/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import { compareOpen } from "./compare-open-bench.mjs";
const samples = () =>
  [0, 1, 2].map((sample) => ({
    benchmark: "native-open-v1",
    sample,
    samples: 3,
    warmups: 1,
    software_only: true,
    project_sha256: "project",
    preferences_sha256: "preferences",
    engine_build: "same-producer",
    kernel: "same-kernel",
    graphics: "same-adapter",
    environment: { available_parallelism: 32, resource_limits: { "memory.max": "8589934592" } },
    engine_sha256: "engine",
    workbench_sha256: "ui",
    cache_state: "uncontrolled",
    first_frame_readback_us: 100 + sample,
    open_to_published_us: 200 + sample,
    open_to_source_readback_us: 300 + sample,
    launch_to_source_readback_us: 400 + sample,
  }));
test("native comparison permits changed executables and reports medians", () => {
  const after = samples().map((r) => ({
    ...r,
    workbench_sha256: "new-ui",
    engine_sha256: "new-engine",
    open_to_published_us: r.open_to_published_us / 2,
  }));
  const rows = compareOpen(samples(), after);
  assert.equal(rows[1].speedup, 2);
  assert.equal(rows[1].baseline_mad_us, 1);
});
test("native comparison refuses incomplete, duplicate, mixed and incomparable samples", () => {
  for (const mutate of [
    (r) => r.pop(),
    (r) => {
      r[1].samples = 4;
    },
    (r) => {
      r[1].sample = 0;
    },
    (r) => {
      r[0].project_sha256 = "other";
    },
    (r) => {
      for (const x of r) x.preferences_sha256 = "different-view-state";
    },
    (r) => {
      r[0].engine_sha256 = "different-during-run";
    },
    (r) => {
      for (const x of r) x.environment.resource_limits["memory.max"] = "4294967296";
    },
    (r) => {
      for (const x of r) x.environment.available_parallelism = 8;
    },
    (r) => {
      for (const x of r) delete x.environment.resource_limits;
    },
    (r) => {
      r[1].launch_to_source_readback_us = NaN;
    },
  ]) {
    const after = samples();
    mutate(after);
    assert.throws(() => compareOpen(samples(), after));
  }
});
