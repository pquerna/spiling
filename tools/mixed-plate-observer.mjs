/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Independent analytic observers over the checked actual-program display.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { decodeReplayDisplay } from "../packages/protocol/src/replay.ts";
export async function auditMixedPlate(count, folder, returned) {
  const job = returned.manufacturing.intent.printer.workflow.job;
  const pose = (i) => ({ translation_mm: [20 + (i % 4) * 50, 20 + Math.floor(i / 4) * 25, 0] });
  const plan = JSON.parse(await readFile(join(folder, "returned/plan.json"), "utf8"));
  const report = JSON.parse(await readFile(join(folder, "returned/verification.json"), "utf8"));
  const selection = plan.interior_selection;
  assert.equal(selection.reason, "improved_modeled_work");
  assert.equal(selection.candidates.length, 3);
  assert.ok(report.deposition_segments < 100_000);
  assert.equal(
    report.routing.selection_count,
    plan.layers.length * 2,
    "tool selection is per plate layer, never per clone",
  );
  const decoded = await decodeReplayDisplay(
    Uint8Array.from(await readFile(join(folder, "actual.splr"))).buffer,
    returned.manufacturing_replay.replay_display,
  );
  const model = Array.from({ length: count }, () => [0, 0]);
  for (let i = 0; i < decoded.eventCount; i++) {
    if (![1, 2, 3, 4, 5, 8].includes(decoded.kind(i))) continue;
    const association = decoded.association(i);
    assert.ok(association);
    const point = decoded.position(i),
      width = association.beadWidthMm;
    if (!(width > 0)) continue;
    const owner = job.assignments.findIndex(
      (a) =>
        point[0] >= a.work_envelope_mm.min[0] &&
        point[0] <= a.work_envelope_mm.max[0] &&
        point[1] >= a.work_envelope_mm.min[1] &&
        point[1] <= a.work_envelope_mm.max[1],
    );
    assert.ok(owner >= 0, "actual model bead must have a plate reservation");
    const interior = typeof association.regionalOwner === "number";
    assert.equal(decoded.activeTool(i), interior ? 1 : 0);
    model[owner][interior ? 1 : 0]++;
    const from = decoded.position(i - 1);
    const [x, y] = pose(owner).translation_mm;
    for (const endpoint of [from, point]) {
      assert.ok(
        endpoint[0] >= x + width * 0.5 - 0.005001 && endpoint[0] <= x + 20 - width * 0.5 + 0.005001,
      );
      assert.ok(
        endpoint[1] >= y + width * 0.5 - 0.005001 &&
          endpoint[1] <= y + (owner === 0 ? 20 : 10) - width * 0.5 + 0.005001,
      );
    }
    if (owner === 0) {
      const dx = point[0] - from[0],
        dy = point[1] - from[1];
      const t = Math.max(
        0,
        Math.min(1, ((30 - from[0]) * dx + (30 - from[1]) * dy) / (dx * dx + dy * dy)),
      );
      assert.ok(
        Math.hypot(from[0] + t * dx - 30, from[1] + t * dy - 30) >= 3 + width * 0.5 - 0.005001,
      );
    }
  }
  assert.ok(model.every(([fine, bulk]) => fine > 0 && bulk > 0));
  return {
    definitions: returned.definitions.length,
    model_events: model,
    actual_segments: report.deposition_segments,
    tool_selections: report.routing.selection_count,
    geometry_work: plan.native_geometry.geometry_work,
    choice: selection.selected,
    candidate_work: selection.candidates,
  };
}
