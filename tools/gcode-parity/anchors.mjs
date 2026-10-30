/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Independent nominal-capsule observer over emitted XYZ/E only. It does not
// certify planner anchors, bead shape, strength or physical bridge performance.
const MAX_PAIRS = 8_000_000;
const MAX_EXAMPLES = 12;
const dot = (a, b) => a[0] * b[0] + a[1] * b[1];
const sub = (a, b) => [a[0] - b[0], a[1] - b[1]];
function band(value, change, lower, upper) {
  if (Math.abs(change) < 1e-14) return value >= lower && value <= upper ? [0, 1] : null;
  const a = (lower - value) / change,
    b = (upper - value) / change;
  return [Math.max(0, Math.min(a, b)), Math.min(1, Math.max(a, b))];
}
function circle(p, v, center, radius) {
  const d = sub(p, center),
    a = dot(v, v),
    b = dot(d, v),
    c = dot(d, d) - radius ** 2;
  const discriminant = b * b - a * c;
  if (discriminant <= 0) return null; // isolated tangencies supply no interval
  const root = Math.sqrt(discriminant);
  return [Math.max(0, (-b - root) / a), Math.min(1, (-b + root) / a)];
}
function pointDistance(p, a, b) {
  const v = sub(b, a),
    d = sub(p, a),
    t = Math.max(0, Math.min(1, dot(d, v) / dot(v, v)));
  return Math.hypot(d[0] - t * v[0], d[1] - t * v[1]);
}
export function observeGaps(stroke, previous, widthMm, charge = () => {}) {
  const p = stroke.from,
    v = sub(stroke.to, p),
    length = Math.hypot(...v),
    intervals = [];
  if (!(widthMm > 0) || !Number.isFinite(widthMm) || !(length > 0))
    throw new Error("anchor observer needs finite positive width and stroke length");
  let leftEndpoint = false,
    rightEndpoint = false;
  for (const lower of previous) {
    charge();
    const axis = sub(lower.to, lower.from),
      n = Math.hypot(...axis);
    if (!(n > 0)) throw new Error("anchor observer encountered empty lower stroke");
    const unit = axis.map((x) => x / n),
      normal = [-unit[1], unit[0]],
      relative = sub(p, lower.from);
    const along = band(dot(relative, unit), dot(v, unit), 0, n);
    const across = band(dot(relative, normal), dot(v, normal), -widthMm / 2, widthMm / 2);
    const strip =
      along && across ? [Math.max(along[0], across[0]), Math.min(along[1], across[1])] : null;
    for (const interval of [
      strip,
      circle(p, v, lower.from, widthMm / 2),
      circle(p, v, lower.to, widthMm / 2),
    ])
      if (interval && interval[1] > interval[0]) intervals.push(interval);
    // Separate endpoint disk overlap; it does not dilate interior capsules.
    leftEndpoint ||= pointDistance(p, lower.from, lower.to) < widthMm;
    rightEndpoint ||= pointDistance(stroke.to, lower.from, lower.to) < widthMm;
  }
  intervals.sort((a, b) => a[0] - b[0]);
  const merged = [];
  for (const interval of intervals) {
    if (merged.length && interval[0] <= merged.at(-1)[1])
      merged.at(-1)[1] = Math.max(merged.at(-1)[1], interval[1]);
    else merged.push([...interval]);
  }
  const gaps = [];
  let end = 0;
  for (const interval of [...merged, [1, 1]]) {
    if (interval[0] > end)
      gaps.push({
        startMm: end * length,
        endMm: interval[0] * length,
        spanMm: (interval[0] - end) * length,
        left: end > 0 || leftEndpoint,
        right: interval[0] < 1 || rightEndpoint,
      });
    end = Math.max(end, interval[1]);
  }
  return { gaps, coveredLengthMm: length - gaps.reduce((sum, g) => sum + g.spanMm, 0) };
}

