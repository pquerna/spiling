/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { spawn } from "node:child_process";
import { copyFile, mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join, resolve, delimiter } from "node:path";
import { fileURLToPath } from "node:url";
import { homedir } from "node:os";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const desktop = join(root, "apps", "desktop");
const binSuffix = process.platform === "win32" ? ".exe" : "";
const environment = {
  ...process.env,
  PATH: `${join(homedir(), ".cargo", "bin")}${delimiter}${process.env.PATH}`,
  CEF_PATH: process.env.CEF_PATH ?? join(root, ".cache", "cef"),
  CARGO_BUILD_JOBS: process.env.CARGO_BUILD_JOBS ?? "4",
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

async function hostTriple() {
  const version = await run("rustc", ["-vV"], { capture: true });
  const host = /^host: (.+)$/m.exec(version)?.[1];
  if (!host) throw new Error("rustc did not report its host target");
  return host;
}

async function contracts(check = false) {
  await run("cargo", [
    "run",
    "--locked",
    "-p",
    "spiling-contracts",
    "--bin",
    "generate",
    "--",
    ...(check ? ["--check"] : []),
  ]);
}

async function stageEngine(release = false) {
  await run("cargo", [
    "build",
    "--locked",
    "-p",
    "spiling-engine",
    "-p",
    "spiling-cli",
    ...(release ? ["--release"] : []),
  ]);
  const triple = await hostTriple();
  const source = join(root, "target", release ? "release" : "debug", `spiling-engine${binSuffix}`);
  const destination = join(
    desktop,
    "src-tauri",
    "binaries",
    `spiling-engine-${triple}${binSuffix}`,
  );
  await mkdir(dirname(destination), { recursive: true });
  await copyFile(source, destination);
  return source;
}

async function copyNotices(packageRoot, output) {
  await mkdir(output, { recursive: true });
  // License texts are copied from the resolved dependency, never relabeled.
  const entries = await readdir(packageRoot, { withFileTypes: true });
  for (const entry of entries) {
    if (entry.isFile() && /^(licen[sc]e|copying|notice|authors)([._-].*)?$/i.test(entry.name)) {
      await copyFile(join(packageRoot, entry.name), join(output, entry.name));
    }
    if (entry.isDirectory() && /^(licenses?|notices?)$/i.test(entry.name)) {
      await copyNoticeDirectory(join(packageRoot, entry.name), join(output, entry.name));
    }
  }
}

async function copyNoticeDirectory(source, destination) {
  await mkdir(destination, { recursive: true });
  for (const entry of await readdir(source, { withFileTypes: true })) {
    if (entry.isDirectory())
      await copyNoticeDirectory(join(source, entry.name), join(destination, entry.name));
    else if (entry.isFile())
      await copyFile(join(source, entry.name), join(destination, entry.name));
  }
}

async function cefArchiveRecords(directory = environment.CEF_PATH, depth = 0) {
  if (!existsSync(directory) || depth > 4) return [];
  const records = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) records.push(...(await cefArchiveRecords(path, depth + 1)));
    else if (entry.isFile() && entry.name === "archive.json") {
      records.push(JSON.parse(await readFile(path, "utf8")));
    }
  }
  return records;
}

async function provenance() {
  const directory = join(root, ".cache", "release");
  await mkdir(directory, { recursive: true });
  const metadata = JSON.parse(
    await run("cargo", ["metadata", "--locked", "--format-version", "1"], { capture: true }),
  );
  const rustPackages = metadata.packages.filter((item) => item.source !== null);
  const nodeLicenses = JSON.parse(
    await run("pnpm", ["licenses", "list", "--json"], { capture: true }),
  );
  const npmPackages = [];
  for (const group of Object.values(nodeLicenses)) {
    if (!Array.isArray(group)) throw new Error("Unsupported pnpm license record schema");
    for (const dependency of group) {
      if (!Array.isArray(dependency.paths)) throw new Error("Missing pnpm dependency paths");
      for (const path of dependency.paths) {
        const pkg = JSON.parse(await readFile(join(path, "package.json"), "utf8"));
        npmPackages.push({
          name: pkg.name,
          version: pkg.version,
          license: pkg.license ?? dependency.license,
          repository: pkg.repository,
          path,
        });
      }
    }
  }
  for (const pkg of rustPackages) {
    await copyNotices(
      dirname(pkg.manifest_path),
      join(directory, "third-party", "rust", `${pkg.name}-${pkg.version}`),
    );
  }
  for (const pkg of npmPackages) {
    await copyNotices(
      pkg.path,
      join(directory, "third-party", "npm", `${pkg.name.replaceAll("/", "_")}-${pkg.version}`),
    );
  }
  const commit = await run("git", ["rev-parse", "HEAD"], { capture: true });
  const dirty = (await run("git", ["status", "--porcelain"], { capture: true })).length > 0;
  const lockHashes = {};
  for (const file of ["Cargo.lock", "pnpm-lock.yaml"]) {
    lockHashes[file] = createHash("sha256")
      .update(await readFile(join(root, file)))
      .digest("hex");
  }
  const record = {
    app: "Spiling",
    version: "0.1.0",
    license: "OSL-3.0",
    source: "https://github.com/pquerna/spiling",
    commit,
    dirty,
    generatedAt: new Date().toISOString(),
    platform: process.platform,
    arch: process.arch,
    rust: await run("rustc", ["--version"], { capture: true }),
    node: process.version,
    pnpm: await run("pnpm", ["--version"], { capture: true }),
    lockHashes,
    rustDependencies: rustPackages.map(({ name, version, license, source }) => ({
      name,
      version,
      license,
      source,
    })),
    npmDependencies: npmPackages.map(({ name, version, license, repository }) => ({
      name,
      version,
      license,
      repository,
    })),
    cefDistributions: await cefArchiveRecords(),
  };
  await writeFile(join(directory, "provenance.json"), JSON.stringify(record, null, 2) + "\n");
  await copyFile(join(root, "LICENSE.md"), join(directory, "LICENSE.md"));
  await copyFile(join(root, "NOTICE.md"), join(directory, "NOTICE.md"));
  console.error(`Collected dependency notices in ${directory}`);
}

