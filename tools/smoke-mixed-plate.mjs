/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Source-diverse plate scale on the real CLI/engine, never a mock planner.
// Run sequentially inside an externally bounded scope with coherent binaries.
import assert from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, readFile, unlink, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { auditMixedPlate } from "./mixed-plate-observer.mjs";
import { projectSmoke, cgroupPeakBytes } from "./project-smoke.mjs";
const root = resolve(import.meta.dirname, "..");
const target = resolve(root, process.env.CARGO_TARGET_DIR ?? "target");
await mkdir(join(root, ".cache"), { recursive: true });
const output = await mkdtemp(join(root, ".cache", "mixed-plate-"));
const evidence = { software_only: true, commands: [], plates: [] };
const invoke = projectSmoke({ root, target, output, evidence, timeoutMs: 600_000 });
const counts = (process.env.SPILING_PLATE_COUNTS ?? "1,2,8,32").split(",").map(Number);
assert.ok(counts.every((n) => [1, 2, 8, 32].includes(n)));
console.error(`Evidence: ${output}`);
for (const count of counts) {
  const folder = join(output, String(count));
  await mkdir(folder);
  const bore = join(folder, "bore.step"),
    box = join(folder, "box.step"),
    project = join(folder, "plate.spil");
  await copyFile(join(root, "fixtures/geometry/through-hole.step"), bore);
  const pose = (i) => ({
    translation_mm: [20 + (i % 4) * 50, 20 + Math.floor(i / 4) * 25, 0],
    rotation_xyzw: [0, 0, 0, 1],
  });
  await invoke(`${count}-create`, [
    "create",
    project,
    "--import",
    bore,
    "--pose",
    "1",
    JSON.stringify(pose(0)),
  ]);
  const additions = [];
  if (count > 1) {
    await copyFile(join(root, "fixtures/geometry/box-mm.step"), box);
    const imported = await invoke(`${count}-box`, [
      "edit",
      project,
      "--import",
      box,
      "--pose",
      "2",
      JSON.stringify(pose(1)),
      "--save",
    ]);
    const id = imported.occurrences.find((o) => o.occurrence_id === 2).definition_id;
    for (let i = 2; i < count; i++) additions.push("--add", id, JSON.stringify(pose(i)));
  }
  const intent = JSON.parse(
    await readFile(join(root, "fixtures/manufacturing/mixed-width-combined.intent.json"), "utf8"),
  );
  const job = intent.printer.workflow.job,
    template = structuredClone(job.assignments[0]);
  for (const tool of job.tools) for (const point of tool.waste_line_xy_mm) point[1] = 215;
  job.assignments = Array.from({ length: count }, (_, i) => {
    const [x, y] = pose(i).translation_mm;
    return {
      ...structuredClone(template),
      occurrence_id: i + 1,
      work_envelope_mm: { min: [x - 2, y - 2, 0], max: [x + 22, y + (i === 0 ? 22 : 12), 8.01] },
    };
  });
  const input = join(folder, "intent.json");
  await writeFile(input, JSON.stringify(intent));
  const exported = join(folder, "original");
  const compiled = await invoke(`${count}-compile`, [
    "edit",
    project,
    ...additions,
    "--intent",
    input,
    "--preflight",
    "--compile",
    "--verify",
    "--manufacturing-out",
    exported,
    ...(count === 8 ? ["--compile", "--verify"] : []),
    "--save",
  ]);
  assert.equal(compiled.occurrences.length, count);
  assert.equal(compiled.definitions.length, count === 1 ? 1 : 2);
  assert.equal(compiled.unique_mesh_chunks, count === 1 ? 1 : 2);
  await unlink(bore);
  if (count > 1) await unlink(box);
  await unlink(input);
  const returned = await invoke(`${count}-return`, [
    "inspect",
    project,
    "--verify",
    "--replay-out",
    join(folder, "actual.splr"),
    "--manufacturing-out",
    join(folder, "returned"),
  ]);
  assert.equal(
    await readFile(join(exported, "program.gcode"), "utf8"),
    await readFile(join(folder, "returned/program.gcode"), "utf8"),
  );
  const plate = {
    count,
    ...(await auditMixedPlate(count, folder, returned)),
    compile_elapsed_us: compiled.operations
      .filter((op) => op.operation === "compile")
      .map((op) => op.elapsed_us),
    cgroup_peak_bytes: await cgroupPeakBytes(),
    node_cpu: process.cpuUsage(),
  };
  evidence.plates.push(plate);
  await writeFile(join(output, "evidence.json"), JSON.stringify(evidence, null, 2));
  console.error(
    JSON.stringify({
      count,
      choice: plate.choice,
      segments: plate.actual_segments,
      selections: plate.tool_selections,
      peak_bytes: plate.cgroup_peak_bytes,
    }),
  );
  if (count === 1 && process.env.SPILING_PLATE_COPY === "1") {
    await invoke("copy-native-single", [
      "edit",
      project,
      "--save-as",
      join(folder, "native-single.spil"),
    ]);
    const copied = await invoke("make-copies", [
      "edit",
      project,
      "--make-copies",
      "1",
      "2",
      "--apply-layout",
      "--undo",
      "--redo",
      "--preflight",
      "--compile",
      "--verify",
      "--manufacturing-out",
      join(folder, "copies"),
      "--save",
    ]);
    assert.equal(copied.occurrences.length, 2);
    assert.equal(copied.definitions.length, 1);
    assert.equal(copied.manufacturing.intent.printer.workflow.job.assignments.length, 2);
    assert.ok(copied.layout_proposals[0].verification.verified);
    await invoke("copies-return", [
      "inspect",
      project,
      "--verify",
      "--manufacturing-out",
      join(folder, "copies-returned"),
    ]);
    assert.equal(
      await readFile(join(folder, "copies/program.gcode"), "utf8"),
      await readFile(join(folder, "copies-returned/program.gcode"), "utf8"),
    );
    evidence.copies = {
      occurrences: 2,
      shared_definitions: 1,
      native_single: join(folder, "native-single.spil"),
      layout: copied.layout_proposals[0],
    };
    await writeFile(join(output, "evidence.json"), JSON.stringify(evidence, null, 2));
  }
}
console.log(
  JSON.stringify({ evidence: output, plate_counts: counts, source_independent_replay: true }),
);
