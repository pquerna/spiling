/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { spawn } from "node:child_process";
import { readdir } from "node:fs/promises";
import { dirname, join, resolve, delimiter } from "node:path";
import { fileURLToPath } from "node:url";
import { homedir } from "node:os";
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const binSuffix = process.platform === "win32" ? ".exe" : "";
const cargoTarget = resolve(root, process.env.CARGO_TARGET_DIR ?? "target");
const environment = {
  ...process.env,
  PATH: `${join(homedir(), ".cargo", "bin")}${delimiter}${process.env.PATH}`,
  CARGO_BUILD_JOBS: process.env.CARGO_BUILD_JOBS ?? "1",
  RUST_TEST_THREADS: process.env.RUST_TEST_THREADS ?? "1",
  RAYON_NUM_THREADS: process.env.RAYON_NUM_THREADS ?? "1",
};
// Debug typed RPC futures need the same bounded stack as the native host.
// Apply only to test processes; development and packaged runtime keep their own stacks.
const testEnvironment = {
  RUST_MIN_STACK: process.env.RUST_MIN_STACK ?? String(8 * 1024 * 1024),
};
const children = new Set();
let stopping = false;
for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => {
    stopping = true;
    for (const child of children) child.kill(signal);
  });
}

export function run(command, args, options = {}) {
  console.error(`> ${command} ${args.join(" ")}`);
  return new Promise((resolveResult, reject) => {
    // Windows .cmd launchers require cmd.exe; our arguments are fixed developer
    // commands, not remotely supplied shell input.
    const child = spawn(command, args, {
      cwd: options.cwd ?? root,
      env: { ...environment, ...options.env },
      stdio: options.capture ? ["ignore", "pipe", "inherit"] : "inherit",
      shell: process.platform === "win32" && ["pnpm", "npm"].includes(command),
      windowsHide: true,
    });
    children.add(child);
    let output = "";
    if (options.capture)
      child.stdout.setEncoding("utf8").on("data", (chunk) => {
        output += chunk;
      });
    child.once("error", (error) => {
      children.delete(child);
      reject(error);
    });
    child.once("exit", (code, signal) => {
      children.delete(child);
      if (code === 0) resolveResult(output.trim());
      else
        reject(new Error(`${command} exited ${code ?? signal}${stopping ? " (interrupted)" : ""}`));
    });
  });
}

// Test suites have independent acceptance surfaces. A failed suite must remain
// fatal without hiding later evidence; interruption still stops new children.
export async function runTestSuites(suites) {
  const failures = [];
  for (const [name, execute] of suites) {
    if (stopping) throw new Error("Test run interrupted");
    try {
      await execute();
    } catch (error) {
      if (stopping) throw error;
      failures.push({ name, error });
      console.error(`Test suite ${name} failed: ${error.message}`);
    }
  }
  if (failures.length) {
    throw new AggregateError(
      failures.map(({ error }) => error),
      `Failed test suites:\n${failures.map(({ name, error }) => `  ${name}: ${error.message}`).join("\n")}`,
    );
  }
}

async function contracts(check = false, execute = run) {
  await execute("cargo", ["xtask", "contracts", ...(check ? ["--check"] : [])]);
  await execute("node", [
    "--experimental-strip-types",
    "tools/generate-hardware-catalog.mjs",
    ...(check ? ["--check"] : []),
  ]);
}
async function stageEngine(release = false, execute = run) {
  if (release) throw new Error("Use cargo xtask package for release builds");
  await execute("cargo", ["xtask", "build"]);
  return join(cargoTarget, "debug", `spiling-engine${binSuffix}`);
}
async function check() {
  await contracts(true);
  await run("node", ["tools/material-catalogs.mjs", "--check"]);
  await run("pnpm", ["-r", "--if-present", "check"]);
  await run("pnpm", ["exec", "prettier", "--check", "."]);
}
async function cliSmoke(execute = run) {
  await execute("node", ["--experimental-strip-types", "tools/smoke-cli.mjs"]);
}
// Optional cross-language observers, separate from cargo xtask's native suite.
export async function toolingTests(execute = run) {
  await stageEngine(false, execute);
  const tests = (await readdir(join(root, "tools")))
    .filter((name) => name.endsWith(".test.mjs"))
    .sort()
    .map((name) => `tools/${name}`);
  await runTestSuites([
    ["generated authoring data", () => contracts(true, execute)],
    ["tooling", () => execute("node", ["--test", ...tests])],
    ["CLI smoke", () => cliSmoke(execute)],
    ["TypeScript protocol", () => execute("pnpm", ["--filter", "@spiling/protocol", "test"])],
    [
      "G-code parity",
      () =>
        execute("node", ["--experimental-strip-types", "--test", "tools/gcode-parity/test.mjs"]),
    ],
  ]);
}
async function sliceBenchmark() {
  // Cargo runs a bench in its package directory; root commands own root-relative paths.
  const args = process.argv.slice(3);
  for (let i = 0; i < args.length - 1; i++) {
    if (["--bundle", "--write-bundle"].includes(args[i])) {
      args[i + 1] = resolve(root, args[i + 1]);
      i++;
    }
  }
  await run("cargo", [
    "bench",
    "--locked",
    "--profile",
    "profiling",
    "-p",
    "spiling-manufacturing",
    "-p",
    "spiling-engine",
    "-p",
    "spiling-cli",
    "-p",
    "spiling-workbench",
    "--bench",
    "pipeline",
    "--",
    ...args,
  ]);
}

