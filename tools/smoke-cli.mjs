/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFile, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { decodeTriangle } from "../packages/protocol/src/index.ts";
import { decodeMesh, validateMeshManifest } from "../packages/protocol/src/mesh.ts";
import { decodeSection, validateSectionManifest } from "../packages/protocol/src/section.ts";

const suffix = process.platform === "win32" ? ".exe" : "";
const target = resolve(process.env.CARGO_TARGET_DIR ?? "target");
const engine = resolve(target, `debug/spiling-engine${suffix}`);
const cli = resolve(target, `debug/spiling-cli${suffix}`);
const directory = await mkdtemp(join(tmpdir(), "spiling-protocol-"));
function invoke(args) {
  const result = spawnSync(cli, args, { encoding: "utf8", timeout: 120000 });
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
  assert.ok(report.hello.geometry_capabilities.length > 0);
  assert.match(report.hello.session_id, /^[0-9a-f-]{36}$/);
  assert.equal(report.hello.kernel_identity.name, "monstertruck");
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
  assert.match(jobReport.operation.name, /^diagnostics\/default\/operations\/[0-9a-f-]{36}$/);
  const requestId = "518947e2-c3d1-4c9d-8e80-9a83ff7d6c54";
  const store = join(directory, "operations");
  const durableArgs = [
    "job",
    "--engine",
    engine,
    "--store",
    store,
    "--request-id",
    requestId,
    "--chunks",
    "2",
    "--delay-ms",
    "1",
    "--input-revision",
    "retained-diagnostic",
  ];
  const retained = invoke(durableArgs);
  assert.equal(retained.status, 0, retained.stderr);
  const reconnected = invoke(durableArgs);
  assert.equal(reconnected.status, 0, reconnected.stderr);
  assert.equal(
    JSON.parse(retained.stdout).operation.name,
    JSON.parse(reconnected.stdout).operation.name,
  );
  const geometryOut = join(directory, "geometry");
  const geometry = invoke([
    "geometry",
    "--scene",
    resolve("fixtures/geometry/scenes/two-parts.scene.json"),
    "--section",
    "0,0,4:0,0,1",
    "--out",
    geometryOut,
    "--engine",
    engine,
  ]);
  assert.equal(geometry.status, 0, geometry.stderr);
  const geometryReport = JSON.parse(geometry.stdout);
  assert.equal(geometryReport.scene.definition_count, 2);
  assert.equal(geometryReport.scene.occurrence_count, 3);
  assert.deepEqual(
    JSON.parse(await readFile(join(geometryOut, "manifest.json"), "utf8")),
    geometryReport,
  );
  for (const definition of geometryReport.definitions) {
    const identity = {
      session_id: geometryReport.hello.session_id,
      artifact_id: definition.record.mesh_artifact_id,
      definition_id: definition.record.definition_id,
    };
    validateMeshManifest(definition.chunks, identity);
    for (const chunk of definition.chunks) {
      const bytes = await readFile(
        join(
          geometryOut,
          `mesh-${String(identity.artifact_id).padStart(6, "0")}-${String(chunk.chunk_index).padStart(6, "0")}.splm`,
        ),
      );
      await decodeMesh(bytes, chunk, { ...identity, chunk_index: chunk.chunk_index });
    }
  }
  const section = geometryReport.section;
  const sectionIdentity = {
    session_id: geometryReport.hello.session_id,
    artifact_id: section.summary.artifact_id,
    revision: geometryReport.scene.revision,
  };
  const loops = section.loops.map((loop) => loop.metadata);
  validateSectionManifest(section.summary, section.chunks, loops, sectionIdentity);
  for (const chunk of section.chunks) {
    const bytes = await readFile(
      join(
        geometryOut,
        `section-${String(sectionIdentity.artifact_id).padStart(6, "0")}-${String(chunk.chunk_index).padStart(6, "0")}.spls`,
      ),
    );
    await decodeSection(
      bytes,
      chunk,
      section.summary,
      loops.slice(chunk.first_loop_ordinal, chunk.first_loop_ordinal + chunk.loop_count),
      { ...sectionIdentity, chunk_index: chunk.chunk_index },
    );
  }
  const source = join(directory, "operator-source.step");
  await copyFile(resolve("fixtures/geometry/box-mm.step"), source);
  const project = join(directory, "project");
  const firstExport = join(directory, "first-export");
  const created = invoke([
    "project",
    "create",
    project,
    "--import",
    source,
    "--intent",
    resolve("fixtures/manufacturing/solid-fill.intent.json"),
    "--compile",
    "--verify",
    "--manufacturing-out",
    firstExport,
    "--engine",
    engine,
  ]);
  assert.equal(created.status, 0, created.stderr);
  const createdReport = JSON.parse(created.stdout);
  await rm(source);
  const freshExport = join(directory, "fresh-export");
  const reopened = invoke([
    "project",
    "inspect",
    project,
    "--verify",
    "--manufacturing-out",
    freshExport,
    "--engine",
    engine,
  ]);
  assert.equal(reopened.status, 0, reopened.stderr);
  const reopenedReport = JSON.parse(reopened.stdout);
  assert.notEqual(createdReport.hello.session_id, reopenedReport.hello.session_id);
  assert.deepEqual(createdReport.manufacturing.artifact, reopenedReport.manufacturing.artifact);
  assert.equal(reopenedReport.manufacturing_export.software_only, true);
  assert.equal(reopenedReport.manufacturing_export.not_machine_ready, true);
  assert.equal(reopenedReport.manufacturing_export.verification.verified, true);
  assert.deepEqual(
    createdReport.definitions.map((d) => d.record.provenance),
    reopenedReport.definitions.map((d) => d.record.provenance),
  );
  for (const name of ["plan.json", "program.gcode", "verification.json", "provenance.json"]) {
    const before = await readFile(join(firstExport, name));
    const after = await readFile(join(freshExport, name));
    assert.equal(
      createHash("sha256").update(before).digest("hex"),
      createHash("sha256").update(after).digest("hex"),
      name,
    );
  }
  assert.deepEqual(
    JSON.parse(await readFile(join(freshExport, "verification.json"), "utf8")),
    reopenedReport.manufacturing_export.verification,
    "export must use the exact fresh replay result",
  );
  for (const report of [geometryReport, createdReport, reopenedReport]) {
    assert.throws(
      () => process.kill(report.hello.pid, 0),
      { code: "ESRCH" },
      "completion must follow actual child reap",
    );
  }
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
        geometry: {
          definitions: geometryReport.scene.definition_count,
          occurrences: geometryReport.scene.occurrence_count,
          uniqueMeshBytes: geometryReport.unique_mesh_bytes,
          sections: section.occurrences,
        },
        manufacturing: reopenedReport.manufacturing_export,
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
