/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Independent original OCCT recipe: a 24x18x10 box with every edge filleted
// at 2 mm, minus the radius-3 blind cylinder above Z=4.13. No native sections,
// plan paths, source-face labels or reference role comments supply these facts.
export const housing = Object.freeze({
  source: "fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step",
  sha256: "114c2badf2c1b3742c48415debf6aa0ce25749ae2de8a35b248ab6c1421439db",
  heightMm: 10,
  floorMm: 4.13,
  pocket: [12, 9, 3],
  layerMm: 0.1,
  widthMm: 0.45,
  skinMm: 0.8,
  shapeAllowanceMm: 0.1,
  commandAllowanceMm: 0.002,
  coverageGridMm: 0.25,
  bandMm: 0.2,
  core: {
    min: [4, 5],
    max: [7, 13],
    heights: [
      [1.2, 3],
      [5.2, 8],
    ],
  },
});
const EPS = housing.commandAllowanceMm;
const RADIUS = housing.widthMm / 2;
const total = (values, key) => values.reduce((sum, value) => sum + value[key], 0);
const near = (a, b) => a.every((v, i) => Math.abs(v - b[i]) < 1e-6);

export function housingOuterDistance([x, y, z]) {
  const q = [Math.abs(x - 12) - 10, Math.abs(y - 9) - 7, Math.abs(z - 5) - 3];
  return Math.hypot(...q.map((v) => Math.max(v, 0))) + Math.min(Math.max(...q), 0) - 2;
}
export function housingSectionArea(z) {
  if (!(z >= 0 && z <= 10)) return 0;
  const q = Math.sqrt(Math.max(0, 4 - (2 - Math.min(z, 10 - z, 2)) ** 2));
  return 280 + 68 * q + Math.PI * q * q - (z > housing.floorMm ? 9 * Math.PI : 0);
}
function outerPrimitive(z) {
  const cap = (v) => {
    const t = v - 2;
    return (
      280 * v +
      34 * (t * Math.sqrt(Math.max(0, 4 - t * t)) + 4 * Math.asin(t / 2)) +
      Math.PI * (4 * v - t ** 3 / 3)
    );
  };
  const low = (v) => cap(v) - cap(0);
  const middleArea = 416 + 4 * Math.PI;
  const whole = 2 * low(2) + 6 * middleArea;
  if (z <= 0) return 0;
  if (z < 2) return low(z);
  if (z <= 8) return low(2) + (z - 2) * middleArea;
  if (z < 10) return whole - low(10 - z);
  return whole;
}
export function housingBandVolume(lower, upper) {
  if (![lower, upper].every(Number.isFinite) || upper < lower) throw Error("invalid source band");
  return (
    outerPrimitive(upper) -
    outerPrimitive(lower) -
    9 * Math.PI * Math.max(0, Math.min(upper, 10) - Math.max(lower, housing.floorMm))
  );
}
function segmentDistance(point, a, b) {
  const dx = b[0] - a[0],
    dy = b[1] - a[1];
  const t = Math.max(
    0,
    Math.min(1, ((point[0] - a[0]) * dx + (point[1] - a[1]) * dy) / (dx * dx + dy * dy)),
  );
  return Math.hypot(point[0] - a[0] - t * dx, point[1] - a[1] - t * dy);
}
function coreFraction(s) {
  let a = 0,
    b = 1;
  for (let axis = 0; axis < 2; axis++) {
    const delta = s.to[axis] - s.from[axis],
      lo = housing.core.min[axis],
      hi = housing.core.max[axis];
    if (delta === 0) {
      if (s.from[axis] < lo || s.from[axis] > hi) return 0;
    } else {
      const p = (lo - s.from[axis]) / delta,
        q = (hi - s.from[axis]) / delta;
      a = Math.max(a, Math.min(p, q));
      b = Math.min(b, Math.max(p, q));
    }
  }
  return Math.max(0, b - a);
}
function wallRing(z, inset, probe) {
  const q = Math.sqrt(Math.max(0, 4 - (2 - Math.min(z, 10 - z, 2)) ** 2));
  const radius = Math.max(0, q - inset),
    shrink = Math.max(0, inset - q);
  const centers = [
    [22 - shrink, 16 - shrink],
    [2 + shrink, 16 - shrink],
    [2 + shrink, 2 + shrink],
    [22 - shrink, 2 + shrink],
  ];
  for (let corner = 0; corner < 4; corner++) {
    const c = centers[corner],
      start = (corner * Math.PI) / 2;
    for (let n = 0; n <= 16; n++)
      probe([
        c[0] + radius * Math.cos(start + (n * Math.PI) / 32),
        c[1] + radius * Math.sin(start + (n * Math.PI) / 32),
      ]);
    const next = centers[(corner + 1) % 4],
      angle = start + Math.PI / 2;
    const a = [c[0] + radius * Math.cos(angle), c[1] + radius * Math.sin(angle)];
    const b = [next[0] + radius * Math.cos(angle), next[1] + radius * Math.sin(angle)];
    const count = Math.max(1, Math.ceil(Math.hypot(b[0] - a[0], b[1] - a[1]) / 0.2));
    for (let i = 1; i < count; i++)
      probe([a[0] + ((b[0] - a[0]) * i) / count, a[1] + ((b[1] - a[1]) * i) / count]);
  }
}

