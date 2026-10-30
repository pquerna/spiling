/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
import { spawn } from "node:child_process";
import { readFile, writeFile, mkdir, mkdtemp, stat, realpath } from "node:fs/promises";
import { resolve, dirname, join, delimiter } from "node:path";
import { fileURLToPath } from "node:url";
import { platform, release, arch } from "node:os";
import { decodeGcode } from "./gcode-parity/decode.mjs";
import { evaluateReferenceProbes } from "./gcode-parity/probes.mjs";
import { auditCube, compareAudits } from "./gcode-parity/analyze.mjs";
import {
  cases,
  cubeSource,
  intentFor,
  referenceProfile,
  referenceProbeSettings,
  sha256,
} from "./gcode-parity/corpus.mjs";
import { reportHtml, overlayBands } from "./gcode-parity/report.mjs";
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const options = {};
for (let i = 2; i < process.argv.length; i++) {
  const key = process.argv[i];
  if (key === "--observe") options.observe = true;
  else if (
    ["--cli", "--engine", "--reference", "--reference-sha256"].includes(key) &&
    process.argv[i + 1]
  )
    options[key.slice(2)] = process.argv[++i];
  else throw new Error(`unsupported argument ${key}`);
}
const target = resolve(root, process.env.CARGO_TARGET_DIR ?? "target");
const suffix = process.platform === "win32" ? ".exe" : "";
const cli = resolve(options.cli ?? join(target, "debug", `spiling-cli${suffix}`));
const engine = resolve(options.engine ?? join(target, "debug", `spiling-engine${suffix}`));
async function executable(name) {
  if (name.includes("/") || name.includes("\\")) return realpath(resolve(name));
  for (const directory of (process.env.PATH ?? "").split(delimiter)) {
    const candidate = join(directory, name + suffix);
    try {
      if ((await stat(candidate)).isFile()) return realpath(candidate);
    } catch {}
  }
  throw new Error(`reference executable ${name} is unavailable; install PrusaSlicer 2.9.4`);
}
const reference = await executable(options.reference ?? "prusa-slicer");
await mkdir(join(root, ".cache"), { recursive: true });
const output = await mkdtemp(join(root, ".cache", "slicer-parity-"));
console.error(`Evidence: ${output}`);
const manifest = {
  schema: 1,
  environment: { platform: platform(), release: release(), arch: arch(), node: process.version },
  commands: [],
  files: {},
};
const save = async (name, value) =>
  writeFile(
    join(output, name),
    typeof value === "string" ? value : JSON.stringify(value, null, 2) + "\n",
    { flag: "wx" },
  );
let active;
function stop(signal) {
  if (active) {
    try {
      process.platform === "win32" ? active.kill(signal) : process.kill(-active.pid, signal);
    } catch {}
  }
}
for (const signal of ["SIGINT", "SIGTERM"])
  process.once(signal, () => {
    stop("SIGKILL");
    process.exit(130);
  });
