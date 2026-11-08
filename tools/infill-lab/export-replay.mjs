/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
// Research consumer of the production checked replay decoder. It grants no
// native geometry, physical bead or mechanical authority.
import { readFile, writeFile } from "node:fs/promises";
import { decodeReplayDisplay } from "../../packages/protocol/src/replay.ts";
const [receiptFile, replayFile, outputFile] = process.argv.slice(2);
if (!outputFile) throw new Error("expected RECEIPT SPLR NEW_JSON");
const receipt = JSON.parse(await readFile(receiptFile, "utf8"));
const m = receipt.manufacturing;
const report = m?.verification;
if (!m?.replay_display || !report?.verified) throw new Error("fresh verified replay required");
const buffer = await readFile(replayFile);
const replay = await decodeReplayDisplay(Uint8Array.from(buffer).buffer, m.replay_display);
const roads = [];
let previous, previousE;
let motionSeconds = 0;
for (let i = 0; i < replay.eventCount; ++i) {
  const p = replay.position(i),
    kind = replay.kind(i),
    a = replay.association(i);
  if (previous && replay.knownStart(i) && kind !== 14) {
    const length = Math.hypot(...p.map((x, j) => x - previous[j]));
    const distance = [11, 12].includes(kind) ? Math.abs(replay.extrusion(i) - previousE) : length;
    if (replay.feed(i) > 0) motionSeconds += distance / (replay.feed(i) / 60);
    if (a && ![7, 9, 10, 15].includes(kind)) {
      if (Math.abs(p[2] - previous[2]) > 1e-7) throw new Error("nonplanar road is outside study");
      if (length > 0)
        roads.push({
          start: previous,
          end: p,
          width: a.beadWidthMm,
          lower: a.physicalZMm[0],
          upper: a.physicalZMm[1],
          kind,
          owner: a.regionalOwner,
          layer: replay.layer(i),
          extrusion: replay.extrusion(i),
        });
    }
  }
  previous = p;
  previousE = replay.extrusion(i);
}
await writeFile(
  outputFile,
  JSON.stringify({
    schema: 1,
    software_only: true,
    engine_build: receipt.hello.engine_build,
    descriptor: m.replay_display,
    report,
    roads,
    motion_seconds_lower_bound: motionSeconds,
  }),
  { flag: "wx" },
);