export function auditHousing(
  decoded,
  { translation = [0, 0, 0], prime = null, doseLimits = null } = {},
) {
  if (translation.length !== 3 || !translation.every(Number.isFinite))
    throw Error("invalid source translation");
  const violations = [],
    groups = new Map();
  const problem = (code, witness) => {
    if (violations.length < 40 && !violations.some((v) => v.code === code))
      violations.push({ code, witness });
  };
  let work = 0,
    previousZ = 0,
    primeCount = 0;
  const charge = () => {
    if (++work > 40_000_000) throw Error("mechanical observer work limit");
  };
  for (const original of decoded.strokes) {
    charge();
    if (
      prime &&
      near(original.from, prime.from) &&
      near(original.to, prime.to) &&
      Math.abs(original.filamentMm - prime.filamentMm) < 1e-7
    ) {
      primeCount++;
      continue;
    }
    const s = {
      ...original,
      from: original.from.map((v, i) => v - translation[i]),
      to: original.to.map((v, i) => v - translation[i]),
    };
    if (Math.abs(s.from[2] - s.to[2]) > 2e-6) problem("nonplanar_deposition", s.line);
    const z = Number(s.to[2].toFixed(6));
    if (z < previousZ - 1e-6) problem("descending_deposition", s.line);
    previousZ = z;
    if (!(z > 0 && z <= 10 + EPS)) problem("model_height", { line: s.line, z });
    if (s.feedMmMin / 60 > 150 + EPS) problem("feed_limit", s.line);
    if (((s.volumeMm3 / s.lengthMm) * s.feedMmMin) / 60 > 12 + EPS) problem("flow_limit", s.line);
    // Nominal horizontal capsule samples are diagnostics, not physical beads.
    for (const p of [s.from, s.to])
      for (let i = 0; i < 16; i++) {
        charge();
        const point = [
          p[0] + RADIUS * Math.cos((i * Math.PI) / 8),
          p[1] + RADIUS * Math.sin((i * Math.PI) / 8),
          p[2],
        ];
        if (housingOuterDistance(point) > housing.shapeAllowanceMm + EPS)
          problem("outside_source", { line: s.line, point });
      }
    if (
      z > housing.floorMm + housing.shapeAllowanceMm + EPS &&
      segmentDistance(housing.pocket, s.from, s.to) - RADIUS <
        housing.pocket[2] - housing.shapeAllowanceMm - EPS
    )
      problem("protected_pocket", { line: s.line, z });
    if (!groups.has(z)) groups.set(z, []);
    groups.get(z).push(s);
  }
  if (primeCount !== (prime ? 1 : 0)) problem("prime_count", primeCount);
  if (decoded.stationaryFilamentMm > 1e-8)
    problem("stationary_deposition", decoded.stationaryFilamentMm);
  const sorted = [...groups].sort((a, b) => a[0] - b[0]);
  if (sorted.length > 500) throw Error("mechanical observer layer limit");
  if (Math.abs((sorted.at(-1)?.[0] ?? 0) - 10) > EPS) problem("model_height", sorted.at(-1)?.[0]);
  const bands = Array.from({ length: 50 }, (_, i) => ({
    lowerMm: i * 0.2,
    upperMm: (i + 1) * 0.2,
    sourceVolumeMm3: housingBandVolume(i * 0.2, (i + 1) * 0.2),
    volumeMm3: 0,
    lengthMm: 0,
    coreVolumeMm3: 0,
    coreLengthMm: 0,
  }));
  const layers = [];
  let lower = 0;
  for (const [z, strokes] of sorted) {
    const height = z - lower,
      middle = (z + lower) / 2;
    if (!(height > 0 && height <= housing.layerMm + EPS)) problem("layer_interval", { lower, z });
    const row = {
      lowerMm: lower,
      upperMm: z,
      volumeMm3: total(strokes, "volumeMm3"),
      lengthMm: total(strokes, "lengthMm"),
      coreVolumeMm3: 0,
      coreLengthMm: 0,
      coverage: Object.fromEntries(
        ["wall", "pocket", "skin", "floor"].map((kind) => [
          kind,
          { samples: 0, nominalMisses: 0, misses: 0, maximumNominalGapMm: 0 },
        ]),
      ),
    };
    const grid = new Map();
    let refs = 0;
    for (const s of strokes) {
      charge();
      if (doseLimits) {
        const area = (w, h) => h * (w - h * (1 - Math.PI / 4));
        const h0 = Math.max(0, height - 1e-6),
          h1 = height + 1e-6;
        const lengthError = Math.SQRT2 * 1e-6;
        const quant = Math.PI * (1.75 / 2) ** 2 * 1e-8;
        const min =
          Math.max(0, s.lengthMm - lengthError) * area(doseLimits.min_bead_width_mm, h0) - quant;
        const max = (s.lengthMm + lengthError) * area(doseLimits.max_bead_width_mm, h1) + quant;
        if (s.volumeMm3 < min || s.volumeMm3 > max)
          problem("local_dose", { line: s.line, volume: s.volumeMm3, min, max });
      }
      const radius = RADIUS + housing.shapeAllowanceMm + EPS;
      const lo = [0, 1].map((axis) =>
        Math.floor((Math.min(s.from[axis], s.to[axis]) - radius) * 2),
      );
      const hi = [0, 1].map((axis) =>
        Math.floor((Math.max(s.from[axis], s.to[axis]) + radius) * 2),
      );
      for (let x = lo[0]; x <= hi[0]; x++)
        for (let y = lo[1]; y <= hi[1]; y++) {
          charge();
          if (++refs > 400_000) throw Error("mechanical observer index limit");
          const key = `${x},${y}`;
          if (!grid.has(key)) grid.set(key, []);
          grid.get(key).push(s);
        }
      const xy = coreFraction(s);
      const coreHeight = housing.core.heights.reduce(
        (sum, [a, b]) => sum + Math.max(0, Math.min(z, b) - Math.max(lower, a)),
        0,
      );
      row.coreVolumeMm3 += (s.volumeMm3 * xy * coreHeight) / height;
      row.coreLengthMm += (s.lengthMm * xy * coreHeight) / height;
      for (const band of bands) {
        charge();
        const overlap = Math.max(0, Math.min(z, band.upperMm) - Math.max(lower, band.lowerMm));
        if (!overlap) continue;
        band.volumeMm3 += (s.volumeMm3 * overlap) / height;
        band.lengthMm += (s.lengthMm * overlap) / height;
        const h = housing.core.heights.reduce(
          (sum, [a, b]) =>
            sum + Math.max(0, Math.min(z, band.upperMm, b) - Math.max(lower, band.lowerMm, a)),
          0,
        );
        band.coreVolumeMm3 += (s.volumeMm3 * xy * h) / height;
        band.coreLengthMm += (s.lengthMm * xy * h) / height;
      }
    }
    const probe = (kind, p) => {
      charge();
      row.coverage[kind].samples++;
      const candidates = grid.get(`${Math.floor(p[0] * 2)},${Math.floor(p[1] * 2)}`) ?? [];
      const closest = candidates.reduce((best, s) => {
        charge();
        return Math.min(best, segmentDistance(p, s.from, s.to));
      }, Infinity);
      const gap = Math.max(0, closest - RADIUS);
      // Strict nominal misses remain visible. The admission predicate is the
      // declared ORIGINAL-source-to-nominal-footprint distance, not perfect
      // physical filling and not a new tolerance fitted to a particular miss.
      if (gap > EPS) row.coverage[kind].nominalMisses++;
      row.coverage[kind].maximumNominalGapMm = Math.max(
        row.coverage[kind].maximumNominalGapMm,
        Number.isFinite(gap) ? gap : 10,
      );
      if (gap > housing.shapeAllowanceMm + EPS) {
        row.coverage[kind].misses++;
        problem(`${kind}_coverage`, { z, point: p, gapMm: Number.isFinite(gap) ? gap : null });
      }
    };
    wallRing(middle, 0.15, (p) => probe("wall", p));
    if (lower > housing.floorMm + housing.shapeAllowanceMm + EPS)
      for (let n = 0; n < 96; n++)
        probe("pocket", [
          12 + 3.15 * Math.cos((n * Math.PI) / 48),
          9 + 3.15 * Math.sin((n * Math.PI) / 48),
        ]);
    const skin = z <= housing.skinMm + EPS || lower >= 10 - housing.skinMm + EPS;
    const floor = lower >= housing.floorMm - housing.skinMm + EPS && z <= housing.floorMm + EPS;
    if (skin || floor)
      for (let x = 0.125; x < 24; x += 0.25)
        for (let y = 0.125; y < 18; y += 0.25) {
          charge();
          const radial = Math.hypot(x - 12, y - 9);
          if (housingOuterDistance([x, y, middle]) > -0.45) continue;
          if (middle > housing.floorMm && radial < 3.45) continue;
          if (skin) probe("skin", [x, y]);
          else if (floor && radial < 2.65) probe("floor", [x, y]);
        }
    layers.push(row);
    lower = z;
  }
  const modelVolumeMm3 = total(layers, "volumeMm3"),
    coreVolumeMm3 = total(layers, "coreVolumeMm3");
  if (
    Math.abs(total(bands, "volumeMm3") - modelVolumeMm3) > 1e-7 ||
    Math.abs(total(bands, "coreVolumeMm3") - coreVolumeMm3) > 1e-7
  )
    problem("band_accounting", "actual intervals do not reconcile");
  const lengthMm = total(layers, "lengthMm");
  const stadium = (h) => h * (housing.widthMm - h * (1 - Math.PI / 4));
  const fixedNominalVolumeMm3 = lengthMm * stadium(housing.layerMm);
  const actualIntervalNominalMm3 = layers.reduce(
    (sum, layer) => sum + layer.lengthMm * stadium(layer.upperMm - layer.lowerMm),
    0,
  );
  const materialAccounting = {
    fixedNominalVolumeMm3,
    actualIntervalAdjustmentMm3: actualIntervalNominalMm3 - fixedNominalVolumeMm3,
    commandedDoseResidualMm3: modelVolumeMm3 - actualIntervalNominalMm3,
    scope:
      "Complete commanded-path accounting at nominal width: fixed-height length term, actual-interval correction, and remaining actual XYZ/E dose. This identity locates differences; it does not explain why paths, widths or flow policies differ.",
  };
  return {
    correctnessPassed: violations.length === 0,
    violations,
    layerCount: layers.length,
    modelVolumeMm3,
    coreVolumeMm3,
    lengthMm,
    materialAccounting,
    coreLengthMm: total(layers, "coreLengthMm"),
    layers,
    bands,
    work,
    scope:
      "Frozen rounded housing recipe; commanded XYZ/E, analytic material/pocket and nominal 0.45 mm capsule diagnostics. Source shape allowance 0.1 mm and command observation allowance 0.002 mm remain separate. Strict nominal misses remain visible; required probes use the predeclared 0.1 mm original-source-to-footprint distance. Coverage uses finite probes, not continuous/physical coverage. Common 0.2 mm bands distribute each actual interval's commanded dose for accounting; no layer-index equivalence or physical print qualification.",
  };
}

