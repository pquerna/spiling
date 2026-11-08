import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

const samples = () =>
  [0, 1, 2].map((sample) => ({
    benchmark: "native-manufacturing-v1",
    software_only: true,
    phase: "compile",
    input_hash: "same-input",
    program_hash: "same-program",
    samples: 3,
    sample,
    elapsed_us: 100 + sample,
    compute: {
      settings: { default_backend: "cpu", fea_backend: null },
      default_device: { backend: "cpu", name: "LLVM", hardware_accelerated: false },
      fea_device: { backend: "cpu", name: "LLVM", hardware_accelerated: false },
    },
  }));

test("slice comparisons retain compute identity and complete sample guards", async () => {
  const directory = await mkdtemp(join(tmpdir(), "spiling-slice-comparison-"));
  const before = join(directory, "before.ndjson");
  const after = join(directory, "after.ndjson");
  const encode = (rows) => rows.map((row) => JSON.stringify(row)).join("\n");
  try {
    await writeFile(before, encode(samples()));
    const compare = () =>
      spawnSync(process.execPath, ["tools/compare-slice-bench.mjs", before, after], {
        encoding: "utf8",
      });
    await writeFile(
      after,
      encode(
        samples().map((row) => ({
          ...row,
          elapsed_us: row.elapsed_us / 2,
        })),
      ),
    );
    const valid = compare();
    assert.equal(valid.status, 0, valid.stderr);
    assert.equal(JSON.parse(valid.stdout).comparisons[0].speedup, 2);
    for (const mutate of [
      (rows) => {
        rows.pop();
      },
      (rows) => {
        rows[1].sample = 0;
      },
      (rows) => {
        rows[1].compute.settings.default_backend = "auto";
      },
      (rows) => {
        rows[1].compute.default_device.name = "different-device";
      },
      (rows) => {
        rows[1].compute.fea_device.backend = "vulkan";
      },
      (rows) => {
        delete rows[1].compute;
      },
      (rows) => {
        rows[1].program_hash = "different-program";
      },
    ]) {
      const rows = samples();
      mutate(rows);
      await writeFile(after, encode(rows));
      assert.notEqual(compare().status, 0);
    }
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
