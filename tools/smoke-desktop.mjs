/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : fallback;
};
const executable = option("--executable");
const port = Number(option("--port", "9222"));
const scenario = option("--scenario", "normal");
if (!executable)
  throw new Error(
    "Usage: pnpm smoke:desktop --executable PATH [--port 9222] [--scenario normal|gpu-unavailable|engine-operations]; provide DISPLAY on Linux or launch under xvfb-run",
  );
if (!["normal", "gpu-unavailable", "engine-operations"].includes(scenario))
  throw new Error("Unknown smoke scenario");
const child = spawn(resolve(executable), [], {
  env: {
    ...process.env,
    SPILING_CEF_DEBUG_PORT: String(port),
  },
  stdio: ["ignore", "inherit", "inherit"],
});
let childExited = false;
let launchError;
child.once("error", (error) => {
  launchError = error;
});
child.once("exit", () => {
  childExited = true;
});
let socket;
let nextId = 0;
const pending = new Map();
let enginePid;
const deadline = Date.now() + 90000;
async function until(action, description, timeoutMs = 20000) {
  const end = Date.now() + timeoutMs;
  while (Date.now() < end) {
    const result = await action();
    if (result) return result;
    if (launchError) throw launchError;
    if (childExited) throw new Error(`Desktop exited before ${description}`);
    await delay(150);
  }
  throw new Error(`Timed out: ${description}`);
}
function cdp(method, params = {}) {
  const id = ++nextId;
  return new Promise((resolveResult, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`CDP timeout: ${method}`));
    }, 15000);
    pending.set(id, { resolve: resolveResult, reject, timer });
    socket.send(JSON.stringify({ id, method, params }));
  });
}
async function evaluate(expression) {
  const response = await cdp("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (response.exceptionDetails)
    throw new Error(
      response.exceptionDetails.text +
        ": " +
        (response.exceptionDetails.exception?.description ?? ""),
    );
  return response.result?.value;
}
async function text(id) {
  return evaluate(`document.querySelector('[data-testid="${id}"]')?.textContent ?? ''`);
}
async function click(id) {
  assert.equal(
    await evaluate(`!!document.querySelector('[data-testid="${id}"]')`),
    true,
    `Missing ${id}`,
  );
  await evaluate(`document.querySelector('[data-testid="${id}"]').click()`);
}
async function screenshot(name) {
  await mkdir("artifacts", { recursive: true });
  const capture = await cdp("Page.captureScreenshot", { format: "png" });
  await writeFile(`artifacts/${name}.png`, Buffer.from(capture.data, "base64"));
}
function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error.code === "ESRCH") return false;
    throw error;
  }
}
try {
  const target = await until(
    async () => {
      try {
        const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
        return targets.find((item) => item.type === "page" && item.url !== "about:blank");
      } catch {
        return undefined;
      }
    },
    "CEF debug target",
    Math.max(1, deadline - Date.now()),
  );
  socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolveResult, reject) => {
    socket.addEventListener("open", resolveResult, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    const waiter = pending.get(message.id);
    if (!waiter) return;
    pending.delete(message.id);
    clearTimeout(waiter.timer);
    if (message.error) waiter.reject(new Error(message.error.message));
    else waiter.resolve(message.result);
  });
  await cdp("Page.enable");
  await cdp("Runtime.enable");
  if (scenario !== "normal") {
    await cdp("Page.addScriptToEvaluateOnNewDocument", {
      source: "Object.defineProperty(navigator, 'gpu', {value: undefined, configurable: true});",
    });
    await cdp("Page.reload");
  }
  await until(async () => (await evaluate("document.readyState")) === "complete", "frontend load");
  if (await evaluate("!!document.querySelector('[data-testid=\"license-assent\"]')")) {
    await screenshot(`b0-${scenario}-license`);
    await click("license-assent");
    await click("accept-license");
  }
  if (scenario === "gpu-unavailable") {
    const body = await until(async () => {
      const value = await evaluate("document.body.innerText");
      return /unsupported|unavailable|not available/i.test(value) && /webgpu/i.test(value)
        ? value
        : false;
    }, "unsupported GPU diagnostic");
    assert.ok(
      !/\bRunning\b/.test(await text("engine-status")),
      "No engine session on unsupported GPU",
    );
    const status = await evaluate('window.__TAURI_INTERNALS__.invoke("engine_status")');
    assert.equal(status.state, "stopped", "GPU admission must leave the native engine stopped");
    assert.equal(status.hello, null, "GPU admission must not negotiate an engine session");
    await screenshot("b0-gpu-unavailable");
    console.log(
      JSON.stringify(
        {
          result: "pass",
          scenario,
          diagnostic: body,
          nativeStatus: status,
          qualification:
            "GPU-unavailable branch injected before page startup; not hardware acceptance",
        },
        null,
        2,
      ),
    );
  } else if (scenario === "engine-operations") {
    // Explicit native bridge diagnostic, bypassing GPU admission; no rendering claim.
    await until(async () => /unsupported/i.test(await text("engine-status")), "GPU gate settled");
    const invoke = (command, parameters = {}) =>
      evaluate(
        `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(parameters)})`,
      );
    const hello = await invoke("engine_start");
    enginePid = hello.pid;
    const operation = await invoke("engine_run_diagnostic", { requestId: crypto.randomUUID() });
    const partial = await until(async () => {
      const current = await invoke("engine_get_operation", { name: operation.name });
      return current.outputs.length > 0 && !current.done ? current : false;
    }, "native partial output");
    const bytes = await evaluate(
      `window.__TAURI_INTERNALS__.invoke("engine_read_artifact", {name:${JSON.stringify(partial.outputs[0].name)}}).then(bytes => Array.from(new Uint8Array(bytes)))`,
    );
    assert.equal(bytes.length, 64);
    assert.deepEqual(bytes.slice(0, 4), [83, 80, 76, 84]);
    await invoke("engine_cancel_operation", { name: operation.name });
    const terminal = await until(async () => {
      const current = await invoke("engine_get_operation", { name: operation.name });
      return current.done ? current : false;
    }, "native terminal operation");
    assert.equal(terminal.state, "cancelled");
    assert.equal(terminal.error_code, 1);
    const concurrent = await invoke("engine_status");
    assert.equal(concurrent.state, "running", "cancelling work preserves engine");
    const interrupted = await invoke("engine_run_diagnostic", { requestId: crypto.randomUUID() });
    await invoke("engine_interrupt");
    await until(async () => !alive(enginePid), "interrupted child reaped");
    const restarted = await invoke("engine_restart");
    assert.notEqual(restarted.pid, enginePid);
    enginePid = restarted.pid;
    const recovered = await invoke("engine_get_operation", { name: interrupted.name });
    assert.equal(recovered.state, "interrupted");
    assert.equal(recovered.error_code, 10);
    console.log(
      JSON.stringify(
        {
          result: "pass",
          scenario,
          partial,
          terminal,
          recovered,
          qualification:
            "Real CEF invoke/job/binary/cancellation/restart path; explicit GPU admission bypass, no rendering or hardware acceptance",
        },
        null,
        2,
      ),
    );
  } else {
    const partialUi = await until(
      async () =>
        evaluate(`(() => {
      const p = document.querySelector('[data-testid="operation-progress"]');
      return p && p.dataset.done === 'false' && Number(p.dataset.completed) > 0;
    })()`),
      "UI partial operation progress",
    );
    assert.equal(partialUi, true);
    await until(
      async () =>
        evaluate(
          `document.querySelector('[data-testid="operation-progress"]')?.dataset.done === 'true'`,
        ),
      "UI terminal progress",
    );
    await until(async () => /running/i.test(await text("engine-status")), "engine running");
    enginePid = Number((await text("engine-pid")).match(/\d+/)?.[0]);
    assert.ok(Number.isSafeInteger(enginePid) && enginePid > 0, "Live engine PID");
    assert.ok(alive(enginePid));
    await until(async () => await evaluate("!!document.querySelector('canvas')"), "WebGPU canvas");
    const renderer = await evaluate(
      "({gpu:!!navigator.gpu, canvas:{width:document.querySelector('canvas').width,height:document.querySelector('canvas').height},text:document.body.innerText})",
    );
    assert.equal(renderer.gpu, true);
    assert.ok(renderer.canvas.width > 0 && renderer.canvas.height > 0);
    await screenshot("b0-running");
    process.kill(enginePid, "SIGKILL");
    await until(
      async () => /interrupted/i.test(await text("engine-status")),
      "external engine crash detected",
    );
    await screenshot("b0-interrupted");
    await click("restart-engine");
    await until(async () => /running/i.test(await text("engine-status")), "restart running");
    const restarted = Number((await text("engine-pid")).match(/\d+/)?.[0]);
    assert.notEqual(restarted, enginePid);
    assert.ok(alive(restarted));
    enginePid = restarted;
    await click("stop-engine");
    await until(async () => /stopped/i.test(await text("engine-status")), "intentional stop");
    await until(async () => !alive(enginePid), "stopped child exit");
    await click("restart-engine");
    await until(
      async () => /running/i.test(await text("engine-status")),
      "engine before native window close",
    );
    enginePid = Number((await text("engine-pid")).match(/\d+/)?.[0]);
    console.log(
      JSON.stringify(
        {
          result: "pass",
          scenario,
          renderer,
          restartedPid: restarted,
          finalEnginePid: enginePid,
          softwareGpuDiagnostic: process.env.SPILING_CEF_SOFTWARE_GPU === "1",
        },
        null,
        2,
      ),
    );
  }
  // Do not await an IPC promise whose page is deliberately being destroyed.
  await evaluate(
    '(window.__TAURI_INTERNALS__.invoke("plugin:window|close", {label:"main"}), true)',
  );
  const closedAt = Date.now();
  while (!childExited && Date.now() - closedAt < 15000) await delay(100);
  assert.equal(childExited, true, "Native window close terminates desktop");
  if (enginePid) assert.equal(alive(enginePid), false, "Native window close releases engine child");
  console.log("PASS: native window close exits application and engine");
} finally {
  for (const waiter of pending.values()) {
    clearTimeout(waiter.timer);
    waiter.reject(new Error("Smoke finished"));
  }
  socket?.close();
  if (!childExited) child.kill("SIGTERM");
}