// Faults operate on independently decoded actual commands. They never alter
// production settings or loosen admission when an accepted case fails.
export function checkHousingFaults(decoded, options) {
  const baseline = auditHousing(decoded, options);
  if (!baseline.correctnessPassed) throw Error("housing fault controls require a passing baseline");
  const translation = options.translation;
  const local = (s) => ({
    ...s,
    from: s.from.map((v, i) => v - translation[i]),
    to: s.to.map((v, i) => v - translation[i]),
  });
  const controls = [];
  const check = (id, strokes, expectedCode, extra = {}) => {
    const audit = auditHousing({ ...decoded, strokes }, options);
    if (audit.correctnessPassed || !audit.violations.some((v) => v.code === expectedCode))
      throw Error(`housing observer missed ${id}: ${JSON.stringify(audit.violations)}`);
    controls.push({ id, rejected: true, expectedCode, violations: audit.violations, ...extra });
  };
  const remove = (id, predicate, code) => {
    const strokes = decoded.strokes.filter((s) => !predicate(local(s)));
    if (strokes.length === decoded.strokes.length) throw Error(`empty housing fault ${id}`);
    check(id, strokes, code, { removedStrokes: decoded.strokes.length - strokes.length });
  };
  remove(
    "missing_left_wall",
    (s) => Math.min(s.from[0], s.to[0]) < 2 && s.to[2] > 2 && s.to[2] < 3,
    "wall_coverage",
  );
  remove(
    "missing_pocket_wall",
    (s) => s.to[2] > 5 && s.to[2] < 6 && segmentDistance(housing.pocket, s.from, s.to) < 3.8,
    "pocket_coverage",
  );
  remove(
    "missing_top_skin",
    (s) => s.to[2] > 9.3 && Math.min(s.from[0], s.to[0]) > 3 && Math.max(s.from[0], s.to[0]) < 21,
    "skin_coverage",
  );
  remove(
    "missing_pocket_floor",
    (s) => s.to[2] > 3.4 && s.to[2] <= 4.13 && segmentDistance(housing.pocket, s.from, s.to) < 2.9,
    "floor_coverage",
  );
  const indices = decoded.strokes.flatMap((s, i) => {
    const p = local(s);
    return p.to[2] > 5 && p.to[2] < 6 && p.lengthMm > 1 ? [i] : [];
  });
  if (indices.length < 2 || !options.doseLimits)
    throw Error("missing native local-dose fault controls");
  const [first, second] = indices;
  const changed = (index, update) =>
    decoded.strokes.map((s, i) => (i === index ? { ...s, ...update } : s));
  const original = decoded.strokes[first];
  const shifted = (x, y) =>
    Object.fromEntries(
      ["from", "to"].map((key) => [
        key,
        [translation[0] + x + (key === "to" ? 1 : 0), translation[1] + y, original[key][2]],
      ]),
    );
  check("extrusion_into_pocket", changed(first, shifted(11.5, 9)), "protected_pocket");
  check("extrusion_outside_source", changed(first, shifted(-2, 9)), "outside_source");
  const half = changed(first, {
    volumeMm3: original.volumeMm3 / 2,
    filamentMm: original.filamentMm / 2,
  });
  check("half_local_flow", half, "local_dose");
  const compensated = half.map((s, i) =>
    i === second
      ? {
          ...s,
          volumeMm3: s.volumeMm3 + original.volumeMm3 / 2,
          filamentMm: s.filamentMm + original.filamentMm / 2,
        }
      : s,
  );
  const totalChange = total(compensated, "volumeMm3") - total(decoded.strokes, "volumeMm3");
  if (Math.abs(totalChange) > 1e-7) throw Error("balanced housing fault changed total dose");
  check("balanced_local_flow_corruption", compensated, "local_dose", {
    totalVolumeChangeMm3: totalChange,
  });
  return { passed: true, controls };
}

