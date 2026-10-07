/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { decodeTriangle } from "../packages/protocol/src/index.ts";

const suffix = process.platform === "win32" ? ".exe" : "";
const engine = resolve(`target/debug/spiling-engine${suffix}`);
const cli = resolve(`target/debug/spiling-cli${suffix}`);
const directory = await mkdtemp(join(tmpdir(), "spiling-protocol-"));
function invoke(args) {
  const result = spawnSync(cli, args, { encoding: "utf8", timeout: 15000 });
  if (result.error) throw result.error;
  return result;
}
function rejects(buffer) {
  assert.throws(() => decodeTriangle(buffer), "corrupt payload must be rejected");
}
try {
  const diagnostic = invoke(["diagnose", "--engine", engine]);
  assert.equal(diagnostic.status, 0, diagnostic.stderr);
  const report = JSON.parse(diagnostic.stdout);
  assert.match(report.hello.instance_id, /^[0-9a-f-]{36}$/);
  assert.equal(report.hello.kernel, "monstertruck");
  assert.deepEqual(report.hello.geometry_capabilities, []);
  assert.ok(report.hello.pid > 0);
  const output = join(directory, "triangle.bin");
  const transfer = invoke(["triangle", "--engine", engine, "--output", output]);
  assert.equal(transfer.status, 0, transfer.stderr);
  const bytes = await readFile(output);
  const payload = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  assert.equal(payload.byteLength, 64);
  const triangle = decodeTriangle(payload);
  assert.deepEqual(Array.from(triangle.indices), [0, 1, 2]);
  assert.equal(triangle.positions[2], 0);
  assert.ok(Math.abs(triangle.positions[0] + 0.75) < 1e-7);
  assert.ok(Math.abs(triangle.positions[7] - 0.75) < 1e-7);
  rejects(payload.slice(0, 63));
  rejects(new ArrayBuffer(4 * 1024 * 1024 + 1));
  for (const [offset, value, method] of [
    [4, 2, "setUint16"],
    [6, 1, "setUint16"],
    [8, 0xffffffff, "setUint32"],
    [52, 3, "setUint32"],
  ]) {
    const corrupt = payload.slice(0);
    new DataView(corrupt)[method](offset, value, true);
    rejects(corrupt);
  }
  const nonfinite = payload.slice(0);
  new DataView(nonfinite).setFloat32(16, Number.NaN, true);
  rejects(nonfinite);
  const job = invoke(["job", "--engine", engine, "--chunks", "4", "--delay-ms", "150"]);
  assert.equal(job.status, 0, job.stderr);
  const jobReport = JSON.parse(job.stdout);
  assert.equal(jobReport.operation.done, true);
  assert.equal(jobReport.operation.state, "succeeded");
  assert.equal(jobReport.bytes, 256);
  assert.ok(jobReport.partial_updates > 0, "real partial output must arrive before completion");
  if (process.platform === "linux") {
    // Linux permits invalid UTF-8 filenames; macOS rejects this fixture with EILSEQ.
    // A shell expands the controlled glob as native bytes, unlike Node argv strings.
    const engineBytes = Buffer.concat([
      Buffer.from(join(directory, "engine-")),
      Buffer.from([255]),
    ]);
    const outputBytes = Buffer.concat([
      Buffer.from(join(directory, "output-")),
      Buffer.from([255]),
    ]);
    await symlink(engine, engineBytes);
    await writeFile(outputBytes, new Uint8Array());
    for (const [command, outputArgs] of [
      ["diagnose", ""],
      ["triangle", ' --output "$2"/output-*'],
    ]) {
      const result = spawnSync(
        "/bin/sh",
        [
          "-c",
          `"$1" ${command} --engine "$2"/engine-*${outputArgs}`,
          "spiling-smoke",
          cli,
          directory,
        ],
        { encoding: "utf8", timeout: 15000 },
      );
      if (result.error) throw result.error;
      assert.equal(result.status, 0, result.stderr);
      const nonUtf = JSON.parse(result.stdout);
      assert.match(nonUtf.engine ?? nonUtf.output, /\uFFFD$/);
      assert.throws(
        () => process.kill(nonUtf.hello?.pid ?? nonUtf.pid, 0),
        { code: "ESRCH" },
        "successful reporting must follow child shutdown",
      );
    }
    assert.deepEqual(await readFile(outputBytes), bytes);
  }
  console.log(
    JSON.stringify(
      {
        result: "pass",
        actualEngineHandshake: report,
        binaryBytes: payload.byteLength,
        decoder:
          "cross-language native transfer; truncation, oversized allocation, schema, reserved, count overflow, index bounds, nonfinite rejected",
        operations: jobReport,
        nonUtfPaths:
          process.platform === "linux"
            ? "JSON reports and child cleanup passed"
            : "Not exercised: Linux byte-path boundary",
      },
      null,
      2,
    ),
  );
} finally {
  await rm(directory, { recursive: true, force: true });
}
