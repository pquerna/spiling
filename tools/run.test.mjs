/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import test from "node:test";
import { run, runTestSuites, toolingTests } from "./run.mjs";
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

test("optional tooling continues independent observers after authoring fails", async (context) => {
  context.mock.method(console, "error", () => {});
  const visits = [];
  await assert.rejects(
    toolingTests(async (command, args) => {
      visits.push({ command, args });
      if (args.includes("tools/generate-hardware-catalog.mjs")) throw new Error("authoring drift");
    }),
    /authoring drift/,
  );
  assert.ok(visits.some(({ args }) => args.includes("tools/smoke-cli.mjs")));
  assert.ok(visits.some(({ args }) => args.includes("@spiling/protocol")));
  assert.ok(visits.some(({ args }) => args.includes("tools/gcode-parity/test.mjs")));
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