export function compareHousingRevision(before, after) {
  const ratio = after.coreVolumeMm3 / before.coreVolumeMm3;
  const aligned = [before, after].every(
    (audit) =>
      audit.bands?.length === 50 &&
      Math.abs(total(audit.bands, "volumeMm3") - audit.modelVolumeMm3) < 1e-7 &&
      Math.abs(total(audit.bands, "coreVolumeMm3") - audit.coreVolumeMm3) < 1e-7 &&
      audit.bands.every(
        (band, i) =>
          Math.abs(band.lowerMm - i * housing.bandMm) < 1e-9 &&
          Math.abs(band.upperMm - (i + 1) * housing.bandMm) < 1e-9 &&
          [band.volumeMm3, band.coreVolumeMm3].every((v) => Number.isFinite(v) && v >= 0),
      ),
  );
  return {
    passed:
      aligned &&
      before.coreVolumeMm3 > 0 &&
      before.correctnessPassed &&
      after.correctnessPassed &&
      Number.isFinite(ratio) &&
      ratio >= 1.4 &&
      ratio <= 2.6 &&
      after.modelVolumeMm3 > before.modelVolumeMm3,
    requestedSparseFraction: [0.25, 0.5],
    coreDoseRatio: ratio,
    modelVolumeChangeMm3: after.modelVolumeMm3 - before.modelVolumeMm3,
    bands: aligned
      ? before.bands.map((band, i) => ({
          lowerMm: band.lowerMm,
          upperMm: band.upperMm,
          volumeChangeMm3: after.bands[i].volumeMm3 - band.volumeMm3,
          coreVolumeChangeMm3: after.bands[i].coreVolumeMm3 - band.coreVolumeMm3,
        }))
      : [],
    scope:
      "The same fixed left interior box over 1.2..3 and 5.2..8 mm isolates ordinary sparse construction away from walls, the pocket and mandatory skins. Doubling requested fraction must increase actual interior dose within the predeclared finite-row 1.4..2.6 ratio; no whole-job doubling or physical strength inference.",
  };
}

