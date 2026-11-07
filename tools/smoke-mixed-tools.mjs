/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Real native CLI/engine journey. Requires coherent prebuilt binaries and an
// external whole-process resource bound. This runner never builds or prints.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { copyFile, mkdir, mkdtemp, readFile, unlink, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { decodeReplayDisplay } from "../packages/protocol/src/replay.ts";

const root = resolve(import.meta.dirname, "..");
const target = resolve(root, process.env.CARGO_TARGET_DIR ?? "target");
const suffix = process.platform === "win32" ? ".exe" : "";
const cli = join(target, "debug", `spiling-cli${suffix}`);
const engine = join(target, "debug", `spiling-engine${suffix}`);
await mkdir(join(root, ".cache"), { recursive: true });
const output = await mkdtemp(join(root, ".cache", "mixed-tools-"));
console.error(`Evidence: ${output}`);
const evidence = { software_only: true, commands: [] };
async function invoke(name, args, expectedFailure = false) {
  const command = ["project", ...args, "--engine", engine];
  let stdout = "",
    stderr = "",
    size = 0;
  const code = await new Promise((done, fail) => {
    const child = spawn(cli, command, {
      cwd: root,
      env: { ...process.env, RAYON_NUM_THREADS: "1" },
    });
    const timer = setTimeout(() => child.kill("SIGKILL"), 300_000);
    for (const [stream, key] of [
      [child.stdout, "out"],
      [child.stderr, "err"],
    ]) {
      stream.setEncoding("utf8").on("data", (text) => {
        size += Buffer.byteLength(text);
        if (size > 16 * 1024 * 1024) child.kill("SIGKILL");
        else if (key === "out") stdout += text;
        else stderr += text;
      });
    }
    child.once("error", fail);
    child.once("close", (code) => {
      clearTimeout(timer);
      done(code);
    });
  });
  await writeFile(join(output, `${name}.stdout.json`), stdout);
  await writeFile(join(output, `${name}.stderr.txt`), stderr);
  evidence.commands.push({ name, command, code });
  assert.equal(code === 0, !expectedFailure, `${name}: ${stderr.slice(-4000)}`);
  if (expectedFailure) return stderr;
  const report = JSON.parse(stdout);
  evidence.engine_build ??= report.hello.engine_build;
  assert.equal(report.hello.engine_build, evidence.engine_build);
  return report;
}
const source = join(output, "owned-box.step"),
  project = join(output, "Shared material.spil");
await copyFile(join(root, "fixtures/geometry/box-mm.step"), source);
const created = await invoke("create", [
  "create",
  project,
  "--import",
  source,
  "--pose",
  "1",
  JSON.stringify({ translation_mm: [20, 20, 0], rotation_xyzw: [0, 0, 0, 1] }),
]);
const intent = JSON.parse(
  await readFile(
    join(root, "fixtures/manufacturing/fixed-dual-shared-material.intent.json"),
    "utf8",
  ),
);
const intentFile = join(output, "intent.json");
await writeFile(intentFile, JSON.stringify(intent));
const originalExport = join(output, "original-export");
await invoke("compile", [
  "edit",
  project,
  "--add",
  created.definitions[0].record.definition_id,
  JSON.stringify({ translation_mm: [120, 20, 0], rotation_xyzw: [0, 0, 0, 1] }),
  "--intent",
  intentFile,
  "--preflight",
  "--compile",
  "--verify",
  "--manufacturing-out",
  originalExport,
  "--save",
]);
await unlink(source);
await unlink(intentFile);
const replay = join(output, "actual.splr"),
  recoveredExport = join(output, "returned-export");
const returned = await invoke("saved-return", [
  "inspect",
  project,
  "--verify",
  "--manufacturing-out",
  recoveredExport,
  "--replay-out",
  replay,
]);
const job = returned.manufacturing.intent.printer.workflow.job;
assert.deepEqual(job.tools[0].material, job.tools[1].material);
assert.deepEqual(
  job.assignments.map((a) => a.tool_id),
  job.tools.map((t) => t.id),
);
const decoded = await decodeReplayDisplay(
  Uint8Array.from(await readFile(replay)).buffer,
  returned.manufacturing.replay_display,
);
const counts = [0, 0];
for (let i = 0; i < decoded.eventCount; i++) {
  if (![1, 2, 3, 4, 5, 8].includes(decoded.kind(i))) continue;
  const tool = decoded.activeTool(i),
    point = decoded.position(i);
  assert.ok(tool === 0 || tool === 1);
  const expected = point[0] < 80 ? 0 : 1;
  assert.equal(
    tool,
    expected,
    `actual model endpoint assigned to wrong same-material tool: ${point}`,
  );
  counts[tool]++;
}
assert.ok(counts.every((n) => n > 0));
assert.deepEqual(
  await readFile(join(originalExport, "program.gcode")),
  await readFile(join(recoveredExport, "program.gcode")),
);
assert.equal(
  JSON.parse(await readFile(join(recoveredExport, "verification.json"), "utf8")).verified,
  true,
);
const invalid = structuredClone(intent);
invalid.printer.workflow.job.assignments[1].tool_id = null;
const invalidFile = join(output, "ambiguous.intent.json");
await writeFile(invalidFile, JSON.stringify(invalid));
const error = await invoke("ambiguous-binding", ["edit", project, "--intent", invalidFile], true);
assert.match(error, /explicit tool binding/);
const after = await invoke("after-refusal", ["inspect", project]);
assert.deepEqual(after.manufacturing.intent, returned.manufacturing.intent);
evidence.model_events_per_tool = counts;
await writeFile(join(output, "acceptance.json"), JSON.stringify(evidence, null, 2) + "\n");
console.log(
  JSON.stringify({
    evidence: output,
    same_material_tools: true,
    source_independent_replay: true,
    model_events_per_tool: counts,
  }),
);
