/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
import { depositionRuns } from "./bridges.mjs";
const WIDTH = 0.45,
  HEIGHT = 0.2,
  TOP = 4;
const filamentArea = Math.PI * (1.75 / 2) ** 2;
const sum = (xs, f) => xs.reduce((v, x) => v + f(x), 0);
const length = (s) => Math.hypot(s.to[0] - s.from[0], s.to[1] - s.from[1]);
const stadium = (w, h) => h * (w - h * (1 - Math.PI / 4));
const distance = (a, b) => Math.max(...a.map((v, i) => Math.abs(v - b[i])));
const edgeError = (a, b) =>
  Math.min(
    Math.max(distance(a.from, b.from), distance(a.to, b.to)),
    Math.max(distance(a.from, b.to), distance(a.to, b.from)),
  );

// A forward mathematical hypothesis for the frozen external rectilinear roof.
// No decoded pitch, row count, role hint or E value is an input. This observer
// does not implement manufacturing and is never linked into the native planner.
export function predictExternalRoof(source, { angleDeg = 0, flowMultiplier = 1 } = {}) {
  const widths = { "span-2": 6, "span-6": 10, "span-10": 14, "one-ended": 10 };
  if (
    widths[source.id] !== source.widthMm ||
    ![0, 90].includes(angleDeg) ||
    ![0.95, 1].includes(flowMultiplier)
  )
    throw Error("external roof prediction requires a frozen source and captured controls");
  const alongMm = angleDeg === 0 ? 6 : source.widthMm;
  const crossMm = angleDeg === 0 ? source.widthMm : 6;
  const nominalPitchMm = WIDTH - HEIGHT * (1 - Math.PI / 4);
  const fillBoundaryMm = (WIDTH + nominalPitchMm) / 2;
  const count = Math.floor((crossMm - 2 * fillBoundaryMm) / nominalPitchMm);
  const pitchMm = (crossMm - 2 * fillBoundaryMm) / count;
  if (!(pitchMm < 1.2 * nominalPitchMm))
    throw Error("external roof prediction excludes the reference spacing cap");
  const turnInsetMm = fillBoundaryMm + nominalPitchMm / 2;
  const freeEndInsetMm = fillBoundaryMm + 0.05 * nominalPitchMm;
  const point = (cross, along) => (angleDeg === 0 ? [cross, along, TOP] : [along, cross, TOP]);
  const box = [
    [WIDTH / 2, WIDTH / 2, TOP],
    [source.widthMm - WIDTH / 2, WIDTH / 2, TOP],
    [source.widthMm - WIDTH / 2, 6 - WIDTH / 2, TOP],
    [WIDTH / 2, 6 - WIDTH / 2, TOP],
  ];
  const walls = box.map((from, i) => ({
    from,
    to: box[(i + 1) % 4],
    bucket: "perimeter",
    doseMm2: stadium(WIDTH, HEIGHT) * flowMultiplier,
  }));
  const candidates = [false, true].map((reflectAlong) => {
    const p = (cross, along) => point(cross, reflectAlong ? alongMm - along : along);
    const strokes = [...walls];
    for (let i = 0; i < count; i++) {
      const cross = fillBoundaryMm + (i + 0.5) * pitchMm;
      let start = i % 2 === 0 ? alongMm - turnInsetMm : turnInsetMm;
      let end = i % 2 === 0 ? turnInsetMm : alongMm - turnInsetMm;
      if (i === 0) start = alongMm - freeEndInsetMm;
      if (i === count - 1) end = i % 2 === 0 ? freeEndInsetMm : alongMm - freeEndInsetMm;
      strokes.push({
        from: p(cross, start),
        to: p(cross, end),
        bucket: "row",
        doseMm2: HEIGHT * pitchMm * flowMultiplier,
      });
      if (i < count - 1)
        strokes.push({
          from: p(cross, end),
          to: p(cross + pitchMm, end),
          bucket: "link",
          doseMm2: HEIGHT * pitchMm * flowMultiplier,
        });
    }
    return { reflectAlong, strokes };
  });
  return {
    source: { id: source.id, widthMm: source.widthMm },
    angleDeg,
    flowMultiplier,
    alongMm,
    crossMm,
    nominalPitchMm,
    fillBoundaryMm,
    rowCount: count,
    linkCount: count - 1,
    pitchMm,
    doseMm2: HEIGHT * pitchMm * flowMultiplier,
    turnInsetMm,
    freeEndInsetMm,
    // Reference XYZ rounds to 0.001 mm after micrometre-grid construction.
    // One grid rounding per fitted step plus boundary rounding bounds the
    // pre-serialization drift; the baseline 0.002 mm coverage gate is untouched.
    positionAllowanceMm: 0.0005 + 2 * (count + 3) * 1e-6,
    expectedVolumeMm3: sum(candidates[0].strokes, (s) => length(s) * s.doseMm2),
    candidates,
    scope:
      "Terminal 3.8..4.0 mm reference external solid layer only; classic single 0.45 mm wall, zero overlap, rectilinear 0/90-degree fill, 0.2 mm height and captured 0.95/1 flow. Geometry/E predictions are nominal software evidence, not physical bead, strength or universal slicer authority.",
  };
}

