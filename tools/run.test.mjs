/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import test from "node:test";
import { copyNotices, run, runTestSuites, workspaceTests } from "./run.mjs";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

test("native Zstandard sources retain separate upstream notices", async () => {
  const temporary = await mkdtemp(join(tmpdir(), "spiling-native-notices-"));
  try {
    const source = join(temporary, "zstd-sys-test");
    const output = join(temporary, "notices");
    await mkdir(join(source, "zstd", "lib", "common"), { recursive: true });
    await mkdir(join(source, "zstd", "lib", "dictBuilder"), { recursive: true });
    await writeFile(join(source, "LICENSE"), "Rust binding license\n");
    await writeFile(join(source, "zstd", "LICENSE"), "Native license\n");
    await writeFile(join(source, "zstd", "COPYING"), "Alternative upstream license\n");
    const hashNotice = "/* Copyright original hash author; upstream license reference. */";
    const sortNotice = "/* Copyright original sort author; complete upstream license. */";
    await writeFile(join(source, "zstd", "lib", "common", "xxhash.h"), `${hashNotice}\ncode`);
    await writeFile(
      join(source, "zstd", "lib", "dictBuilder", "divsufsort.c"),
      `${sortNotice}\ncode`,
    );
    await copyNotices(source, output);
    assert.equal(await readFile(join(output, "LICENSE"), "utf8"), "Rust binding license\n");
    assert.equal(await readFile(join(output, "zstd", "LICENSE"), "utf8"), "Native license\n");
    assert.equal(
      await readFile(join(output, "zstd", "COPYING"), "utf8"),
      "Alternative upstream license\n",
    );
    assert.equal(
      await readFile(join(output, "zstd", "xxhash.h.notice"), "utf8"),
      `${hashNotice}\n`,
    );
    assert.equal(
      await readFile(join(output, "zstd", "divsufsort.c.notice"), "utf8"),
      `${sortNotice}\n`,
    );
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
});

test("independent suites finish sequentially", async () => {
  const visits = [];
  await runTestSuites([
    [
      "first",
      async () => {
        visits.push("first start");
        await new Promise(setImmediate);
        visits.push("first end");
      },
    ],
    [
      "second",
      async () => {
        visits.push("second");
      },
    ],
  ]);
  assert.deepEqual(visits, ["first start", "first end", "second"]);
});

test("workspace gate prepares binaries and contracts once and retains independent consumer failures", async (context) => {
  context.mock.method(console, "error", () => {});
  for (const failingAction of ["test", "run"]) {
    const calls = [];
    const suiteFailure = new Error(`${failingAction} failed`);
    await assert.rejects(
      workspaceTests(async (command, args) => {
        calls.push({ command, args });
        if (
          command === "cargo" &&
          args[0] === failingAction &&
          (failingAction !== "test" || args.includes("--workspace"))
        )
          throw suiteFailure;
      }),
      (error) => {
        assert.deepEqual(error.errors, [suiteFailure]);
        return true;
      },
    );
    const count = (command, predicate) =>
      calls.filter((c) => c.command === command && predicate(c.args)).length;
    assert.equal(
      count("cargo", (args) => args[0] === "build"),
      1,
    );
    assert.equal(
      count("cargo", (args) => args.includes("generate") && args.includes("--check")),
      1,
    );
    assert.equal(
      count("node", (args) => args.includes("tools/generate-hardware-catalog.mjs")),
      failingAction === "run" ? 0 : 1,
    );
    assert.equal(
      count("node", (args) => args.includes("tools/smoke-cli.mjs")),
      1,
    );
    assert.equal(
      count("pnpm", (args) => args.includes("@spiling/protocol")),
      1,
    );
    assert.equal(
      count("node", (args) => args.includes("tools/gcode-parity/test.mjs")),
      1,
    );
    for (const name of [
      "tests::real::manufacturing_bundle_survives_save_restart_and_source_removal",
      "manufacturing_fresh_process_replay_export_invalidation_and_undo",
    ]) {
      assert.equal(
        count(
          "cargo",
          (args) => args.includes(name) && args.includes("--ignored") && args.includes("--exact"),
        ),
        1,
      );
    }
    assert.equal(calls[0].command, "cargo");
    assert.equal(calls[0].args[0], "build");
  }
});

test("every failed suite remains fatal after later suites run", async (context) => {
  context.mock.method(console, "error", () => {});
  const visits = [];
  const first = new Error("first failure");
  const last = new Error("last failure");
  await assert.rejects(
    runTestSuites([
      [
        "first",
        () => {
          visits.push("first");
          throw first;
        },
      ],
      [
        "middle",
        async () => {
          visits.push("middle");
        },
      ],
      [
        "last",
        async () => {
          visits.push("last");
          throw last;
        },
      ],
    ]),
    (error) => {
      assert.ok(error instanceof AggregateError);
      assert.deepEqual(error.errors, [first, last]);
      assert.match(error.message, /first: first failure/);
      assert.match(error.message, /last: last failure/);
      return true;
    },
  );
  assert.deepEqual(visits, ["first", "middle", "last"]);
});

test("child failures retain their exit status in the suite summary", async (context) => {
  context.mock.method(console, "error", () => {});
  await assert.rejects(
    runTestSuites([["child", () => run(process.execPath, ["-e", "process.exit(7)"])]]),
    (error) => {
      assert.match(error.message, /child: .* exited 7/);
      assert.equal(error.errors.length, 1);
      return true;
    },
  );
});

test("operator interruption never launches the next suite", { timeout: 10_000 }, async () => {
  const child = spawn(
    process.execPath,
    [
      "--input-type=module",
      "-e",
      `
      import { run, runTestSuites } from ${JSON.stringify(new URL("./run.mjs", import.meta.url).href)};
      try {
        await runTestSuites([
          ["interrupted", () => {
            setImmediate(() => process.emit("SIGTERM"));
            return run(process.execPath, ["-e", "setTimeout(() => {}, 5000)"]);
          }],
          ["must not run", async () => { console.log("NEXT_SUITE_STARTED"); }],
        ]);
      } catch (error) {
        console.error(error.message);
        process.exitCode = 1;
      }
    `,
    ],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  let output = "";
  child.stdout.on("data", (chunk) => {
    output += chunk;
  });
  child.stderr.on("data", (chunk) => {
    output += chunk;
  });
  try {
    const [code, signal] = await once(child, "close");
    assert.equal(code, 1, output);
    assert.equal(signal, null, output);
    assert.match(output, /interrupted/);
    assert.doesNotMatch(output, /NEXT_SUITE_STARTED/);
  } finally {
    child.kill("SIGKILL");
  }
});
