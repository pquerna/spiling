/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Real native CLI/engine journey. Requires coherent prebuilt binaries and an
// external whole-process resource bound. This runner never builds or prints.
import assert from "node:assert/strict";
import { projectSmoke } from "./project-smoke.mjs";
import { copyFile, mkdir, mkdtemp, readFile, unlink, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { decodeReplayDisplay } from "../packages/protocol/src/replay.ts";

const root = resolve(import.meta.dirname, "..");
const target = resolve(root, process.env.CARGO_TARGET_DIR ?? "target");
await mkdir(join(root, ".cache"), { recursive: true });
const output = await mkdtemp(join(root, ".cache", "mixed-tools-"));
console.error(`Evidence: ${output}`);
const evidence = { software_only: true, commands: [] };
const invoke = projectSmoke({ root, target, output, evidence });
const intentName = process.env.SPILING_MIXED_INTENT ?? "fixed-dual-shared-material";
const intent = JSON.parse(
  await readFile(join(root, `fixtures/manufacturing/${intentName}.intent.json`), "utf8"),
);
const selectionCase = process.env.SPILING_INTERIOR_CASE ?? "automatic";
assert.ok(["automatic", "slow-bulk", "no-timing"].includes(selectionCase));
if (selectionCase === "slow-bulk") intent.printer.workflow.job.tools[1].material_profile.selection.overrides.print_speed_mm_s = 1;
if (selectionCase === "no-timing") intent.printer.workflow.job.timing_model = null;
const source = join(output, "owned-box.step"),
  project = join(output, "Shared material.spil");
const singlePart = intent.printer.workflow.job.assignments.length === 1;
await copyFile(
  join(root, singlePart ? "fixtures/geometry/through-hole.step" : "fixtures/geometry/box-mm.step"),
  source,
);
const created = await invoke("create", [
  "create",
  project,
  "--import",
  source,
  "--pose",
  "1",
  JSON.stringify({ translation_mm: [20, 20, 0], rotation_xyzw: [0, 0, 0, 1] }),
]);
const intentFile = join(output, "intent.json");
await writeFile(intentFile, JSON.stringify(intent));
const originalExport = join(output, "original-export");
await invoke("compile", [
  "edit",
  project,
  ...(singlePart
    ? []
    : [
        "--add",
        created.definitions[0].record.definition_id,
        JSON.stringify({ translation_mm: [120, 20, 0], rotation_xyzw: [0, 0, 0, 1] }),
      ]),
  "--intent",
  intentFile,
  "--preflight",
  "--compile",
  "--verify",
  "--manufacturing-out",
  originalExport,
  "--save",
]);
if (intent.printer.workflow.job.tools[0].material_profile) {
  const selected = structuredClone(intent.printer.workflow.job.tools[0].material_profile.selection);
  selected.overrides.print_speed_mm_s = 27;
  const profileFile = join(output, "tool-profile.json");
  await writeFile(profileFile, JSON.stringify(selected));
  // Exercise engine resolution and ordinary persistent history, not just full-intent ingest.
  await invoke("profile-history", [
    "edit",
    project,
    "--tool-profile",
    intent.printer.workflow.job.tools[0].id,
    profileFile,
    "--undo",
    "--redo",
    "--compile",
    "--verify",
    "--save",
  ]);
  await unlink(profileFile);
  const changed = await invoke("profile-capture", ["inspect", project]);
  assert.equal(
    changed.manufacturing.intent.printer.workflow.job.tools[0].material_profile.selection.overrides
      .print_speed_mm_s,
    27,
  );
  assert.deepEqual(
    changed.manufacturing.intent.printer.workflow.job.tools[1],
    intent.printer.workflow.job.tools[1],
  );
  // Original export above predates the intentional process edit; compare the captured revision.
  await invoke("profile-export", [
    "inspect",
    project,
    "--verify",
    "--manufacturing-out",
    join(output, "profile-export"),
  ]);
}
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
  job.tools.slice(0, job.assignments.length).map((t) => t.id),
);
const decoded = await decodeReplayDisplay(
  Uint8Array.from(await readFile(replay)).buffer,
  returned.manufacturing.replay_display,
);
const plan = JSON.parse(await readFile(join(recoveredExport, "plan.json"), "utf8"));
const selection = plan.interior_selection;
const primaryOnly = selectionCase !== "automatic";
if (singlePart) {
  assert.ok(selection);
  assert.equal(selection.selected, primaryOnly ? "primary" : intentName === "mixed-width-combined" ? "grouped" : "synchronized");
  assert.equal(selection.reason, primaryOnly ? selectionCase === "no-timing" ? "no_timing_model" : "objective_not_met" : "improved_modeled_work");
  assert.equal(selection.candidates.length, 3);
  evidence.interior_selection = selection;
}
const counts = [0, 0];
let coarseEvents = 0;
for (let i = 0; i < decoded.eventCount; i++) {
  if (decoded.kind(i) === 15 && intentName === "mixed-width-combined") {
    const association = decoded.association(i);
    assert.ok(association);
    assert.ok(
      Math.abs(association.physicalZMm[1] - association.physicalZMm[0] - 0.2) < 1e-8,
      "auxiliary strips must retain their admitted physical cadence",
    );
  }
  if (![1, 2, 3, 4, 5, 8].includes(decoded.kind(i))) continue;
  const tool = decoded.activeTool(i),
    point = decoded.position(i);
  assert.ok(tool === 0 || tool === 1);
  const association = decoded.association(i);
  const interior = singlePart && typeof association?.regionalOwner === "number";
  const expected = singlePart ? (interior && !primaryOnly ? 1 : 0) : point[0] < 80 ? 0 : 1;
  if (singlePart) {
    assert.ok(association);
    if (interior) {
      assert.equal(association.beadWidthMm, primaryOnly ? intent.recipe.bead_width_mm : job.assignments[0].interior.bead_width_mm);
      if (association.physicalZMm[1]-association.physicalZMm[0] > 0.21) coarseEvents++;
    }
    // Independent analytic fixture observer: actual XYZ6 full beads retain the
    // original20x20 source and radius3 bore, under the existing0.005 mm policy.
    const from = decoded.position(i - 1),
      radius = association.beadWidthMm * 0.5;
    const dx = point[0] - from[0],
      dy = point[1] - from[1];
    const t = Math.max(
      0,
      Math.min(1, ((30 - from[0]) * dx + (30 - from[1]) * dy) / (dx * dx + dy * dy)),
    );
    assert.ok(
      Math.hypot(from[0] + t * dx - 30, from[1] + t * dy - 30) >= 3 + radius - 0.005001,
      "actual full bead intrudes into protected bore",
    );
    for (const p of [from, point])
      for (const v of p.slice(0, 2))
        assert.ok(
          v - radius >= 20 - 0.005001 && v + radius <= 40 + 0.005001,
          "actual full bead exceeds exterior",
        );
  }
  assert.equal(
    tool,
    expected,
    `actual model endpoint assigned to wrong same-material tool: ${point}`,
  );
  const selected = job.tools[tool].material_profile?.selection;
  if (selected) {
    const profile = selected.profile,
      policy = selected.overrides.process ?? profile.process;
    const base = selected.overrides.print_speed_mm_s ?? profile.print_speed_mm_s;
    const layer = decoded.layer(i);
    const speed =
      base *
      (layer === 0 ? policy.first_layer_speed_factor : 1) *
      (decoded.kind(i) === 8 ? policy.bridge_speed_factor : 1);
    assert.ok(
      Math.abs(decoded.feed(i) - speed * 60) < 0.000002,
      `tool${tool} actual model feed must consume its capture`,
    );
  }
  counts[tool]++;
}
assert.ok(primaryOnly ? counts[0] > 0 && counts[1] === 0 : counts.every((n) => n > 0));
if (intentName === "mixed-width-combined" && !primaryOnly)
  assert.ok(coarseEvents > 0, "the selected bulk tool must occupy real thicker intervals");
