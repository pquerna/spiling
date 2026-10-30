/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Actual engine/CLI lifecycle smoke. Uses the independent parity byte decoder;
// this is not another planner, reference comparator or material calibration.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFile, writeFile, mkdir, mkdtemp, unlink, stat } from "node:fs/promises";
import { resolve, join } from "node:path";
import { cubeSource, intentFor, cases, sha256 } from "./gcode-parity/corpus.mjs";
import { decodeGcode } from "./gcode-parity/decode.mjs";
const root = resolve(import.meta.dirname, "..");
const target = resolve(root, process.env.CARGO_TARGET_DIR ?? "target");
const suffix = process.platform === "win32" ? ".exe" : "";
const cli = join(target, "debug", `spiling-cli${suffix}`);
const engine = join(target, "debug", `spiling-engine${suffix}`);
await mkdir(join(root, ".cache"), { recursive: true });
const output = await mkdtemp(join(root, ".cache", "material-profiles-"));
console.error(`Evidence: ${output}`);
const manifest = { commands: [], files: {}, softwareOnly: true };
async function save(name, data) {
  const text = typeof data === "string" ? data : JSON.stringify(data, null, 2) + "\n";
  await writeFile(join(output, name), text, { flag: "wx" });
  manifest.files[name] = sha256(text);
}
async function run(name, args, expectedError = null) {
  const command = ["project", ...args, "--engine", engine];
  const entry = { name, args: command };
  manifest.commands.push(entry);
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
    const abort = (message) => {
      aborted = new Error(message);
      child.kill("SIGKILL");
    };
    const timer = setTimeout(() => abort("material smoke child exceeded five minutes"), 300_000);
    for (const [stream, key] of [
      [child.stdout, "stdout"],
      [child.stderr, "stderr"],
    ])
      stream.setEncoding("utf8").on("data", (chunk) => {
        bytes += Buffer.byteLength(chunk);
        if (bytes > 16 * 1024 * 1024) abort("material smoke log budget exceeded");
        else if (key === "stdout") stdout += chunk;
        else stderr += chunk;
      });
    child.once("error", (error) => {
      clearTimeout(timer);
      fail(error);
    });
    child.once("close", (code) => {
      clearTimeout(timer);
      entry.exitCode = code;
      if (aborted) fail(aborted);
      else if (expectedError ? code === 0 || !stderr.includes(expectedError) : code !== 0)
        fail(new Error(`${name}: unexpected exit ${code}: ${stderr.slice(-3000)}`));
      else done();
    });
  }).finally(async () => {
    await save(`${name}.stdout.json`, stdout);
    await save(`${name}.stderr.txt`, stderr);
  });
  if (expectedError) return JSON.parse(stderr.trim().split("\n").at(-1));
  const result = JSON.parse(stdout);
  assert.match(result.hello.engine_build, /;verifier17;/);
  manifest.engineBuild ??= result.hello.engine_build;
  assert.equal(result.hello.engine_build, manifest.engineBuild);
  return result;
}
async function program(directory, selection) {
  const text = await readFile(join(output, directory, "program.gcode"), "utf8");
  const verification = JSON.parse(
    await readFile(join(output, directory, "verification.json"), "utf8"),
  );
  const provenance = JSON.parse(await readFile(join(output, directory, "provenance.json"), "utf8"));
  assert.equal(verification.verified, true);
  assert.deepEqual(provenance.intent.material_profile.selection, selection);
  const decoded = decodeGcode(text, {
    filamentDiameterMm: selection.profile.material.filament_diameter_mm,
  });
  const p = selection.profile,
    controls = decoded.controls;
  assert(
    controls.some(
      (c) =>
        c.op === "M109" &&
        c.fields.R === (selection.overrides.nozzle_temperature_c ?? p.nozzle_temperature_c),
    ),
  );
  assert(
    controls.some(
      (c) =>
        c.op === "M190" &&
        c.fields.R === (selection.overrides.bed_temperature_c ?? p.bed_temperature_c),
    ),
  );
  const model = decoded.strokes.filter((s) =>
    [s.from, s.to].every((v) => v.every((x) => x >= 0 && x <= 10)),
  );
  const speed = selection.overrides.print_speed_mm_s ?? p.print_speed_mm_s;
  const process = selection.overrides.process ?? p.process;
  assert(
    model.some((s) => s.feedMmMin === speed * 60 && s.fanPwm === process.fan_after_first_layers),
  );
  assert(
    model.some(
      (s) =>
        s.feedMmMin === speed * process.bridge_speed_factor * 60 &&
        s.fanPwm === process.bridge_fan_pwm,
    ),
  );
  for (const s of decoded.strokes)
    assert(((s.volumeMm3 / s.lengthMm) * s.feedMmMin) / 60 <= p.max_volumetric_flow_mm3_s + 0.0001);
  manifest.files[`${directory}/program.gcode`] = sha256(text);
  return { text, volume: model.reduce((sum, s) => sum + s.volumeMm3, 0), provenance, decoded };
}
try {
  for (const name of [
    "tools/smoke-material-profiles.mjs",
    "tools/gcode-parity/decode.mjs",
    "tools/gcode-parity/corpus.mjs",
  ])
    manifest.files[name] = sha256(await readFile(join(root, name)));
  manifest.cliHash = sha256(await readFile(cli));
  manifest.engineHash = sha256(await readFile(engine));
  const source = await readFile(join(root, "fixtures/geometry/box-mm.step"), "utf8");
  manifest.originalSourceHash = sha256(source);
  await save("cube.step", cubeSource(source));
  const base = intentFor(
    JSON.parse(
      await readFile(join(root, "fixtures/manufacturing/complete-sparse.intent.json"), "utf8"),
    ),
    cases.find((c) => c.walls === 1 && c.infillFraction === 0.25),
  );
  await save("base.intent.json", base);
  const a = JSON.parse(
    await readFile(join(root, "fixtures/manufacturing/material-profile-a.json"), "utf8"),
  );
  const b = JSON.parse(
    await readFile(join(root, "fixtures/manufacturing/material-profile-b.json"), "utf8"),
  );
  await save("a.json", a);
  await save("b.json", b);
  const commandSemantics = (decoded) => ({
    strokes: decoded.strokes.map(({ line, roleHint, ...stroke }) => stroke),
    travels: decoded.travels.map(({ line, roleHint, ...travel }) => travel),
    controls: decoded.controls.map(({ line, ...control }) => control),
    stationaryFilamentMm: decoded.stationaryFilamentMm,
    retractedFilamentMm: decoded.retractedFilamentMm,
    recoveredFilamentMm: decoded.recoveredFilamentMm,
  });
  const project = join(output, "project.spiling");
  const first = await run("create-a", [
    "create",
    project,
    "--import",
    join(output, "cube.step"),
    "--intent",
    join(output, "base.intent.json"),
    "--material-profile",
    join(output, "a.json"),
    "--compile",
    "--verify",
    "--manufacturing-out",
    join(output, "a-export"),
    "--save",
  ]);
  const pa = await program("a-export", a);
  assert.equal(
    pa.provenance.intent.printer.max_volumetric_flow_mm3_s,
    base.printer.max_volumetric_flow_mm3_s,
  );
  // Hold geometry and explicit process values fixed while changing metadata.
  const labelOnly = structuredClone(a);
  labelOnly.profile.revision = "label-control";
  labelOnly.profile.material.polymer_family = "SyntheticPCTGLabelOnly";
  await save("label-only.json", labelOnly);
  await run("label-only", [
    "create",
    join(output, "label-only.spiling"),
    "--import",
    join(output, "cube.step"),
    "--intent",
    join(output, "base.intent.json"),
    "--material-profile",
    join(output, "label-only.json"),
    "--compile",
    "--verify",
    "--manufacturing-out",
    join(output, "label-export"),
  ]);
  assert.deepEqual(
    commandSemantics((await program("label-export", labelOnly)).decoded),
    commandSemantics(pa.decoded),
  );
  const bridgeAccepted = structuredClone(a);
  bridgeAccepted.profile.process.max_bridge_span_mm = 1.19;
  await save("bridge-accepted.json", bridgeAccepted);
  await run("bridge-accepted", [
    "create",
    join(output, "bridge-accepted.spiling"),
    "--import",
    join(output, "cube.step"),
    "--intent",
    join(output, "base.intent.json"),
    "--material-profile",
    join(output, "bridge-accepted.json"),
    "--compile",
    "--verify",
    "--manufacturing-out",
    join(output, "bridge-accepted-export"),
  ]);
  assert.deepEqual(
    commandSemantics((await program("bridge-accepted-export", bridgeAccepted)).decoded),
    commandSemantics(pa.decoded),
  );
  const bridgeRejected = structuredClone(bridgeAccepted);
  bridgeRejected.profile.process.max_bridge_span_mm = 1.17;
  await save("bridge-rejected.json", bridgeRejected);
  const failure = await run(
    "bridge-rejected",
    [
      "create",
      join(output, "bridge-rejected.spiling"),
      "--import",
      join(output, "cube.step"),
      "--intent",
      join(output, "base.intent.json"),
      "--material-profile",
      join(output, "bridge-rejected.json"),
      "--compile",
      "--verify",
      "--manufacturing-out",
      join(output, "bridge-rejected-export"),
    ],
    "neither orthogonal native raster has bounded deposited anchors",
  );
  assert.equal(failure.error.code, "unsupported_geometry");
  assert.equal(failure.manufacturing.artifact, null);
  assert.equal(failure.manufacturing.intent.recipe.process.max_bridge_span_mm, 1.17);
  await assert.rejects(stat(join(output, "bridge-rejected-export")), { code: "ENOENT" });
  const undo = await run("undo-to-a", [
    "edit",
    project,
    "--material-profile",
    join(output, "b.json"),
    "--undo",
    "--compile",
    "--verify",
    "--manufacturing-out",
    join(output, "undo-export"),
    "--save",
  ]);
  assert.equal((await program("undo-export", a)).text, pa.text);
  const second = await run("redo-to-b", [
    "edit",
    project,
    "--material-profile",
    join(output, "b.json"),
    "--undo",
    "--redo",
    "--compile",
    "--verify",
    "--manufacturing-out",
    join(output, "b-export"),
    "--save",
  ]);
  const pb = await program("b-export", b);
  assert.notEqual(pa.text, pb.text);
  assert(Math.abs(pb.volume - pa.volume * 0.95) < 0.0001);
  assert.notEqual(pa.provenance.input_hash, pb.provenance.input_hash);
  const bad = structuredClone(b);
  bad.profile.printer_revision = "incompatible";
  await save("bad.json", bad);
  await run(
    "rejected-context",
    ["edit", project, "--material-profile", join(output, "bad.json")],
    "printer revision/nozzle",
  );
  const low = structuredClone(b);
  low.profile.max_volumetric_flow_mm3_s = 0.01;
  await save("low-flow.json", low);
  await run(
    "rejected-flow",
    ["edit", project, "--material-profile", join(output, "low-flow.json")],
    "flow limits",
  );
  // All external authoring files used by this project disappear before reopen.
  for (const file of [
    "cube.step",
    "base.intent.json",
    "a.json",
    "b.json",
    "bad.json",
    "low-flow.json",
    "label-only.json",
    "bridge-accepted.json",
    "bridge-rejected.json",
  ])
    await unlink(join(output, file));
  const reopened = await run("fresh-reopen", [
    "inspect",
    project,
    "--verify",
    "--manufacturing-out",
    join(output, "reopened-export"),
  ]);
  assert.equal(reopened.project.revision, second.project.revision);
  assert.equal((await program("reopened-export", b)).text, pb.text);
  manifest.outcome = {
    capturedProfiles: 2,
    labelDoesNotSelectBehavior: true,
    declaredBridgeLimitEnforced: true,
    unsupportedBridgePublishesNothing: true,
    undoRestoredProgram: true,
    redoAppliedProgram: true,
    rejectedEditsAtomic: true,
    externalInputsAbsent: true,
    freshProcessVerified: true,
    reopenedProgramIdentical: true,
    materialAVolumeMm3: pa.volume,
    materialBVolumeMm3: pb.volume,
    firstRevision: first.project.revision,
    undoRevision: undo.project.revision,
    finalRevision: second.project.revision,
  };
  console.error(JSON.stringify(manifest.outcome));
} catch (error) {
  manifest.error = error.stack;
  process.exitCode = 1;
  console.error(error.stack);
}
await save("manifest.json", manifest);
