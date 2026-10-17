/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { basename, dirname, isAbsolute, relative, resolve } from "node:path";
import { tmpdir } from "node:os";
import { decodeMesh, validateMeshManifest, SECTION_PLANE_TOLERANCE_MM } from "../packages/protocol/src/index.ts";
import { setTimeout as delay } from "node:timers/promises";
import { crc32, deflateSync } from "node:zlib";
import { createHash } from "node:crypto";

// Original acceptance image encoding; production image interpretation stays native.
function grayPng(pattern) {
  const chunk = (name, data) => {
    const type = Buffer.from(name, "ascii");
    const output = Buffer.alloc(12 + data.length);
    output.writeUInt32BE(data.length);
    type.copy(output, 4);
    data.copy(output, 8);
    output.writeUInt32BE(crc32(output.subarray(4, 8 + data.length)), 8 + data.length);
    return output;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(pattern.width, 0);
  header.writeUInt32BE(pattern.height, 4);
  header[8] = 8;
  header[9] = 0;
  const rows = Buffer.concat(pattern.pixels.map((row) => Buffer.from([0, ...row])));
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(rows)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

function imageRowHeights(sourceZ, parameters) {
  const y = -imagePattern.height * ((sourceZ + parameters.z_phase_mm) / parameters.z_period_mm);
  const row = Math.floor(y);
  const fraction = y - row;
  const wrap = (value, size) => ((value % size) + size) % size;
  return imagePattern.pixels[0].map((_, column) => parameters.depth_mm / 255 * (
    imagePattern.pixels[wrap(row, imagePattern.height)][column] * (1 - fraction) +
    imagePattern.pixels[wrap(row + 1, imagePattern.height)][column] * fraction
  ));
}

// Independent fixture geometry reference, not a second production mapper.
function imageReferenceAdded(sourceZ, carrier, parameters) {
  const heights = imageRowHeights(sourceZ, parameters);
  const variation = heights.reduce((sum, a, i) => sum + Math.abs(a - heights[(i + 1) % heights.length]), 0);
  if (carrier.kind === "cylinder") {
    const radius = carrier.radius_mm;
    const area = heights.reduce((sum, a, i) => {
      const b = heights[(i + 1) % heights.length];
      return sum + radius * (a + b) / 2 + (a * a + a * b + b * b) / 6;
    }, 0) * 2 * Math.PI / heights.length;
    const bound = 0.005 * 2 * Math.PI * parameters.depth_mm +
      0.001 * (2 * Math.PI * (radius + parameters.depth_mm) + parameters.u_repeats * variation) +
      Math.PI * 0.001 ** 2 + 0.00001;
    return { area, bound };
  }
  // This runner's original box is20x10x8; no production code uses these dimensions.
  const width = Math.abs(carrier.normal[0]) > 0.5 ? 10 : 20;
  const margin = parameters.edge_margin_mm;
  const active = width - 2 * margin;
  const value = (s) => {
    const e = Math.min(s - margin, width - margin - s);
    if (e <= 0) return 0;
    const taper = e >= margin ? 1 : (1 - Math.cos(Math.PI * e / margin)) / 2;
    const x = heights.length * (parameters.u_repeats * (s - margin) / active + parameters.u_phase_fraction);
    const column = Math.floor(x);
    const fraction = x - column;
    const wrap = (i) => ((i % heights.length) + heights.length) % heights.length;
    return taper * (heights[wrap(column)] * (1 - fraction) + heights[wrap(column + 1)] * fraction);
  };
  const intervals = 4096;
  const step = active / intervals;
  let area = value(margin) + value(width - margin);
  for (let i = 1; i < intervals; i++) area += (i % 2 ? 4 : 2) * value(margin + i * step);
  return {
    area: area * step / 3,
    bound: 0.001 * (2 * width + 4 * parameters.depth_mm + parameters.u_repeats * variation) + 0.00001,
  };
}

async function inspectCanonicalImagePreview(record, reload = false) {
  if (reload) {
    await until(async () => !(await surfaceState()).working, "idle native image preview");
    await click("surface-image-preview");
  }
  const state = await until(async () => {
    const current = await surfaceState();
    return !current.working && current.imagePreview?.record.image_hash === record.image_hash
      ? current : false;
  }, "canonical owned image preview", 65000);
  assert.equal(state.imagePreview.width, imagePattern.width);
  assert.equal(state.imagePreview.height, imagePattern.height);
  const canvas = await evaluate(`(() => {
    const canvas = document.querySelector('[data-testid="surface-image-preview-canvas"]');
    return {width:canvas.width,height:canvas.height,pixels:Array.from(canvas.getContext('2d').getImageData(0,0,canvas.width,canvas.height).data)};
  })()`);
  assert.equal(canvas.width, imagePattern.width);
  assert.equal(canvas.height, imagePattern.height);
  assert.deepEqual(canvas.pixels, imagePattern.pixels.flat().flatMap((value) => [value, value, value, 255]));
  return state;
}

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : fallback;
};
const executable = option("--executable");
const port = Number(option("--port", "9222"));
const scenario = option("--scenario", "normal");
const manufacturingDiagnostic = args.includes("--diagnostic-no-gpu");
const customManufacturingSource = option("--manufacturing-source");
const requireHardwareGpu = args.includes("--require-hardware-gpu");
if (requireHardwareGpu && (scenario !== "manufacturing" || manufacturingDiagnostic ||
    process.env.SPILING_CEF_SOFTWARE_GPU === "1" || process.env.SPILING_CEF_UNSANDBOXED === "1"))
  throw new Error("--require-hardware-gpu requires normal sandboxed manufacturing without software GPU overrides");
if (args.includes("--manufacturing-source") &&
    (!customManufacturingSource || customManufacturingSource.startsWith("--")))
  throw new Error("--manufacturing-source requires a relative STEP source path");
if (customManufacturingSource && (scenario !== "manufacturing" ||
    isAbsolute(customManufacturingSource) || !/\.(step|stp)$/i.test(customManufacturingSource)))
  throw new Error("--manufacturing-source accepts a relative STEP path only with --scenario manufacturing");
const imageReliefScenario = scenario === "image-relief" || scenario === "image-relief-planar";
const cylindricalFinishScenario = scenario === "cylindrical-finish" || scenario === "image-relief";
const surfaceFinishScenario = scenario === "surface-finish" || cylindricalFinishScenario || imageReliefScenario;
const manufacturingSource = customManufacturingSource ? basename(customManufacturingSource)
  : cylindricalFinishScenario
    ? "independent-exporters/occt-cylinder-mm.step"
    : "box-mm.step";
const manufacturingScenario = scenario === "manufacturing" || surfaceFinishScenario;
if (!executable)
  throw new Error(
    "Usage: pnpm smoke:desktop --executable PATH [--port 9222] [--scenario normal|geometry|project-bridge|manufacturing|surface-finish|cylindrical-finish|image-relief|image-relief-planar|gpu-unavailable|engine-operations] [--diagnostic-no-gpu] [--require-hardware-gpu (normal sandboxed manufacturing only)] [--manufacturing-source RELATIVE_STEP (plain manufacturing only, relative to cwd)]; provide DISPLAY on Linux or launch under xvfb-run",
  );
if (
  ![
    "normal",
    "geometry",
    "project-bridge",
    "manufacturing",
    "surface-finish",
    "cylindrical-finish",
    "image-relief",
    "image-relief-planar",
    "gpu-unavailable",
    "engine-operations",
  ].includes(scenario)
)
  throw new Error("Unknown smoke scenario");
const sourceFixtureRoot = resolve(option("--fixture-root", "fixtures/geometry"));
const imagePattern = imageReliefScenario
  ? JSON.parse(await readFile(resolve("fixtures/manufacturing/image-relief.pattern.json"), "utf8"))
  : null;
const fixtureRoot =
  scenario === "project-bridge" || manufacturingScenario
    ? await mkdtemp(resolve(tmpdir(), "spiling-authoring-cef-"))
    : sourceFixtureRoot;
if (scenario === "project-bridge" || manufacturingScenario) {
  const sources =
    scenario === "project-bridge" ? ["box-mm.step", "cylinder.step"] : [manufacturingSource];
  for (const name of sources) {
    await mkdir(dirname(resolve(fixtureRoot, name)), { recursive: true });
    await copyFile(
      customManufacturingSource ? resolve(customManufacturingSource) : resolve(sourceFixtureRoot, name),
      resolve(fixtureRoot, name),
    );
  }
}
if (manufacturingScenario) {
  await copyFile(
    resolve("fixtures/manufacturing/complete-sparse.intent.json"),
    resolve(fixtureRoot, "operator.intent.json"),
  );
  await writeFile(resolve(fixtureRoot, "invalid.intent.json"), "{");
}
if (imageReliefScenario) {
  await writeFile(resolve(fixtureRoot, "operator-relief.png"), grayPng(imagePattern));
}
const scenePath = resolve(option("--scene", "fixtures/geometry/scenes/two-parts.scene.json"));
const recipe = scenario === "geometry" ? JSON.parse(await readFile(scenePath, "utf8")) : null;
// Linux app-local storage contains the durable bounded operations ledger.
// Keep restart/deduplication inside this run without consuming the operator's
// retained history or inheriting capacity exhaustion from earlier smoke runs.
const smokeDataRoot = process.platform === "linux"
  ? await mkdtemp(resolve(tmpdir(), "spiling-desktop-data-"))
  : null;

const child = spawn(resolve(executable), [], {
  env: {
    ...process.env,
    ...(smokeDataRoot ? { XDG_DATA_HOME: smokeDataRoot } : {}),
    SPILING_CEF_DEBUG_PORT: String(port),
    ...(scenario === "geometry" || scenario === "project-bridge" || manufacturingScenario
      ? { SPILING_GEOMETRY_FIXTURE_ROOT: fixtureRoot }
      : {}),
    ...(manufacturingScenario && manufacturingDiagnostic
      ? { SPILING_MANUFACTURING_DIAGNOSTIC: "1" }
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
  await until(
    () =>
      evaluate(`(() => {
        const button = document.querySelector('[data-testid="${id}"]');
        if (!button || button.disabled) return false;
        button.click();
        return true;
      })()`),
    `enabled ${id}`,
  );
}
async function screenshot(name) {
  await mkdir("artifacts", { recursive: true });
  const capture = await cdp("Page.captureScreenshot", { format: "png" });
  const sourceTag = customManufacturingSource ? `-${basename(customManufacturingSource).replace(/[^a-zA-Z0-9_-]/g, "-")}` : "";
  await writeFile(`artifacts/${name}${sourceTag}.png`, Buffer.from(capture.data, "base64"));
}
async function geometryState() {
  return JSON.parse(await text("geometry-state"));
}
async function input(selector, value) {
  await until(() => evaluate(`(() => {
    const element = document.querySelector(${JSON.stringify(selector)});
    if (!element || element.disabled) return false;
    const prototype = element.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype
      : element.tagName === "SELECT" ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, "value").set.call(element, ${JSON.stringify(String(value))});
    element.dispatchEvent(new Event(element.tagName === "SELECT" ? "change" : "input", {bubbles:true}));
    return true;
  })()`), `enabled input ${selector}`);
}
async function cancelAcceptedOperation(testId) {
  await until(() => evaluate(
    `!!document.querySelector('[data-testid="${testId}"]:not(:disabled)')`,
  ), `enabled native operation ${testId}`);
  return evaluate(`new Promise(resolve => {
    const observer = new MutationObserver(cancelAccepted);
    const timeout = setTimeout(() => { observer.disconnect(); resolve(false); }, 10000);
    function cancelAccepted() {
      const button = document.querySelector('[data-testid="manufacturing-cancel"]');
      if (!button || button.disabled) return;
      observer.disconnect();
      clearTimeout(timeout);
      button.click();
      resolve(true);
    }
    observer.observe(document.querySelector('[data-testid="manufacturing-panel"]'), {
      childList: true, subtree: true, attributes: true, attributeFilter: ["disabled"],
    });
    document.querySelector('[data-testid="${testId}"]').click();
    cancelAccepted();
  })`);
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
  await click("fit-scene");
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
  assert.equal(response.type, "operation_accepted", JSON.stringify(response));
  return until(
    async () => {
      const operation = await invoke("engine_native_operation", { name: response.operation.name });
      assert.equal(operation.session_id, sessionId);
      if (["failed", "cancelled", "interrupted"].includes(operation.status))
        throw new Error(JSON.stringify(operation.error));
      return operation.status === "completed" ? operation.result : false;
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
      project_revision: saved.info.revision,
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
    assert.equal(definition.display.kind, "mesh", "rendered fixture display");
    const page = await invoke("geometry_control", {
      command: {
        op: "get_artifact_page",
        session_id: hello.session_id,
        artifact_id: definition.display.artifact_id,
        kind: "chunks",
        offset: 0,
      },
    });
    assert.equal(page.type, "artifact_page");
    assert.equal(page.next_offset, null);
    const metadata = page.chunks.map((chunk) => chunk.metadata);
    const expected = {
      session_id: hello.session_id,
      artifact_id: definition.display.artifact_id,
      definition_id: definition.definition_id,
    };
    validateMeshManifest(metadata, expected);
    for (const chunk of metadata) {
      const bytes = await evaluate(`(async () => {
        const bytes = await window.__TAURI_INTERNALS__.invoke("geometry_chunk", ${JSON.stringify({
          resource: page.resources[chunk.chunk_index],
          expected: { kind: "mesh", metadata: chunk },
          summary: page.summary,
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
      project_revision: readonly.revision,
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

async function manufacturingState() {
  return JSON.parse(await text("manufacturing-state"));
}
async function surfaceState() {
  return JSON.parse(await text("surface-state"));
}
async function inspectManufacturingSource(stage) {
  const scene = await until(async () => {
    const current = await geometryState();
    return current.occurrences.length === 1 && !current.active_job &&
      !current.stale && !current.transferring ? current : false;
  }, "settled native source inspection", 65000);
  const occurrence = scene.occurrences[0];
  const definition = scene.definitions.find((row) => row.definition_id === occurrence.definition_id);
  assert.ok(definition?.face_count > 0, "Source has inspectable native faces");
  await until(async () => !(await surfaceState()).working, "native face list ready");
  await input('[data-testid="surface-occurrence"]', occurrence.occurrence_id);
  const faceId = await until(() => evaluate(
    "[...document.querySelector('[data-testid=\"surface-face\"]').options].map(option => option.value).find(Boolean)",
  ), "native source face list");
  await input('[data-testid="surface-face"]', faceId);
  const inspected = await until(async () => {
    const current = await surfaceState();
    return current.selectedFaceRef?.face_id === faceId ? current.selectedFaceRef : false;
  }, "native source face inspection");
  assert.equal(inspected.occurrence_id, occurrence.occurrence_id);
  assert.equal(inspected.definition_id, occurrence.definition_id);
  await until(async () => (await geometryState()).inspection?.reference.face_id === faceId,
    "native inspected carrier response");
  const carrier = await evaluate(
    "JSON.parse(document.querySelector('[data-testid=\"face-inspection\"] pre').textContent)",
  );
  let picked = null;
  let adapter = null;
  if (!manufacturingDiagnostic) {
    assert.equal(definition.display.kind, "mesh", "Manufacturing acceptance requires certified source display");
    await input('[data-testid="surface-occurrence"]', "");
    await until(async () => (await geometryState()).inspection === null,
      "cleared list inspection before actual canvas picking");
    await evaluate("document.querySelector('[data-testid=\"viewport\"]').scrollIntoView({block:'center'})");
    adapter = await evaluate(
      "[...document.querySelectorAll('dt')].find(node => node.textContent === 'WebGPU')?.nextElementSibling?.textContent",
    );
    assert.ok(adapter && adapter !== "No active device", "Active WebGPU adapter is reported");
    if (requireHardwareGpu)
      assert.doesNotMatch(adapter, /swiftshader|lavapipe|llvmpipe|software|fallback|unknown|identity not disclosed/i,
        "Hardware acceptance rejects software or unidentified adapters");
    const bounds = await evaluate(
      "(() => {const r=document.querySelector('[data-testid=\"viewport\"]').getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height};})()",
    );
    for (const y of [0.5, 0.35, 0.65, 0.2, 0.8]) {
      for (const x of [0.5, 0.35, 0.65, 0.2, 0.8, 0.1, 0.9]) {
        const position = {x:bounds.x + bounds.width*x, y:bounds.y + bounds.height*y, button:"left", clickCount:1};
        await cdp("Input.dispatchMouseEvent", {...position, type:"mousePressed"});
        await cdp("Input.dispatchMouseEvent", {...position, type:"mouseReleased"});
        await delay(100);
        picked = (await geometryState()).inspection;
        if (picked) break;
      }
      if (picked) break;
    }
    assert.ok(picked, "Actual viewport pointer route resolves a native source face");
    assert.equal(picked.reference.occurrence_id, occurrence.occurrence_id);
    assert.equal(picked.reference.definition_id, occurrence.definition_id);
    assert.equal(picked.reference.session_id, scene.session_id);
    assert.equal(picked.reference.scene_revision, scene.scene.revision);
    assert.match(picked.face.face_id, /^step:[0-9]+$/);
  }
  await screenshot(`manufacturing-${stage}-source-inspection`);
  return {reference:inspected, carrier, picked, adapter, display:definition.display,
    canvasPicking: !manufacturingDiagnostic, hardwareRequired:requireHardwareGpu};
}
async function inspectSurfaceFinishAuthoring() {
  await until(
    () => evaluate("!!document.querySelector('[data-testid=\"surface-panel\"]')"),
    "surface editor",
  );
  await until(async () => !(await surfaceState()).working, "surface authority");
  const before = await surfaceState();
  const intent = (await manufacturingState()).selected_intent;
  const occurrence = (await geometryState()).occurrences[0];
  await input('[data-testid="surface-occurrence"]', occurrence.occurrence_id);
  const faceIds = await evaluate(
    "[...document.querySelector('[data-testid=\"surface-face\"]').options].map(option => option.value).filter(Boolean)",
  );
  let reference;
  let selectedCarrier;
  for (const faceId of faceIds) {
    await input('[data-testid="surface-face"]', faceId);
    await until(
      async () => (await surfaceState()).selectedFaceRef?.face_id === faceId,
      "native source face inspection",
    );
    const carrier = await evaluate(
      "JSON.parse(document.querySelector('[data-testid=\"face-inspection\"] pre').textContent)",
    );
    if (
      cylindricalFinishScenario
        ? carrier.kind === "cylinder"
        : carrier.kind === "plane" && Math.abs(carrier.normal[2]) < 1e-12
    ) {
      selectedCarrier = carrier;
      reference = (await surfaceState()).selectedFaceRef;
      break;
    }
  }
  assert.ok(reference, "The admitted source must expose the requested inspected native carrier");
  await input('[data-testid="surface-scope"]', "occurrence");
  const parameters = imageReliefScenario
    ? { ...imagePattern.parameters, edge_margin_mm: cylindricalFinishScenario ? 0 : 1 }
    : null;
  const finish = imageReliefScenario
    ? { kind: "image_relief", parameters }
    : cylindricalFinishScenario
      ? { kind: "cylindrical_ribs", depth_mm: 0.1, rib_count: 8, phase_fraction: 0 }
      : { kind: "vertical_ribs", depth_mm: 0.1, pitch_mm: 2, edge_margin_mm: 1, phase_mm: 0 };
  await input('[data-testid="surface-kind"]', finish.kind);
  if (imageReliefScenario) {
    await input('[data-testid="surface-image-path"]', "operator-relief.png");
    await click("surface-image-debug-select");
    await until(async () => (await surfaceState()).selectedImage, "native single-use image selection");
  }
  const fields = parameters ?? Object.fromEntries(Object.entries(finish).filter(([key]) => key !== "kind"));
  for (const [key, value] of Object.entries(fields)) await input(`[data-testid="surface-${key}"]`, value);
  await click("surface-apply");
  const applied = await until(async () => {
    const current = await surfaceState();
    return !current.working &&
      current.revision > before.revision &&
      current.surface_overlays.layers.some((layer) => layer.finish.kind === finish.kind)
      ? current
      : false;
  }, "durable native face finish Apply");
  const layer = applied.surface_overlays.layers.at(-1);
  if (imageReliefScenario) {
    assert.deepEqual(layer.finish.parameters, parameters);
    const image = applied.surface_overlays.images.find((image) => image.image_hash === layer.finish.image_hash);
    assert.ok(image);
    const canonical = Buffer.concat([Buffer.from(`P5\n${imagePattern.width} ${imagePattern.height}\n255\n`), Buffer.from(imagePattern.pixels.flat())]);
    assert.equal(image.image_hash, createHash("sha256").update(canonical).digest("hex"));
    assert.equal(image.source_hash, createHash("sha256").update(await readFile(resolve(fixtureRoot, "operator-relief.png"))).digest("hex"));
    assert.equal(image.byte_count, canonical.length);
    await inspectCanonicalImagePreview(image);
    await evaluate("document.querySelector('[data-testid=\"surface-image-display\"]').scrollIntoView({block:'start'})");
    await screenshot(`${scenario}-owned-image-preview`);
  } else assert.deepEqual(layer.finish, finish);
  assert.equal(layer.face_id, reference.face_id);
  assert.deepEqual(layer.scope, {
    kind: "occurrence",
    occurrence_id: reference.occurrence_id,
    expected_definition_id: reference.definition_id,
  });
  assert.deepEqual(
    applied.selectedFaceRef,
    reference,
    "Metadata authoring preserves native scene identity",
  );
  assert.deepEqual(
    (await manufacturingState()).selected_intent,
    intent,
    "Finish does not replace material/process intent",
  );
  await click("project-undo");
  await until(async () => {
    const current = await surfaceState();
    return (
      !current.working &&
      current.revision > applied.revision &&
      JSON.stringify(current.surface_overlays) === JSON.stringify(before.surface_overlays)
    );
  }, "finish Undo restores the complete authored document");
  await click("project-redo");
  await until(async () => {
    const current = await surfaceState();
    return (
      !current.working &&
      JSON.stringify(current.surface_overlays) === JSON.stringify(applied.surface_overlays)
    );
  }, "finish Redo restores original layer identity");
  if (imageReliefScenario) {
    // Stored mapping must remain editable after the operator's original file disappears.
    await rm(resolve(fixtureRoot, "operator-relief.png"));
    await input('[data-testid="surface-image-asset"]', layer.finish.image_hash);
    const changedPhase = (parameters.u_phase_fraction + 0.25) % 1;
    await input('[data-testid="surface-u_phase_fraction"]', changedPhase);
    const beforeRemap = await surfaceState();
    await click("surface-apply");
    const remapped = await until(async () => {
      const current = await surfaceState();
      return !current.working && current.revision > beforeRemap.revision &&
        current.surface_overlays.layers.at(-1)?.finish.parameters?.u_phase_fraction === changedPhase
        ? current : false;
    }, "source-independent stored image remapping");
    assert.deepEqual(remapped.surface_overlays.images, applied.surface_overlays.images, "Remapping shares the sole captured image record");
    assert.deepEqual(remapped.selectedFaceRef, reference, "Stored image remapping preserves native face identity");
    await click("project-undo");
    await until(async () => JSON.stringify((await surfaceState()).surface_overlays) === JSON.stringify(applied.surface_overlays),
      "stored mapping Undo restores complete original intent");
  }
  await evaluate(
    "document.querySelector('[data-testid=\"surface-face\"]').scrollIntoView({block:'start'})",
  );
  await screenshot(`${scenario}-native-face-authoring`);
  return {
    reference,
    carrier: selectedCarrier,
    layer_id: layer.layer_id,
    document: applied.surface_overlays,
  };
}

async function inspectManufacturing() {
  await until(
    () => evaluate("!!document.querySelector('[data-testid=\"manufacturing-panel\"]')"),
    "production manufacturing panel",
  );
  await until(
    async () => /running|unsupported|error/.test(await text("engine-status")),
    "GPU admission settled",
  );
  if (manufacturingDiagnostic) {
    assert.equal(
      await text("engine-status"),
      "unsupported",
      "Product GPU gate is preserved before explicit diagnostic attach",
    );
    const stopped = await invoke("engine_status");
    assert.equal(stopped.state, "stopped");
    assert.equal(stopped.hello, null);
    const runtime = await invoke("runtime_info");
    assert.equal(
      runtime.manufacturing_diagnostic_enabled,
      true,
      "Native startup capability gates attachment",
    );
    await click("manufacturing-diagnostic-attach");
  } else if ((await text("engine-status")) !== "running") {
    throw new Error(
      "Manufacturing visual smoke requires working WebGPU. For explicitly labelled native-panel-only evidence, rerun --scenario manufacturing --diagnostic-no-gpu; no silent GPU fallback.",
    );
  }
  await until(
    async () => (await text("engine-status")) === "running",
    "manufacturing native session",
  );
  enginePid = (await invoke("engine_status")).hello.pid;
  let state = await until(async () => {
    const current = await manufacturingState();
    return current.session_id && !current.working ? current : false;
  }, "manufacturing status");
  assert.equal(state.diagnostic_no_gpu, manufacturingDiagnostic);
  assert.equal(state.selected_intent, null, "There is no implicitly selected printer intent");
  assert.equal(state.selected_artifact, null);
  if (manufacturingDiagnostic && customManufacturingSource) {
    await input('[data-testid="manufacturing-diagnostic-source"]', manufacturingSource);
    await click("manufacturing-diagnostic-source-import");
  } else if (manufacturingDiagnostic) {
    await click(
      cylindricalFinishScenario
        ? "manufacturing-diagnostic-cylinder-import"
        : "manufacturing-diagnostic-import",
    );
  } else {
    await input('[data-testid="geometry-debug-paths"]', JSON.stringify([manufacturingSource]));
    await click("geometry-debug-import");
  }
  await until(
    async () => {
      const current = await geometryState();
      return current.occurrences.length === 1 && !current.active_job && !current.stale;
    },
    "real native token import",
    65000,
  );
  const sourceInspection = scenario === "manufacturing"
    ? await inspectManufacturingSource("imported") : null;
  await input('[data-testid="manufacturing-intent-path"]', "operator.intent.json");
  await click("manufacturing-debug-intent");
  await until(async () => {
    const current = await manufacturingState();
    return current.draft_changed && !current.working;
  }, "native bounded inert profile selection and single-use load");
  await input('[data-testid="manufacturing-top_skin_thickness_mm"]', customManufacturingSource ? "0.8" : "0.6");
  await input('[data-testid="manufacturing-bottom_skin_thickness_mm"]', customManufacturingSource ? "0.8" : "0.4");
  await input('[data-testid="manufacturing-infill_fraction"]', "0.25");
  const beforeInvalidLoad = await manufacturingState();
  const beforeInvalidMessage = await text("manufacturing-message");
  await input('[data-testid="manufacturing-intent-path"]', "invalid.intent.json");
  await click("manufacturing-debug-intent");
  await until(async () => {
    const current = await manufacturingState();
    return !current.working && (await text("manufacturing-message")) !== beforeInvalidMessage;
  }, "recoverable native malformed profile admission");
  assert.equal((await manufacturingState()).project_revision, beforeInvalidLoad.project_revision);
  assert.equal((await manufacturingState()).selected_intent, null);
  assert.equal(
    await evaluate(
      "document.querySelector('[data-testid=\"manufacturing-top_skin_thickness_mm\"]').value",
    ),
    customManufacturingSource ? "0.8" : "0.6",
    "Failed profile loading preserves the existing operator draft",
  );
  await click("manufacturing-apply");
  state = await until(async () => {
    const current = await manufacturingState();
    return current.selected_intent && !current.working && !current.draft_changed ? current : false;
  }, "full intent Apply");
  assert.equal(state.selected_intent.recipe.top_skin_thickness_mm, customManufacturingSource ? 0.8 : 0.6);
  assert.equal(state.selected_intent.recipe.bottom_skin_thickness_mm, customManufacturingSource ? 0.8 : 0.4);
  assert.equal(state.selected_intent.recipe.infill_fraction, 0.25);
  assert.equal(state.selected_intent.printer.workflow.kind, "complete_marlin");
  if (customManufacturingSource)
    assert.deepEqual(state.selected_intent,
      JSON.parse(await readFile(resolve(fixtureRoot, "operator.intent.json"), "utf8")),
      "Custom-source acceptance preserves the entire unchanged complete-sparse intent");
  assert.equal(
    await evaluate("document.querySelector('[data-testid=\"manufacturing-apply\"]').disabled"),
    true,
    "Unchanged intent is not submitted",
  );
  const beforeBedScene = await geometryState();
  const beforeBedProject = await manufacturingState();
  const bedOccurrence = beforeBedScene.occurrences[0].occurrence_id;
  const originalPose = beforeBedScene.occurrences[0].pose;
  await input('[data-testid="manufacturing-bed-occurrence"]', bedOccurrence);
  const bedCancellationRequested = await cancelAcceptedOperation("manufacturing-center-bed");
  assert.equal(bedCancellationRequested, true,
    "Bed preparation exposes the actual cancellable native operation");
  const afterBedCancellation = await until(async () => {
    const current = await manufacturingState();
    return !current.working && !(await geometryState()).active_job ? current : false;
  }, "bed cancellation/ACK-winning terminal completion", 65000);
  assert.equal(afterBedCancellation.project_revision, beforeBedProject.project_revision,
    "Cancelling bed preparation cannot mutate the project");
  assert.deepEqual((await geometryState()).scene, beforeBedScene.scene,
    "Cancelling bed preparation cannot replace the scene");
  assert.deepEqual((await geometryState()).occurrences, beforeBedScene.occurrences,
    "Cancelling bed preparation cannot move an occurrence");
  await click("manufacturing-center-bed");
  const prepared = await until(async () => {
    const current = await manufacturingState();
    return current.bed_proposal && !current.working ? current : false;
  }, "read-only native center-and-bed proposal", 65000);
  const proposal = prepared.bed_proposal;
  assert.equal(prepared.bed_proposal_stale, false);
  assert.equal(prepared.project_revision, beforeBedProject.project_revision, "Prepare cannot mutate the project");
  assert.deepEqual((await geometryState()).scene, beforeBedScene.scene, "Prepare cannot change the native scene");
  assert.deepEqual((await geometryState()).occurrences, beforeBedScene.occurrences, "Prepare cannot move an occurrence");
  assert.deepEqual(proposal.original_pose, originalPose);
  assert.deepEqual(proposal.pose.rotation_xyzw, originalPose.rotation_xyzw, "Arbitrary rigid orientation is preserved");
  assert.ok(proposal.contact_tolerance_mm > 0 && proposal.contact_tolerance_mm <= SECTION_PLANE_TOLERANCE_MM);
  assert.ok(proposal.maximum_contact_gap_mm >= 0 &&
    proposal.maximum_contact_gap_mm <= proposal.contact_tolerance_mm);
  const tolerance = proposal.contact_tolerance_mm + 1e-8;
  for (let axis = 0; axis < 3; axis++) {
    const delta = proposal.pose.translation_mm[axis] - originalPose.translation_mm[axis];
    const lower = proposal.minimum_lower_mm[axis] + delta;
    const upper = proposal.maximum_upper_mm[axis] + delta;
    assert.ok(lower >= proposal.envelope_mm.min[axis] - 1e-8, "Native candidate must not penetrate the declared envelope");
    assert.ok(upper <= proposal.envelope_mm.max[axis] + 1e-8, "Native candidate must fit the declared envelope");
    if (axis === 2) {
      assert.ok(Math.abs(lower - proposal.envelope_mm.min[axis]) <= tolerance, "Certified source contacts the bed within native tolerance");
      assert.ok(proposal.minimum_upper_mm[axis] + delta - proposal.envelope_mm.min[axis] <= tolerance);
    } else {
      const center = (proposal.envelope_mm.min[axis] + proposal.envelope_mm.max[axis]) / 2;
      const centerLower = (proposal.minimum_lower_mm[axis] + proposal.maximum_lower_mm[axis]) / 2 + delta;
      const centerUpper = (proposal.minimum_upper_mm[axis] + proposal.maximum_upper_mm[axis]) / 2 + delta;
      assert.ok(centerLower <= center + tolerance && centerUpper >= center - tolerance,
        "Native original-source support brackets certify XY centering");
    }
  }
  if (!customManufacturingSource) {
    const expected = cylindricalFinishScenario ? [110, 110, 0] : [100, 105, 0];
    proposal.pose.translation_mm.forEach((value, axis) =>
      assert.ok(Math.abs(value - expected[axis]) <= tolerance, "Known physical fixture placement within contact tolerance"));
  }
  await evaluate("document.querySelector('[data-testid=\"manufacturing-bed-review\"]').scrollIntoView({block:'start'})");
  await screenshot(`${scenario}-native-bed-proposal`);
  await input('[data-testid="manufacturing-flow_multiplier"]', "1.1");
  await until(async () => (await manufacturingState()).bed_proposal_stale,
    "changed manufacturing draft visibly invalidates the captured proposal");
  assert.equal(
    await evaluate("document.querySelector('[data-testid=\"manufacturing-bed-apply\"]').disabled"),
    true, "A stale proposal cannot be applied",
  );
  await click("manufacturing-reset-draft");
  await until(async () => !(await manufacturingState()).draft_changed, "restore authoritative intent draft");
  assert.equal((await manufacturingState()).bed_proposal_stale, true, "Reset must not revive an invalidated proposal");
  await click("manufacturing-bed-clear");
  await until(async () => !(await manufacturingState()).bed_proposal, "explicitly cleared proposal");
  assert.equal((await manufacturingState()).project_revision, beforeBedProject.project_revision);
  assert.deepEqual((await geometryState()).occurrences, beforeBedScene.occurrences);
  await click("manufacturing-center-bed");
  await until(async () => {
    const current = await manufacturingState();
    return current.bed_proposal && !current.bed_proposal_stale && !current.working;
  }, "fresh read-only proposal after explicit Clear", 65000);
  await click("manufacturing-bed-apply");
  const appliedBed = await until(async () => {
    const current = await geometryState();
    const manufacturing = await manufacturingState();
    const row = current.occurrences.find(row => row.occurrence_id === bedOccurrence);
    return !manufacturing.working && manufacturing.project_revision > beforeBedProject.project_revision &&
      current.scene.revision > beforeBedScene.scene.revision && row &&
      row.pose.translation_mm.every((value, axis) =>
        Math.abs(value - proposal.pose.translation_mm[axis]) <= tolerance) ? current : false;
  }, "explicit revision-owned center-and-bed Apply");
  assert.deepEqual(appliedBed.occurrences[0].pose.rotation_xyzw, originalPose.rotation_xyzw);
  await click("project-undo");
  await until(async () => JSON.stringify((await geometryState()).occurrences[0].pose) === JSON.stringify(originalPose),
    "one Core Undo restores the complete original bed pose");
  await click("project-redo");
  await until(async () => JSON.stringify((await geometryState()).occurrences[0].pose) === JSON.stringify(appliedBed.occurrences[0].pose),
    "one Core Redo restores the reviewed bed pose");
  const beforePreflight = await manufacturingState();
  await click("manufacturing-preflight");
  state = await until(async () => {
    const current = await manufacturingState();
    return !current.working &&
      current.preflight?.input_revision === beforePreflight.project_revision
      ? current
      : false;
  }, "revision-captured read-only native Preflight");
  assert.equal(state.preflight.disposition, "admitted");
  assert.equal(state.preflight.software_only, true);
  assert.ok(state.preflight.checks.length > 0, "Preflight exposes actual bounded checks");
  for (const check of state.preflight.checks) {
    assert.ok(check.code && check.message);
    if (check.required) assert.equal(check.evidence, "certified",
      `Admitted required check ${check.code} must have certified evidence`);
  }
  const preflight = state.preflight;
  assert.equal(state.project_revision, beforePreflight.project_revision);
  assert.deepEqual(state.selected_artifact, beforePreflight.selected_artifact);
  const appliedRevision = state.project_revision;
  await input('[data-testid="manufacturing-perimeter_count"]', "9");
  await click("manufacturing-apply");
  await until(
    async () => /invalid_specification/.test(await text("manufacturing-message")),
    "recoverable failed Apply",
  );
  assert.equal(
    await evaluate(
      "document.querySelector('[data-testid=\"manufacturing-perimeter_count\"]').value",
    ),
    "9",
    "Failed Apply retains the operator draft",
  );
  assert.equal((await manufacturingState()).project_revision, appliedRevision);
  await click("manufacturing-reset-draft");
  await input('[data-testid="manufacturing-flow_multiplier"]', "1.1");
  await click("manufacturing-apply");
  await until(async () => {
    const current = await manufacturingState();
    return (
      current.selected_intent?.recipe.flow_multiplier === 1.1 &&
      !current.working &&
      !current.draft_changed
    );
  }, "scalar Apply");
  await click("project-undo");
  await until(
    async () => (await manufacturingState()).selected_intent?.recipe.flow_multiplier === 1,
    "Core intent Undo",
  );
  await click("project-redo");
  await until(
    async () => (await manufacturingState()).selected_intent?.recipe.flow_multiplier === 1.1,
    "Core intent Redo",
  );
  await click("project-undo");
  await until(
    async () => (await manufacturingState()).selected_intent?.recipe.flow_multiplier === 1,
    "Core intent restored",
  );
  let baseline = null;
  if (cylindricalFinishScenario || imageReliefScenario) {
    await click("manufacturing-compile");
    await until(
      async () => {
        const current = await manufacturingState();
        return current.selected_artifact && !current.working;
      },
      "untextured native cylinder compile",
      65000,
    );
    await click("manufacturing-verify-motion");
    await until(
      async () => {
        const current = await manufacturingState();
        return current.fresh && current.replay && !current.working;
      },
      "independent untextured cylinder motion",
      65000,
    );
    await input('[data-testid="manufacturing-export-path"]', "baseline-export");
    await click("manufacturing-debug-export");
    await until(
      async () => /Committed/.test(await text("manufacturing-message")),
      "baseline export",
    );
    baseline = {
      plan: JSON.parse(
        await readFile(resolve(fixtureRoot, "baseline-export", "plan.json"), "utf8"),
      ),
      program: await readFile(resolve(fixtureRoot, "baseline-export", "program.gcode"), "utf8"),
    };
  }
  const surface = surfaceFinishScenario ? await inspectSurfaceFinishAuthoring() : null;
  await click("manufacturing-compile");
  state = await until(
    async () => {
      const current = await manufacturingState();
      return current.selected_artifact && !current.working ? current : false;
    },
    "native immutable compile",
    65000,
  );
  const record = state.selected_artifact;
  assert.equal(state.fresh, false, "Stored verified flags confer no fresh authority");
  await click("manufacturing-verify-report");
  state = await until(
    async () => {
      const current = await manufacturingState();
      return current.fresh && !current.working ? current : false;
    },
    "fresh report-only verification",
    65000,
  );
  assert.equal(state.replay, null);
  assert.equal(state.report.verified, true);
  assert.deepEqual(state.selected_artifact, record, "Inspection does not change the Core artifact");
  await click("manufacturing-verify-motion");
  state = await until(
    async () => {
      const current = await manufacturingState();
      return current.fresh && current.replay && !current.working ? current : false;
    },
    "actual replay transport and panel",
    65000,
  );
  assert.equal(state.replay.byte_count, 96 + 48 * state.replay.event_count);
  assert.equal(
    state.replay.event_count,
    state.replay.deposition_count +
      state.replay.travel_count +
      state.replay.lifecycle_count +
      state.replay.process_count,
  );
  assert.equal(state.replay.lifecycle_count, state.report.lifecycle_motion_count);
  assert.equal(state.replay.process_count, 2 * state.report.retraction_count);
  assert.equal(
    state.replay.lifecycle_count,
    state.selected_intent.printer.workflow.kind === "complete_marlin" ? 4 : 0,
  );
  assert.equal(state.replay.display_ready, !manufacturingDiagnostic);
  assert.deepEqual(state.selected_artifact, record);
  const cancellationRequested = await cancelAcceptedOperation("manufacturing-verify-motion");
  assert.equal(
    cancellationRequested,
    true,
    "Actual panel cancellation reaches the accepted native operation",
  );
  const cancelled = await until(
    async () => {
      const current = await manufacturingState();
      return !current.working ? current : false;
    },
    "native cancellation/ACK-winning terminal completion",
    65000,
  );
  assert.deepEqual(
    cancelled.selected_artifact,
    record,
    "Cancellation never mutates the immutable Core bundle",
  );
  if (cancelled.fresh)
    assert.ok(
      cancelled.report.verified && cancelled.replay,
      "ACK-winning late cancellation may retain only a complete terminal report/motion result",
    );
  await click("manufacturing-verify-motion");
  state = await until(
    async () => {
      const current = await manufacturingState();
      return current.fresh && current.replay && !current.working ? current : false;
    },
    "fresh replay after cancellation",
    65000,
  );
  await input('[data-testid="manufacturing-events"]', "1");
  await until(
    async () => JSON.parse(await text("manufacturing-event")).event_index === 0,
    "event cursor",
  );
  const first = JSON.parse(await text("manufacturing-event"));
  const completeJob = state.selected_intent.printer.workflow.kind === "complete_marlin";
  assert.equal(first.kind, completeJob ? "lifecycle_travel" : "travel");
  assert.equal(first.layer, completeJob ? null : 0);
  assert.equal(first.known_start, false, "No invented origin segment");
  assert.ok(first.source_line > 0 && first.feed_mm_min > 0);
  await input('[data-testid="manufacturing-layer"]', "0");
  await click("manufacturing-travel");
  await input('[data-testid="manufacturing-events"]', String(state.replay.event_count));
  if (!manufacturingDiagnostic) {
    await evaluate("document.querySelector('[data-testid=\"viewport\"]').scrollIntoView({block:'center'})");
    await screenshot(`${scenario}-actual-program-viewport`);
  }
  await input('[data-testid="project-debug-path"]', "manufacturing-project");
  await click("project-debug-save");
  await until(
    async () =>
      /manufacturing-project/.test(await text("project-saved")) &&
      /Clean/.test(await text("project-dirty")),
    "Core checkpoint",
  );
  await until(
    async () => (await manufacturingState()).fresh,
    "unchanged checkpoint content remains fresh",
  );
  await input('[data-testid="manufacturing-export-path"]', "export-one");
  await click("manufacturing-debug-export");
  await until(
    async () => /Committed/.test(await text("manufacturing-message")),
    "common four-file export",
  );
  const exported = Object.fromEntries(
    await Promise.all(
      ["plan.json", "program.gcode", "provenance.json", "verification.json"].map(async (name) => [
        name,
        await readFile(resolve(fixtureRoot, "export-one", name), "utf8"),
      ]),
    ),
  );
  assert.deepEqual(JSON.parse(exported["verification.json"]), state.report);
  assert.equal(JSON.parse(exported["provenance.json"]).input_hash, record.input_hash);
  const plan = JSON.parse(exported["plan.json"]);
  assert.ok(plan.layers.every((layer) => layer.region_coverage.requested_infill_fraction === 0.25));
  const kinds = new Set(plan.layers.flatMap((layer) => layer.paths.map((path) => path.kind)));
  assert.ok(kinds.has("bottom_skin"));
  assert.ok(kinds.has("top_skin"));
  assert.ok(kinds.has("sparse_fill"));
  if (surface) {
    const provenance = JSON.parse(exported["provenance.json"]);
    assert.deepEqual(provenance.surface_overlays, surface.document);
    if (baseline) {
      assert.notEqual(
        exported["program.gcode"],
        baseline.program,
        "Finish changes actual emitted bytes",
      );
      assert.notDeepEqual(
        (imageReliefScenario ? plan.layers.find((layer) => layer.region_coverage.surface_added_area_mm2 > 1e-8) : plan.layers[0]).paths.map((path) => ({kind:path.kind,points:path.points_mm.map((point) => point.slice(0,2))})),
        baseline.plan.layers[0].paths.map((path) => ({kind:path.kind,points:path.points_mm.map((point) => point.slice(0,2))})),
        "Finish changes manufactured paths, not only provenance",
      );
    }
    for (const [index, layer] of plan.layers.entries()) {
      const coverage = layer.region_coverage;
      const originalArea = baseline
        ? baseline.plan.layers[imageReliefScenario ? 0 : index].region_coverage.section_area_mm2
        : 200;
      if (imageReliefScenario && cylindricalFinishScenario) {
        // Image events change native sample Z. Independent native sections may
        // choose different certified chords; compare each to the source
        // circle's declared sampling envelope, not bitwise to another sample.
        const radius = surface.carrier.radius_mm;
        const tolerance = plan.sampling_tolerance_mm;
        const lower = Math.PI * Math.max(0, radius - tolerance) ** 2;
        const upper = Math.PI * (radius + tolerance) ** 2;
        for (const area of [originalArea, coverage.source_section_area_mm2]) {
          assert.ok(area >= lower && area <= upper,
            "Native source area stays inside its independent sampling certificate");
        }
        assert.equal(
          coverage.source_section_area_mm2,
          plan.layers[0].region_coverage.source_section_area_mm2,
          "Captured invariant native source footprint does not follow varying relief",
        );
      } else {
        assert.ok(
          Math.abs(coverage.source_section_area_mm2 - originalArea) <= 1e-8,
          "Original native footprint remains distinct from derived relief",
        );
      }
      const depth = imageReliefScenario ? surface.document.layers.at(-1).finish.parameters.depth_mm : surface.document.layers.at(-1).finish.depth_mm;
      const additionBound = cylindricalFinishScenario
        ? Math.PI *
          ((surface.carrier.radius_mm + depth + 0.00001) ** 2 -
            (surface.carrier.radius_mm - 0.005) ** 2)
        : imageReliefScenario ? 20 * depth : 2;
      assert.ok(
        coverage.surface_added_area_mm2 >= 0 && coverage.surface_added_area_mm2 <= additionBound &&
          (imageReliefScenario || coverage.surface_added_area_mm2 > 0),
        "Requested relief contributes bounded actual material; blank image rows remain unchanged",
      );
      assert.ok(
        Math.abs(
          coverage.section_area_mm2 -
            coverage.source_section_area_mm2 -
            coverage.surface_added_area_mm2,
        ) <= 1e-8,
      );
      if (imageReliefScenario) {
        const sourceZ = layer.section_z_mm;
        const expected = imageReferenceAdded(sourceZ, surface.carrier, surface.document.layers.at(-1).finish.parameters);
        assert.ok(Math.abs(coverage.surface_added_area_mm2 - expected.area) <= expected.bound,
          `Mapped source-Z ${sourceZ} must agree with the independently integrated image area`);
        if (expected.area === 0) assert.ok(Math.abs(coverage.surface_added_area_mm2) <= 1e-8, "Blank rows preserve exact original material");
      }
    }
    if (imageReliefScenario) {
      const areas = plan.layers.map((layer) => layer.region_coverage.surface_added_area_mm2);
      assert.ok(Math.min(...areas) <= 1e-8 && Math.max(...areas) > 0.01, "Real zero and raised image bands must reach manufacturing");
      assert.ok(new Set(areas.map((area) => Math.round(area * 1e6))).size >= 3, "Native extrusion reuse must not flatten a spatial image");
      assert.ok(plan.layers.some((layer) => Math.abs(layer.height_mm - state.selected_intent.recipe.layer_height_mm) > 1e-7), "Non-grid image events affect the actual scheduled layers");
      const contents = provenance.surface_images;
      assert.equal(contents.length, 1);
      assert.equal(contents[0].image_hash, surface.document.images[0].image_hash);
      assert.deepEqual(contents[0].bytes, [...Buffer.from(`P5\n${imagePattern.width} ${imagePattern.height}\n255\n`), ...imagePattern.pixels.flat()]);
      assert.ok(state.selected_artifact.summary.surface_resolution.evaluated_witnesses > 0, "Nominal feature diagnostics remain separate from emitted-byte verification");
    }
  }
  await click("manufacturing-debug-export");
  await until(
    async () => /new directory|new destination/.test(await text("manufacturing-message")),
    "no-overwrite export rejection",
  );
  assert.equal(
    await readFile(resolve(fixtureRoot, "export-one", "program.gcode"), "utf8"),
    exported["program.gcode"],
  );
  if (surface) await click(`surface-remove-${surface.layer_id}`);
  else {
    await input('[data-testid="manufacturing-flow_multiplier"]', "1.1");
    await click("manufacturing-apply");
  }
  await until(async () => {
    const current = await manufacturingState();
    return !current.fresh && !current.selected_artifact && !current.working;
  }, "content change invalidates inspection/export");
  assert.equal(
    await evaluate("document.querySelector('[data-testid=\"manufacturing-export\"]').disabled"),
    true,
  );
  await click("project-undo");
  await until(
    async () => (await manufacturingState()).selected_artifact?.hash === record.hash,
    "Core artifact Undo",
  );
  assert.equal(
    (await manufacturingState()).fresh,
    false,
    "Historical result cannot reactivate on Undo",
  );
  if (surface)
    assert.deepEqual(
      (await surfaceState()).surface_overlays,
      surface.document,
      "Undo restores the authored finish and matching immutable bundle together",
    );
  await click("project-save");
  await until(
    async () =>
      /Clean/.test(await text("project-dirty")) &&
      (await evaluate("!document.querySelector('[data-testid=\"project-debug-open\"]').disabled")),
    "completed checkpoint before reopen",
  );
  await rm(resolve(fixtureRoot, manufacturingSource));
  if (imageReliefScenario) await rm(resolve(fixtureRoot, "operator-relief.png"), {force:true});
  await click("project-read-only");
  await click("project-debug-open");
  state = await until(
    async () => {
      const current = await manufacturingState();
      return current.read_only && current.selected_artifact?.hash === record.hash ? current : false;
    },
    "source-independent read-only reopen",
    65000,
  );
  assert.equal(state.fresh, false, "Reopen requires fresh Verify");
  assert.equal(
    await evaluate("document.querySelector('[data-testid=\"manufacturing-compile\"]').disabled"),
    true,
  );
  if (surface) {
    assert.deepEqual((await surfaceState()).surface_overlays, surface.document);
    assert.equal(
      await evaluate("document.querySelector('[data-testid=\"surface-apply\"]').disabled"),
      true,
      "Read-only surface authoring stays disabled",
    );
  }
  if (imageReliefScenario) {
    const beforePreview = (await surfaceState()).revision;
    await inspectCanonicalImagePreview(surface.document.images[0], true);
    assert.equal((await surfaceState()).revision, beforePreview, "Read-only image retrieval does not edit the project");
  }
  const reopenedRevision = state.project_revision;
  await click("manufacturing-verify-motion");
  state = await until(
    async () => {
      const current = await manufacturingState();
      return current.fresh && current.replay && !current.working ? current : false;
    },
    "read-only fresh replay",
    65000,
  );
  assert.equal(state.project_revision, reopenedRevision);
  assert.deepEqual(state.selected_artifact, record);
  await input('[data-testid="manufacturing-export-path"]', "export-readonly");
  await click("manufacturing-debug-export");
  await until(
    async () => /Committed/.test(await text("manufacturing-message")),
    "read-only fresh export",
  );
  assert.equal(
    await readFile(resolve(fixtureRoot, "export-readonly", "program.gcode"), "utf8"),
    exported["program.gcode"],
  );
  const previousSession = state.session_id,
    previousPid = enginePid;
  await click("stop-engine");
  await until(
    async () => (await text("engine-status")) === "stopped" && !alive(previousPid),
    "native stop reaps manufacturing session",
  );
  await click("restart-engine");
  if (manufacturingDiagnostic) {
    await until(
      async () => (await text("engine-status")) === "unsupported",
      "ordinary restart retains product GPU gate",
    );
    assert.equal((await invoke("engine_status")).hello, null);
    await click("manufacturing-diagnostic-attach");
  }
  await until(
    async () => (await text("engine-status")) === "running",
    "new session reopens attached checkpoint",
    65000,
  );
  enginePid = (await invoke("engine_status")).hello.pid;
  state = await until(async () => {
    const current = await manufacturingState();
    return current.session_id !== previousSession &&
      current.read_only &&
      current.selected_artifact?.hash === record.hash
      ? current
      : false;
  }, "session-bound saved manufacturing status");
  assert.equal(state.fresh, false, "Previous-session verification cannot become active");
  await click("manufacturing-verify-motion");
  state = await until(
    async () => {
      const current = await manufacturingState();
      return current.fresh && current.replay && !current.working ? current : false;
    },
    "fresh session-independent replay after restart",
    65000,
  );
  assert.equal(state.replay.display_ready, !manufacturingDiagnostic);
  assert.equal(state.report.verified, true);
  assert.deepEqual(state.selected_artifact, record);
  if (customManufacturingSource)
    assert.deepEqual(state.selected_intent,
      JSON.parse(await readFile(resolve(fixtureRoot, "operator.intent.json"), "utf8")),
      "Fresh engine retains unchanged complete-sparse intent");
  const reopenedSourceInspection = scenario === "manufacturing"
    ? await inspectManufacturingSource("fresh-engine") : null;
  if (sourceInspection) {
    for (const key of ["occurrence_id", "definition_id", "face_id"])
      assert.equal(reopenedSourceInspection.reference[key], sourceInspection.reference[key],
        `Source deletion and fresh-engine reopen preserve native ${key}`);
    assert.notEqual(reopenedSourceInspection.reference.session_id, sourceInspection.reference.session_id);
  }
  await input('[data-testid="manufacturing-export-path"]', "export-fresh-engine");
  await click("manufacturing-debug-export");
  await until(async () => /Committed/.test(await text("manufacturing-message")),
    "fresh-engine read-only exclusive four-file export");
  for (const name of ["plan.json", "program.gcode", "provenance.json"]) {
    assert.equal(await readFile(resolve(fixtureRoot, "export-fresh-engine", name), "utf8"),
      exported[name], `Fresh engine exports identical immutable ${name}`);
  }
  assert.deepEqual(JSON.parse(await readFile(
    resolve(fixtureRoot, "export-fresh-engine", "verification.json"), "utf8")), state.report);
  if (imageReliefScenario) {
    const beforePreview = (await surfaceState()).revision;
    await inspectCanonicalImagePreview(surface.document.images[0], true);
    assert.equal((await surfaceState()).revision, beforePreview);
    await evaluate("document.querySelector('[data-testid=\"surface-image-display\"]').scrollIntoView({block:'start'})");
    await screenshot(`${scenario}-fresh-readonly-image-preview`);
  }
  await evaluate(
    "document.querySelector('[data-testid=\"manufacturing-inspection\"]').scrollIntoView({block:'start'})",
  );
  await screenshot(surface ? `${scenario}-production-panel` : "manufacturing-production-panel");
  return {
    sourceInspection,
    reopenedSourceInspection,
    preflight,
    record,
    report: state.report,
    provenance: state.provenance,
    replay: state.replay,
    surface,
    firstActualEndpoint: first,
    cancellationRequested,
    previousSession,
    enginePid,
    diagnosticNoGpu: manufacturingDiagnostic,
    qualification: manufacturingDiagnostic
      ? "Real production panel and native bridge; explicit startup diagnostic; no GPU/picking/hardware/dialog certification"
      : "Real WebGPU panel/native bridge; hardware, physical picking, dialogs and machine certification are separate gates",
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
    const waiter = pending.get(message.id);
    if (!waiter) return;
    pending.delete(message.id);
    clearTimeout(waiter.timer);
    if (message.error) waiter.reject(new Error(message.error.message));
    else waiter.resolve(message.result);
  });
  await cdp("Page.enable");
  await cdp("Runtime.enable");
  if (
    ["gpu-unavailable", "engine-operations", "project-bridge"].includes(scenario) ||
    (manufacturingScenario && manufacturingDiagnostic)
  ) {
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
  if (scenario === "project-bridge") {
    const proof = await inspectProjectBridge();
    console.log(JSON.stringify({ result: "pass", scenario, ...proof }, null, 2));
  } else if (manufacturingScenario) {
    const proof = await inspectManufacturing();
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
    const restarted = await invoke("engine_restart", { discardChanges: true });
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
    await until(
      async () => /running/i.test(await text("engine-status")),
      "engine running before diagnostic",
    );
    await click("run-diagnostic");
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
    const geometry = scenario === "geometry" ? await inspectGeometry() : null;
    process.kill(enginePid, "SIGKILL");
    await until(
      async () => /interrupted/i.test(await text("engine-status")),
      "external engine crash detected",
    );
    await screenshot("b0-interrupted");
    await restartWorkbench();
    await until(async () => /running/i.test(await text("engine-status")), "restart running");
    const restarted = Number((await text("engine-pid")).match(/\d+/)?.[0]);
    assert.notEqual(restarted, enginePid);
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
          geometry,
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
} catch (error) {
  if (manufacturingScenario && socket && !childExited) {
    try {
      console.error("Manufacturing failure surface:", await evaluate("document.body.innerText"));
      await screenshot("manufacturing-failure");
    } catch (captureError) {
      console.error("Failure capture unavailable:", String(captureError));
    }
  }
  throw error;
} finally {
  for (const waiter of pending.values()) {
    clearTimeout(waiter.timer);
    waiter.reject(new Error("Smoke finished"));
  }
  socket?.close();
  if (!childExited) child.kill("SIGTERM");
  if (scenario === "project-bridge" || manufacturingScenario) {
    if (childExited && (!enginePid || !alive(enginePid)))
      await rm(fixtureRoot, { recursive: true, force: true });
    else console.error(`Preserved live-writer smoke directory: ${fixtureRoot}`);
  }
  if (smokeDataRoot) {
    if (childExited && (!enginePid || !alive(enginePid)))
      await rm(smokeDataRoot, { recursive: true, force: true });
    else console.error(`Preserved live-writer smoke app data: ${smokeDataRoot}`);
  }
}