async function prepare(release = false) {
  await contracts();
  const enginePath = await stageEngine(release);
  await provenance();
  return enginePath;
}

async function bootstrap() {
  if (Number(process.versions.node.split(".")[0]) !== 24)
    throw new Error("Spiling requires Node 24.x");
  const pnpmVersion = await run("pnpm", ["--version"], { capture: true });
  if (pnpmVersion !== "10.32.1")
    throw new Error("Install pnpm 10.32.1: npm install --global pnpm@10.32.1");
  await run("rustup", ["show", "active-toolchain"]);
  if (process.platform === "linux") await run("pkg-config", ["--atleast-version=4.6", "gtk4"]);
  await run("pnpm", ["install", "--frozen-lockfile"]);
  await run("cargo", ["fetch", "--locked"]);
  await prepare();
  // Compile the actual native runtime here: its pinned CEF build utility obtains
  // the matching native distribution. A successful Rust-only bootstrap is not
  // enough to establish that CEF can be linked on the host.
  await run("pnpm", ["--filter", "@spiling/desktop", "build"]);
  await run("cargo", ["build", "--locked", "-p", "spiling-desktop"]);
  await provenance();
}

async function check() {
  await contracts(true);
  await run("cargo", ["fmt", "--all", "--", "--check"]);
  await run("cargo", [
    "clippy",
    "--locked",
    "--workspace",
    "--all-targets",
    "--",
    "-D",
    "warnings",
  ]);
  await run("pnpm", ["-r", "--if-present", "check"]);
  await run("pnpm", ["exec", "prettier", "--check", "."]);
}

async function cliSmoke() {
  await contracts(true);
  await stageEngine();
  await run("node", ["--import", "tsx", "tools/smoke-cli.mjs"]);
}

async function benchmark() {
  const engine = await stageEngine();
  const cli = join(root, "target", "debug", `spiling-cli${binSuffix}`);
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
    case "bootstrap":
      await bootstrap();
      break;
    case "contracts":
      await contracts(process.argv.includes("--check"));
      break;
    case "dev": {
      const engine = await prepare();
      await run("pnpm", ["exec", "tauri", "dev"], {
        cwd: desktop,
        env: { SPILING_ENGINE_PATH: engine },
      });
      break;
    }
    case "build": {
      await prepare();
      await run("pnpm", ["--filter", "@spiling/desktop", "build"]);
      await run("cargo", ["build", "--locked", "-p", "spiling-desktop"]);
      break;
    }
    case "package": {
      await prepare(true);
      const bundles =
        process.platform === "linux" ? "deb" : process.platform === "darwin" ? "app" : "nsis";
      await run("pnpm", ["exec", "tauri", "build", "--bundles", bundles], { cwd: desktop });
      break;
    }
    case "check":
      await check();
      break;
    case "test":
      await stageEngine();
      await run("cargo", ["test", "--locked", "--workspace"]);
      await cliSmoke();
      await run("pnpm", ["--filter", "@spiling/protocol", "test"]);
      break;
    case "smoke:cli":
      await cliSmoke();
      break;
    case "bench:smoke":
      await benchmark();
      break;
    case "smoke:desktop":
      await run("node", ["tools/smoke-desktop.mjs", ...process.argv.slice(3)]);
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
