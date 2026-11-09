/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Actual CLI/engine saved return and independent decoded-road observation.
// Prebuilt coherent binaries and an external whole-process bound are required.
import assert from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, readFile, unlink, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { projectSmoke, cgroupPeakBytes } from "./project-smoke.mjs";
import { decodeReplayDisplay } from "../packages/protocol/src/replay.ts";

const root = resolve(import.meta.dirname, "..");
const target = resolve(root, process.env.CARGO_TARGET_DIR ?? "target");
await mkdir(join(root, ".cache"), { recursive: true });
const output = await mkdtemp(join(root, ".cache", "web-roof-"));
console.error(`Evidence: ${output}`);
const evidence = { software_only: true, strength_qualified: false, commands: [] };
const invoke = projectSmoke({ root, target, output, evidence });
const source = join(output, "owned-box.step");
const intentFile = join(output, "owned-intent.json");
const project = join(output, "Web and roof.spil");
await copyFile(join(root, "fixtures/geometry/box-mm.step"), source);
await copyFile(join(root, "fixtures/manufacturing/web-roof.intent.json"), intentFile);
const original = join(output, "original-export");
await invoke("create-compile", [
  "create",
  project,
  "--import",
  source,
  "--pose",
  "1",
  JSON.stringify({ translation_mm: [20, 20, 0], rotation_xyzw: [0, 0, 0, 1] }),
  "--intent",
  intentFile,
  "--compile",
  "--verify",
  "--save",
  "--manufacturing-out",
  original,
]);
await unlink(source);
await unlink(intentFile);
const returnedExport = join(output, "returned-export");
const replayFile = join(output, "actual.splr");
const returned = await invoke("saved-return", [
  "inspect",
  project,
  "--verify",
  "--manufacturing-out",
  returnedExport,
  "--replay-out",
  replayFile,
]);
assert.equal(returned.manufacturing.intent.recipe.sparse_infill.kind, "web_roof");
for (const name of ["program.gcode", "plan.json", "provenance.json", "verification.json"]) {
  assert.deepEqual(
    await readFile(join(original, name)),
    await readFile(join(returnedExport, name)),
    name,
  );
}
const decoded = await decodeReplayDisplay(
  Uint8Array.from(await readFile(replayFile)).buffer,
  returned.manufacturing.replay_display,
);
const roads = [];
const directions = new Set();
const layers = new Map();
for (let event = 1; event < decoded.eventCount; event++) {
  if (decoded.kind(event) !== 5) continue;
  const start = decoded.position(event - 1),
    end = decoded.position(event);
  const association = decoded.association(event);
  assert.ok(association);
  assert.equal(association.beadWidthMm, 0.45);
  assert.ok(start[0] === end[0] || start[1] === end[1]);
  directions.add(start[0] === end[0] ? "Y" : "X");
  for (const point of [start, end])
    for (const [axis, v] of point.slice(0, 2).entries())
      assert.ok(
        v - 0.225 >= 20 - 0.005001 && v + 0.225 <= [40, 30][axis] + 0.005001,
        "actual full nominal road exceeds the source rectangle",
      );
  const z = end[2];
  const layer = layers.get(z) ?? { count: 0, length_mm: 0 };
  layer.count++;
  layer.length_mm += Math.hypot(end[0] - start[0], end[1] - start[1]);
  layers.set(z, layer);
  roads.push({
    start,
    end,
    width: association.beadWidthMm,
    lower: association.physicalZMm[0],
    upper: association.physicalZMm[1],
  });
}
assert.deepEqual([...directions].sort(), ["X", "Y"]);
assert.ok(layers.size > 5);
const ordered = [...layers].sort((a, b) => a[0] - b[0]);
assert.ok(
  ordered.some(([, layer]) => layer.length_mm > ordered[0][1].length_mm + 0.01),
  "roof branches must grow in the actual emitted roads",
);
// A separate project keeps the control's durable transactions away from the
// captured pattern project; omitting Save would not isolate project edits.
const solidIntent = JSON.parse(
  await readFile(join(root, "fixtures/manufacturing/web-roof.intent.json"), "utf8"),
);
solidIntent.recipe.sparse_infill = { kind: "fine" };
solidIntent.recipe.infill_fraction = 1;
const solidFile = join(output, "solid.intent.json");
const solidExport = join(output, "solid-export");
await writeFile(solidFile, JSON.stringify(solidIntent));
await invoke("solid-control", [
  "create",
  join(output, "Solid control.spil"),
  "--import",
  join(root, "fixtures/geometry/box-mm.step"),
  "--pose",
  "1",
  JSON.stringify({ translation_mm: [20, 20, 0], rotation_xyzw: [0, 0, 0, 1] }),
  "--intent",
  solidFile,
  "--compile",
  "--verify",
  "--manufacturing-out",
  solidExport,
]);
const retained = await invoke("pattern-retained", ["inspect", project]);
assert.equal(retained.manufacturing.intent.recipe.sparse_infill.kind, "web_roof");
const actual = JSON.parse(await readFile(join(returnedExport, "verification.json"), "utf8"));
const solid = JSON.parse(await readFile(join(solidExport, "verification.json"), "utf8"));
assert.ok(actual.verified && solid.verified);
assert.ok(actual.deposited_volume_mm3 < solid.deposited_volume_mm3);
evidence.deposited_volume_mm3 = actual.deposited_volume_mm3;
evidence.solid_deposited_volume_mm3 = solid.deposited_volume_mm3;
evidence.material_saved_fraction = 1 - actual.deposited_volume_mm3 / solid.deposited_volume_mm3;
evidence.sparse_roads = roads.length;
evidence.sparse_layers = ordered;
evidence.peak_bytes = await cgroupPeakBytes();
await writeFile(join(output, "roads.json"), JSON.stringify({ roads, software_only: true }));
await writeFile(join(output, "evidence.json"), JSON.stringify(evidence, null, 2) + "\n");
console.log(JSON.stringify({ output, ...evidence }, null, 2));