assert.deepEqual(
  await readFile(
    join(
      intent.printer.workflow.job.tools[0].material_profile
        ? join(output, "profile-export")
        : originalExport,
      "program.gcode",
    ),
  ),
  await readFile(join(recoveredExport, "program.gcode")),
);
assert.equal(
  JSON.parse(await readFile(join(recoveredExport, "verification.json"), "utf8")).verified,
  true,
);
const verification = JSON.parse(await readFile(join(recoveredExport, "verification.json"), "utf8"));
if (job.timing_model) {
  const time = verification.job_time;
  assert.ok(time, "fresh replay must consume the captured timing model");
  assert.deepEqual(time.model, job.timing_model);
  assert.equal(time.model_moves, verification.deposition_segments);
  assert.equal(time.auxiliary_moves, verification.routing.tools.reduce((sum, t) => sum + t.waste_deposition_segments, 0));
  assert.equal(time.travel_moves, verification.travel_segments + verification.lifecycle_motion_count);
  assert.equal(time.feeder_moves, 2 * verification.retraction_count - 2);
  assert.deepEqual(time.selections_per_tool, [verification.routing.selection_count / 2, verification.routing.selection_count / 2]);
  for (const key of ["model_deposition_seconds", "auxiliary_deposition_seconds", "travel_seconds", "feeder_seconds", "activation_seconds", "command_overhead_seconds"])
    assert.ok(Number.isFinite(time[key]) && time[key] > 0, `complete modeled work term ${key}`);
  evidence.job_time = time;
}
const invalid = structuredClone(intent);
invalid.printer.workflow.job.assignments.at(-1).tool_id = null;
const invalidFile = join(output, "ambiguous.intent.json");
await writeFile(invalidFile, JSON.stringify(invalid));
const error = await invoke("ambiguous-binding", ["edit", project, "--intent", invalidFile], true);
assert.match(error, /explicit tool binding/);
const after = await invoke("after-refusal", ["inspect", project]);
assert.deepEqual(after.manufacturing.intent, returned.manufacturing.intent);
evidence.model_events_per_tool = counts;
evidence.coarse_model_events = coarseEvents;
await writeFile(join(output, "acceptance.json"), JSON.stringify(evidence, null, 2) + "\n");
console.log(
  JSON.stringify({
    evidence: output,
    same_material_tools: true,
    source_independent_replay: true,
    model_events_per_tool: counts,
  }),
);
