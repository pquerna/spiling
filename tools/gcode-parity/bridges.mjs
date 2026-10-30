/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
import { observeGaps } from "./anchors.mjs";
const EPS = 0.002;
const near = (a, b) => a.every((v, i) => Math.abs(v - b[i]) < 1e-6);
const distance = (p, s) => {
  const dx = s.to[0] - s.from[0],
    dy = s.to[1] - s.from[1];
  const t = Math.max(
    0,
    Math.min(1, ((p[0] - s.from[0]) * dx + (p[1] - s.from[1]) * dy) / (dx * dx + dy * dy)),
  );
  return Math.hypot(p[0] - s.from[0] - t * dx, p[1] - s.from[1] - t * dy);
};
// A source partition may split one uninterrupted straight deposition into
// multiple G1 records. Anchor endpoints belong to the continuous run; retain
// the raw records for process/dose observations. Travel, reversal or a corner
// closes the run. Comments cannot join disconnected commands.
export function depositionRuns(strokes, travels = []) {
  const runs = [];
  let travelIndex = 0,
    lastTravel = -1;
  for (const stroke of strokes) {
    while (travelIndex < travels.length && travels[travelIndex].line < stroke.line)
      lastTravel = travels[travelIndex++].line;
    const previous = runs.at(-1);
    const a = previous && [previous.to[0] - previous.from[0], previous.to[1] - previous.from[1]];
    const b = [stroke.to[0] - stroke.from[0], stroke.to[1] - stroke.from[1]];
    if (
      previous &&
      lastTravel <= previous.lastLine &&
      near(previous.to, stroke.from) &&
      a[0] * b[0] + a[1] * b[1] > 0 &&
      Math.abs(a[0] * b[1] - a[1] * b[0]) <= 1e-8 * Math.hypot(...a)
    ) {
      previous.to = stroke.to;
      previous.lastLine = stroke.line;
      previous.lines.push(stroke.line);
      previous.lengthMm += stroke.lengthMm;
    } else runs.push({ ...stroke, lastLine: stroke.line, lines: [stroke.line] });
  }
  return runs;
}
// Independent analytic source and nominal-width observations. No plan paths,
// role comments, native receipts or reference preview supply geometry here.
export function auditBridge(decoded, source, { prime = null, widthMm = 0.45 } = {}) {
  if (
    ![6, 10, 14].includes(source.widthMm) ||
    !["span-2", "span-6", "span-10", "one-ended"].includes(source.id)
  )
    throw Error("unsupported frozen bridge source");
  const violations = [],
    layers = new Map();
  let previousZ = 0,
    primeCount = 0,
    work = 0;
  const charge = () => {
    if (++work > 40_000_000) throw Error("bridge observer work limit");
  };
  const problem = (code, witness) => {
    if (violations.length < 40 && !violations.some((v) => v.code === code))
      violations.push({ code, witness });
  };
  const maxX = source.widthMm;
  for (const s of decoded.strokes) {
    if (
      prime &&
      near(s.from, prime.prime_start_mm) &&
      near(s.to, prime.prime_end_mm) &&
      Math.abs(s.filamentMm - prime.prime_filament_length_mm) < 1e-7
    ) {
      primeCount++;
      continue;
    }
    if (
      ![s.from, s.to].every(
        (p) =>
          p[0] >= -EPS &&
          p[0] <= maxX + EPS &&
          p[1] >= -EPS &&
          p[1] <= 6 + EPS &&
          p[2] > 0 &&
          p[2] <= 4 + EPS,
      )
    )
      problem("outside_source_bounds", { line: s.line, from: s.from, to: s.to });
    if (s.to[2] < previousZ - 1e-6) problem("descending_deposition", { line: s.line });
    previousZ = s.to[2];
    const z = Number(s.to[2].toFixed(6));
    if (!layers.has(z)) layers.set(z, []);
    layers.get(z).push(s);
  }
  if (prime && primeCount !== 1) problem("prime_count", primeCount);
  if (decoded.stationaryFilamentMm > 1e-8)
    problem("stationary_deposition", decoded.stationaryFilamentMm);
  const sorted = [...layers].sort((a, b) => a[0] - b[0]);
  if (Math.abs((sorted.at(-1)?.[0] ?? 0) - 4) > EPS) problem("model_height", sorted.at(-1)?.[0]);
  let lower = 0,
    previous = [];
  const rows = [];
  for (const [z, strokes] of sorted) {
    const height = z - lower;
    if (height <= 1e-6 || height > 0.202) problem("layer_interval", { lower, z });
    const postOnly = z <= 3 + 1e-6;
    const intervals = postOnly
      ? source.id === "one-ended"
        ? [[0, 2]]
        : [
            [0, 2],
            [maxX - 2, maxX],
          ]
      : [[0, maxX]];
    const row = {
      lowerMm: lower,
      upperMm: z,
      volumeMm3: 0,
      lengthMm: 0,
      strokeCount: strokes.length,
      gapStrokeCount: 0,
      twoEndedGapCount: 0,
      oneOrZeroEndedGapCount: 0,
      maximumGapMm: 0,
      gapProcess: [],
      doseMm2Range: [Infinity, 0],
      feedMmS: [],
      fanPwm: [],
      examples: [],
      rawOneOrZeroEndedGapCount: 0,
      requiredGridSamples: 0,
      uncoveredGridSamples: 0,
      uncoveredExamples: [],
      uncoveredPoints: [],
    };
    const feeds = new Set(),
      fans = new Set(),
      seen = new Set();
    for (const s of strokes) {
      row.volumeMm3 += s.volumeMm3;
      row.lengthMm += s.lengthMm;
      row.doseMm2Range[0] = Math.min(row.doseMm2Range[0], s.volumeMm3 / s.lengthMm);
      row.doseMm2Range[1] = Math.max(row.doseMm2Range[1], s.volumeMm3 / s.lengthMm);
      feeds.add(s.feedMmMin / 60);
      fans.add(s.fanPwm);
      const key = [s.from.join(","), s.to.join(",")].sort().join("/");
      if (seen.has(key)) problem("duplicate_stroke", { line: s.line, z });
      seen.add(key);
      // Whole straight centerline remains inside one convex source rectangle.
      if (
        !intervals.some(([lo, hi]) =>
          [s.from, s.to].every((p) => p[0] >= lo - EPS && p[0] <= hi + EPS),
        )
      )
        problem("extrusion_in_source_void", { line: s.line, z, from: s.from, to: s.to });
      if (previous.length) {
        const gaps = observeGaps(s, previous, widthMm, charge).gaps.filter((g) => g.spanMm > EPS);
        if (gaps.length) {
          row.gapStrokeCount++;
          row.gapProcess.push({
            feedMmS: s.feedMmMin / 60,
            fanPwm: s.fanPwm,
            doseMm2: s.volumeMm3 / s.lengthMm,
          });
          row.rawOneOrZeroEndedGapCount += gaps.filter((g) => !g.left || !g.right).length;
        }
      }
    }
    const runs = depositionRuns(strokes, decoded.travels);
    row.continuousRunCount = runs.length;
    if (previous.length)
      for (const run of runs) {
        const gaps = observeGaps(run, previous, widthMm, charge).gaps.filter((g) => g.spanMm > EPS);
        for (const g of gaps) {
          row.maximumGapMm = Math.max(row.maximumGapMm, g.spanMm);
          if (g.left && g.right) row.twoEndedGapCount++;
          else row.oneOrZeroEndedGapCount++;
        }
        if (gaps.length && row.examples.length < 4)
          row.examples.push({ lines: run.lines, from: run.from, to: run.to, gaps });
      }
    // Interior grid covers all requested solid posts and roof, independently
    // per physical interval. Nominal footprint and actual XYZ/E dose are kept
    // separate: circular versus stadium flow is not an inferred bead width.
    for (const [lo, hi] of intervals)
      for (let x = lo + 0.137; x < hi - 0.1; x += 0.15)
        for (let y = 0.137; y < 5.9; y += 0.15) {
          row.requiredGridSamples++;
          let covered = false;
          for (const s of strokes) {
            charge();
            if (distance([x, y], s) <= widthMm / 2 + EPS) {
              covered = true;
              break;
            }
          }
          if (!covered) {
            row.uncoveredGridSamples++;
            row.uncoveredPoints.push([x, y]);
            if (row.uncoveredExamples.length < 4) row.uncoveredExamples.push([x, y]);
            problem("mandatory_nominal_coverage", { z, point: [x, y] });
          }
        }
    row.feedMmS = [...feeds].sort((a, b) => a - b);
    row.fanPwm = [...fans].sort((a, b) => a - b);
    rows.push(row);
    previous = strokes;
    lower = z;
  }
  if (!rows.length) problem("missing_model", null);
  return {
    correctnessPassed: violations.length === 0,
    violations,
    layerCount: rows.length,
    modelVolumeMm3: rows.reduce((sum, r) => sum + r.volumeMm3, 0),
    layers: rows,
    work,
    assumptions:
      "Frozen analytic planar prism; 0.15 mm interior grid, declared 0.45 mm nominal capsules and 0.002 mm command observation allowance. Actual extrusion dose retained separately. Lower-layer anchors observe uninterrupted collinear XYZ deposition runs; raw split-command endpoint counts remain separate; no strength, sag, physical grip or production geometry proof.",
  };
}
