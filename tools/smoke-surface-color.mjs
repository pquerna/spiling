/*
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
import assert from "node:assert/strict";
import { decodeReplayDisplay } from "../packages/protocol/src/replay.ts";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFile, mkdir, mkdtemp, readFile, unlink, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Real separate CLI/engine processes; artifacts are retained for diagnosis.
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const target = resolve(process.env.CARGO_TARGET_DIR ?? join(root, "target"));
const suffix = process.platform === "win32" ? ".exe" : "";
const cli = join(target, "debug", `spiling-cli${suffix}`);
const engine = join(target, "debug", `spiling-engine${suffix}`);
await mkdir(join(root, ".cache"), { recursive: true });
const output = await mkdtemp(join(root, ".cache", "surface-color-"));
let sequence = 0;
async function invoke(args, failure = false) {
  const result = spawnSync(cli, [...args, "--engine", engine], {
    cwd: root, encoding: "utf8", timeout: 330_000, maxBuffer: 16 * 1024 * 1024,
  });
  const receipt = join(output, `${String(++sequence).padStart(3, "0")}.json`);
  await writeFile(receipt, JSON.stringify({ args, status: result.status, stdout: result.stdout, stderr: result.stderr }, null, 2));
  if (result.error) throw result.error;
  if (failure) {
    assert.notEqual(result.status, 0, `unsupported construction succeeded: ${receipt}`);
    const diagnostic = result.stderr.trim().split("\n").map((line) => { try { return JSON.parse(line); } catch { return null; } }).find((row) => row?.domain === "manufacturing");
    assert.ok(diagnostic?.error, `missing typed refusal: ${receipt}`);
    return diagnostic.error;
  }
  assert.equal(result.status, 0, `${receipt}: ${result.stderr}`);
  const report = JSON.parse(result.stdout);
  assert.match(report.hello.engine_build, /planner30;emitter11;verifier14/);
  return report;
}
const json = async (path) => JSON.parse(await readFile(path, "utf8"));
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const results = [];
const cases = ["box-mm.step", "through-hole.step"].flatMap((source) => [[source, "full"], [source, "window"]]);
cases.push(["box-mm.step", "cycle"], ["box-mm.step", "all-white"]);
for (const [sourceName, mode] of cases) {
  const name = sourceName.replace(/\.step$/, "") + "-" + mode;
  const source = join(output, sourceName);
  const project = join(output, `${name}.spiling`);
  await copyFile(join(root, "fixtures", "geometry", sourceName), source);
  const created = await invoke(["project", "create", project, "--import", source,
    "--pose", "1", JSON.stringify({ translation_mm: [20, 20, 0], rotation_xyzw: [0, 0, 0, 1] })]);
  const faces = created.definitions[0].faces;
  const inspected = await invoke(["project", "inspect", project,
    ...faces.flatMap((face) => ["--inspect-face", "1", face.face_id])]);
  const selected = inspected.face_inspections.find((row) => {
    const plane = row.face.geometry.analytic;
    return plane?.kind === "plane" && plane.normal[1] < -0.5 && plane.normal[2] === 0;
  });
  assert.ok(selected, "native source has no selected upright exterior face");
  const intent = await json(join(root, "fixtures", "manufacturing", "fixed-dual.intent.json"));
  const job = intent.printer.workflow.job;
  job.assignments = job.assignments.slice(0, 1);
  job.assignments[0].material = { kind: "uniform", material_id: job.tools[1].material.id };
  const bounds = created.definitions[0].record.bounds_mm;
  job.assignments[0].work_envelope_mm = {
    min: bounds.min.map((v, axis) => v + [20, 20, 0][axis] - 1),
    max: bounds.max.map((v, axis) => v + [20, 20, 0][axis] + 1),
  };
  job.surface_constructions = [{
    occurrence_id: 1, definition_id: selected.reference.definition_id,
    source_hash: selected.provenance.source_hash,
    faces: [{ face_id: selected.face.face_id, skin_depth_mm: 1.2, backing_depth_mm: 1.2, mask: mode === "window" ? { min_u_mm: 4, max_u_mm: 16 } : null }],
    shell: { kind: "uniform", material_id: job.tools[0].material.id },
    white_material_id: job.tools[1].material.id,
  }];
  if (mode === "cycle") {
    job.surface_constructions[0].shell = {
      kind: "layer_cycle", material_ids: job.tools.map((tool) => tool.material.id),
      recipe: { slots: [{ material: 0, height_mm: 0.2 }, { material: 1, height_mm: 0.2 }], phase_slot: 0, origin_z_mm: 0.03 },
    };
  } else if (mode === "all-white") {
    job.surface_constructions[0].shell.material_id = job.tools[1].material.id;
  }
  intent.recipe.infill_fraction = 0.25;
  const intentFile = join(output, `${name}.intent.json`);
  await writeFile(intentFile, JSON.stringify(intent, null, 2));
  const exported = join(output, `${name}-baseline`);
  const compiled = await invoke(["project", "edit", project, "--intent", intentFile,
    "--preflight", "--compile", "--verify", "--save", "--manufacturing-out", exported]);
  assert.equal(compiled.manufacturing.verification.verified, true);
  const plan = await json(join(exported, "plan.json"));
  for (const role of ["shell", "backing", "core"]) {
    assert.ok(plan.material_domains.some((d) => d.reference.role === role));
    assert.ok(plan.layers.some((l) => l.paths.some((p) => p.material_domain?.role === role)));
  }
  assert.ok(plan.layers.some((l) => l.region_coverage.sparse_interior_area_mm2 > 0));
  assert.ok(plan.layers.every((l) => l.region_coverage.process_regions.every((r) =>
    r.material_domain.role === "core" || r.fraction === 1)));
  if (sourceName === "through-hole.step") {
    assert.ok(plan.layers.every((l) => l.region_coverage.protected_hole_area_mm2 > 0));
    assert.ok(plan.material_domains.some((d) => d.polygons.some((p) => p.holes_mm.length > 0)));
  }
  if (mode === "window") {
    assert.deepEqual(plan.material_frames[0].selected_u_mm, [4, 16]);
    assert.deepEqual(plan.material_frames[0].horizontal_direction, [1, 0, 0]);
    assert.ok(plan.layers.flatMap((l) => l.paths).filter((p) => p.material_domain.role === "shell")
      .every((p) => p.points_mm.every((point) => point[0] > 24 && point[0] < 36)));
  }
  const baseline = await readFile(join(exported, "program.gcode"));
  const changed = structuredClone(intent);
  if (mode === "cycle") {
    changed.printer.workflow.job.surface_constructions[0].shell.recipe.phase_slot = 1;
  } else if (mode === "window") {
    changed.printer.workflow.job.surface_constructions[0].faces[0].mask = { min_u_mm: 5, max_u_mm: 17 };
  } else {
    changed.printer.workflow.job.surface_constructions[0].faces[0].skin_depth_mm = 1.35;
  }
  const changedFile = join(output, `${name}-changed.intent.json`);
  await writeFile(changedFile, JSON.stringify(changed, null, 2));
  const changedExport = join(output, `${name}-changed`);
  await invoke(["project", "edit", project, "--intent", changedFile, "--compile", "--verify", "--save", "--manufacturing-out", changedExport]);
  assert.notEqual(hash(await readFile(join(changedExport, "program.gcode"))), hash(baseline), "construction edit did not change actual output");
  const restoredExport = join(output, `${name}-restored`);
  await invoke(["project", "edit", project, "--intent", intentFile, "--compile", "--verify", "--save", "--manufacturing-out", restoredExport]);
  assert.deepEqual(await readFile(join(restoredExport, "program.gcode")), baseline);
  await invoke(["project", "edit", project, "--intent", changedFile, "--undo", "--redo", "--undo", "--verify", "--save"]);
  await unlink(source); // Never remove repository fixtures or Core-owned assets.
  const recoveredExport = join(output, `${name}-source-absent`);
  const replay = join(output, `${name}.splr`);
  const recovered = await invoke(["project", "inspect", project, "--verify", "--manufacturing-out", recoveredExport, "--replay-out", replay]);
  const bytes = Uint8Array.from(await readFile(replay)).buffer;
  const decoded = await decodeReplayDisplay(bytes, recovered.manufacturing_replay.replay_display);
  let wasteOnly = 0;
  for (let layer = 0; layer < plan.layers.length; layer++) {
    for (const tool of [0, 1]) {
      let epochs = 0, waste = 0, model = 0;
      for (let event = 0; event < decoded.eventCount; event++) {
        if (decoded.layer(event) !== layer || decoded.activeTool(event) !== tool) continue;
        const kind = decoded.kind(event);
        if (kind === 14) epochs++;
        else if (kind === 15) waste++;
        else if ([1, 2, 3, 4, 5, 8].includes(kind)) model++;
      }
      assert.equal(epochs, 1);
      assert.equal(waste, 1);
      if (model === 0) wasteOnly++;
    }
  }
  if (mode === "cycle" || mode === "all-white") assert.ok(wasteOnly > 0);
  if (mode === "all-white") {
    assert.equal(compiled.manufacturing.verification.routing.tools[0].model_deposition_segments, 0);
  }
  assert.deepEqual(await readFile(join(recoveredExport, "program.gcode")), baseline);
  const replayAgain = join(output, `${name}-again.splr`);
  await invoke(["project", "inspect", project, "--verify", "--replay-out", replayAgain]);
  assert.deepEqual(await readFile(replayAgain), await readFile(replay));
  for (const [key, alter] of [
    ["wrong-source", (c) => { c.source_hash = "0".repeat(64); }],
    ["thin-skin", (c) => { c.faces[0].skin_depth_mm = 0.01; }],
    ["lost-core", (c) => { c.faces[0].backing_depth_mm = 100; }],
    ["mask-outside-face", (c) => { c.faces[0].mask = { min_u_mm: 1, max_u_mm: 1000 }; }],
    ["thin-window", (c) => { c.faces[0].mask = { min_u_mm: 4, max_u_mm: 4.01 }; }],
  ]) {
    const invalid = structuredClone(intent);
    alter(invalid.printer.workflow.job.surface_constructions[0]);
    const invalidFile = join(output, `${name}-${key}.json`);
    await writeFile(invalidFile, JSON.stringify(invalid));
    await invoke(["project", "edit", project, "--intent", invalidFile, "--compile"], true);
  }
  results.push({ source: sourceName, mode, program_sha256: hash(baseline), replay_sha256: hash(await readFile(replay)), layers: plan.layers.length });
}
await writeFile(join(output, "result.json"), JSON.stringify({ software_only: true, results }, null, 2));
console.log(JSON.stringify({ output, software_only: true, results }, null, 2));
