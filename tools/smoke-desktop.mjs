/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, relative, resolve } from "node:path";
import { tmpdir } from "node:os";
import { setTimeout as delay } from "node:timers/promises";
import { PROTOCOL_VERSION } from "../packages/protocol/src/generated.ts";
import { decodeMesh, validateMeshManifest } from "../packages/protocol/src/index.ts";

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : fallback;
};
const executable = option("--executable");
const port = Number(option("--port", "9222"));
const scenario = option("--scenario", "normal");
const sourceFixtureRoot = resolve(option("--fixture-root", "fixtures/geometry"));
const fixtureRoot =
  scenario === "project-bridge"
    ? await mkdtemp(resolve(tmpdir(), "spiling-authoring-cef-"))
    : sourceFixtureRoot;
if (scenario === "project-bridge") {
  for (const name of ["box-mm.step", "cylinder.step"])
    await copyFile(resolve(sourceFixtureRoot, name), resolve(fixtureRoot, name));
}
const scenePath = resolve(option("--scene", "fixtures/geometry/scenes/two-parts.scene.json"));
const recipe = scenario === "geometry" ? JSON.parse(await readFile(scenePath, "utf8")) : null;
if (!executable)
  throw new Error(
    "Usage: pnpm smoke:desktop --executable PATH [--port 9222] [--scenario normal|geometry|project-bridge|mismatch|gpu-unavailable] [--scene RECIPE] [--fixture-root DIRECTORY]; provide DISPLAY on Linux or launch under xvfb-run",
  );
if (!["normal", "geometry", "project-bridge", "mismatch", "gpu-unavailable"].includes(scenario))
  throw new Error("Unknown smoke scenario");
