/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
// Capture reusable native projects through the real CLI/engine, outside measured opening.
import assert from "node:assert/strict";
import { mkdir, copyFile, unlink, writeFile, readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { resolve, join } from "node:path";
import { projectSmoke } from "./project-smoke.mjs";
const root = resolve(import.meta.dirname, "..");
const [directory, profile = "profiling", ...extra] = process.argv.slice(2);
assert.ok(
  directory && !extra.length && ["debug", "release", "profiling"].includes(profile),
  "usage: node tools/prepare-open-bench.mjs NEW_DIRECTORY [profiling|release|debug]",
);
const output = resolve(root, directory);
await mkdir(output);
const evidence = { software_only: true, commands: [], cohorts: [], sources: [] };
const invoke = projectSmoke({
  root,
  target: resolve(root, process.env.CARGO_TARGET_DIR ?? "target"),
  profile,
  output,
  evidence,
  timeoutMs: 600_000,
});
const sources = [
  "box-mm.step",
  "through-hole.step",
  "cylinder.step",
  "box-inch.step",
  "independent-exporters/occt-box-mm.step",
  "independent-exporters/occt-through-hole-mm.step",
  "independent-exporters/occt-cylinder-mm.step",
  "independent-exporters/occt-tapered-fitting-mm.step",
  "independent-exporters/occt-rounded-housing-mm.step",
  "independent-exporters/occt-spline-housing-mm.step",
];
for (const source of sources) {
  const original = join(root, "fixtures/geometry", source),
    name = source.replaceAll("/", "-");
  await copyFile(original, join(output, name));
  evidence.sources.push({
    source,
    name,
    sha256: createHash("sha256")
      .update(await readFile(original))
      .digest("hex"),
  });
}
const source = (index) => join(output, evidence.sources[index].name);
const pose = (i) =>
  JSON.stringify({
    translation_mm: [20 + (i % 8) * 30, 20 + Math.floor(i / 8) * 30, 0],
    rotation_xyzw: [0, 0, 0, 1],
  });
const project = (name) => join(output, `${name}.spil`);
const captured = (name, report) => {
  assert.ok(
    report.definitions.some((d) => d.record.display.kind === "mesh"),
    `${name}: benchmark requires actual source display`,
  );
  const display_refusals = report.definitions
    .filter((d) => d.record.display.kind !== "mesh")
    .map((d) => ({ source: d.record.provenance.source_name, display: d.record.display }));
  evidence.cohorts.push({
    name,
    project: `${name}.spil`,
    display_refusals,
    project_id: report.project.project_id,
    definitions: report.definitions.length,
    occurrences: report.occurrences.length,
    mesh_bytes: report.unique_mesh_bytes,
    layouts: report.project.layouts,
  });
};
const shared = await invoke("shared-create", ["create", project("shared"), "--import", source(0)]);
const sharedId = shared.definitions[0].record.definition_id;
const additions = [];
for (let i = 1; i < 128; i++) additions.push("--add", sharedId, pose(i));
captured(
  "shared",
  await invoke("shared-capture", ["edit", project("shared"), ...additions, "--save"]),
);
const imports = (indices) =>
  indices.flatMap((index, i) => ["--import", source(index), "--pose", String(i + 1), pose(i)]);
captured(
  "distinct",
  await invoke("distinct-capture", [
    "create",
    project("distinct"),
    ...imports(sources.map((_, i) => i)),
  ]),
);
captured(
  "curved",
  await invoke("curved-capture", ["create", project("curved"), ...imports([7, 8, 9])]),
);
const programArgs = [
  "create",
  project("programs"),
  "--import",
  source(1),
  "--pose",
  "1",
  pose(0),
  "--intent",
  join(root, "fixtures/manufacturing/complete-sparse.intent.json"),
  "--compile",
  "--verify",
];
for (let i = 2; i <= 8; i++)
  programArgs.push("--plate-duplicate", `Plate ${i}`, "--compile", "--verify");
captured("programs", await invoke("programs-capture", programArgs));
// Manufacturing-heavy return: one protected bore plus 31 shared box occurrences.
const mixedPose = (i) => ({
  translation_mm: [20 + (i % 4) * 50, 20 + Math.floor(i / 4) * 25, 0],
  rotation_xyzw: [0, 0, 0, 1],
});
await invoke("mixed32-create", [
  "create",
  project("mixed32"),
  "--import",
  source(1),
  "--pose",
  "1",
  JSON.stringify(mixedPose(0)),
]);
const mixedBox = await invoke("mixed32-box", [
  "edit",
  project("mixed32"),
  "--import",
  source(0),
  "--pose",
  "2",
  JSON.stringify(mixedPose(1)),
  "--save",
]);
const boxId = mixedBox.occurrences.find((o) => o.occurrence_id === 2).definition_id;
const mixedAdd = [];
for (let i = 2; i < 32; i++) mixedAdd.push("--add", boxId, JSON.stringify(mixedPose(i)));
const intent = JSON.parse(
  await readFile(join(root, "fixtures/manufacturing/mixed-width-combined.intent.json"), "utf8"),
);
const job = intent.printer.workflow.job,
  template = structuredClone(job.assignments[0]);
for (const tool of job.tools) for (const point of tool.waste_line_xy_mm) point[1] = 215;
job.assignments = Array.from({ length: 32 }, (_, i) => {
  const [x, y] = mixedPose(i).translation_mm;
  return {
    ...structuredClone(template),
    occurrence_id: i + 1,
    work_envelope_mm: { min: [x - 2, y - 2, 0], max: [x + 22, y + (i === 0 ? 22 : 12), 8.01] },
  };
});
const intentPath = join(output, "mixed32.intent.json");
await writeFile(intentPath, JSON.stringify(intent), { flag: "wx" });
captured(
  "mixed32",
  await invoke("mixed32-capture", [
    "edit",
    project("mixed32"),
    ...mixedAdd,
    "--intent",
    intentPath,
    "--preflight",
    "--compile",
    "--verify",
    "--save",
  ]),
);
await unlink(intentPath);
for (const row of evidence.sources) await unlink(join(output, row.name));
for (const row of evidence.cohorts) {
  const returned = await invoke(`${row.name}-return`, ["inspect", project(row.name)]);
  assert.equal(returned.project.project_id, row.project_id);
  assert.equal(returned.definitions.length, row.definitions);
  row.project_sha256 = createHash("sha256")
    .update(await readFile(project(row.name)))
    .digest("hex");
}
await writeFile(join(output, "cohort.json"), JSON.stringify(evidence, null, 2), { flag: "wx" });
console.error(`Prepared source-independent native opening cohort: ${output}`);
