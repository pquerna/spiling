/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, writeFile, stat } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { execFileSync } from "node:child_process";
import {
  inspectMachO,
  auditMachO,
  bundleMacosDevelopment,
  crossEnvironment,
  macosApp,
  macosCommand,
} from "./macos.mjs";

function executable({
  cpu = 0x0100000c,
  minimum = 13 << 16,
  dylib = "/usr/lib/libSystem.B.dylib",
} = {}) {
  const name = Buffer.from(dylib + "\0");
  const dylibSize = Math.ceil((24 + name.length) / 8) * 8;
  const commands = Buffer.alloc(24 + dylibSize);
  commands.writeUInt32LE(0x32, 0);
  commands.writeUInt32LE(24, 4);
  commands.writeUInt32LE(1, 8);
  commands.writeUInt32LE(minimum, 12);
  commands.writeUInt32LE(0xc, 24);
  commands.writeUInt32LE(dylibSize, 28);
  commands.writeUInt32LE(24, 32);
  name.copy(commands, 48);
  const header = Buffer.alloc(32);
  header.writeUInt32LE(0xfeedfacf, 0);
  header.writeUInt32LE(cpu, 4);
  header.writeUInt32LE(2, 12);
  header.writeUInt32LE(2, 16);
  header.writeUInt32LE(commands.length, 20);
  return Buffer.concat([header, commands]);
}
const inspect = (bytes) => inspectMachO(bytes.subarray(0, 32), bytes.subarray(32), bytes.length);

test("Mach-O audit independently rejects wrong OS/CPU, newer OS and developer dylibs", () => {
  assert.equal(inspect(executable()).architecture, "arm64");
  assert.throws(() => inspect(Buffer.alloc(40)), /Mach-O/);
  assert.throws(() => inspect(executable({ cpu: 0x01000007 })), /ARM64/);
  assert.throws(() => inspect(executable({ minimum: 14 << 16 })), /requires macOS/);
  for (const dylib of [
    "/opt/homebrew/lib/libzstd.dylib",
    "@rpath/libfoo.dylib",
    "/usr/lib/../../tmp/libfake.dylib",
  ])
    assert.throws(() => inspect(executable({ dylib })), /dylib|noncanonical/);
});

test("Mach-O audit rejects truncated, malformed and ambiguous load commands", () => {
  for (const mutate of [
    (b) => b.writeUInt32LE(0xffffffff, 20),
    (b) => b.writeUInt32LE(7, 36),
    (b) => b.writeUInt32LE(0xffffffff, 64),
    (b) => b.fill(65, 80),
    (b) => b.writeUInt32LE(0, 32),
  ]) {
    const bytes = executable();
    mutate(bytes);
    assert.throws(() => inspect(bytes));
  }
});