function failureDetail(stderr) {
  try {
    const diagnostic = JSON.parse(stderr.trim().split("\n").at(-1));
    return diagnostic.error?.message ?? diagnostic.message ?? stderr.slice(-1200);
  } catch {
    return stderr.slice(-1200);
  }
}
async function run(command, args, logName, { cwd = root } = {}) {
  const entry = { command, args, cwd };
  manifest.commands.push(entry);
  let stdout = "",
    stderr = "",
    bytes = 0;
  try {
    await new Promise((done, fail) => {
      const child = spawn(command, args, {
        cwd,
        env: { ...process.env, RAYON_NUM_THREADS: process.env.RAYON_NUM_THREADS ?? "1" },
        detached: process.platform !== "win32",
        stdio: ["ignore", "pipe", "pipe"],
      });
      active = child;
      let failure;
      const abort = (message) => {
        failure = new Error(message);
        stop("SIGKILL");
      };
      const timer = setTimeout(() => abort("child exceeded 10 minute limit"), 600_000);
      for (const [stream, key] of [
        [child.stdout, "stdout"],
        [child.stderr, "stderr"],
      ])
        stream.setEncoding("utf8").on("data", (chunk) => {
          bytes += Buffer.byteLength(chunk);
          if (bytes > 16 * 1024 * 1024) abort("child exceeded 16 MiB log limit");
          else if (key === "stdout") stdout += chunk;
          else stderr += chunk;
        });
      child.once("error", (error) => {
        clearTimeout(timer);
        active = null;
        fail(error);
      });
      child.once("close", (code, signal) => {
        clearTimeout(timer);
        active = null;
        entry.exitCode = code;
        entry.signal = signal;
        if (failure) fail(failure);
        else if (code !== 0)
          fail(new Error(`${command} exited ${code ?? signal}: ${failureDetail(stderr)}`));
        else done();
      });
    });
  } finally {
    await save(`${logName}.stdout.txt`, stdout);
    await save(`${logName}.stderr.txt`, stderr);
  }
  return stdout;
}
async function boundedText(path) {
  if ((await stat(path)).size > 16 * 1024 * 1024)
    throw new Error(`oversize evidence input ${path}`);
  return readFile(path, "utf8");
}
const report = {
  schema: 1,
  scope:
    "Analytic linear planar one-tool 10 mm cubes; software commands only; no physical/firmware/collision qualification.",
  cases: [],
};
const overlays = {};
try {
  manifest.commit = (await run("git", ["rev-parse", "HEAD"], "git-commit")).trim();
  manifest.workingChanges = (await run("git", ["status", "--short"], "git-status")).trim();
  for (const [name, path] of Object.entries({ cli, engine, reference }))
    manifest[name] = { path, sha256: sha256(await readFile(path)) };
  if (options["reference-sha256"] && manifest.reference.sha256 !== options["reference-sha256"])
    throw new Error("reference executable SHA-256 mismatch");
  const help = await run(reference, ["--help"], "reference-help");
  if (!/^PrusaSlicer-2\.9\.4(?:\+[^\s]+)?\s/m.test(help))
    throw new Error(
      "comparison contract requires PrusaSlicer 2.9.4; retain and review a separate contract for another version",
    );
  manifest.reference.version = help.split("\n").find((l) => l.startsWith("PrusaSlicer-"));
  const source = await boundedText(join(root, "fixtures/geometry/box-mm.step"));
  manifest.originalSourceSha256 = sha256(source);
  await save("cube-10-derived.step", cubeSource(source));
  const base = JSON.parse(
    await boundedText(join(root, "fixtures/manufacturing/complete-sparse.intent.json")),
  );
  const r = base.recipe,
    p = r.process;
  if (
    r.layer_height_mm !== 0.2 ||
    r.bead_width_mm !== 0.45 ||
    r.filament_diameter_mm !== 1.75 ||
    r.top_skin_thickness_mm !== 0.8 ||
    r.bottom_skin_thickness_mm !== 0.8 ||
    r.print_speed_mm_s !== 35 ||
    r.travel_speed_mm_s !== 100 ||
    r.flow_multiplier !== 1 ||
    r.rectilinear_axis !== "alternating_xy" ||
    r.top_skin_axis !== "alternating_xy" ||
    p.bridge_speed_factor !== 0.6 ||
    p.bridge_flow_factor !== 1 ||
    p.bridge_fan_pwm !== 255 ||
    p.fan_after_first_layers !== 128 ||
    p.first_layer_speed_factor !== 0.5 ||
    p.first_layer_flow_factor !== 1 ||
    base.printer.nozzle_diameter_mm !== 0.4
  )
    throw new Error("base intent no longer matches frozen nominal comparison contract");
  manifest.settingsNote =
    "Nominal dimensions, widths, counts, fill fraction/axes, skins, speeds, flow and temperatures matched. Classic rounded-bead spacing, fill fitting, bridging, anchor qualification and schedule semantics can differ. Reference fan50%=127.5 versus Spiling128 commanded PWM. Complete-job startup differs; only declared Spiling prime is excluded from model. Full resolved reference configuration retained; it is not identical semantic intent.";
  for (const c of cases) {
    console.error(`Comparing ${c.id}`);
    const directory = join(output, c.id);
    await mkdir(directory);
    const record = { id: c.id, specification: c };
    report.cases.push(record);
    try {
      const intent = intentFor(base, c),
        prime = intent.printer.workflow.job;
      await save(`${c.id}/spiling.intent.json`, intent);
      await save(`${c.id}/prusa.ini`, referenceProfile(c));
      overlays[c.id] = {};
      await run(
        reference,
        [
          "--load",
          join(directory, "prusa.ini"),
          "--export-gcode",
          "--threads",
          "1",
          "--center",
          "5,5",
          "--save",
          join(directory, "prusa-resolved.ini"),
          "--output",
          join(directory, "prusa.gcode"),
          join(output, "cube-10-derived.step"),
        ],
        `${c.id}/prusa`,
      );
      const referenceDecoded = decodeGcode(await boundedText(join(directory, "prusa.gcode")));
      record.reference = auditCube(referenceDecoded, c);
      overlays[c.id].reference = overlayBands(referenceDecoded);
      await save(`${c.id}/reference-audit.json`, record.reference);
      const stdout = await run(
        cli,
        [
          "project",
          "create",
          join(directory, "project.spiling"),
          "--import",
          join(output, "cube-10-derived.step"),
          "--intent",
          join(directory, "spiling.intent.json"),
          "--compile",
          "--verify",
          "--manufacturing-out",
          join(directory, "spiling-export"),
          "--save",
          "--engine",
          engine,
        ],
        `${c.id}/spiling`,
      );
      const result = JSON.parse(stdout);
      manifest.engineBuild = result.hello?.engine_build;
      // Export is the real CLI's fresh independent replay surface, not plan preview.
      const verified = JSON.parse(
        await boundedText(join(directory, "spiling-export/verification.json")),
      );
      if (verified.verified !== true) throw new Error("native replay did not pass");
      const decoded = decodeGcode(
        await boundedText(join(directory, "spiling-export/program.gcode")),
      );
      record.spiling = auditCube(decoded, {
        ...c,
        prime: {
          from: prime.prime_start_mm,
          to: prime.prime_end_mm,
          filamentMm: prime.prime_filament_length_mm,
        },
      });
      overlays[c.id].spiling = overlayBands(decoded);
      await save(`${c.id}/spiling-audit.json`, record.spiling);
      const reopenedStdout = await run(
        cli,
        [
          "project",
          "inspect",
          join(directory, "project.spiling"),
          "--verify",
          "--manufacturing-out",
          join(directory, "reopened-export"),
          "--engine",
          engine,
        ],
        `${c.id}/spiling-reopened`,
      );
      if (JSON.parse(reopenedStdout).hello?.engine_build !== manifest.engineBuild)
        throw new Error("reopened engine identity changed");
      const reopenedVerification = JSON.parse(
        await boundedText(join(directory, "reopened-export/verification.json")),
      );
      const originalProgram = await boundedText(join(directory, "spiling-export/program.gcode"));
      const reopenedProgram = await boundedText(join(directory, "reopened-export/program.gcode"));
      if (reopenedVerification.verified !== true || originalProgram !== reopenedProgram)
        throw new Error("fresh-process saved-project replay/export changed the program or failed");
      record.recovery = { freshProcessVerified: true, programIdentical: true };
      let scheduleProbe = null;
      if (record.spiling.layerCount !== record.reference.layerCount) {
        const diagnostic = structuredClone(intent);
        diagnostic.recipe.native_shape_error_mm = 0.101;
        await save(`${c.id}/shape-bound-diagnostic.intent.json`, diagnostic);
        const probeStdout = await run(
          cli,
          [
            "project",
            "create",
            join(directory, "shape-bound-diagnostic.spiling"),
            "--import",
            join(output, "cube-10-derived.step"),
            "--intent",
            join(directory, "shape-bound-diagnostic.intent.json"),
            "--compile",
            "--verify",
            "--manufacturing-out",
            join(directory, "shape-bound-diagnostic-export"),
            "--engine",
            engine,
          ],
          `${c.id}/shape-bound-diagnostic`,
        );
        if (JSON.parse(probeStdout).hello?.engine_build !== manifest.engineBuild)
          throw new Error("diagnostic engine identity changed");
        const probeVerification = JSON.parse(
          await boundedText(join(directory, "shape-bound-diagnostic-export/verification.json")),
        );
        if (probeVerification.verified !== true) throw new Error("diagnostic replay failed");
        const probeAudit = auditCube(
          decodeGcode(
            await boundedText(join(directory, "shape-bound-diagnostic-export/program.gcode")),
          ),
          {
            ...c,
            prime: {
              from: prime.prime_start_mm,
              to: prime.prime_end_mm,
              filamentMm: prime.prime_filament_length_mm,
            },
          },
        );
        const baselinePlan = JSON.parse(
          await boundedText(join(directory, "spiling-export/plan.json")),
        );
        const diagnosticPlan = JSON.parse(
          await boundedText(join(directory, "shape-bound-diagnostic-export/plan.json")),
        );
        scheduleProbe = {
          correctnessPassed: probeAudit.correctnessPassed,
          baseline: baselinePlan.native_geometry,
          diagnostic: diagnosticPlan.native_geometry,
          elevations: probeAudit.layers.map((l) => l.upperMm),
        };
        await save(`${c.id}/shape-bound-diagnostic-audit.json`, probeAudit);
      }
      if (c.infillFraction < 1) {
        record.referenceProbes = [];
        for (const probe of referenceProbeSettings) {
          const prefix = `reference-${probe.id}`;
          await save(`${c.id}/${prefix}.ini`, referenceProfile(c, probe.settings));
          await run(
            reference,
            [
              "--load",
              join(directory, `${prefix}.ini`),
              "--export-gcode",
              "--threads",
              "1",
              "--center",
              "5,5",
              "--save",
              join(directory, `${prefix}-resolved.ini`),
              "--output",
              join(directory, `${prefix}.gcode`),
              join(output, "cube-10-derived.step"),
            ],
            `${c.id}/${prefix}`,
          );
          const probeAudit = auditCube(
            decodeGcode(await boundedText(join(directory, `${prefix}.gcode`))),
            c,
          );
          await save(`${c.id}/${prefix}-audit.json`, probeAudit);
          record.referenceProbes.push({ ...probe, audit: probeAudit });
        }
        record.referenceProbeEvidence = evaluateReferenceProbes(
          record.reference,
          record.referenceProbes,
        );
      }
      record.comparison = compareAudits(record.spiling, record.reference, {
        scheduleProbe,
        referenceProbeEvidence: record.referenceProbeEvidence,
      });
      await save(`${c.id}/comparison.json`, record.comparison);
      console.error(
        `  correctness ${record.comparison.correctnessPassed}; unexplained ${record.comparison.differences.filter((d) => d.status === "unresolved").length}`,
      );
    } catch (error) {
      record.error = error.message;
      console.error(error.message);
    }
  }
} catch (error) {
  report.error = error.message;
  console.error(error.message);
}
// Hash the evidence consumed by each comparison, including the observer itself.
for (const name of [
  "decode.mjs",
  "analyze.mjs",
  "anchors.mjs",
  "construction.mjs",
  "probes.mjs",
  "corpus.mjs",
  "report.mjs",
])
  manifest.files[`tools/gcode-parity/${name}`] = sha256(
    await readFile(join(root, "tools/gcode-parity", name)),
  );