export function observeExternalRoof(decoded, prediction) {
  if (decoded.strokes.length > 100_000) throw Error("external roof observer input limit");
  const strokes = decoded.strokes.filter((s) => Math.abs(s.to[2] - TOP) < 1e-6);
  const byLine = new Map(strokes.map((s) => [s.line, s]));
  const runs = depositionRuns(strokes, decoded.travels);
  const candidates = prediction.candidates.map((candidate) => {
    const unmatched = [...candidate.strokes];
    const matches = [],
      violations = [];
    for (const run of runs) {
      const index = unmatched.findIndex(
        (expected) => edgeError(run, expected) <= prediction.positionAllowanceMm,
      );
      if (index < 0) {
        violations.push({
          code: "unpredicted_geometry",
          lines: run.lines,
          from: run.from,
          to: run.to,
        });
        continue;
      }
      const expected = unmatched.splice(index, 1)[0];
      const commands = run.lines.map((line) => {
        const s = byLine.get(line);
        const residualMm3 = s.volumeMm3 - s.lengthMm * expected.doseMm2;
        // Two XYZ endpoints and two absolute E coordinates; E has five
        // decimals in the pinned reference. This bound is local per command,
        // not a percentage of job/layer material. Splits cannot cancel defects.
        const allowanceMm3 =
          2 * Math.SQRT2 * prediction.positionAllowanceMm * expected.doseMm2 +
          filamentArea * 1e-5 * prediction.flowMultiplier;
        if (Math.abs(residualMm3) > allowanceMm3)
          violations.push({ code: "unpredicted_dose", line, residualMm3, allowanceMm3 });
        return { line, lengthMm: s.lengthMm, volumeMm3: s.volumeMm3, residualMm3, allowanceMm3 };
      });
      matches.push({
        bucket: expected.bucket,
        lines: run.lines,
        from: run.from,
        to: run.to,
        predictedFrom: expected.from,
        predictedTo: expected.to,
        positionErrorMm: edgeError(run, expected),
        expectedDoseMm2: expected.doseMm2,
        commands,
      });
    }
    if (unmatched.length)
      violations.push({
        code: "missing_predicted_geometry",
        count: unmatched.length,
        examples: unmatched.slice(0, 3),
      });
    return {
      reflectAlong: candidate.reflectAlong,
      matches,
      violations,
      accepted: violations.length === 0 && runs.length === candidate.strokes.length,
    };
  });
  const best =
    candidates.find((c) => c.accepted) ??
    candidates.sort(
      (a, b) => b.matches.length - a.matches.length || a.violations.length - b.violations.length,
    )[0];
  const buckets = Object.fromEntries(
    ["perimeter", "row", "link"].map((bucket) => {
      const rows = best.matches.filter((m) => m.bucket === bucket),
        commands = rows.flatMap((m) => m.commands);
      return [
        bucket,
        {
          runCount: rows.length,
          commandCount: commands.length,
          lengthMm: sum(commands, (s) => s.lengthMm),
          volumeMm3: sum(commands, (s) => s.volumeMm3),
        },
      ];
    }),
  );
  return {
    ...best,
    buckets,
    observedStrokeCount: strokes.length,
    observedRunCount: runs.length,
    observedVolumeMm3: sum(strokes, (s) => s.volumeMm3),
    forwardVolumeResidualMm3: sum(strokes, (s) => s.volumeMm3) - prediction.expectedVolumeMm3,
    everyCommandAccounted:
      best.matches.reduce((n, m) => n + m.commands.length, 0) === strokes.length,
  };
}

