/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Actual CLI/engine processes: private fleet -> portable capture -> standalone replay.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, writeFile, unlink, rename } from "node:fs/promises";
import { join, resolve } from "node:path";
import { cubeSource, sha256 } from "./gcode-parity/corpus.mjs";
const root = resolve(import.meta.dirname, "..");
const target = resolve(root, process.env.CARGO_TARGET_DIR ?? "target");
const suffix = process.platform === "win32" ? ".exe" : "";
const cli = join(target, "debug", `spiling-cli${suffix}`);
const engine = join(target, "debug", `spiling-engine${suffix}`);
await mkdir(join(root, ".cache"), { recursive: true });
const output = await mkdtemp(join(root, ".cache", "hardware-library-"));
const library = join(output, "My library.spil");
const project = join(output, "Hardware job.spil");
const evidence = { software_only: true, machine_ready: false, commands: [], files: {} };
console.error(`Evidence: ${output}`);
async function save(name, value) {
  const text = typeof value === "string" ? value : JSON.stringify(value, null, 2) + "\n";
  await writeFile(join(output, name), text, { flag: "wx" });
  evidence.files[name] = sha256(text);
}
async function run(name, args) {
  const command = [...args, "--engine", engine];
  evidence.commands.push({ name, args: command });
  let stdout = "",
    stderr = "",
    length = 0;
  await new Promise((accept, reject) => {
    const child = spawn(cli, command, {
      cwd: root,
      env: { ...process.env, RAYON_NUM_THREADS: "1" },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let failure;
    const timer = setTimeout(() => {
      failure = new Error("hardware smoke exceeded five minutes");
      child.kill("SIGKILL");
    }, 300000);
    for (const [stream, out] of [
      [child.stdout, true],
      [child.stderr, false],
    ]) {
      stream.setEncoding("utf8").on("data", (chunk) => {
        length += Buffer.byteLength(chunk);
        if (length > 16 * 1024 * 1024) {
          failure = new Error("hardware smoke log budget exceeded");
          child.kill("SIGKILL");
        } else if (out) stdout += chunk;
        else stderr += chunk;
      });
    }
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once("close", (code) => {
      clearTimeout(timer);
      if (failure) reject(failure);
      else if (code) reject(new Error(`${name}: ${code}: ${stderr.slice(-3000)}`));
      else accept();
    });
  }).finally(async () => {
    await save(`${name}.stdout.json`, stdout);
    await save(`${name}.stderr.txt`, stderr);
  });
  return JSON.parse(stdout);
}
try {
  const catalog = await run("catalog", ["library", "hardware-catalog", "--library", library]);
  assert(catalog.result.models.some((m) => m.id === "bambulab.h2c"));
  assert(!catalog.result.setups.some((s) => s.model.id === "bambulab.h2c"));
  for (let i = 0; i < 3; i++)
    await run(`machine-${i}`, [
      "library",
      "add-machine",
      "PRIVATE same-name workshop",
      "--library",
      library,
    ]);
  for (let i = 0; i < 2; i++)
    await run(`plate-${i}`, ["library", "add-plate", "PRIVATE sheet name", "--library", library]);
  const fleet = (await run("fleet", ["library", "hardware", "--library", library])).result.hardware;
  assert.equal(fleet.machines.length, 3);
  assert.equal(fleet.plates.length, 2);
  assert.notEqual(fleet.machines[0].id, fleet.machines[1].id);
  const machine = fleet.machines[0].id,
    plate = fleet.plates[0].id;
  await save(
    "cube.step",
    cubeSource(await readFile(join(root, "fixtures/geometry/box-mm.step"), "utf8")),
  );
  await save(
    "intent.json",
    await readFile(join(root, "fixtures/manufacturing/solid-fill.intent.json"), "utf8"),
  );
  const created = await run("create", [
    "project",
    "create",
    project,
    "--import",
    join(output, "cube.step"),
    "--library",
    library,
    "--hardware-local",
    machine,
    plate,
    "textured",
    "--intent",
    join(output, "intent.json"),
    "--compile",
    "--verify",
    "--manufacturing-out",
    join(output, "first-export"),
    "--save",
  ]);
  assert.equal(created.project.hardware.face, "textured");
  const remembered = (await run("remembered", ["library", "hardware", "--library", library])).result
    .hardware;
  assert.equal(remembered.bindings.length, 1);
  assert.equal(remembered.bindings[0].machine_id, machine);
  const sentinels = ["PRIVATE same-name workshop", "PRIVATE sheet name", machine, plate];
  for (const sentinel of sentinels)
    assert(
      !Buffer.from(await readFile(project)).includes(Buffer.from(sentinel)),
      `private project leak: ${sentinel}`,
    );
  for (const name of ["cube.step", "intent.json"]) await unlink(join(output, name));
  await rename(library, join(output, "Unavailable library.spil"));
  const reopened = await run("standalone", [
    "project",
    "inspect",
    project,
    "--library",
    library,
    "--verify",
    "--manufacturing-out",
    join(output, "second-export"),
  ]);
  assert.deepEqual(reopened.project.hardware, created.project.hardware);
  assert.equal(
    (await run("missing", ["library", "hardware", "--library", library])).result.identity,
    null,
  );
  for (const name of ["plan.json", "program.gcode", "verification.json", "provenance.json"]) {
    const first = await readFile(join(output, "first-export", name));
    const second = await readFile(join(output, "second-export", name));
    assert.deepEqual(first, second, name);
    for (const sentinel of sentinels)
      assert(!first.includes(Buffer.from(sentinel)), `private export leak: ${sentinel}`);
    evidence.files[`first-export/${name}`] = sha256(first);
  }
  assert.equal(
    JSON.parse(await readFile(join(output, "second-export", "verification.json"), "utf8")).verified,
    true,
  );
  await run("recompile", [
    "project",
    "open",
    project,
    "--library",
    library,
    "--compile",
    "--verify",
    "--manufacturing-out",
    join(output, "third-export"),
  ]);
  for (const name of ["program.gcode", "verification.json"]) {
    assert.deepEqual(
      await readFile(join(output, "first-export", name)),
      await readFile(join(output, "third-export", name)),
      `recompiled ${name}`,
    );
  }
  const firstPlan = JSON.parse(await readFile(join(output, "first-export", "plan.json"), "utf8"));
  const freshPlan = JSON.parse(await readFile(join(output, "third-export", "plan.json"), "utf8"));
  assert(
    firstPlan.native_geometry.geometry_work > 0 && freshPlan.native_geometry.geometry_work > 0,
  );
  evidence.recompile_metadata = {
    geometry_work: [
      firstPlan.native_geometry.geometry_work,
      freshPlan.native_geometry.geometry_work,
    ],
  };
  freshPlan.native_geometry.geometry_work = firstPlan.native_geometry.geometry_work;
  assert.deepEqual(freshPlan, firstPlan, "semantic normalized plan");
  const firstProvenance = JSON.parse(
    await readFile(join(output, "first-export", "provenance.json"), "utf8"),
  );
  const freshProvenance = JSON.parse(
    await readFile(join(output, "third-export", "provenance.json"), "utf8"),
  );
  assert.equal(freshProvenance.input_revision, created.project.revision);
  assert.equal(freshProvenance.inspection.input_revision, freshProvenance.input_revision);
  evidence.recompile_metadata.input_revision = [
    firstProvenance.input_revision,
    freshProvenance.input_revision,
  ];
  freshProvenance.input_revision = firstProvenance.input_revision;
  freshProvenance.inspection.input_revision = firstProvenance.inspection.input_revision;
  assert.deepEqual(
    freshProvenance,
    firstProvenance,
    "complete provenance except declared compilation revision",
  );
  await save("evidence.json", evidence);
  console.log(
    JSON.stringify(
      { output, verified: true, standalone_recompile: true, private_selection_remembered: true },
      null,
      2,
    ),
  );
} catch (error) {
  await save("failure.json", { message: error.message, stack: error.stack, evidence });
  throw error;
}
