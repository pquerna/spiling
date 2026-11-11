/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Prebuilt native CLI/engine acceptance; run inside an external resource scope.
// Optional argument: a redistributable/local STEP part, kept outside the repo.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { copyFile, mkdir, mkdtemp, readFile, unlink, writeFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";
import { projectSmoke } from "./project-smoke.mjs";

const root = resolve(import.meta.dirname, "..");
const target = resolve(root, process.env.CARGO_TARGET_DIR ?? "target");
const sourceInput = resolve(root, process.argv[2] ?? "fixtures/geometry/through-hole.step");
const lift = Number(process.env.SPILING_SUPPORTS_LIFT_MM ?? (process.argv[2] ? 0 : 2));
assert.ok(Number.isFinite(lift) && lift >= 0);
await mkdir(join(root, ".cache"), { recursive: true });
const output = await mkdtemp(join(root, ".cache", "automatic-supports-journey-"));
console.error(`Evidence: ${output}`);
const source = join(output, "owned-source.step"),
  project = join(output, "Automatic supports.spil");
const evidence = {
  software_only: true,
  source: basename(sourceInput),
  sha256: createHash("sha256")
    .update(await readFile(sourceInput))
    .digest("hex"),
  commands: [],
};
const invoke = projectSmoke({ root, target, output, evidence });
try {
  await copyFile(sourceInput, source);
  const catalog = JSON.parse(
    await readFile(join(root, "crates/contracts/data/hardware-catalog.json"), "utf8"),
  );
  const setup = catalog.setups.find((s) => s.id === "spiling.reference.marlin.04");
  assert.ok(setup);
  const hardware = join(output, "hardware.json"),
    profile = join(output, "material.json");
  await writeFile(
    hardware,
    JSON.stringify({ setup, plate: catalog.plates[0], face: "textured", accessories: [] }),
  );
  await copyFile(join(root, "fixtures/manufacturing/material-profile-a.json"), profile);
  // No intent declaration: creation must run through the production factory.
  const created = await invoke("create", [
    "create",
    project,
    "--import",
    source,
    "--hardware",
    hardware,
    "--material-profile",
    profile,
    "--center-all-on-bed",
    "--save",
  ]);
  const captured = created.manufacturing.intent;
  assert.equal(captured.recipe.supports.kind, "breakaway");
  const placements = created.occurrences.map((o) => ({ ...o, pose: structuredClone(o.pose) }));
  const poseArgs = () =>
    placements.flatMap((o) => ["--pose", String(o.occurrence_id), JSON.stringify(o.pose)]);
  for (const o of placements) o.pose.translation_mm[2] += lift;
  const compiled = await invoke("compile", [
    "edit",
    project,
    ...poseArgs(),
    "--compile",
    "--verify",
    "--save",
  ]);
  assert.ok(compiled.manufacturing.artifact.summary.support_deposition_segments > 0);
  evidence.initial = compiled.manufacturing.artifact.summary;

  const off = structuredClone(captured);
  off.recipe.supports = { kind: "disabled" };
  const offFile = join(output, "off.json");
  await writeFile(offFile, JSON.stringify(off));
  const disabled = await invoke("explicit-off", [
    "edit",
    project,
    "--intent",
    offFile,
    "--material-profile",
    profile,
    "--save",
  ]);
  assert.equal(disabled.manufacturing.intent.recipe.supports.kind, "disabled");
  const onFile = join(output, "automatic.json");
  await writeFile(onFile, JSON.stringify(captured));

  // A real placement revision must invalidate and rebuild through the same path.
  for (const o of placements) o.pose.translation_mm[0] += 1;
  const revisedExport = join(output, "revised-export");
  const revised = await invoke("revise", [
    "edit",
    project,
    "--intent",
    onFile,
    "--undo",
    "--redo",
    ...poseArgs(),
    "--compile",
    "--verify",
    "--save",
    "--manufacturing-out",
    revisedExport,
  ]);
  assert.ok(revised.manufacturing.artifact.summary.support_deposition_segments > 0);
  evidence.revised = revised.manufacturing.artifact.summary;
  for (const file of [source, hardware, profile, offFile, onFile]) await unlink(file);
  const returnedExport = join(output, "returned-export");
  const returned = await invoke("saved-return", [
    "inspect",
    project,
    "--verify",
    "--manufacturing-out",
    returnedExport,
  ]);
  assert.deepEqual(returned.manufacturing.intent, captured);
  assert.deepEqual(returned.manufacturing.artifact, revised.manufacturing.artifact);
  // manifest.json is a per-invocation CLI receipt, including timings and jobs.
  // Compare the four captured manufacturing files without formatting huge diffs.
  for (const file of ["plan.json", "program.gcode", "provenance.json", "verification.json"]) {
    assert.ok(
      (await readFile(join(returnedExport, file))).equals(
        await readFile(join(revisedExport, file)),
      ),
      `${file} differs after saved return`,
    );
  }
  evidence.passed = true;
  console.log(
    `PASS: automatic creation, explicit Off, placement revision and source-absent fresh replay/export (${output})`,
  );
} finally {
  await writeFile(join(output, "evidence.json"), JSON.stringify(evidence, null, 2));
}
