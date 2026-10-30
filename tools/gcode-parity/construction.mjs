/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Emitted geometry/E accounting. No planner paths or role comments are inputs.
const EPS = 0.002;
const stadiumArea = (width, height) => height * (width - height * (1 - Math.PI / 4));
const sum = (xs, f) => xs.reduce((n, x) => n + f(x), 0);
function totals(strokes) {
  return {
    strokes: strokes.length,
    lengthMm: sum(strokes, (s) => s.lengthMm),
    volumeMm3: sum(strokes, (s) => s.volumeMm3),
  };
}
export function parallelRows(strokes) {
  if (strokes.length < 2) return null;
  const longest = strokes.reduce((a, b) => (a.lengthMm > b.lengthMm ? a : b));
  const dx = (longest.to[0] - longest.from[0]) / longest.lengthMm;
  const dy = (longest.to[1] - longest.from[1]) / longest.lengthMm;
  const aligned = strokes.filter((s) => {
    const x = s.to[0] - s.from[0],
      y = s.to[1] - s.from[1];
    return Math.abs(x * dy - y * dx) <= 2 * EPS;
  });
  if (aligned.length !== strokes.length) return null;
  const offsets = aligned.map((s) => -dy * s.from[0] + dx * s.from[1]).sort((a, b) => a - b);
  const distinct = offsets.filter((v, i) => i === 0 || v - offsets[i - 1] > EPS);
  if (distinct.length < 2) return null;
  const pitches = distinct.slice(1).map((v, i) => v - distinct[i]);
  return {
    count: distinct.length,
    direction: [dx, dy],
    offsetRangeMm: [distinct[0], distinct.at(-1)],
    pitchRangeMm: [Math.min(...pitches), Math.max(...pitches)],
    meanPitchMm: sum(pitches, (v) => v) / pitches.length,
  };
}
export function observeCubeConstruction(decoded, specification) {
  const { sideMm: side, lineWidthMm: width, walls } = specification;
  const groups = new Map();
  for (const s of decoded.strokes) {
    if (![s.from, s.to].every((p) => p.every((v) => v >= -EPS && v <= side + EPS))) continue;
    if (!groups.has(s.to[2])) groups.set(s.to[2], []);
    groups.get(s.to[2]).push(s);
  }
  if (groups.size > 100 || decoded.strokes.length > 100_000)
    throw Error("construction observer work limit");
  let lower = 0;
  const layers = [];
  for (const [z, strokes] of [...groups].sort((a, b) => a[0] - b[0])) {
    const height = z - lower;
    const wall = [],
      rows = [],
      links = [];
    const roundedPitch = width - height * (1 - Math.PI / 4);
    for (const s of strokes) {
      const isWall = [0, 1].some((axis) => {
        if (Math.abs(s.from[axis] - s.to[axis]) > EPS) return false;
        const distance = Math.min(s.from[axis], side - s.from[axis]);
        return Array.from({ length: walls }, (_, i) => i).some((i) =>
          [width, roundedPitch].some(
            (pitch) => Math.abs(distance - (width / 2 + i * pitch)) <= EPS,
          ),
        );
      });
      if (isWall) wall.push(s);
      else if (s.lengthMm > side * 0.6) rows.push(s);
      else links.push(s);
    }
    const processRows = rows.filter((s) => s.feedMmMin === 1260 && s.fanPwm === 255);
    const layout = parallelRows(processRows);
    const circleDiameter = layout ? layout.meanPitchMm - 0.05 : null;
    const circleArea = circleDiameter > 0 ? (Math.PI * circleDiameter ** 2) / 4 : null;
    const nominalArea = stadiumArea(width, height);
    const circularAgreement =
      circleArea !== null &&
      processRows.length >= 3 &&
      layout.pitchRangeMm[1] - layout.pitchRangeMm[0] <= 2 * EPS &&
      processRows.every((s) => Math.abs(s.volumeMm3 / s.lengthMm - circleArea) <= 0.001);
    layers.push({
      lowerMm: lower,
      upperMm: z,
      heightMm: height,
      walls: totals(wall),
      longFill: {
        ...totals(rows),
        layout: parallelRows(rows),
        widthRangeMm: rows.length
          ? [
              Math.min(
                ...rows.map((s) => s.volumeMm3 / s.lengthMm / height + height * (1 - Math.PI / 4)),
              ),
              Math.max(
                ...rows.map((s) => s.volumeMm3 / s.lengthMm / height + height * (1 - Math.PI / 4)),
              ),
            ]
          : null,
      },
      linksAndShortFill: totals(links),
      nominalStadiumAreaMm2: nominalArea,
      allStadiumResidualMm3: sum(strokes, (s) => s.volumeMm3 - s.lengthMm * nominalArea),
      bridgeRows: {
        ...totals(processRows),
        layout,
        areaRangeMm2: processRows.length
          ? [
              Math.min(...processRows.map((s) => s.volumeMm3 / s.lengthMm)),
              Math.max(...processRows.map((s) => s.volumeMm3 / s.lengthMm)),
            ]
          : null,
        fittedCircularDiameterMm: circleDiameter,
        circularAreaMm2: circleArea,
        circularAgreement,
        // Keep a countermodel, so the circular fit cannot become assumed truth.
        stadiumResidualMm3: sum(processRows, (s) => s.volumeMm3 - s.lengthMm * nominalArea),
        circularResidualMm3:
          circleArea === null
            ? null
            : sum(processRows, (s) => s.volumeMm3 - s.lengthMm * circleArea),
        examples: processRows.slice(0, 3).map((s) => ({
          line: s.line,
          from: s.from,
          to: s.to,
          lengthMm: s.lengthMm,
          volumeMm3: s.volumeMm3,
        })),
      },
      reconciliationResidualMm3:
        sum(strokes, (s) => s.volumeMm3) - sum([...wall, ...rows, ...links], (s) => s.volumeMm3),
    });
    lower = z;
  }
  return {
    layers,
    assumptions:
      "Geometry buckets and nominal cross-section fits only; wall identification admits the two declared cube wall pitches. A circular fit uses measured parallel-row pitch minus the reference declared 0.05mm bridge gap. Neither fitted model proves deposited shape or physical coverage. Role comments are unused.",
  };
}