export function housingOverlay(decoded, audit, { translation = [0, 0, 0], prime = null } = {}) {
  const layers = new Map(audit.layers.map((l) => [Number(l.upperMm.toFixed(6)), l]));
  const paths = Array.from({ length: 50 }, () => []);
  for (const stroke of decoded.strokes) {
    if (prime && near(stroke.from, prime.from) && near(stroke.to, prime.to)) continue;
    const a = stroke.from.map((v, i) => v - translation[i]),
      b = stroke.to.map((v, i) => v - translation[i]);
    const layer = layers.get(Number(b[2].toFixed(6)));
    if (!layer) continue;
    const command = `M${a[0]},${a[1]}L${b[0]},${b[1]}`;
    for (let n = 0; n < 50; n++)
      if (layer.upperMm > n * 0.2 + 1e-6 && layer.lowerMm < (n + 1) * 0.2 - 1e-6)
        paths[n].push(command);
  }
  return paths.map((p) => p.join(""));
}
export function housingSourceOverlay() {
  return Array.from({ length: 50 }, (_, i) => {
    const z = (i + 0.5) * 0.2,
      points = [];
    wallRing(z, 0, (p) => points.push(p));
    let path = points.map((p, n) => `${n ? "L" : "M"}${p[0]},${p[1]}`).join("") + "Z";
    if (z > housing.floorMm) path += "M15,9A3,3 0 1,0 9,9A3,3 0 1,0 15,9Z";
    return path;
  });
}
