/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import { spawn } from "node:child_process";
import {
  copyFile,
  mkdir,
  readFile,
  readdir,
  rm,
  writeFile,
  chmod,
  symlink,
} from "node:fs/promises";
import { existsSync } from "node:fs";
import { createHash } from "node:crypto";
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
  return join(cargoTarget, release ? "release" : "debug", `spiling-engine${binSuffix}`);
}

async function copyNotices(packageRoot, output) {
  await mkdir(output, { recursive: true });
  // License texts are copied from the resolved dependency, never relabeled.
  const entries = await readdir(packageRoot, { withFileTypes: true });
  for (const entry of entries) {
    if (
      entry.isFile() &&
      /^(licen[sc]e|copying|notice|authors|attribution)([._-].*)?$/i.test(entry.name)
    ) {
      await copyFile(join(packageRoot, entry.name), join(output, entry.name));
    }
    if (entry.isDirectory() && /^(licenses?|notices?)$/i.test(entry.name)) {
      await copyNoticeDirectory(join(packageRoot, entry.name), join(output, entry.name));
    }
  }
  // epaint's bundled font licenses use upstream filenames inside fonts/.
  // Keep the copyright/license text alongside the crate's provenance, without
  // copying the embedded font binaries into the notice directory.
  if (/^epaint_default_fonts-/.test(packageRoot.split(/[\\/]/).at(-1))) {
    const fonts = join(packageRoot, "fonts");
    for (const entry of await readdir(fonts)) {
      if (entry.endsWith(".txt")) {
        await mkdir(join(output, "fonts"), { recursive: true });
        await copyFile(join(fonts, entry), join(output, "fonts", entry));
      }
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

async function provenance() {
  const directory = join(root, ".cache", "release");
  await rm(directory, { recursive: true, force: true });
  await mkdir(directory, { recursive: true });
  const metadata = JSON.parse(
    await run("cargo", ["metadata", "--locked", "--format-version", "1"], { capture: true }),
  );
  const workspacePackages = new Set(metadata.workspace_members);
  // Collect the installed binaries' normal/build closure, including target-specific
  // dependencies. Development-only dependencies are not shipped.
  const nodes = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
  const closure = new Set();
  function visit(id) {
    if (closure.has(id)) return;
    closure.add(id);
    for (const dep of nodes.get(id)?.deps ?? [])
      if (dep.dep_kinds.some((kind) => kind.kind !== "dev")) visit(dep.pkg);
  }
  for (const pkg of metadata.packages)
    if (["spiling-engine", "spiling-workbench", "spiling-cli"].includes(pkg.name)) visit(pkg.id);
  const rustPackages = metadata.packages.filter(
    (item) => closure.has(item.id) && (item.source !== null || !workspacePackages.has(item.id)),
  );
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
  const sourceComponents = [
    {
      name: "prusa-fdm-mixer",
      source: "https://github.com/prusa3d/prusa-fdm-mixer",
      revision: "007bb922994cdf985c324cb734f6504e805f6790",
      license: "MIT",
      path: "crates/appearance/src/prusa_v7.rs",
      modifications:
        "Allocation-free Rust port of the pinned TypeScript v7 reference functions; no measurement dataset imported.",
    },
  ];
  await copyNotices(
    join(root, "crates", "appearance"),
    join(directory, "third-party", "source", "prusa-fdm-mixer-007bb922"),
  );
  await copyNoticeDirectory(
    join(root, "apps", "workbench", "licenses", "egui"),
    join(directory, "third-party", "source", "egui-0.37.0"),
  );
  const vendorNotices = join(directory, "third-party", "source", "solidcraft-geometry-step");
  await copyNotices(join(root, "vendor", "solidcraft"), vendorNotices);
  await copyFile(
    join(root, "vendor", "solidcraft", "UPSTREAM.toml"),
    join(vendorNotices, "UPSTREAM.toml"),
  );
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
    sourceComponents,
  };
  await writeFile(join(directory, "provenance.json"), JSON.stringify(record, null, 2) + "\n");
  await copyFile(join(root, "LICENSE.md"), join(directory, "LICENSE.md"));
  await copyFile(join(root, "NOTICE.md"), join(directory, "NOTICE.md"));
  console.error(`Collected dependency notices in ${directory}`);
}

async function nativeBuild(release = false) {
  await run("cargo", [
    "build",
    "--locked",
    "-p",
    "spiling-engine",
    "-p",
    "spiling-workbench",
    "-p",
    "spiling-cli",
    ...(release ? ["--release"] : []),
  ]);
}

async function bootstrap() {
  if (Number(process.versions.node.split(".")[0]) !== 24)
    throw new Error("Spiling requires Node 24.x");
  if ((await run("pnpm", ["--version"], { capture: true })) !== "10.32.1")
    throw new Error("Install pnpm 10.32.1: npm install --global pnpm@10.32.1");
  await run("rustup", ["show", "active-toolchain"]);
  await run("pnpm", ["install", "--frozen-lockfile"]);
  await run("cargo", ["fetch", "--locked"]);
  await contracts(true);
  await nativeBuild();
}

async function packageNative() {
  // A package always has an exactly recoverable source tree. Never silently
  // attribute dirty binaries to HEAD or archive private/untracked workspace data.
  if (await run("git", ["status", "--porcelain"], { capture: true }))
    throw new Error("Commit the package source first; packaging requires a clean worktree");
  await contracts(true);
  await nativeBuild(true);
  await provenance();
  const triple = await hostTriple();
  const name = `spiling-0.1.0-${triple}`;
  const output = join(cargoTarget, "release", "bundle");
  const stage = join(output, name);
  await rm(stage, { recursive: true, force: true });
  await mkdir(stage, { recursive: true });
  const app = process.platform === "darwin" ? join(stage, "Spiling.app", "Contents") : stage;
  const binaries = process.platform === "darwin" ? join(app, "MacOS") : app;
  await mkdir(binaries, { recursive: true });
  for (const bin of ["spiling-workbench", "spiling-engine", "spiling-cli"]) {
    const target = join(binaries, `${bin}${binSuffix}`);
    await copyFile(join(cargoTarget, "release", `${bin}${binSuffix}`), target);
    if (process.platform !== "win32") await chmod(target, 0o755);
  }
  const resources = process.platform === "darwin" ? join(app, "Resources") : app;
  await copyNoticeDirectory(join(root, ".cache", "release"), join(resources, "notices"));
  await copyNoticeDirectory(join(root, "apps", "workbench", "icons"), join(resources, "icons"));
  await copyFile(
    join(root, "docs", "quality", "SOURCE.md"),
    join(resources, "notices", "SOURCE.md"),
  );
  await run("git", [
    "archive",
    "--format=tar.gz",
    "--prefix=spiling-source/",
    `--output=${join(resources, "notices", "source.tar.gz")}`,
    "HEAD",
  ]);
  if (process.platform === "darwin") {
    await writeFile(
      join(app, "Info.plist"),
      `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleExecutable</key><string>spiling-workbench</string><key>CFBundleIdentifier</key><string>org.spiling.workbench</string><key>CFBundleName</key><string>Spiling</string><key>CFBundleVersion</key><string>0.1.0</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleIconFile</key><string>icon.icns</string><key>NSHighResolutionCapable</key><true/></dict></plist>
`,
    );
    await copyFile(join(resources, "icons", "icon.icns"), join(resources, "icon.icns"));
  }
  await run("tar", ["-czf", join(output, `${name}.tar.gz`), "-C", output, name]);
  if (process.platform === "linux") {
    const deb = join(output, `${name}-deb`);
    await rm(deb, { recursive: true, force: true });
    await copyNoticeDirectory(stage, join(deb, "opt", "spiling"));
    for (const bin of ["spiling-workbench", "spiling-engine", "spiling-cli"])
      await chmod(join(deb, "opt", "spiling", bin), 0o755);
    await mkdir(join(deb, "DEBIAN"), { recursive: true });
    const arch = await run("dpkg", ["--print-architecture"], { capture: true });
    const depsRoot = join(output, `${name}-dependencies`);
    await mkdir(join(depsRoot, "debian"), { recursive: true });
    await writeFile(
      join(depsRoot, "debian", "control"),
      `Source: spiling
Section: graphics
Priority: optional
Maintainer: Spiling contributors <spiling@pquerna.com>

Package: spiling
Architecture: any
Description: Experimental native workbench
`,
    );
    const linked = await run(
      "dpkg-shlibdeps",
      [
        "-O",
        ...["spiling-workbench", "spiling-engine", "spiling-cli"].map(
          (bin) => `-e${join(deb, "opt", "spiling", bin)}`,
        ),
      ],
      { cwd: depsRoot, capture: true },
    );
    const linkedDependencies = /^shlibs:Depends=(.+)$/m.exec(linked)?.[1];
    if (!linkedDependencies)
      throw new Error("dpkg-shlibdeps did not return native ABI dependencies");
    const dependencies = linkedDependencies.split(",").map((value) => value.trim());
    for (const library of [
      "libwayland-client0",
      "libx11-6",
      "libxkbcommon0",
      "libvulkan1",
      "xdg-desktop-portal",
    ])
      if (!dependencies.some((value) => value.split(/[ (]/)[0] === library))
        dependencies.push(library);
    await rm(depsRoot, { recursive: true, force: true });
    await writeFile(
      join(deb, "DEBIAN", "control"),
      `Package: spiling
Version: 0.1.0
Architecture: ${arch}
Maintainer: Spiling contributors <spiling@pquerna.com>
Section: graphics
Priority: optional
Depends: ${dependencies.join(", ")}
Recommends: xdg-desktop-portal-gtk
Description: Native BREP manufacturing workbench (experimental, not machine ready)
License: OSL-3.0
`,
    );
    await mkdir(join(deb, "usr", "bin"), { recursive: true });
    await symlink("/opt/spiling/spiling-workbench", join(deb, "usr", "bin", "spiling"));
    const applications = join(deb, "usr", "share", "applications");
    await mkdir(applications, { recursive: true });
    await writeFile(
      join(applications, "org.spiling.Workbench.desktop"),
      `[Desktop Entry]
Type=Application
Name=Spiling
Comment=Experimental native BREP manufacturing workbench
Exec=/opt/spiling/spiling-workbench
Icon=/opt/spiling/icons/icon.png
Categories=Graphics;Engineering;
Terminal=false
`,
    );
    await run("desktop-file-validate", [join(applications, "org.spiling.Workbench.desktop")]);
    await run("dpkg-deb", [
      "--root-owner-group",
      "--threads-max=1",
      "--build",
      deb,
      join(output, `${name}.deb`),
    ]);
  }
  console.error(`Native distribution: ${output}`);
}

async function check() {
  await contracts(true);
  // Format workspace-owned targets; --all also traverses excluded upstream path dependencies.
  await run("cargo", ["fmt", "--", "--check"]);
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
    case "bootstrap":
      await bootstrap();
      break;
    case "contracts":
      await contracts(process.argv.includes("--check"));
      break;
    case "provenance":
      await provenance();
      break;
    case "build:native":
      await nativeBuild();
      break;
    case "dev":
    case "dev:native":
      await nativeBuild();
      await run(join(cargoTarget, "debug", `spiling-workbench${binSuffix}`), process.argv.slice(3));
      break;
    case "gallery:native":
      await run("cargo", ["run", "--locked", "-p", "spiling-workbench", "--bin", "gallery"]);
      break;
    case "test:native":
      await nativeBuild();
      await run("cargo", ["test", "--locked", "-p", "spiling-workbench", "--lib"]);
      await run(
        "cargo",
        [
          "test",
          "--locked",
          "-p",
          "spiling-workbench",
          "--lib",
          "--",
          "--ignored",
          "--test-threads=1",
        ],
        {
          env: { SPILING_TEST_ENGINE: join(cargoTarget, "debug", `spiling-engine${binSuffix}`) },
        },
      );
      break;
    case "build":
      await contracts(true);
      await nativeBuild();
      break;
    case "package":
      await packageNative();
      break;
    case "check":
      await check();
      break;
    case "compare:slicers":
      await run("cargo", ["build", "--locked", "-p", "spiling-engine", "-p", "spiling-cli"]);
      await run("node", ["tools/compare-slicers.mjs", ...process.argv.slice(3)]);
      break;
    case "test:materials":
      await run("cargo", ["build", "--locked", "-p", "spiling-engine", "-p", "spiling-cli"]);
      await run("node", ["tools/smoke-material-profiles.mjs"]);
      break;
    case "test:parity":
      await run("node", ["--test", "tools/gcode-parity/test.mjs"]);
      break;
    case "test":
      await stageEngine();
      await run("cargo", ["test", "--locked", "--workspace"]);
      await cliSmoke();
      await run("pnpm", ["--filter", "@spiling/protocol", "test"]);
      await run("node", ["--import", "tsx", "--test", "tools/gcode-parity/test.mjs"]);
      break;
    case "smoke:cli":
      await cliSmoke();
      break;
    case "bench:smoke":
      await benchmark();
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
