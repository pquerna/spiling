/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Separate actual CLI/engine processes; immutable project data outlives personal stock.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFile, writeFile, mkdir, mkdtemp, unlink, rename } from "node:fs/promises";
import { resolve, join } from "node:path";
import { cubeSource, intentFor, cases, sha256 } from "./gcode-parity/corpus.mjs";
const root = resolve(import.meta.dirname, "..");
const target = resolve(root, process.env.CARGO_TARGET_DIR ?? "target");
const suffix = process.platform === "win32" ? ".exe" : "";
const cli = join(target, "debug", `spiling-cli${suffix}`),
  engine = join(target, "debug", `spiling-engine${suffix}`);
await mkdir(join(root, ".cache"), { recursive: true });
const output = await mkdtemp(join(root, ".cache", "filament-library-"));
console.error(`Evidence: ${output}`);
const library = join(output, "My library.spil"),
  project = join(output, "Job.spil");
const manifest = { commands: [], files: {}, softwareOnly: true };
async function save(name, data) {
  const text = typeof data === "string" ? data : JSON.stringify(data, null, 2) + "\n";
  await writeFile(join(output, name), text, { flag: "wx" });
  manifest.files[name] = sha256(text);
}
async function run(name, args) {
  const command = [...args, "--engine", engine];
  manifest.commands.push({ name, args: command });
  let stdout = "",
    stderr = "",
    bytes = 0;
  await new Promise((done, fail) => {
    const child = spawn(cli, command, {
      cwd: root,
      env: { ...process.env, RAYON_NUM_THREADS: "1" },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let aborted;
    const timer = setTimeout(() => {
      aborted = new Error("library smoke exceeded five minutes");
      child.kill("SIGKILL");
    }, 300_000);
    for (const [stream, key] of [
      [child.stdout, "stdout"],
      [child.stderr, "stderr"],
    ])
      stream.setEncoding("utf8").on("data", (chunk) => {
        bytes += Buffer.byteLength(chunk);
        if (bytes > 16 * 1024 * 1024) {
          aborted = new Error("log budget");
          child.kill("SIGKILL");
        } else if (key === "stdout") stdout += chunk;
        else stderr += chunk;
      });
    child.once("error", (error) => {
      clearTimeout(timer);
      fail(error);
    });
    child.once("close", (code) => {
      clearTimeout(timer);
      if (aborted) fail(aborted);
      else if (code !== 0) fail(new Error(`${name}: ${code}: ${stderr.slice(-3000)}`));
      else done();
    });
  }).finally(async () => {
    await save(`${name}.stdout.json`, stdout);
    await save(`${name}.stderr.txt`, stderr);
  });
  return JSON.parse(stdout);
}
try {
  const selection = JSON.parse(
    await readFile(join(root, "fixtures/manufacturing/material-profile-a.json"), "utf8"),
  );
  await save("profile.json", selection);
  const missing = await run("missing", ["library", "status", "--library", library]);
  assert.equal(missing.result.identity, null);
  const added = await run("add-filament", [
    "library",
    "add-filament",
    "My reviewed PLA",
    "--profile",
    join(output, "profile.json"),
    "--hex",
    "CF524E",
    "--library",
    library,
  ]);
  assert.equal(added.result.receipt.publication, "confirmed");
  const listed = await run("filaments", ["library", "filaments", "--library", library]);
  const id = listed.result[0].entry.current.id;
  assert.equal(listed.result.length, 1);
  for (let i = 0; i < 2; i++)
    await run(`spool-${i}`, [
      "library",
      "add-spool",
      id,
      "--library",
      library,
      ...(i === 0 ? [] : ["--grams", "530"]),
    ]);
  const stock = await run("stock", ["library", "spools", "--library", library]);
  assert.equal(stock.result.length, 2);
  assert.equal(stock.result.filter((s) => s.quantity === null).length, 1);
  const base = intentFor(
    JSON.parse(
      await readFile(join(root, "fixtures/manufacturing/complete-sparse.intent.json"), "utf8"),
    ),
    cases.find((c) => c.walls === 1 && c.infillFraction === 0.25),
  );
  await save("intent.json", base);
  await save(
    "cube.step",
    cubeSource(await readFile(join(root, "fixtures/geometry/box-mm.step"), "utf8")),
  );
  const created = await run("create", [
    "project",
    "create",
    project,
    "--import",
    join(output, "cube.step"),
    "--intent",
    join(output, "intent.json"),
    "--library",
    library,
    "--library-use",
    id,
    "--project-material-profile",
    id,
    selection.profile.id,
    selection.profile.revision,
    JSON.stringify(selection.setup),
    "--compile",
    "--verify",
    "--manufacturing-out",
    join(output, "first-export"),
    "--save",
  ]);
  assert.equal(created.project.materials.filament_count, 1);
  const verify = JSON.parse(
    await readFile(join(output, "first-export", "verification.json"), "utf8"),
  );
  assert.equal(verify.verified, true);
  const provenance = JSON.parse(
    await readFile(join(output, "first-export", "provenance.json"), "utf8"),
  );
  assert.deepEqual(provenance.intent.material_profile.selection, selection);
  for (const name of ["cube.step", "profile.json", "intent.json"]) await unlink(join(output, name));
  await rename(library, join(output, "Library unavailable.spil"));
  const reopened = await run("reopen", [
    "project",
    "inspect",
    project,
    "--library",
    library,
    "--project-materials",
    "--verify",
    "--manufacturing-out",
    join(output, "second-export"),
  ]);
  assert.equal(reopened.project.materials.filament_count, 1);
  for (const name of ["plan.json", "program.gcode", "verification.json", "provenance.json"]) {
    const a = await readFile(join(output, "first-export", name)),
      b = await readFile(join(output, "second-export", name));
    assert.deepEqual(a, b, name);
    manifest.files[`first-export/${name}`] = sha256(a);
  }
  const again = JSON.parse(
    await readFile(join(output, "second-export", "provenance.json"), "utf8"),
  );
  assert.deepEqual(again.intent, provenance.intent);
  await save("manifest.json", manifest);
  console.log(JSON.stringify({ output, verified: true, standaloneProject: true }, null, 2));
} catch (error) {
  await save("failure.json", { message: error.message, stack: error.stack, manifest });
  throw error;
}
