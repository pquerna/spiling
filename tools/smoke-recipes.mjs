/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Separate real CLI/engine processes: capture → personal library → standalone reuse → fresh replay.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { readFile, writeFile, mkdir, mkdtemp, copyFile, rename, unlink } from "node:fs/promises";
import { resolve, join } from "node:path";
const root = resolve(import.meta.dirname, "..");
const target = resolve(root, process.env.CARGO_TARGET_DIR ?? "target");
const suffix = process.platform === "win32" ? ".exe" : "";
const cli = join(target, "debug", `spiling-cli${suffix}`);
const engine = join(target, "debug", `spiling-engine${suffix}`);
const hash = (value) => createHash("sha256").update(value).digest("hex");
await mkdir(join(root, ".cache"), { recursive: true });
const output = await mkdtemp(join(root, ".cache", "recipes-"));
console.error(`Recipe evidence: ${output}`);
const manifest = {
  commands: [],
  binaries: { cli: hash(await readFile(cli)), engine: hash(await readFile(engine)) },
  softwareOnly: true,
};
const json = async (path) => JSON.parse(await readFile(path, "utf8"));
async function save(name, value) {
  const path = join(output, name);
  await writeFile(path, JSON.stringify(value, null, 2) + "\n", { flag: "wx" });
  return path;
}
async function run(name, args, expectedFailure = false) {
  const command = [...args, "--engine", engine];
  manifest.commands.push({ name, args: command });
  let stdout = "",
    stderr = "",
    bytes = 0;
  const status = await new Promise((done, fail) => {
    const child = spawn(cli, command, {
      cwd: root,
      env: { ...process.env, RAYON_NUM_THREADS: "1" },
    });
    const deadline = setTimeout(() => {
      child.kill("SIGKILL");
      fail(new Error(`CLI deadline: ${name}`));
    }, 330_000);
    child.stdout.on("data", (part) => {
      bytes += part.length;
      if (bytes > 16 * 1024 * 1024) {
        child.kill("SIGKILL");
        fail(new Error("CLI output bound"));
      } else stdout += part;
    });
    child.stderr.on("data", (part) => {
      bytes += part.length;
      if (bytes > 16 * 1024 * 1024) {
        child.kill("SIGKILL");
        fail(new Error("CLI output bound"));
      } else stderr += part;
    });
    child.on("error", fail);
    child.on("close", (code) => {
      clearTimeout(deadline);
      done(code);
    });
  });
  await save(`${name}.receipt.json`, { command, status, stdout, stderr });
  if (expectedFailure) {
    assert.notEqual(status, 0, `${name} unexpectedly succeeded`);
    return;
  }
  assert.equal(status, 0, `${name}: ${stderr}`);
  const report = JSON.parse(stdout);
  if (report.hello) manifest.engineBuild = report.hello.engine_build;
  return report;
}
const library = join(output, "My library.spil"),
  first = join(output, "Source.spil"),
  reused = join(output, "Reused.spil");