async function benchmark() {
  const engine = await stageEngine();
  const cli = join(cargoTarget, "debug", `spiling-cli${binSuffix}`);
  const results = [];
  for (let index = 0; index < 10; index++) {
    const start = performance.now();
    await run(cli, ["triangle", "--engine", engine], { capture: true });
    results.push(performance.now() - start);
  }
  results.sort((a, b) => a - b);
  console.log(
    JSON.stringify(
      {
        workload: "B0 synthetic triangle: fresh process + handshake + 64-byte transfer + shutdown",
        runs: 10,
        p50Ms: results[4],
        p95Ms: results[9],
        samplesMs: results,
        platform: process.platform,
        arch: process.arch,
      },
      null,
      2,
    ),
  );
}

async function main() {
  const command = process.argv[2];
  switch (command) {
    case "infill:lab": {
      const args = process.argv.slice(3);
      const mode = args[0];
      const pythonArgs = [
        "report",
        "budget",
        "summary",
        "sweep",
        "campaign",
        "cohort_report",
      ].includes(mode)
        ? [`tools/infill-lab/${mode}.py`, ...args.slice(1)]
        : mode === "test"
          ? ["-m", "pytest", "-q", "tools/infill-lab/test_lab.py", ...args.slice(1)]
          : ["tools/infill-lab/lab.py", ...args];
      await run(process.env.SPILING_UV ?? "uv", [
        "run",
        "--locked",
        "--project",
        "tools/infill-lab",
        "--all-extras",
        "python",
        ...pythonArgs,
      ]);
      break;
    }
    case "contracts":
      await contracts(process.argv.includes("--check"));
      break;
    case "check:tooling":
      await check();
      break;
    case "compare:slicers":
      await run("cargo", ["build", "--locked", "-p", "spiling-engine", "-p", "spiling-cli"]);
      await run("node", ["tools/compare-slicers.mjs", ...process.argv.slice(3)]);
      break;
    case "test:geometry-stress":
      await runTestSuites([
        [
          "oversized geometry integration",
          () =>
            run(
              "cargo",
              [
                "test",
                "--locked",
                "-p",
                "spiling-e2e-tests",
                "--test",
                "geometry_stress",
                "--",
                "--ignored",
                "--test-threads=1",
              ],
              { env: testEnvironment },
            ),
        ],
        [
          "oversized native support",
          () =>
            run(
              "cargo",
              [
                "test",
                "--locked",
                "-p",
                "spiling-geometry",
                "--lib",
                "analysis::bounds::tests::native_plate_support_does_not_require_exact_authoring_chart_closure",
                "--",
                "--exact",
                "--ignored",
                "--test-threads=1",
              ],
              { env: testEnvironment },
            ),
        ],
      ]);
      break;
    case "test:hardware":
      await contracts(true);
      await stageEngine();
      await run("node", ["--experimental-strip-types", "tools/smoke-hardware-library.mjs"]);
      break;
    case "materials:sync":
      await run("node", ["tools/material-catalogs.mjs", "--sync", ...process.argv.slice(3)]);
      break;
    case "materials:check":
      await run("node", ["tools/material-catalogs.mjs", "--check"]);
      break;
    case "test:material-catalogs":
      await run("node", ["--test", "tools/material-catalogs.test.mjs"]);
      break;
    case "test:materials":
      await run("cargo", ["build", "--locked", "-p", "spiling-engine", "-p", "spiling-cli"]);
      await run("node", ["tools/smoke-material-profiles.mjs"]);
      break;
    case "test:parity":
      await run("node", ["--test", "tools/gcode-parity/test.mjs"]);
      break;
    case "test:tooling":
      await toolingTests();
      break;
    case "smoke:cli":
      await contracts(true);
      await stageEngine();
      await cliSmoke();
      break;
    case "bench:smoke":
      await benchmark();
      break;
    case "bench:open":
      await run("cargo", [
        "build",
        "--locked",
        "--profile",
        "profiling",
        "-p",
        "spiling-engine",
        "-p",
        "spiling-cli",
        "-p",
        "spiling-workbench",
        "--bin",
        "spiling-engine",
        "--bin",
        "spiling-cli",
        "--bin",
        "spiling-workbench",
      ]);
      await run("node", ["tools/bench-open.mjs", ...process.argv.slice(3)]);
      break;
    case "bench:slice":
      await sliceBenchmark();
      break;
    default:
      throw new Error(`Unknown command ${command ?? "(missing)"}`);
  }
}

if (resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
