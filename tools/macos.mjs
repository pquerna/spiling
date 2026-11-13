/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
import { copyFile, mkdir, open, readFile, rm, stat, writeFile, chmod } from "node:fs/promises";
import { createReadStream } from "node:fs";
import { createHash } from "node:crypto";
import { join, resolve, dirname, isAbsolute } from "node:path";
import { homedir } from "node:os";
import { fileURLToPath } from "node:url";

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export const macosApp = JSON.parse(
  await readFile(join(repository, "tools/macos/app.json"), "utf8"),
);
const queries = [
  "libs",
  "libnames",
  "system-libs",
  "libdir",
  "includedir",
  "version",
  "cflags",
  "cxxflags",
];
const sha256 = async (path) => {
  const hash = createHash("sha256");
  for await (const bytes of createReadStream(path)) hash.update(bytes);
  return hash.digest("hex");
};
const xml = (s) => s.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");

export async function writeMacosMetadata(contents, version) {
  if (!/^\d+\.\d+\.\d+$/.test(version))
    throw new Error("macOS bundle version must be three numeric components");
  const strings = {
    CFBundleExecutable: macosApp.executable,
    CFBundleIdentifier: macosApp.identifier,
    CFBundleName: macosApp.name,
    CFBundleDisplayName: macosApp.name,
    CFBundleShortVersionString: version,
    CFBundleVersion: "1",
    CFBundlePackageType: "APPL",
    CFBundleIconFile: "icon.icns",
    LSMinimumSystemVersion: macosApp.minimumSystemVersion,
  };
  await mkdir(join(contents, "Resources"), { recursive: true });
  const fields = Object.entries(strings)
    .map(([k, v]) => `<key>${k}</key><string>${xml(v)}</string>`)
    .join("\n");
  await writeFile(
    join(contents, "Info.plist"),
    `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>${fields}
<key>NSHighResolutionCapable</key><true/></dict></plist>\n`,
  );
  await copyFile(
    join(repository, "apps/workbench/icons/icon.icns"),
    join(contents, "Resources/icon.icns"),
  );
}

const versionNumber = (text) => {
  const [major, minor = 0, patch = 0] = text.split(".").map(Number);
  return major * 65536 + minor * 256 + patch;
};
const versionText = (v) => `${v >>> 16}.${(v >>> 8) & 255}.${v & 255}`;

// Independent byte inspection: no target executable or platform-specific otool is run.
export function inspectMachO(header, commands, fileSize, minimum = macosApp.minimumSystemVersion) {
  if (header.length !== 32 || header.readUInt32LE(0) !== 0xfeedfacf)
    throw new Error("expected a thin little-endian 64-bit Mach-O executable");
  if (header.readUInt32LE(4) !== 0x0100000c || (header.readUInt32LE(8) & 0xffffff) !== 0)
    throw new Error("expected ARM64 Mach-O (not Intel or arm64e)");
  if (header.readUInt32LE(12) !== 2) throw new Error("Mach-O is not an executable");
  const count = header.readUInt32LE(16),
    size = header.readUInt32LE(20);
  if (size !== commands.length || size > fileSize - 32 || count > size / 8)
    throw new Error("invalid Mach-O load-command bounds");
  const dylibs = [],
    rpaths = [],
    versions = [];
  let offset = 0;
  const stringAt = (start, end) => {
    const zero = commands.indexOf(0, start);
    if (zero < start || zero >= end) throw new Error("unterminated Mach-O load string");
    return new TextDecoder("utf-8", { fatal: true }).decode(commands.subarray(start, zero));
  };
  for (let i = 0; i < count; i++) {
    if (offset + 8 > size) throw new Error("truncated Mach-O load command");
    const kind = commands.readUInt32LE(offset),
      bytes = commands.readUInt32LE(offset + 4),
      end = offset + bytes;
    if (bytes < 8 || bytes % 8 !== 0 || end > size)
      throw new Error("invalid Mach-O load-command size");
    if ([0xc, 0x80000018, 0x8000001f, 0x20, 0x80000023].includes(kind)) {
      if (bytes < 24) throw new Error("truncated Mach-O dylib command");
      const start = commands.readUInt32LE(offset + 8);
      if (start < 24 || start >= bytes) throw new Error("invalid Mach-O dylib string offset");
      const name = stringAt(offset + start, end);
      if (!name.startsWith("/System/Library/Frameworks/") && !name.startsWith("/usr/lib/"))
        throw new Error(`non-system dylib requires explicit bundling: ${name}`);
      if (name.split("/").includes("..")) throw new Error("noncanonical dylib path");
      dylibs.push(name);
    } else if (kind === 0x8000001c) {
      if (bytes < 16) throw new Error("truncated Mach-O rpath command");
      const start = commands.readUInt32LE(offset + 8);
      if (start < 12 || start >= bytes) throw new Error("invalid Mach-O rpath offset");
      const path = stringAt(offset + start, end);
      if (!/^@(executable_path|loader_path)(\/[^.][^/]*)*$/.test(path))
        throw new Error(`developer/external rpath: ${path}`);
      rpaths.push(path);
    } else if (kind === 0x32) {
      if (
        bytes < 24 ||
        commands.readUInt32LE(offset + 8) !== 1 ||
        24 + 8 * commands.readUInt32LE(offset + 20) > bytes
      )
        throw new Error("invalid macOS build-version command");
      versions.push(commands.readUInt32LE(offset + 12));
    } else if (kind === 0x24) {
      if (bytes < 16) throw new Error("truncated macOS minimum-version command");
      versions.push(commands.readUInt32LE(offset + 8));
    }
    offset = end;
  }
  if (offset !== size || versions.length !== 1)
    throw new Error("missing, duplicate or inconsistent Mach-O deployment metadata");
  if (versions[0] > versionNumber(minimum))
    throw new Error(
      `binary requires macOS ${versionText(versions[0])}; manifest promises ${minimum}`,
    );
  return { architecture: "arm64", minimumSystemVersion: versionText(versions[0]), dylibs, rpaths };
}