export function observeCubeAnchors(decoded, specification) {
  const layers = new Map();
  for (const s of decoded.strokes) {
    if (![s.from, s.to].every((p) => p.every((v) => v >= 0 && v <= specification.sideMm))) continue;
    if (!layers.has(s.to[2])) layers.set(s.to[2], []);
    layers.get(s.to[2]).push(s);
  }
  const result = {
    workPairs: 0,
    nominalWidthMm: specification.lineWidthMm,
    layers: [],
    examples: [],
  };
  let previous = [],
    lowerZ = 0;
  for (const [z, strokes] of [...layers].sort((a, b) => a[0] - b[0])) {
    const row = {
      lowerMm: lowerZ,
      upperMm: z,
      strokes: strokes.length,
      gapStrokeCount: 0,
      maximumGapMm: 0,
      twoEndedGapCount: 0,
      oneOrZeroEndedGapCount: 0,
      gapStrokeVolumeMm3: 0,
      gapLengthMm: 0,
      processSignatureVolumeMm3: 0,
      processSignatureWithGapsMm3: 0,
      ordinaryWithGapsMm3: 0,
      strokeLengthMm: 0,
      interiorStraightRows: 0,
      interiorLengthMm: 0,
      interiorVolumeMm3: 0,
    };
    for (const s of strokes) {
      row.strokeLengthMm += s.lengthMm;
      const interior = [0, 1].some(
        (axis) =>
          Math.abs(s.from[axis] - s.to[axis]) < 1e-8 &&
          s.from[axis] > specification.walls * specification.lineWidthMm &&
          s.from[axis] < specification.sideMm - specification.walls * specification.lineWidthMm &&
          s.lengthMm > specification.sideMm / 2,
      );
      if (interior) {
        row.interiorStraightRows++;
        row.interiorLengthMm += s.lengthMm;
        row.interiorVolumeMm3 += s.volumeMm3;
      }
      const signature = s.feedMmMin === 1260 && s.fanPwm === 255;
      if (signature) row.processSignatureVolumeMm3 += s.volumeMm3;
      if (!previous.length) continue; // bed support is not a previous deposited layer
      const observed = observeGaps(s, previous, specification.lineWidthMm, () => {
        if (++result.workPairs > MAX_PAIRS)
          throw new Error("anchor observer pair-work limit exceeded");
      });
      // XYZ3 reference positions/widths are approximate observations. Do not
      // label microscopic representation differences as substantial free spans.
      const gaps = observed.gaps.filter((g) => g.spanMm > 0.002);
      if (!gaps.length) continue;
      row.gapStrokeCount++;
      row.gapStrokeVolumeMm3 += s.volumeMm3;
      if (signature) row.processSignatureWithGapsMm3 += s.volumeMm3;
      else row.ordinaryWithGapsMm3 += s.volumeMm3;
      for (const gap of gaps) {
        row.maximumGapMm = Math.max(row.maximumGapMm, gap.spanMm);
        row.gapLengthMm += gap.spanMm;
        if (gap.left && gap.right) row.twoEndedGapCount++;
        else row.oneOrZeroEndedGapCount++;
      }
      if (
        interior &&
        lowerZ >= specification.skinThicknessMm &&
        z <= specification.sideMm - specification.skinThicknessMm &&
        result.examples.length < MAX_EXAMPLES
      )
        result.examples.push({
          line: s.line,
          lowerMm: lowerZ,
          upperMm: z,
          from: s.from,
          to: s.to,
          feedMmS: s.feedMmMin / 60,
          fanPwm: s.fanPwm,
          volumeMm3: s.volumeMm3,
          previousStrokeLines: previous
            .filter((p) => observeGaps(s, [p], specification.lineWidthMm).coveredLengthMm > 0)
            .map((p) => p.line),
          gaps,
        });
    }
    result.layers.push(row);
    previous = strokes;
    lowerZ = z;
  }
  result.assumptions =
    "Independent emitted-XYZ centerline intersection with declared-width lower capsules; endpoint disk overlap is separate. Gaps <=0.002mm are retained only in raw calculations, excluded from substantial-gap counts. Command precision and nominal bead geometry are observations, not production anchor certificates or physical printability. Speed/fan signature is not a semantic role.";
  return result;
}
