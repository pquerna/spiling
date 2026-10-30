/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { decodeReplayDisplay } from "@spiling/protocol";
import { createReplayDisplay, replayBudget } from "../src/replay/index.ts";

test("declared tool colors preserve actual packed geometry, draw ranges, and owned buffer costs", async () => {
  const fixture = JSON.parse(
    await readFile(
      new URL("../../../fixtures/protocol/replay-fixed-dual.json", import.meta.url),
      "utf8",
    ),
  );
  const bytes = Uint8Array.from(Buffer.from(fixture.data, "hex")).buffer;
  const replay = await decodeReplayDisplay(bytes, fixture.descriptor);
  const original = new Uint8Array(bytes).slice();
  const display = createReplayDisplay(fixture.descriptor, replay, null);
  try {
    const batches = display.root.children.filter((child) => child.isLineSegments);
    const positions = batches.map((batch) => Array.from(batch.geometry.attributes.position.array));
    const roleColors = batches.map((batch) => batch.material.color.getHexString());
    const view = { layer: null, eventCount: replay.eventCount, showTravel: true };
    display.setView({ ...view, toolColors: ["#123456", "#ffffff"] });
    const colors = batches.map((batch) => batch.material.color.getHexString());
    assert.ok(colors.includes("123456") && colors.includes("ffffff"));
    // Every recolored batch must connect endpoints belonging to the same
    // independently decoded active tool. Travel/process retain role colors.
    for (const batch of batches) {
      const color = batch.material.color.getHexString();
      if (!["123456", "ffffff"].includes(color)) continue;
      const points = batch.geometry.attributes.position.array;
      for (let offset = 0; offset < points.length; offset += 6) {
        const end = Array.from(points.slice(offset + 3, offset + 6)).map(
          (v, axis) => v + display.origin.getComponent(axis),
        );
        const matches = [];
        for (let event = 1; event < replay.eventCount; event++) {
          if (![1, 2, 3, 4, 5, 8, 13, 15].includes(replay.kind(event))) continue;
          if (replay.position(event).every((v, axis) => Math.abs(v - end[axis]) < 0.001))
            matches.push(replay.activeTool(event));
        }
        assert.ok(matches.includes(color === "123456" ? 0 : 1), "tint follows actual tool epochs");
      }
    }
    display.setView({ ...view, layer: 0, eventCount: 4, toolColors: ["#123456", "#ffffff"] });
    assert.ok(batches.some((batch) => batch.geometry.drawRange.count === 0));
    display.setView(view);
    assert.deepEqual(
      batches.map((batch) => batch.material.color.getHexString()),
      roleColors,
    );
    assert.deepEqual(
      batches.map((batch) => Array.from(batch.geometry.attributes.position.array)),
      positions,
    );
    assert.deepEqual(new Uint8Array(bytes), original);
    assert.deepEqual(
      { rawBytes: display.rawBytes, gpuBytes: display.gpuBytes },
      replayBudget(replay),
    );
    assert.throws(
      () => display.setView({ ...view, toolColors: ["blue", "white"] }),
      /invalid draw state/,
    );
  } finally {
    display.dispose();
  }
});