manifest.files["tools/compare-slicers.mjs"] = sha256(
  await readFile(fileURLToPath(import.meta.url)),
);
for (const c of report.cases)
  for (const file of [
    "spiling.intent.json",
    "prusa.ini",
    "prusa-resolved.ini",
    "prusa.gcode",
    "spiling-export/program.gcode",
    "spiling-export/plan.json",
    "spiling-export/verification.json",
    "spiling-export/provenance.json",
    "reopened-export/plan.json",
    "reopened-export/program.gcode",
    "reopened-export/verification.json",
    "reopened-export/provenance.json",
    ...referenceProbeSettings.flatMap((p) => [
      `reference-${p.id}.ini`,
      `reference-${p.id}-resolved.ini`,
      `reference-${p.id}.gcode`,
      `reference-${p.id}-audit.json`,
    ]),
    "shape-bound-diagnostic.intent.json",
    "shape-bound-diagnostic-export/plan.json",
    "shape-bound-diagnostic-export/program.gcode",
    "shape-bound-diagnostic-export/verification.json",
    "shape-bound-diagnostic-export/provenance.json",
  ]) {
    try {
      manifest.files[`${c.id}/${file}`] = sha256(await readFile(join(output, c.id, file)));
    } catch {}
  }
manifest.files["cube-10-derived.step"] = sha256(
  await readFile(join(output, "cube-10-derived.step")).catch(() => ""),
);
report.correctnessPassed =
  !report.error &&
  report.cases.length === cases.length &&
  report.cases.every((c) => !c.error && c.comparison.correctnessPassed);
report.differencesExplained =
  report.correctnessPassed && report.cases.every((c) => c.comparison.differencesExplained);
await save("manifest.json", manifest);
await save("report.json", report);
await save("report.html", reportHtml(report, overlays));
console.error(`Report: ${join(output, "report.html")}`);
process.exitCode = !report.correctnessPassed
  ? 1
  : !report.differencesExplained && !options.observe
    ? 2
    : 0;
