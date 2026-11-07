/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Shared bounded invocation for actual CLI/engine journeys. Callers supply an
// externally constrained process scope and own the consumer assertions.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

export function projectSmoke({ root, target, output, evidence, timeoutMs = 300_000 }) {
  const suffix = process.platform === "win32" ? ".exe" : "";
  const cli = join(target, "debug", `spiling-cli${suffix}`);
  const engine = join(target, "debug", `spiling-engine${suffix}`);
  return async function invoke(name, args, expectedFailure = false) {
    const command = ["project", ...args, "--engine", engine];
    let stdout = "", stderr = "", size = 0, timedOut = false, outputExceeded = false;
    const started = performance.now();
    const code = await new Promise((done, fail) => {
      const child = spawn(cli, command, { cwd: root, env: { ...process.env, RAYON_NUM_THREADS: "1" } });
      const timer = setTimeout(() => { timedOut = true; child.kill("SIGKILL"); }, timeoutMs);
      for (const [stream, key] of [[child.stdout, "out"], [child.stderr, "err"]]) {
        stream.setEncoding("utf8").on("data", (text) => {
          size += Buffer.byteLength(text);
          if (size > 16 * 1024 * 1024) { outputExceeded = true; child.kill("SIGKILL"); }
          else if (key === "out") stdout += text;
          else stderr += text;
        });
      }
      child.once("error", (error) => { clearTimeout(timer); fail(error); });
      child.once("close", (code) => { clearTimeout(timer); done(code); });
    });
    await writeFile(join(output, `${name}.stdout.json`), stdout);
    await writeFile(join(output, `${name}.stderr.txt`), stderr);
    evidence.commands.push({ name, command, code, timed_out: timedOut, output_exceeded: outputExceeded, elapsed_ms: performance.now() - started });
    assert.ok(!timedOut && !outputExceeded, `${name}: smoke harness bound exceeded; not a domain refusal`);
    assert.ok(Number.isInteger(code), `${name}: child ended by signal; not a domain refusal`);
    assert.equal(code === 0, !expectedFailure, `${name}: ${stderr.slice(-4000)}`);
    if (expectedFailure) return stderr;
    const report = JSON.parse(stdout);
    evidence.engine_build ??= report.hello.engine_build;
    assert.equal(report.hello.engine_build, evidence.engine_build);
    return report;
  };
}

export async function cgroupPeakBytes() {
  if (process.platform !== "linux") return null;
  const line = (await readFile("/proc/self/cgroup", "utf8")).split("\n").find((line) => line.startsWith("0::"));
  if (!line) return null;
  return Number((await readFile(join("/sys/fs/cgroup", line.slice(3), "memory.peak"), "utf8")).trim());
}