export async function auditMachO(path) {
  const file = await open(path, "r");
  try {
    const header = Buffer.alloc(32);
    if (
      (await file.read(header, 0, 32, 0)).bytesRead !== 32 ||
      header.readUInt32LE(0) !== 0xfeedfacf
    )
      throw new Error(`${path}: expected thin 64-bit Mach-O`);
    const size = (await file.stat()).size,
      commandSize = header.readUInt32LE(20);
    if (commandSize > size - 32) throw new Error("Mach-O commands extend beyond file");
    const commands = Buffer.alloc(commandSize);
    if ((await file.read(commands, 0, commandSize, 32)).bytesRead !== commandSize)
      throw new Error("truncated Mach-O command read");
    return {
      ...inspectMachO(header, commands, size),
      sha256: await sha256(path),
    };
  } finally {
    await file.close();
  }
}

export async function bundleMacosDevelopment(binaries, destination, version, provenance = {}) {
  if (!destination.endsWith(".app")) throw new Error("destination must be a new .app directory");
  try {
    await stat(destination);
    throw new Error("destination already exists");
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  const audits = {};
  for (const name of macosApp.binaries) audits[name] = await auditMachO(join(binaries, name));
  // Audit all helpers before creating a bundle; never package a Linux engine beside a Mac UI.
  await mkdir(destination);
  try {
    const contents = join(destination, "Contents");
    await mkdir(join(contents, "MacOS"), { recursive: true });
    await writeMacosMetadata(contents, version);
    for (const name of macosApp.binaries) {
      const path = join(contents, "MacOS", name);
      await copyFile(join(binaries, name), path);
      await chmod(path, 0o755);
      if ((await sha256(path)) !== audits[name].sha256)
        throw new Error(`binary changed during bundle assembly: ${name}`);
    }
    for (const name of ["LICENSE.md", "NOTICE.md"])
      await copyFile(join(repository, name), join(contents, "Resources", name));
    await copyFile(
      join(repository, "apps/workbench/icons/icon.icns.license"),
      join(contents, "Resources/icon.icns.license"),
    );
    const receipt = {
      purpose: "local development only; unsigned, not a release distribution",
      ...provenance,
      target: macosApp.target,
      deploymentTarget: macosApp.minimumSystemVersion,
      binaries: audits,
    };
    await writeFile(
      join(contents, "Resources/development-build.json"),
      JSON.stringify(receipt, null, 2) + "\n",
    );
    return receipt;
  } catch (error) {
    await rm(destination, { recursive: true, force: true });
    throw error;
  }
}

async function existingAbsolute(path, directory = false) {
  if (typeof path !== "string" || !isAbsolute(path))
    throw new Error("toolchain paths must be absolute");
  const info = await stat(path);
  if (directory ? !info.isDirectory() : !info.isFile())
    throw new Error(`invalid toolchain path: ${path}`);
  return path;
}

export async function crossEnvironment(configPath) {
  const config = JSON.parse(await readFile(configPath, "utf8"));
  if (
    config.target !== macosApp.target ||
    !/^[a-f0-9]{40}$/.test(config.osxcrossRevision ?? "") ||
    !/^[a-f0-9]{64}$/.test(config.sdkArchiveSha256 ?? "")
  )
    throw new Error(
      "toolchain must record ARM64 target, exact OSXCross revision and private SDK archive SHA-256",
    );
  const sdk = await existingAbsolute(config.sdk, true);
  const sdkSettings = JSON.parse(await readFile(join(sdk, "SDKSettings.json"), "utf8"));
  if (!config.sdkVersion || sdkSettings.Version !== config.sdkVersion)
    throw new Error("SDK version differs from toolchain receipt");
  const env = { SDKROOT: sdk, MACOSX_DEPLOYMENT_TARGET: macosApp.minimumSystemVersion };
  for (const [variable, key] of [
    ["CC", "cc"],
    ["CXX", "cxx"],
    ["AR", "ar"],
    ["RANLIB", "ranlib"],
  ])
    env[`${variable}_aarch64_apple_darwin`] = await existingAbsolute(config[key]);
  env.CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER = env.CC_aarch64_apple_darwin;
  env.TRACEL_LLVM_TARGET_PREFIX = await existingAbsolute(config.llvmPrefix, true);
  env.TRACEL_LLVM_TARGET_CONFIG = await existingAbsolute(config.llvmConfig, true);
  if (
    (await readFile(join(config.llvmConfig, "target.txt"), "utf8")).trim() !== macosApp.target ||
    (await readFile(join(config.llvmConfig, "version.txt"), "utf8")).trim() !== macosApp.llvmVersion
  )
    throw new Error("target LLVM query capture target/version mismatch");
  for (const name of queries) await readFile(join(config.llvmConfig, `${name}.txt`), "utf8");
  await existingAbsolute(join(config.llvmPrefix, "include/llvm-c/Target.h"));
  await existingAbsolute(join(config.llvmPrefix, "lib"), true);
  return {
    env,
    config,
    sdkSettingsSha256: await sha256(join(sdk, "SDKSettings.json")),
    configSha256: await sha256(configPath),
  };
}

async function captureLlvm(prefix, output, run) {
  if (process.platform !== "darwin" || process.arch !== "arm64")
    throw new Error(
      "capture target LLVM queries on an ARM64 Mac, then copy its LLVM prefix and capture to Linux",
    );
  prefix = await existingAbsolute(resolve(prefix), true);
  const config = join(prefix, "bin/llvm-config");
  const values = {};
  for (const query of queries)
    values[query] = (
      await run(config, ["--link-static", `--${query}`], { capture: true })
    ).replaceAll(prefix, "${LLVM_PREFIX}");
  if (
    values.version.trim() !== macosApp.llvmVersion ||
    values.libdir.trim() !== "${LLVM_PREFIX}/lib" ||
    values.includedir.trim() !== "${LLVM_PREFIX}/include"
  )
    throw new Error("LLVM capture must use the matching self-contained Tracel bundle");
  await mkdir(output); // Capture is immutable; never overwrite an earlier receipt.
  try {
    for (const [name, text] of Object.entries({ ...values, target: macosApp.target }))
      await writeFile(join(output, `${name}.txt`), text.trim() + "\n");
    await writeFile(
      join(output, "capture.json"),
      JSON.stringify(
        {
          target: macosApp.target,
          llvmVersion: macosApp.llvmVersion,
          llvmConfigSha256: await sha256(config),
          capturedOn: await run("sw_vers", [], { capture: true }),
        },
        null,
        2,
      ) + "\n",
    );
  } catch (error) {
    await rm(output, { recursive: true, force: true });
    throw error;
  }
}

export async function macosCommand(args, { run, root = repository, cargoTarget }) {
  const [command = "doctor", ...flags] = args;
  const options = {};
  for (let i = 0; i < flags.length; i += 2) {
    if (
      !["--toolchain", "--output", "--prefix"].includes(flags[i]) ||
      !flags[i + 1] ||
      flags[i + 1].startsWith("--") ||
      flags[i] in options
    )
      throw new Error(
        "usage: pnpm macos doctor|check-core|build|bundle|capture-llvm [--toolchain FILE] [--output NEW_PATH] [--prefix LLVM_PREFIX]",
      );
    options[flags[i]] = flags[i + 1];
  }
  if (command === "capture-llvm") {
    const prefix =
      options["--prefix"] ??
      join(
        homedir(),
        "Library/Application Support/tracel",
        `tracel-llvm-${macosApp.llvmBundleVersion}`,
      );
    if (!options["--output"]) throw new Error("capture-llvm requires --output NEW_DIRECTORY");
    await captureLlvm(prefix, resolve(options["--output"]), run);
    return;
  }
  if (command === "check-core") {
    await run("cargo", [
      "check",
      "--locked",
      "--target",
      macosApp.target,
      "-p",
      "spiling-model-import",
      "-p",
      "spiling-contracts",
    ]);
    return;
  }
  if (!["doctor", "build", "bundle"].includes(command))
    throw new Error(`unknown macOS command: ${command}`);
  const host = await run("rustc", ["-vV"], { capture: true });
  const rustHost = /^host: (.+)$/m.exec(host)?.[1];
  const installed = await run("rustup", ["target", "list", "--installed"], { capture: true });
  if (!installed.split(/\s+/).includes(macosApp.target))
    throw new Error(`install pinned Rust target: rustup target add ${macosApp.target}`);
  let toolchain;
  if (process.platform === "linux") {
    const path = options["--toolchain"] ?? process.env.SPILING_MACOS_TOOLCHAIN;
    if (!path)
      throw new Error(
        "Linux cross-build needs --toolchain PRIVATE_TOOLCHAIN.json (Apple SDK, OSXCross and target LLVM); see tools/macos/README.md",
      );
    toolchain = await crossEnvironment(resolve(path));
    toolchain.compiler = await run(toolchain.env.CC_aarch64_apple_darwin, ["--version"], {
      capture: true,
      env: toolchain.env,
    });
    const machine = await run(toolchain.env.CC_aarch64_apple_darwin, ["-dumpmachine"], {
      capture: true,
      env: toolchain.env,
    });
    if (!/^(aarch64|arm64)-apple-darwin/.test(machine))
      throw new Error("cross compiler is not an ARM64 Apple driver");
  } else if (process.platform === "darwin" && rustHost === macosApp.target) {
    const sdk = await run("xcrun", ["--sdk", "macosx", "--show-sdk-path"], { capture: true });
    await existingAbsolute(sdk, true);
    toolchain = {
      env: { SDKROOT: sdk, MACOSX_DEPLOYMENT_TARGET: macosApp.minimumSystemVersion },
      sdkVersion: await run("xcrun", ["--sdk", "macosx", "--show-sdk-version"], { capture: true }),
    };
  } else
    throw new Error(
      "initial macOS build path supports Linux cross-builds or native Apple Silicon Macs",
    );
  const receipt = {
    host: rustHost,
    rust: host,
    toolchain,
    source: await run("git", ["rev-parse", "HEAD"], { capture: true }),
    dirty: Boolean(await run("git", ["status", "--porcelain"], { capture: true })),
    cargoLockSha256: await sha256(join(root, "Cargo.lock")),
    profile: "dev",
  };
  if (command === "doctor") {
    console.log(JSON.stringify(receipt, null, 2));
    return;
  }
  await run(
    "cargo",
    [
      "build",
      "--locked",
      "--target",
      macosApp.target,
      ...macosApp.binaries.flatMap((name) => ["-p", name]),
    ],
    { env: toolchain.env },
  );
  const binaries = join(cargoTarget, macosApp.target, "debug");
  for (const name of macosApp.binaries) await auditMachO(join(binaries, name));
  if (command === "bundle") {
    const version = /^version = "([^"]+)"$/m.exec(
      await readFile(join(root, "Cargo.toml"), "utf8"),
    )?.[1];
    const output = resolve(options["--output"] ?? join(root, "dist/Spiling-dev.app"));
    await mkdir(dirname(output), { recursive: true });
    const audit = await bundleMacosDevelopment(binaries, output, version, receipt);
    console.log(JSON.stringify({ app: output, ...audit }, null, 2));
  }
}
