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
async function invoke(args, failure = false, domain = "manufacturing") {
  const result = spawnSync(cli, [...args, "--engine", engine], {
    cwd: root,
    encoding: "utf8",
    timeout: 330_000,
    maxBuffer: 16 * 1024 * 1024,
  });
  const receipt = join(output, `${String(++sequence).padStart(3, "0")}.json`);
  await writeFile(
    receipt,
    JSON.stringify(
      { args, status: result.status, stdout: result.stdout, stderr: result.stderr },
      null,
      2,
    ),
  );
  if (result.error) throw result.error;
  if (failure) {
    assert.notEqual(result.status, 0, `unsupported construction succeeded: ${receipt}`);
    const diagnostic = result.stderr
      .trim()
      .split("\n")
      .map((line) => {
        try {
          return JSON.parse(line);
        } catch {
          return null;
        }
      })
      .find((row) => row?.domain === domain);
    assert.ok(diagnostic?.error, `missing typed refusal: ${receipt}`);
    return diagnostic.error;
  }
  assert.equal(result.status, 0, `${receipt}: ${result.stderr}`);
  const report = JSON.parse(result.stdout);
  assert.match(report.hello.engine_build, /planner34;emitter11;verifier17/);
  return report;
}
const json = async (path) => JSON.parse(await readFile(path, "utf8"));
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const results = [];
const cases = ["box-mm.step", "through-hole.step"].flatMap((source) => [
  [source, "full"],
  [source, "window"],
]);
cases.push(
  ["box-mm.step", "cycle"],
  ["box-mm.step", "all-white"],
  ["box-mm.step", "regional"],
  ["through-hole.step", "regional"],
  ["box-mm.step", "corner"],
  ["through-hole.step", "corner"],
  ["box-mm.step", "study"],
  ["through-hole.step", "study"],
);
const modes = new Set(process.argv.slice(2));
assert.ok(
  [...modes].every((mode) => cases.some((c) => c[1] === mode)),
  "unknown smoke mode",
);
for (const [sourceName, mode] of cases) {
  if (modes.size && !modes.has(mode)) continue;
  const name = sourceName.replace(/\.step$/, "") + "-" + mode;
  const source = join(output, sourceName);
  const project = join(output, `${name}.spiling`);
  await copyFile(join(root, "fixtures", "geometry", sourceName), source);
  const created = await invoke([
    "project",
    "create",
    project,
    "--import",
    source,
    "--pose",
    "1",
    JSON.stringify({ translation_mm: [20, 20, 0], rotation_xyzw: [0, 0, 0, 1] }),
  ]);
  const faces = created.definitions[0].faces;
  const inspected = await invoke([
    "project",
    "inspect",
    project,
    ...faces.flatMap((face) => ["--inspect-face", "1", face.face_id]),
  ]);
  const selected = inspected.face_inspections.find((row) => {
    const plane = row.face.geometry.analytic;
    return plane?.kind === "plane" && plane.normal[1] < -0.5 && plane.normal[2] === 0;
  });
  assert.ok(selected, "native source has no selected upright exterior face");
  let intent = await json(join(root, "fixtures", "manufacturing", "fixed-dual.intent.json"));
  const job = intent.printer.workflow.job;
  job.assignments = job.assignments.slice(0, 1);
  job.assignments[0].material = { kind: "uniform", material_id: job.tools[1].material.id };
  const bounds = created.definitions[0].record.bounds_mm;
  job.assignments[0].work_envelope_mm = {
    min: bounds.min.map((v, axis) => v + [20, 20, 0][axis] - 1),
    max: bounds.max.map((v, axis) => v + [20, 20, 0][axis] + 1),
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
          mask: mode === "window" || mode === "study" ? { min_u_mm: 4, max_u_mm: 16 } : null,
        },
      ],
      shell: { kind: "uniform", material_id: job.tools[0].material.id },
      white_material_id: job.tools[1].material.id,
    },
  ];
  if (mode === "corner") {
    const side = inspected.face_inspections.find((row) => {
      const plane = row.face.geometry.analytic;
      return plane?.kind === "plane" && plane.normal[0] > 0.5 && plane.normal[2] === 0;
    });
    assert.ok(side, "native source has no adjoining upright exterior face");
    job.surface_constructions[0].faces.push({
      ...job.surface_constructions[0].faces[0],
      face_id: side.face.face_id,
    });
  } else if (mode === "regional") {
    job.surface_constructions.push(structuredClone(job.surface_constructions[0]));
    for (const [index, construction] of job.surface_constructions.entries()) {
      construction.faces[0].mask = { min_u_mm: [2, 12][index], max_u_mm: [8, 18][index] };
      construction.shell = {
        kind: "layer_cycle",
        material_ids: job.tools.map((tool) => tool.material.id),
        recipe: {
          slots: [
            { material: 0, height_mm: 0.2 },
            { material: 1, height_mm: 0.2 },
          ],
          phase_slot: index,
          origin_z_mm: 0.03,
        },
      };
    }
  } else if (mode === "cycle") {
    job.surface_constructions[0].shell = {
      kind: "layer_cycle",
      material_ids: job.tools.map((tool) => tool.material.id),
      recipe: {
        slots: [
          { material: 0, height_mm: 0.2 },
          { material: 1, height_mm: 0.2 },
        ],
        phase_slot: 0,
        origin_z_mm: 0.03,
      },
    };
  } else if (mode === "all-white") {
    job.surface_constructions[0].shell.material_id = job.tools[1].material.id;
  }
  intent.recipe.infill_fraction = 0.25;
  const intentFile = join(output, `${name}.intent.json`);
  await writeFile(intentFile, JSON.stringify(intent, null, 2));
  const exported = join(output, `${name}-baseline`);
  let applyArgs = ["--intent", intentFile];
  let bindingsFile;
  let studyFile;
  let paletteFile;
  if (mode === "study") {
    const palette = await json(join(root, "fixtures", "appearance", "surface-color.palette.json"));
    const study = await json(join(root, "fixtures", "appearance", "optical-layer-cycle.json"));
    study.materials = palette.rows.map((row) => row.key);
    study.recipe = {
      slots: [
        { material: 0, height_mm: 0.2 },
        { material: 1, height_mm: 0.2 },
      ],
      phase_slot: 0,
      origin_z_mm: 0.03,
    };
    study.patch = { origin_mm: [24, 20, 0], normal_xy: [0, -1], width_mm: 12, height_mm: 0.15 };
    study.search = {
      patterns: [{ materials: [0, 1] }, { materials: [1, 0] }],
      slot_heights_mm: [0.2],
      phase_slots: [0, 1],
      origin_z_mm: 0.03,
    };
    paletteFile = join(output, `${name}.palette.json`);
    studyFile = join(output, `${name}.study.json`);
    await writeFile(paletteFile, JSON.stringify(palette));
    await writeFile(studyFile, JSON.stringify(study));
    const retained = await invoke([
      "project",
      "edit",
      project,
      "--intent",
      intentFile,
      "--appearance-palette",
      paletteFile,
      "--optical-study",
      studyFile,
      "--optical-compare",
      "--optical-accept",
      "requested",
      "--save",
    ]);
    const evidence = retained.optical[0].report;
    const bindings = study.materials.map((key, index) => ({
      evidence_key: key,
      evidence_hash: evidence.material_evidence_hashes[index],
      loaded_material_id: job.tools[index].material.id,
      loaded_declaration_hash: job.tools[index].material.declaration_hash,
    }));
    bindingsFile = join(output, `${name}.bindings.json`);
    await writeFile(bindingsFile, JSON.stringify(bindings));
    const wrong = structuredClone(bindings);
    wrong[0].evidence_hash = "0".repeat(64);
    const wrongFile = join(output, `${name}-wrong-binding.json`);
    await writeFile(wrongFile, JSON.stringify(wrong));
    await invoke(["project", "edit", project, "--apply-optical", "0", wrongFile], true, "project");
    applyArgs = ["--apply-optical", "0", bindingsFile];
  }
  const compiled = await invoke([
    "project",
    "edit",
    project,
    ...applyArgs,
    "--preflight",
    "--compile",
    "--verify",
    "--save",
    "--manufacturing-out",
    exported,
  ]);
  if (mode === "study") {
    intent = compiled.manufacturing.intent;
    const applied = intent.printer.workflow.job.surface_constructions[0];
    assert.equal(applied.shell.kind, "layer_cycle");
    assert.equal(applied.optical_origin.materials[0].evidence.key, "software-blue/catalog");
    assert.deepEqual(applied.optical_origin.applied_pose.translation_mm, [20, 20, 0]);
    assert.equal(
      compiled.scene.revision,
      1,
      "reopened application must preserve its initial native scene epoch",
    );
    await writeFile(intentFile, JSON.stringify(intent));
  }
  assert.equal(compiled.manufacturing.verification.verified, true);
  const plan = await json(join(exported, "plan.json"));
  for (const role of ["shell", "backing", "core"]) {
    assert.ok(plan.material_domains.some((d) => d.reference.role === role));
    assert.ok(plan.layers.some((l) => l.paths.some((p) => p.material_domain?.role === role)));
  }
  assert.ok(plan.layers.some((l) => l.region_coverage.sparse_interior_area_mm2 > 0));
  assert.ok(
    plan.layers.every((l) =>
      l.region_coverage.process_regions.every(
        (r) => r.material_domain.role === "core" || r.fraction === 1,
      ),
    ),
  );
  if (sourceName === "through-hole.step") {
    assert.ok(plan.layers.every((l) => l.region_coverage.protected_hole_area_mm2 > 0));
    assert.ok(plan.material_domains.some((d) => d.polygons.some((p) => p.holes_mm.length > 0)));
  }
  if (mode === "window") {
    assert.deepEqual(plan.material_frames[0].selected_u_mm, [4, 16]);
    assert.deepEqual(plan.material_frames[0].horizontal_direction, [1, 0, 0]);
    assert.ok(
      plan.layers
        .flatMap((l) => l.paths)
        .filter((p) => p.material_domain.role === "shell")
        .every((p) => p.points_mm.every((point) => point[0] > 24 && point[0] < 36)),
    );
  }
  if (mode === "regional") {
    assert.deepEqual(
      plan.material_frames.map((f) => f.selected_u_mm),
      [
        [2, 8],
        [12, 18],
      ],
    );
    for (const layer of plan.layers) {
      const tools = [new Set(), new Set()];
      for (const path of layer.paths) {
        const ref = path.material_domain;
        if (ref.role === "shell") {
          const material = job.surface_constructions[ref.construction_index].shell;
          const slot = Math.floor(
            ((path.physical_z_mm[0] + path.physical_z_mm[1]) / 2 - material.recipe.origin_z_mm) /
              0.2,
          );
          tools[ref.construction_index].add((((slot + material.recipe.phase_slot) % 2) + 2) % 2);
        } else assert.equal(ref.construction_index, 0);
      }
      assert.equal(tools[0].size, 1);
      assert.equal(tools[1].size, 1);
      assert.notDeepEqual(tools[0], tools[1]);
    }
  }
  const provenance = await json(join(exported, "provenance.json"));
  assert.equal(provenance.inspection.surface_color.length, plan.material_frames.length);
  for (const face of provenance.inspection.surface_color) {
    assert.equal(face.interval_count, plan.layers.length);
    assert.ok(
      Math.abs(
        face.exposed_height_by_tool_mm.reduce((a, b) => a + b, 0) - face.realized_patch.height_mm,
      ) < 1e-10,
    );
    if (mode === "study") {
      const reference = face.reference;
      assert.equal(reference.backing_model, "unmodelled");
      assert.equal(reference.patch_matches_request, false);
      assert.equal(reference.pose_matches_application, true);
      assert.equal(reference.requested.kind, "estimate");
      assert.equal(reference.realized.kind, "estimate");
      assert.ok(Math.abs(reference.requested.exposure.fractions[0] - 0.8) < 1e-10);
      assert.ok(Math.abs(reference.realized.exposure.fractions[0] - 0.5) < 1e-10);
      assert.notDeepEqual(reference.realized.predicted_lab, reference.requested.predicted_lab);
      assert.ok(
        Math.abs(face.fractions_by_tool[0] - reference.realized.exposure.fractions[0]) < 1e-10,
      );
    } else assert.equal(face.reference, null);
  }
  const baseline = await readFile(join(exported, "program.gcode"));
  const changed = structuredClone(intent);
  if (mode === "corner") {
    for (const face of changed.printer.workflow.job.surface_constructions[0].faces)
      face.skin_depth_mm = 1.35;
  } else if (mode === "cycle" || mode === "regional") {
    changed.printer.workflow.job.surface_constructions[0].shell.recipe.phase_slot = 1;
  } else if (mode === "window") {
    changed.printer.workflow.job.surface_constructions[0].faces[0].mask = {
      min_u_mm: 5,
      max_u_mm: 17,
    };
  } else {
    changed.printer.workflow.job.surface_constructions[0].faces[0].skin_depth_mm = 1.35;
  }
  const changedFile = join(output, `${name}-changed.intent.json`);
  await writeFile(changedFile, JSON.stringify(changed, null, 2));
  const changedExport = join(output, `${name}-changed`);
  await invoke([
    "project",
    "edit",
    project,
    "--intent",
    changedFile,
    "--compile",
    "--verify",
    "--save",
    "--manufacturing-out",
    changedExport,
  ]);
  assert.notEqual(
    hash(await readFile(join(changedExport, "program.gcode"))),
    hash(baseline),
    "construction edit did not change actual output",
  );
  const restoredExport = join(output, `${name}-restored`);
  await invoke([
    "project",
    "edit",
    project,
    "--intent",
    intentFile,
    "--compile",
    "--verify",
    "--save",
    "--manufacturing-out",
    restoredExport,
  ]);
  assert.deepEqual(await readFile(join(restoredExport, "program.gcode")), baseline);
  if (mode === "study") {
    const movedExport = join(output, `${name}-moved`);
    await invoke([
      "project",
      "edit",
      project,
      "--pose",
      "1",
      JSON.stringify({ translation_mm: [21, 20, 0], rotation_xyzw: [0, 0, 0, 1] }),
      "--compile",
      "--verify",
      "--manufacturing-out",
      movedExport,
    ]);
    const moved = (await json(join(movedExport, "provenance.json"))).inspection.surface_color[0];
    assert.equal(moved.reference.pose_matches_application, false);
    assert.equal(moved.realized_patch.origin_mm[0], 25);
    assert.deepEqual(
      moved.reference.requested,
      provenance.inspection.surface_color[0].reference.requested,
    );
    await invoke([
      "project",
      "edit",
      project,
      "--pose",
      "1",
      JSON.stringify({ translation_mm: [20, 20, 0], rotation_xyzw: [0, 0, 0, 1] }),
      "--compile",
      "--verify",
      "--save",
    ]);
  }
  await invoke([
    "project",
    "edit",
    project,
    "--intent",
    changedFile,
    "--undo",
    "--redo",
    "--undo",
    "--verify",
    "--save",
  ]);
  await unlink(source); // Never remove repository fixtures or Core-owned assets.
  if (mode === "study") {
    await unlink(paletteFile);
    await unlink(studyFile);
  }
  const recoveredExport = join(output, `${name}-source-absent`);
  const replay = join(output, `${name}.splr`);
  const recovered = await invoke([
    "project",
    "inspect",
    project,
    "--verify",
    "--manufacturing-out",
    recoveredExport,
    "--replay-out",
    replay,
  ]);
  assert.deepEqual(
    (await json(join(recoveredExport, "provenance.json"))).inspection.surface_color,
    provenance.inspection.surface_color,
    "source-independent fresh reassessment changed",
  );
  const bytes = Uint8Array.from(await readFile(replay)).buffer;
  const decoded = await decodeReplayDisplay(bytes, recovered.manufacturing_replay.replay_display);
  let wasteOnly = 0;
  for (let layer = 0; layer < plan.layers.length; layer++) {
    for (const tool of [0, 1]) {
      let epochs = 0,
        waste = 0,
        model = 0;
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
  if (mode === "regional") {
    const seen = Array.from({ length: plan.layers.length }, () => [new Set(), new Set()]);
    for (let event = 1; event < decoded.eventCount; event++) {
      if (![1, 2, 3, 4, 5, 8].includes(decoded.kind(event))) continue;
      const points = [decoded.position(event - 1), decoded.position(event)];
      for (const region of [0, 1]) {
        const [lo, hi] = region === 0 ? [22, 28] : [32, 38];
        if (points.every((p) => p[0] > lo && p[0] < hi && p[1] > 20 && p[1] < 21.2))
          seen[decoded.layer(event)][region].add(decoded.activeTool(event));
      }
    }
    for (const layer of seen) {
      assert.equal(layer[0].size, 1);
      assert.equal(layer[1].size, 1);
      assert.notDeepEqual(
        layer[0],
        layer[1],
        "actual common-Z region tools lost independent phase",
      );
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
    [
      "wrong-source",
      (c) => {
        c.source_hash = "0".repeat(64);
      },
    ],
    [
      "thin-skin",
      (c) => {
        c.faces[0].skin_depth_mm = 0.01;
      },
    ],
    [
      "lost-core",
      (c) => {
        c.faces[0].backing_depth_mm = 100;
      },
    ],
    [
      "mask-outside-face",
      (c) => {
        c.faces[0].mask = { min_u_mm: 1, max_u_mm: 1000 };
      },
    ],
    [
      "thin-window",
      (c) => {
        c.faces[0].mask = { min_u_mm: 4, max_u_mm: 4.01 };
      },
    ],
  ]) {
    const invalid = structuredClone(intent);
    alter(invalid.printer.workflow.job.surface_constructions[0]);
    const invalidFile = join(output, `${name}-${key}.json`);
    await writeFile(invalidFile, JSON.stringify(invalid));
    await invoke(["project", "edit", project, "--intent", invalidFile, "--compile"], true);
  }
  if (mode === "regional") {
    const invalid = structuredClone(intent);
    invalid.printer.workflow.job.surface_constructions[1].faces[0].mask = {
      min_u_mm: 7,
      max_u_mm: 15,
    };
    const invalidFile = join(output, `${name}-regional-overlap.json`);
    await writeFile(invalidFile, JSON.stringify(invalid));
    const error = await invoke(
      ["project", "edit", project, "--intent", invalidFile, "--compile"],
      true,
    );
    assert.match(error.message, /overlap/);
  }
  if (mode === "corner") {
    assert.equal(plan.material_frames.length, 2);
    const invalid = structuredClone(intent);
    invalid.printer.workflow.job.surface_constructions[0].faces[1].skin_depth_mm = 1.5;
    const invalidFile = join(output, `${name}-conflicting-corner.json`);
    await writeFile(invalidFile, JSON.stringify(invalid));
    const error = await invoke(
      ["project", "edit", project, "--intent", invalidFile, "--compile"],
      true,
    );
    assert.match(error.message, /intersecting face constructions/);
  }
  results.push({
    source: sourceName,
    mode,
    program_sha256: hash(baseline),
    replay_sha256: hash(await readFile(replay)),
    layers: plan.layers.length,
  });
}
await writeFile(
  join(output, "result.json"),
  JSON.stringify({ software_only: true, results }, null, 2),
);
console.log(JSON.stringify({ output, software_only: true, results }, null, 2));