const child = spawn(resolve(executable), [], {
  env: {
    ...process.env,
    SPILING_CEF_DEBUG_PORT: String(port),
    ...(scenario === "mismatch" ? { SPILING_PROTOCOL_VERSION: String(PROTOCOL_VERSION + 1) } : {}),
    ...(["geometry", "project-bridge"].includes(scenario)
      ? { SPILING_GEOMETRY_FIXTURE_ROOT: fixtureRoot }
      : {}),
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
async function geometryState() {
  return JSON.parse(await text("geometry-state"));
}
async function input(selector, value) {
  await evaluate(`(() => {
    const element = document.querySelector(${JSON.stringify(selector)});
    if (!element) throw new Error("Missing input");
    const prototype = element.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, "value").set.call(element, ${JSON.stringify(String(value))});
    element.dispatchEvent(new Event("input", {bubbles:true}));
  })()`);
}
async function selectOccurrence(id) {
  await evaluate(`document.querySelector('[data-occurrence-id="${id}"]').click()`);
  await until(
    () => evaluate(`!!document.querySelector('[data-occurrence-id="${id}"][aria-pressed="true"]')`),
    "selected occurrence",
  );
}
async function applyPose(id, pose) {
  await selectOccurrence(id);
  const before = await geometryState();
  const [x, y, z, w] = pose.rotation_xyzw;
  const m13 = 2 * (x * z + w * y),
    m23 = 2 * (y * z - w * x),
    m33 = 1 - 2 * (x * x + y * y);
  const m12 = 2 * (x * y - w * z),
    m11 = 1 - 2 * (y * y + z * z);
  const m32 = 2 * (y * z + w * x),
    m22 = 1 - 2 * (x * x + z * z);
  const rotation =
    Math.abs(m13) < 0.9999999
      ? [Math.atan2(-m23, m33), Math.asin(Math.max(-1, Math.min(1, m13))), Math.atan2(-m12, m11)]
      : [Math.atan2(m32, m22), Math.asin(Math.max(-1, Math.min(1, m13))), 0];
  for (let axis = 0; axis < 3; axis++) {
    await input(`[aria-label="Translation ${"XYZ"[axis]}"]`, pose.translation_mm[axis]);
    await input(`[aria-label="Rotation ${"XYZ"[axis]}"]`, (rotation[axis] * 180) / Math.PI);
  }
  await click("apply-pose");
  const after = await until(async () => {
    const current = await geometryState();
    return current.scene.revision > before.scene.revision &&
      !current.active_job &&
      !current.transferring &&
      !current.stale
      ? current
      : false;
  }, "placed occurrence display");
  const actual = after.occurrences.find((row) => row.occurrence_id === id).pose;
  assert.deepEqual(actual.translation_mm, pose.translation_mm);
  assert.ok(
    Math.abs(
      Math.abs(actual.rotation_xyzw.reduce((sum, v, i) => sum + v * pose.rotation_xyzw[i], 0)) - 1,
    ) < 1e-10,
  );
  assert.equal(after.scene.unique_mesh_bytes, before.scene.unique_mesh_bytes);
  assert.equal(after.transfer.bytes, 0, "Placement reuses existing mesh buffers");
}
async function inspectGeometry() {
  const initial = await geometryState();
  assert.equal(initial.occurrences.length, 0, "Fresh native session has no synthetic scene");
  await input(
    '[data-testid="geometry-debug-paths"]',
    JSON.stringify(
      recipe.parts.map((part) => relative(fixtureRoot, resolve(dirname(scenePath), part.path))),
    ),
  );
  await click("geometry-debug-import");
  const imported = await until(
    async () => {
      const current = await geometryState();
      return current.occurrences.length === recipe.parts.length &&
        !current.active_job &&
        !current.transferring &&
        !current.stale
        ? current
        : false;
    },
    "native part import and GPU publication",
    65000,
  );
  const partIds = new Map(
    recipe.parts.map((part, index) => [part.key, imported.occurrences[index].occurrence_id]),
  );
  for (const part of recipe.parts) await applyPose(partIds.get(part.key), part.pose);
  for (const instance of recipe.instances) {
    await selectOccurrence(partIds.get(instance.part));
    const before = await geometryState();
    await click("add-instance");
    const after = await until(async () => {
      const current = await geometryState();
      return current.occurrences.length === before.occurrences.length + 1 &&
        !current.transferring &&
        !current.stale
        ? current
        : false;
    }, "shared definition instance");
    assert.equal(after.scene.unique_mesh_bytes, before.scene.unique_mesh_bytes);
    assert.equal(after.transfer.bytes, 0, "Repeated instance does not duplicate transfer");
    const added = after.occurrences.find(
      (row) => !before.occurrences.some((old) => old.occurrence_id === row.occurrence_id),
    );
    await applyPose(added.occurrence_id, instance.pose);
  }
  await selectOccurrence(partIds.get(recipe.parts[0].key));
  const bounds = await evaluate(
    `(() => { const r=document.querySelector('[data-testid="viewport"]').getBoundingClientRect(); return {x:r.x,y:r.y,width:r.width,height:r.height}; })()`,
  );
  let picked;
  for (const y of [0.5, 0.35, 0.65, 0.2, 0.8]) {
    for (const x of [0.5, 0.35, 0.65, 0.2, 0.8, 0.1, 0.9]) {
      const position = {
        x: bounds.x + bounds.width * x,
        y: bounds.y + bounds.height * y,
        button: "left",
        clickCount: 1,
      };
      await cdp("Input.dispatchMouseEvent", { ...position, type: "mousePressed" });
      await cdp("Input.dispatchMouseEvent", { ...position, type: "mouseReleased" });
      await delay(100);
      picked = (await geometryState()).inspection;
      if (picked) break;
    }
    if (picked) break;
  }
  assert.ok(picked, "Actual canvas click resolves a native source face");
  const placed = await geometryState();
  assert.ok(
    placed.occurrences.some(
      (row) =>
        row.occurrence_id === picked.reference.occurrence_id &&
        row.definition_id === picked.reference.definition_id,
    ),
  );
  assert.match(picked.face.face_id, /^step:[0-9]+$/);
  const sceneBounds = placed.scene.bounds_mm;
  const sectionOrigin = [
    sceneBounds.min[0],
    sceneBounds.min[1],
    (sceneBounds.min[2] + sceneBounds.max[2]) / 2,
  ];
  for (let axis = 0; axis < 3; axis++) {
    await input(`[aria-label="Origin (scene mm) ${"XYZ"[axis]}"]`, sectionOrigin[axis]);
    await input(`[aria-label="Normal ${"XYZ"[axis]}"]`, axis === 2 ? 1 : 0);
  }
  await click("start-section");
  const sectioned = await until(async () => {
    const current = await geometryState();
    return current.section && !current.active_job && !current.transferring ? current : false;
  }, "native world section overlay");
  assert.equal(sectioned.section.total_loop_count, sectioned.occurrences.length);
  assert.deepEqual(sectioned.section.plane.origin_mm, sectionOrigin);
  await screenshot("geometry-parts-pick-section");
  await input('[data-testid="geometry-debug-paths"]', JSON.stringify(["perforated-plate.step"]));
  await click("geometry-debug-import");
  await until(async () => (await geometryState()).active_job, "real plate job progress");
  await click("cancel-job");
  const cancelPhase = await until(
    async () => {
      const phase = await text("engine-status");
      if (/interrupted/.test(phase)) return phase;
      const current = await geometryState();
      return /running/.test(phase) && !current.active_job && !current.transferring ? phase : false;
    },
    "native cancellation or watchdog interruption",
    25000,
  );
  if (/running/.test(cancelPhase))
    assert.equal(
      (await geometryState()).scene.revision,
      sectioned.scene.revision,
      "Cancelled import preserves committed scene",
    );
  if (/interrupted/.test(cancelPhase)) {
    await until(async () => !alive(enginePid), "watchdog child exit");
    await restartWorkbench();
    await until(
      async () => /running/.test(await text("engine-status")),
      "fresh session after watchdog",
    );
    enginePid = Number((await text("engine-pid")).match(/\d+/)?.[0]);
  }
  await input('[data-testid="geometry-debug-paths"]', JSON.stringify(["perforated-plate.step"]));
  await click("geometry-debug-import");
  const staged = await until(
    async () => {
      const current = await geometryState();
      return current.transferring ? current : false;
    },
    "real multi-chunk plate transfer",
    65000,
  );
  await click("cancel-transfer");
  const paused = await until(async () => {
    const current = await geometryState();
    return current.stale && !current.transferring ? current : false;
  }, "paused noninteractive stale display");
  assert.equal(paused.scene.revision, staged.scene.revision);
  await click("resume-display");
  const resumed = await until(
    async () => {
      const current = await geometryState();
      return !current.stale && !current.active_job && !current.transferring ? current : false;
    },
    "resumed cached plate display",
    25000,
  );
  assert.equal(
    resumed.scene.revision,
    paused.scene.revision,
    "Resume performs no new native import",
  );
  assert.equal(resumed.scene.unique_mesh_bytes, paused.scene.unique_mesh_bytes);
  await screenshot("geometry-plate-resumed");
  return {
    scene: sectioned.scene,
    picked,
    section: sectioned.section,
    bufferUsage: sectioned.buffer_usage,
    cancelPhase,
    pausedRevision: paused.scene.revision,
    resumedScene: resumed.scene,
    resumedBufferUsage: resumed.buffer_usage,
  };
}
async function restartWorkbench() {
  await click("restart-engine");
  await until(async () => {
    if (await evaluate("!!document.querySelector('[data-testid=\"project-discard-confirm\"]')"))
      await click("project-discard-confirm");
    return /running/i.test(await text("engine-status"));
  }, "explicit project-aware restart");
}

async function invoke(command, args = {}) {
  return evaluate(
    `window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`,
  );
}
async function bridgeScene(sessionId) {
  const response = await invoke("geometry_control", {
    command: { op: "get_scene", session_id: sessionId },
  });
  assert.equal(response.type, "scene");
  return response.summary;
}
async function bridgeProject(sessionId) {
  const response = await invoke("project_control", {
    command: { op: "get", session_id: sessionId },
  });
  assert.equal(response.type, "status");
  return response.info;
}
async function bridgeJob(response, sessionId) {
  assert.equal(response.type, "job_accepted", JSON.stringify(response));
  return until(
    async () => {
      const status = await invoke("geometry_control", {
        command: { op: "get_job", session_id: sessionId, job_id: response.job_id },
      });
      assert.equal(status.type, "job");
      if (["failed", "cancelled"].includes(status.job.status))
        throw new Error(JSON.stringify(status.job.error));
      return status.job.status === "completed" ? status.job.result : false;
    },
    "actual native project job",
    60000,
  );
}
async function bridgeOccurrences(scene) {
  const rows = [];
  let offset = 0;
  do {
    const page = await invoke("geometry_control", {
      command: {
        op: "get_scene_page",
        session_id: scene.session_id,
        revision: scene.revision,
        kind: "occurrences",
        offset,
      },
    });
    assert.equal(page.type, "scene_page");
    rows.push(...page.occurrences);
    offset = page.next_offset;
  } while (offset !== null);
  return rows;
}
async function inspectProjectBridge() {
  await until(
    () => evaluate("!!document.querySelector('[data-testid=\"project-controls\"]')"),
    "real authoring workbench",
  );
  const visualDiagnostic = await evaluate("document.body.innerText");
  await screenshot("project-native-bridge-surface");
  // Deliberately bypass unavailable WebGPU admission only for real native IO proof.
  let hello = await invoke("engine_restart", { discardChanges: true });
  enginePid = hello.pid;
  assert.equal(hello.protocol_version, PROTOCOL_VERSION);
  assert.ok(hello.project_capabilities.includes("recoverable_projects_v2"));
  const originalProject = await bridgeProject(hello.session_id);
  assert.equal(originalProject.path_label, null);
  const identity = { translation_mm: [0, 0, 0], rotation_xyzw: [0, 0, 0, 1] };
  const selected = await invoke("geometry_debug_select_sources", {
    relativePaths: ["box-mm.step", "cylinder.step"],
  });
  for (const [index, source] of selected.entries()) {
    const scene = await bridgeScene(hello.session_id);
    await bridgeJob(
      await invoke("geometry_import_source", {
        token: source.token,
        sessionId: hello.session_id,
        baseRevision: scene.revision,
        initialPose: { ...identity, translation_mm: [index * 40, 0, 0] },
      }),
      hello.session_id,
    );
  }
  let scene = await bridgeScene(hello.session_id);
  let occurrences = await bridgeOccurrences(scene);
  const box = occurrences.find((row) => row.occurrence_id === 1);
  assert.ok(box);
  const added = await invoke("geometry_control", {
    command: {
      op: "add_instance",
      session_id: hello.session_id,
      base_revision: scene.revision,
      definition_id: box.definition_id,
      pose: {
        translation_mm: [80, 0, 0],
        rotation_xyzw: [0, 0, Math.SQRT1_2, Math.SQRT1_2],
      },
    },
  });
  assert.equal(added.type, "scene_changed");
  const target = await invoke("project_debug_select_path", {
    relativePath: "project",
    intent: "save",
  });
  scene = await bridgeScene(hello.session_id);
  const saved = await bridgeJob(
    await invoke("project_save", {
      token: target.token,
      sessionId: hello.session_id,
      baseRevision: scene.revision,
    }),
    hello.session_id,
  );
  assert.equal(saved.kind, "project_saved");
  assert.equal(saved.info.dirty, false);
  assert.equal(saved.info.save_uncertain, false);
  assert.equal(saved.info.can_undo, true, "Save preserves session history");
  assert.equal((await bridgeScene(hello.session_id)).revision, scene.revision);
  const savedOccurrences = await bridgeOccurrences(scene);
  const pose = await invoke("geometry_control", {
    command: {
      op: "set_instance_pose",
      session_id: hello.session_id,
      base_revision: scene.revision,
      occurrence_id: box.occurrence_id,
      pose: { ...identity, translation_mm: [10, 20, 0] },
    },
  });
  assert.equal(pose.type, "scene_changed");
  assert.equal((await bridgeProject(hello.session_id)).dirty, true);
  const undone = await invoke("project_control", {
    command: {
      op: "undo",
      session_id: hello.session_id,
      base_revision: pose.summary.revision,
    },
  });
  assert.equal(undone.type, "scene_changed");
  assert.equal(undone.info.dirty, false, "Undo-to-saved compares content, not revision");
  assert.ok(undone.info.revision > saved.info.revision);
  assert.equal(undone.info.can_redo, true);
  const oldSession = hello.session_id;
  const oldPid = enginePid;
  await rm(resolve(fixtureRoot, "box-mm.step"));
  await rm(resolve(fixtureRoot, "cylinder.step"));
  hello = await invoke("engine_restart", { discardChanges: true });
  enginePid = hello.pid;
  assert.notEqual(hello.session_id, oldSession);
  await until(() => !alive(oldPid), "old writer reaped on restart");
  scene = await bridgeScene(hello.session_id);
  assert.equal(scene.occurrence_count, 0, "Fresh child does not replay unsaved edits");
  await bridgeJob(
    await invoke("project_reopen", {
      sessionId: hello.session_id,
      baseRevision: scene.revision,
    }),
    hello.session_id,
  );
  scene = await bridgeScene(hello.session_id);
  const reopened = await bridgeProject(hello.session_id);
  assert.equal(reopened.project_id, saved.info.project_id);
  assert.equal(reopened.dirty, false);
  occurrences = await bridgeOccurrences(scene);
  assert.deepEqual(occurrences, savedOccurrences);
  assert.equal(scene.unique_mesh_bytes, 12720);
  const face = await invoke("geometry_control", {
    command: {
      op: "inspect_face",
      reference: {
        session_id: hello.session_id,
        scene_revision: scene.revision,
        occurrence_id: box.occurrence_id,
        definition_id: box.definition_id,
        face_id: "step:18",
      },
    },
  });
  assert.equal(face.type, "face_inspection");
  assert.equal(face.inspection.provenance.source_name, "box-mm.step");
  const definitions = await invoke("geometry_control", {
    command: {
      op: "get_scene_page",
      session_id: hello.session_id,
      revision: scene.revision,
      kind: "definitions",
      offset: 0,
    },
  });
  assert.equal(definitions.type, "scene_page");
  let decodedBytes = 0;
  for (const definition of definitions.definitions) {
    const page = await invoke("geometry_control", {
      command: {
        op: "get_artifact_page",
        session_id: hello.session_id,
        artifact_id: definition.mesh_artifact_id,
        kind: "chunks",
        offset: 0,
      },
    });
    assert.equal(page.type, "artifact_page");
    assert.equal(page.next_offset, null);
    const metadata = page.chunks.map((chunk) => chunk.metadata);
    const expected = {
      session_id: hello.session_id,
      artifact_id: definition.mesh_artifact_id,
      definition_id: definition.definition_id,
    };
    validateMeshManifest(metadata, expected);
    for (const chunk of metadata) {
      const bytes = await evaluate(`(async () => {
        const bytes = await window.__TAURI_INTERNALS__.invoke("geometry_chunk", ${JSON.stringify({
          sessionId: hello.session_id,
          artifactId: definition.mesh_artifact_id,
          chunkIndex: chunk.chunk_index,
        })});
        if (!(bytes instanceof ArrayBuffer)) throw new Error("Native bridge did not return raw bytes");
        return Array.from(new Uint8Array(bytes));
      })()`);
      const buffer = Uint8Array.from(bytes).buffer;
      await decodeMesh(buffer, chunk, { ...expected, chunk_index: chunk.chunk_index });
      decodedBytes += buffer.byteLength;
    }
  }
  assert.equal(decodedBytes, scene.unique_mesh_bytes);
  const openToken = await invoke("project_debug_select_path", {
    relativePath: "project",
    intent: "open",
  });
  await bridgeJob(
    await invoke("project_open", {
      token: openToken.token,
      sessionId: hello.session_id,
      baseRevision: scene.revision,
      readOnly: true,
      recoverPrevious: false,
      discardChanges: false,
    }),
    hello.session_id,
  );
  const readonly = await bridgeProject(hello.session_id);
  assert.equal(readonly.read_only, true);
  scene = await bridgeScene(hello.session_id);
  const rejected = await invoke("geometry_control", {
    command: {
      op: "set_instance_pose",
      session_id: hello.session_id,
      base_revision: scene.revision,
      occurrence_id: box.occurrence_id,
      pose: identity,
    },
  });
  assert.equal(rejected.type, "project_error");
  assert.equal(rejected.error.code, "read_only");
  assert.deepEqual(await bridgeScene(hello.session_id), scene);
  const switched = await invoke("project_control", {
    command: {
      op: "new",
      session_id: hello.session_id,
      base_revision: scene.revision,
      discard_changes: false,
    },
  });
  assert.equal(switched.type, "scene_changed", "Read-only does not trap project switching");
  return {
    qualification:
      "Actual native bridge/storage proof; GPU admission deliberately bypassed. No visible authoring, picking or dialog certification.",
    visualDiagnostic,
    savedProject: saved.info,
    reopenedProject: reopened,
    readonlyProject: readonly,
    persistentOccurrences: occurrences,
    independentlyDecodedBytes: decodedBytes,
    face: face.inspection,
    freshSession: hello.session_id,
    replacedWriterReaped: oldPid,
  };
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
    if (message.method === "Page.javascriptDialogOpening") {
      // Only this isolated fixture session is controlled by the smoke runner.
      void cdp("Page.handleJavaScriptDialog", { accept: true });
      return;
    }
    const waiter = pending.get(message.id);
    if (!waiter) return;
    pending.delete(message.id);
    clearTimeout(waiter.timer);
    if (message.error) waiter.reject(new Error(message.error.message));
    else waiter.resolve(message.result);
  });
  await cdp("Page.enable");
  await cdp("Runtime.enable");
  if (scenario === "gpu-unavailable") {
    await cdp("Page.addScriptToEvaluateOnNewDocument", {
      source: "Object.defineProperty(navigator, 'gpu', {value: undefined, configurable: true});",
    });
    await cdp("Page.reload");
  }
  await until(async () => (await evaluate("document.readyState")) === "complete", "frontend load");
  await until(
    async () =>
      evaluate(
        "!!document.querySelector('[data-testid=\"license-assent\"]') || !!document.querySelector('[data-testid=\"engine-status\"]')",
      ),
    "mounted frontend admission",
  );
  if (await evaluate("!!document.querySelector('[data-testid=\"license-assent\"]')")) {
    await screenshot(`b0-${scenario}-license`);
    await click("license-assent");
    await click("accept-license");
  }
  if (scenario === "project-bridge") {
    const proof = await inspectProjectBridge();
    console.log(JSON.stringify({ result: "pass", scenario, ...proof }, null, 2));
  } else if (scenario === "gpu-unavailable") {
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
  } else if (scenario === "mismatch") {
    const body = await until(async () => {
      const value = await evaluate("document.body.innerText");
      return /upgrade|mismatch/i.test(value) ? value : false;
    }, "protocol mismatch diagnostic");
    await screenshot("b0-protocol-mismatch");
    console.log(JSON.stringify({ result: "pass", scenario, diagnostic: body }, null, 2));
  } else {
    await until(async () => {
      const status = await text("engine-status");
      if (/unsupported/i.test(status)) {
        await screenshot(`geometry-${scenario}-unsupported`);
        throw new Error(
          `Actual CEF cannot admit WebGPU: ${await evaluate("document.body.innerText")}`,
        );
      }
      return /running/i.test(status);
    }, "engine running");
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
    const geometry = scenario === "geometry" ? await inspectGeometry() : null;
    if (alive(enginePid)) process.kill(enginePid, "SIGKILL");
    await until(
      async () => /interrupted/i.test(await text("engine-status")),
      "external engine crash detected",
    );
    await screenshot("b0-interrupted");
    await restartWorkbench();
    await until(async () => /running/i.test(await text("engine-status")), "restart running");
    const restarted = Number((await text("engine-pid")).match(/\d+/)?.[0]);
    assert.notEqual(restarted, enginePid);
    const reset = await geometryState();
    assert.equal(reset.occurrences.length, 0, "Restart does not replay imports");
    if (geometry) assert.notEqual(reset.session_id, geometry.scene.session_id);
    assert.ok(alive(restarted));
    enginePid = restarted;
    await click("stop-engine");
    await until(async () => /stopped/i.test(await text("engine-status")), "intentional stop");
    await until(async () => !alive(enginePid), "stopped child exit");
    await restartWorkbench();
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
          geometry,
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
} catch (error) {
  if (socket?.readyState === WebSocket.OPEN) {
    console.error("Actual CEF surface:", await evaluate("document.body.innerText").catch(String));
    await screenshot(`${scenario}-failed`).catch(() => {});
  }
  throw error;
} finally {
  for (const waiter of pending.values()) {
    clearTimeout(waiter.timer);
    waiter.reject(new Error("Smoke finished"));
  }
  socket?.close();
  if (!childExited) child.kill("SIGTERM");
  const cleanupDeadline = Date.now() + 15000;
  while (
    ((!childExited && !launchError) || (enginePid && alive(enginePid))) &&
    Date.now() < cleanupDeadline
  )
    await delay(100);
  if (scenario === "project-bridge") {
    if ((childExited || launchError) && (!enginePid || !alive(enginePid)))
      await rm(fixtureRoot, { recursive: true, force: true });
    else console.error(`Preserved live-writer smoke directory: ${fixtureRoot}`);
  }
}