test("development bundle carries all audited helpers, metadata, icon and permissions through a real archive", async () => {
  const temp = await mkdtemp(join(tmpdir(), "spiling-macos-bundle-"));
  try {
    const binaries = join(temp, "bins");
    await mkdir(binaries);
    for (const name of macosApp.binaries) await writeFile(join(binaries, name), executable());
    const app = join(temp, "Spiling.app");
    const receipt = await bundleMacosDevelopment(binaries, app, "0.1.0", {
      source: "fixture, not compiled native acceptance",
    });
    assert.equal(Object.keys(receipt.binaries).length, 3);
    const plist = await readFile(join(app, "Contents/Info.plist"), "utf8");
    assert.match(plist, /org.spiling.workbench/);
    assert.match(plist, /LSMinimumSystemVersion/);
    assert.match(plist, /<string>13.0<\/string>/);
    assert.ok((await stat(join(app, "Contents/Resources/icon.icns"))).size > 0);
    const archive = join(temp, "app.tar.gz");
    execFileSync("tar", ["-czf", archive, "-C", temp, "Spiling.app"]);
    const extracted = join(temp, "extracted");
    await mkdir(extracted);
    execFileSync("tar", ["-xzf", archive, "-C", extracted]);
    for (const name of macosApp.binaries) {
      const file = join(extracted, "Spiling.app/Contents/MacOS", name);
      if (process.platform !== "win32") assert.equal((await stat(file)).mode & 0o777, 0o755);
      assert.equal((await auditMachO(file)).sha256, receipt.binaries[name].sha256);
    }
    await assert.rejects(bundleMacosDevelopment(binaries, app, "0.1.0"), /already exists/);
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
});

test("a missing or foreign helper cannot produce a superficially successful app", async () => {
  const temp = await mkdtemp(join(tmpdir(), "spiling-macos-helper-"));
  try {
    await writeFile(join(temp, "spiling-workbench"), executable());
    const app = join(temp, "Spiling.app");
    await assert.rejects(bundleMacosDevelopment(temp, app, "0.1.0"));
    await assert.rejects(stat(app), { code: "ENOENT" });
    await writeFile(join(temp, "spiling-engine"), "ELF Linux engine");
    await writeFile(join(temp, "spiling-cli"), executable());
    await assert.rejects(bundleMacosDevelopment(temp, app, "0.1.0"), /Mach-O/);
    await assert.rejects(stat(app), { code: "ENOENT" });
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
});

test("cross environment scopes C tools to the target and rejects stale SDK/LLVM captures", async () => {
  const temp = await mkdtemp(join(tmpdir(), "spiling-macos-env-"));
  try {
    await mkdir(join(temp, "sdk"));
    await writeFile(join(temp, "sdk/SDKSettings.json"), '{"Version":"26.0"}');
    await mkdir(join(temp, "llvm/include/llvm-c"), { recursive: true });
    await writeFile(join(temp, "llvm/include/llvm-c/Target.h"), "fixture");
    await mkdir(join(temp, "llvm/lib"));
    await mkdir(join(temp, "queries"));
    for (const name of [
      "libs",
      "libnames",
      "system-libs",
      "libdir",
      "includedir",
      "version",
      "cflags",
      "cxxflags",
      "target",
    ])
      await writeFile(
        join(temp, "queries", name + ".txt"),
        name === "version" ? macosApp.llvmVersion : name === "target" ? macosApp.target : "fixture",
      );
    await writeFile(join(temp, "clang"), "fixture tool");
    const config = {
      target: macosApp.target,
      osxcrossRevision: "1".repeat(40),
      sdkArchiveSha256: "2".repeat(64),
      sdkVersion: "26.0",
      sdk: join(temp, "sdk"),
      llvmPrefix: join(temp, "llvm"),
      llvmConfig: join(temp, "queries"),
      ...Object.fromEntries(["cc", "cxx", "ar", "ranlib"].map((k) => [k, join(temp, "clang")])),
    };
    const path = join(temp, "config.json");
    await writeFile(path, JSON.stringify(config));
    const { env } = await crossEnvironment(path);
    assert.equal(env.CC_aarch64_apple_darwin, config.cc);
    assert.equal(env.CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER, config.cc);
    for (const name of ["CC", "CXX", "AR", "PKG_CONFIG_ALLOW_CROSS", "RUSTFLAGS"])
      assert.equal(env[name], undefined);
    await writeFile(join(temp, "queries/target.txt"), "x86_64-apple-darwin");
    await assert.rejects(crossEnvironment(path), /target\/version mismatch/);
    await writeFile(join(temp, "queries/target.txt"), macosApp.target);
    config.sdkVersion = "25.0";
    await writeFile(path, JSON.stringify(config));
    await assert.rejects(crossEnvironment(path), /SDK version/);
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
});

test("SDK-free check explicitly selects portable core crates and never tries to run Darwin binaries", async () => {
  const calls = [];
  await macosCommand(["check-core"], { run: async (...args) => calls.push(args) });
  assert.deepEqual(calls[0][1], [
    "check",
    "--locked",
    "--target",
    "aarch64-apple-darwin",
    "-p",
    "spiling-model-import",
    "-p",
    "spiling-contracts",
  ]);
  assert.equal(calls.length, 1);
});