export function evaluateExternalRoofControl({
  prediction,
  controlPrediction,
  baseline,
  control,
  resolvedSettingsChanges,
}) {
  const sameSource =
    prediction.source.id === controlPrediction.source.id &&
    prediction.source.widthMm === controlPrediction.source.widthMm;
  const matchPerimeter = baseline.matches
    .filter((m) => m.bucket === "perimeter")
    .every((m) => control.matches.some((c) => c.bucket === "perimeter" && edgeError(m, c) < 1e-6));
  const perimeterAllowance = 4 * filamentArea * 1e-5 * prediction.flowMultiplier;
  const checks = {
    unchangedSourceAndOnlySelectedControl:
      sameSource &&
      prediction.angleDeg === 0 &&
      controlPrediction.angleDeg === 90 &&
      prediction.flowMultiplier === controlPrediction.flowMultiplier &&
      resolvedSettingsChanges?.length === 1 &&
      resolvedSettingsChanges[0].key === "fill_angle" &&
      resolvedSettingsChanges[0].baseline === "0" &&
      resolvedSettingsChanges[0].control === "90",
    baselineMatchesForwardRowsLinksAndDose: baseline.accepted && baseline.everyCommandAccounted,
    rotatedControlMatchesForwardRowsLinksAndDose: control.accepted && control.everyCommandAccounted,
    rowCountAndPitchChangeAsPredicted:
      prediction.rowCount !== controlPrediction.rowCount &&
      Math.abs(prediction.pitchMm - controlPrediction.pitchMm) > 0.005 &&
      baseline.buckets.row.runCount === prediction.rowCount &&
      control.buckets.row.runCount === controlPrediction.rowCount,
    perimeterGeometryAndDoseInvariant:
      matchPerimeter &&
      baseline.buckets.perimeter.runCount === 4 &&
      control.buckets.perimeter.runCount === 4 &&
      Math.abs(baseline.buckets.perimeter.volumeMm3 - control.buckets.perimeter.volumeMm3) <=
        perimeterAllowance,
  };
  return {
    accepted: Object.values(checks).every((v) => v === true),
    checks,
    explainedPolicy:
      "External solid rectilinear fill fits floor row count to available cross-width and changes dose with pitch. The 90-degree control swaps cross/along extents and independently predicts every row, turn link, free end and E dose while preserving the perimeter.",
    limits:
      "Explains the selected terminal roof construction rule. Strict baseline/probe coverage flags, other reference policies, native actual-height scheduling, complete-comparison status and physical qualification retain their separate evidence/gates.",
  };
}

// Compare complete resolved inert reference settings, excluding only comments
// and blank lines (including the nonsemantic generated-at header).
export function referenceSettingsChanges(baseline, control) {
  const parse = (text) => {
    if (text.length > 1_000_000) throw Error("resolved reference settings limit");
    const map = new Map();
    for (const line of text.split(/\r?\n/)) {
      if (!line.trim() || line.trimStart().startsWith("#")) continue;
      const match = /^([a-z_0-9]+) = (.*)$/.exec(line);
      if (!match || map.has(match[1]))
        throw Error("invalid or duplicate resolved reference setting");
      map.set(match[1], match[2]);
    }
    return map;
  };
  const a = parse(baseline),
    b = parse(control);
  return [...new Set([...a.keys(), ...b.keys()])]
    .sort()
    .filter((key) => a.get(key) !== b.get(key))
    .map((key) => ({ key, baseline: a.get(key) ?? null, control: b.get(key) ?? null }));
}

function pointDistance(point, edge) {
  const dx = edge.to[0] - edge.from[0],
    dy = edge.to[1] - edge.from[1];
  const t = Math.max(
    0,
    Math.min(
      1,
      ((point[0] - edge.from[0]) * dx + (point[1] - edge.from[1]) * dy) / (dx * dx + dy * dy),
    ),
  );
  return Math.hypot(point[0] - edge.from[0] - t * dx, point[1] - edge.from[1] - t * dy);
}
// Freeze these forecasts before emitting the rotated control. The existing
// baseline determines its seam alternative; BOTH permitted rotated seam
// alternatives must cover the same point with room for command quantization.
export function forecastTerminalMisses(
  prediction,
  baselineObservation,
  controlPrediction,
  terminalAudit,
) {
  if (
    terminalAudit.upperMm !== TOP ||
    terminalAudit.uncoveredPoints?.length !== terminalAudit.uncoveredGridSamples
  )
    throw Error("complete terminal nominal coverage witnesses required");
  const baseline = prediction.candidates.find(
    (c) => c.reflectAlong === baselineObservation.reflectAlong,
  );
  const radiusMm = WIDTH / 2 + 0.002;
  const points = terminalAudit.uncoveredPoints.map((point) => {
    const baselineDistanceMm = Math.min(...baseline.strokes.map((s) => pointDistance(point, s)));
    const controlDistancesMm = controlPrediction.candidates.map((c) =>
      Math.min(...c.strokes.map((s) => pointDistance(point, s))),
    );
    return {
      point,
      baselineDistanceMm,
      controlDistancesMm,
      baselineMissPredicted:
        baselineDistanceMm - Math.SQRT2 * prediction.positionAllowanceMm > radiusMm,
      rotatedCoveragePredictedForEitherSeam: controlDistancesMm.every(
        (d) => d + Math.SQRT2 * controlPrediction.positionAllowanceMm < radiusMm,
      ),
    };
  });
  return {
    sampleCount: points.length,
    points,
    radiusMm,
    accepted:
      points.length > 0 &&
      points.every((p) => p.baselineMissPredicted && p.rotatedCoveragePredictedForEitherSeam),
    scope:
      "Forecast for existing terminal baseline misses only, using the unchanged fixed nominal capsule radius and command allowance. Both rotated seam alternatives must cover the same coordinate; no physical footprint or whole-program coverage pass follows.",
  };
}
