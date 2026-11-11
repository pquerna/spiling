/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Real native capacity regression. Prebuilt coherent binaries, sequential work,
// and an external process resource envelope are supplied by the caller.
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, readdir, stat, unlink, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { projectSmoke, cgroupPeakBytes } from "./project-smoke.mjs";
import { decodeReplayDisplay } from "../packages/protocol/src/replay.ts";

const root = resolve(import.meta.dirname, "..");
const target = resolve(root, process.env.CARGO_TARGET_DIR ?? "target");
await mkdir(join(root, ".cache"), { recursive: true });
const output = await mkdtemp(join(root, ".cache", "toolpath-display-"));
console.error(`Evidence: ${output}`);
const evidence = { software_only: true, commands: [] };
const invoke = projectSmoke({ root, target, output, evidence, timeoutMs: 600_000 });
const source = join(output, "owned-96mm.step");
const original = await readFile(join(root, "fixtures/geometry/box-mm.step"), "utf8");
let points = 0;
const scaled = original.replace(/CARTESIAN_POINT\('', \(([^)]+)\)\)/g, (_, values) => {
  points++;
  const coordinates = values.split(",").map(Number);
  assert.equal(coordinates.length, 3);
  assert.ok(coordinates.every(Number.isFinite));
  return `CARTESIAN_POINT('', (${coordinates.map((n, axis) => n * [96 / 20, 96 / 10, 96 / 8][axis]).join(", ")}))`;
});
assert.ok(points > 0);
await writeFile(source, scaled, { flag: "wx" });
const intent = JSON.parse(
  await readFile(join(root, "fixtures/manufacturing/solid-fill.intent.json"), "utf8"),
);
// Explicit campaign policy expands hop/pressure event categories; shipping
// retraction defaults and the geometry/planning admission rules stay intact.
intent.recipe.process.retraction.minimum_travel_mm = 0.01;
const declaration = join(output, "intent.json");
await writeFile(declaration, JSON.stringify(intent), { flag: "wx" });
const project = join(output, "large.spil");
const created = await invoke("compile-save", [
  "create",
  project,
  "--import",
  source,
  "--intent",
  declaration,
  "--center-all-on-bed",
  "--preflight",
  "--compile",
  "--verify",
  "--save",
]);
assert.equal(created.manufacturing.verification.verified, true);
await unlink(source);
await unlink(declaration);
const replay = join(output, "actual.splr"),
  exported = join(output, "export");
const returned = await invoke("source-absent-return", [
  "inspect",
  project,
  "--preflight",
  "--verify",
  "--manufacturing-out",
  exported,
  "--replay-out",
  replay,
]);
assert.equal(returned.manufacturing.verification.verified, true);
assert.notEqual(created.hello.session_id, returned.hello.session_id);
assert.deepEqual(created.manufacturing.artifact, returned.manufacturing.artifact);
assert.equal((await readdir(exported)).length, 5);
const descriptor = returned.manufacturing.replay_display;
const bytes = new Uint8Array(await readFile(replay));
// This observer deliberately uses the independent cross-language decoder. The
// production client/Review use seekable backing instead of a whole raw vector.
const decoded = await decodeReplayDisplay(bytes.buffer, descriptor);
assert.ok(bytes.byteLength > 32 * 1024 * 1024, "exercise the retired omission path");
assert.equal(bytes.byteLength, 96 + 80 * descriptor.info.event_count);
assert.ok(descriptor.pages.length > 1);
assert.equal(descriptor.info.layer_count, 480);
evidence.result = {
  replay_bytes: (await stat(replay)).size,
  events: decoded.eventCount,
  pages: descriptor.pages.length,
  source_absent: true,
  verified: true,
  project,
  cgroup_peak_bytes: await cgroupPeakBytes(),
};
await writeFile(join(output, "result.json"), JSON.stringify(evidence, null, 2));
console.log(JSON.stringify(evidence.result, null, 2));