try {
  for (const [name, hex] of [
    ["Blue ingredient", "1A4DE6"],
    ["White ingredient", "FFFFFF"],
  ])
    await run(`add-${hex}`, ["library", "add-filament", name, "--hex", hex, "--library", library]);
  const listed = await run("filaments", ["library", "filaments", "--library", library]);
  const filaments = ["Blue ingredient", "White ingredient"].map((name) => {
    const row = listed.result.find((entry) => entry.name === name);
    assert.ok(row, `Missing declared filament: ${name}`);
    return row.entry.current;
  });
  assert.equal(filaments.length, 2);
  const source = join(output, "box.step");
  await copyFile(join(root, "fixtures/geometry/box-mm.step"), source);
  const created = await run("source-create", [
    "project",
    "create",
    first,
    "--import",
    source,
    "--pose",
    "1",
    JSON.stringify({ translation_mm: [20, 20, 0], rotation_xyzw: [0, 0, 0, 1] }),
    "--library",
    library,
    ...filaments.flatMap((f) => ["--library-use", f.id]),
    "--save",
  ]);
  const inspected = await run("source-faces", [
    "project",
    "inspect",
    first,
    ...created.definitions[0].faces.flatMap((f) => ["--inspect-face", "1", f.face_id]),
  ]);
  const selected = inspected.face_inspections.find(
    (row) =>
      row.face.geometry.analytic?.kind === "plane" &&
      row.face.geometry.analytic.normal[1] < -0.5 &&
      row.face.geometry.analytic.normal[2] === 0,
  );
  assert.ok(selected);
  const intent = await json(join(root, "fixtures/manufacturing/fixed-dual.intent.json"));
  const job = intent.printer.workflow.job;
  job.assignments = job.assignments.slice(0, 1);
  job.assignments[0].material = { kind: "uniform", material_id: job.tools[1].material.id };
  const bounds = created.definitions[0].record.bounds_mm;
  job.assignments[0].work_envelope_mm = {
    min: bounds.min.map((v, i) => v + [20, 20, 0][i] - 1),
    max: bounds.max.map((v, i) => v + [20, 20, 0][i] + 1),
  };
  job.surface_constructions = [
    {
      optical_origin: null,
      occurrence_id: 1,
      definition_id: selected.reference.definition_id,
      source_hash: selected.provenance.source_hash,
      faces: [
        {
          face_id: selected.face.face_id,
          skin_depth_mm: 1.2,
          backing_depth_mm: 1.2,
          mask: { min_u_mm: 4, max_u_mm: 16 },
        },
      ],
      shell: { kind: "uniform", material_id: job.tools[0].material.id },
      white_material_id: job.tools[1].material.id,
    },
  ];
  intent.recipe.infill_fraction = 0.25;
  const intentFile = await save("intent.json", intent);
  const palette = await json(join(root, "fixtures/appearance/surface-color.palette.json"));
  const request = await json(join(root, "fixtures/appearance/optical-layer-cycle.json"));
  request.materials = palette.rows.map((row) => row.key);
  request.recipe = {
    slots: [
      { material: 0, height_mm: 0.2 },
      { material: 1, height_mm: 0.2 },
    ],
    phase_slot: 0,
    origin_z_mm: 0.03,
  };
  request.patch = { origin_mm: [24, 20, 0], normal_xy: [0, -1], width_mm: 12, height_mm: 0.15 };
  request.search = {
    patterns: [{ materials: [0, 1] }, { materials: [1, 0] }],
    slot_heights_mm: [0.2],
    phase_slots: [1],
    origin_z_mm: 0.03,
  };
  const paletteFile = await save("palette.json", palette),
    requestFile = await save("study.json", request);
  const ingredientFile = await save(
    "ingredients.json",
    filaments.map((filament, i) => ({ filament, palette_key: request.materials[i] })),
  );
  // Explicitly retain candidate zero, whose phase differs from the requested cycle.
  const captured = await run("capture", [
    "project",
    "edit",
    first,
    "--appearance-palette",
    paletteFile,
    "--optical-study",
    requestFile,
    "--optical-compare",
    "--optical-accept",
    "0",
    "--recipe-capture",
    "Reusable blue and white",
    ingredientFile,
    "--project-recipes",
    "--save",
  ]);
  const originalRef = captured.operations.find((r) => r.step === "project_recipes").entries[0]
    .reference;
  const original = await run("inspect-capture", [
    "project",
    "inspect",
    first,
    "--recipe-inspect",
    originalRef.id,
  ]);
  const transfer = original.operations.find((r) => r.step === "recipe_inspect").transfer;
  assert.equal(transfer.recipe.definition.request.recipe.phase_slot, 1);
  assert.equal(transfer.filaments.length, 2);
  const published = await run("save-personal", [
    "project",
    "open",
    first,
    "--library",
    library,
    "--recipe-save",
    originalRef.id,
  ]);
  const recipeRef = published.operations.find((r) => r.step === "recipe_save").reference;
  const personal = await run("inspect-personal", [
    "library",
    "recipe",
    recipeRef.id,
    "--library",
    library,
  ]);
  assert.deepEqual(personal.result.recipe.definition, transfer.recipe.definition);
  const context = { patch: { ...request.patch, height_mm: 0.35 }, cycle_origin_z_mm: 0.03 };
  const contextFile = await save("context.json", context);
  const loaded = await run("reuse", [
    "project",
    "create",
    reused,
    "--import",
    source,
    "--pose",
    "1",
    JSON.stringify({ translation_mm: [20, 20, 0], rotation_xyzw: [0, 0, 0, 1] }),
    "--intent",
    intentFile,
    "--library",
    library,
    "--recipe-use",
    recipeRef.id,
    "--recipe-study",
    recipeRef.id,
    contextFile,
    "--save",
  ]);
  assert.equal(loaded.project.appearance_study.optical.report, null);
  assert.equal(loaded.project.appearance_study.optical.selection, null);
  assert.equal(loaded.project.materials.filament_count, 2);
  assert.deepEqual(loaded.project.appearance_study.optical.request.patch, context.patch);
  await run(
    "duplicate-refusal",
    ["project", "open", reused, "--library", library, "--recipe-use", recipeRef.id],
    true,
  );
  const assessed = await run("reassess", [
    "project",
    "edit",
    reused,
    "--optical-compare",
    "--optical-accept",
    "requested",
    "--save",
  ]);
  const evidence = assessed.optical[0].report;
  assert.deepEqual(evidence.request.patch, context.patch);
  const bindingsFile = await save(
    "bindings.json",
    request.materials.map((key, i) => ({
      evidence_key: key,
      evidence_hash: evidence.material_evidence_hashes[i],
      loaded_material_id: job.tools[i].material.id,
      loaded_declaration_hash: job.tools[i].material.declaration_hash,
    })),
  );
  await unlink(source);
  await rename(library, join(output, "Unavailable.spil"));
  const exportOne = join(output, "first-export"),
    exportTwo = join(output, "reopened-export");
  const compiled = await run("apply-compile", [
    "project",
    "edit",
    reused,
    "--library",
    join(output, "Missing.spil"),
    "--apply-optical",
    "0",
    bindingsFile,
    "--preflight",
    "--compile",
    "--verify",
    "--save",
    "--manufacturing-out",
    exportOne,
  ]);
  assert.equal(compiled.manufacturing.verification.verified, true);
  assert.equal(
    compiled.manufacturing.intent.printer.workflow.job.surface_constructions[0].shell.recipe
      .phase_slot,
    1,
  );
  const reopened = await run("standalone-recompile", [
    "project",
    "open",
    reused,
    "--library",
    join(output, "Missing.spil"),
    "--compile",
    "--verify",
    "--manufacturing-out",
    exportTwo,
  ]);
  assert.equal(reopened.manufacturing.verification.verified, true);
  manifest.exports = {};
  for (const name of ["program.gcode", "verification.json"]) {
    const a = await readFile(join(exportOne, name)),
      b = await readFile(join(exportTwo, name));
    assert.equal(hash(a), hash(b), `standalone ${name} differs`);
    manifest.exports[name] = hash(a);
  }
  const a = await json(join(exportOne, "plan.json")),
    b = await json(join(exportTwo, "plan.json"));
  // Charged query work records cache/refinement history, not the normalized paths or admission proofs.
  manifest.observedGeometryWork = [
    a.native_geometry.geometry_work,
    b.native_geometry.geometry_work,
  ];
  assert.ok(manifest.observedGeometryWork.every((work) => Number.isSafeInteger(work) && work > 0));
  delete a.native_geometry.geometry_work;
  delete b.native_geometry.geometry_work;
  assert.deepEqual(a, b, "standalone normalized plan differs");
  const pa = await json(join(exportOne, "provenance.json")),
    pb = await json(join(exportTwo, "provenance.json"));
  manifest.observedInputRevisions = [pa.input_revision, pb.input_revision];
  for (const provenance of [pa, pb]) {
    assert.equal(provenance.input_revision, provenance.inspection.input_revision);
    delete provenance.input_revision;
    delete provenance.inspection.input_revision;
  }
  assert.deepEqual(
    pa,
    pb,
    "standalone provenance differs beyond the explicitly captured input revision",
  );
  await save("manifest.json", manifest);
  console.log(
    JSON.stringify(
      { output, recipeRef, engineBuild: manifest.engineBuild, exports: manifest.exports },
      null,
      2,
    ),
  );
} catch (error) {
  await save("failure.json", { message: error.message, stack: error.stack, manifest });
  throw error;
}
