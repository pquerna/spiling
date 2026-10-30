/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */

// Analytic cube observer, independent of both slicers' planned paths. Raster
// coverage is a nominal stadium-bead diagnostic, not measured deposited shape.
const EPS = 0.002; // Reference XYZ3 output; includes a sample away from ties.
const GRID = 0.1;
function distanceSquared(point, a, b) {
  const dx = b[0] - a[0],
    dy = b[1] - a[1];
  const t = Math.max(
    0,
    Math.min(1, ((point[0] - a[0]) * dx + (point[1] - a[1]) * dy) / (dx * dx + dy * dy)),
  );
  return (point[0] - a[0] - t * dx) ** 2 + (point[1] - a[1] - t * dy) ** 2;
}
function closePoint(a, b) {
  return a.every((v, i) => Math.abs(v - b[i]) <= 1e-6);
}
function coreLength(stroke, lo, hi) {
  let start = 0,
    end = 1;
  for (let axis = 0; axis < 2; axis++) {
    const a = stroke.from[axis],
      d = stroke.to[axis] - a;
    if (Math.abs(d) < 1e-12) {
      if (a < lo || a > hi) return 0;
    } else {
      const t0 = (lo - a) / d,
        t1 = (hi - a) / d;
      start = Math.max(start, Math.min(t0, t1));
      end = Math.min(end, Math.max(t0, t1));
    }
  }
  return Math.max(0, end - start) * stroke.lengthMm;
}

export function auditCube(decoded, specification) {
  const {
    sideMm: side,
    layerHeightMm: nominal,
    lineWidthMm: width,
    walls,
    infillFraction: infill,
    skinThicknessMm: skin,
    prime = null,
  } = specification;
  if (
    side !== 10 ||
    nominal !== 0.2 ||
    width !== 0.45 ||
    skin !== 0.8 ||
    ![1, 2].includes(walls) ||
    ![0, 0.25, 1].includes(infill)
  )
    throw new Error("unsupported analytic comparison case");
  const violations = [],
    outside = [],
    groups = new Map();
  const problem = (code, message) => {
    if (
      violations.length < 100 &&
      !violations.some((v) => v.code === code && v.message === message)
    )
      violations.push({ code, message });
  };
  let previousZ = 0,
    work = 0;
  for (const stroke of decoded.strokes) {
    const isInside = [stroke.from, stroke.to].every((p) =>
      p.every((v) => v >= -EPS && v <= side + EPS),
    );
    if (!isInside) {
      outside.push(stroke);
      if (
        !prime ||
        !closePoint(stroke.from, prime.from) ||
        !closePoint(stroke.to, prime.to) ||
        Math.abs(stroke.filamentMm - prime.filamentMm) > 1e-7
      )
        problem(
          "unauthorized_extrusion",
          `line ${stroke.line}: deposition outside the analytic cube`,
        );
      continue;
    }
    const z = Number(stroke.to[2].toFixed(6));
    if (z < previousZ - 1e-6)
      problem("deposition_order", `line ${stroke.line}: descending model deposition`);
    previousZ = z;
    if (!groups.has(z)) groups.set(z, []);
    groups.get(z).push(stroke);
    if (stroke.feedMmMin / 60 > 150 + EPS)
      problem("feed_limit", `line ${stroke.line}: feed exceeds declared limit`);
    if (((stroke.volumeMm3 / stroke.lengthMm) * stroke.feedMmMin) / 60 > 12 + EPS)
      problem("flow_limit", `line ${stroke.line}: nominal volumetric flow exceeds declared limit`);
  }
  if (outside.length > (prime ? 1 : 0))
    problem("prime_count", "unexpected extra non-model deposition");
  if (prime && outside.length !== 1) problem("prime_count", "declared priming segment is missing");
  if (decoded.stationaryFilamentMm > 1e-8)
    problem("stationary_deposition", "unassigned stationary extrusion");
  if (!groups.size) problem("missing_model", "no model deposition");
  const sorted = [...groups].sort((a, b) => a[0] - b[0]);
  if (Math.abs((sorted.at(-1)?.[0] ?? 0) - side) > EPS)
    problem("model_height", "final model height does not match the cube");
  const layers = [],
    bands = Array.from({ length: Math.round(side / nominal) }, (_, index) => ({
      lowerMm: index * nominal,
      upperMm: (index + 1) * nominal,
      volumeMm3: 0,
      coreVolumeMm3: 0,
      wallZoneVolumeMm3: 0,
    }));
  let lower = 0;
  for (const [z, strokes] of sorted) {
    const height = z - lower;
    if (height <= 1e-6 || height > nominal + EPS)
      problem("layer_interval", `invalid deposition interval ${lower}..${z}`);
    const pitches = width - height * (1 - Math.PI / 4);
    const wallDepth = width + (walls - 1) * pitches;
    const row = {
      lowerMm: lower,
      upperMm: z,
      heightMm: height,
      strokes: strokes.length,
      volumeMm3: 0,
      coreVolumeMm3: 0,
      wallZoneVolumeMm3: 0,
      widthRangeMm: [Infinity, 0],
      feedRangeMmS: [Infinity, 0],
      fanPwm: [],
      hints: {},
      wallProbes: [],
      coverage: null,
      bridgeProcessVolumeMm3: 0,
    };
    const emitted = new Set();
    const fans = new Set(),
      footprints = [];
    for (const stroke of strokes) {
      const ends = [stroke.from.slice(0, 2).join(","), stroke.to.slice(0, 2).join(",")]
        .sort()
        .join("/");
      if (emitted.has(ends))
        problem(
          "duplicate_stroke",
          `line ${stroke.line}: repeated complete deposition segment at Z${z}`,
        );
      emitted.add(ends);
      const speed = stroke.feedMmMin / 60;
      const first = z <= nominal + EPS;
      const ordinary =
        Math.abs(speed - (first ? 17.5 : 35)) < 1e-6 &&
        (first ? stroke.fanPwm === 0 : [127.5, 128].includes(stroke.fanPwm));
      const bridge = !first && Math.abs(speed - 21) < 1e-6 && stroke.fanPwm === 255;
      if (!ordinary && !bridge)
        problem(
          "process_signature",
          `line ${stroke.line}: undeclared speed/fan pair ${speed}mm/s, PWM${stroke.fanPwm}`,
        );
      const inferredWidth =
        stroke.volumeMm3 / stroke.lengthMm / height + height * (1 - Math.PI / 4);
      if (!(inferredWidth >= 0.3 - EPS && inferredWidth <= 0.75 + EPS))
        problem(
          "bead_width",
          `line ${stroke.line}: stadium-equivalent width ${inferredWidth.toFixed(6)} outside declared range`,
        );
      if (
        [stroke.from, stroke.to].some(
          (p) =>
            p[0] - inferredWidth / 2 < -EPS ||
            p[1] - inferredWidth / 2 < -EPS ||
            p[0] + inferredWidth / 2 > side + EPS ||
            p[1] + inferredWidth / 2 > side + EPS,
        )
      )
        problem(
          "nominal_envelope",
          `line ${stroke.line}: stadium-equivalent footprint exceeds the source box`,
        );
      row.widthRangeMm[0] = Math.min(row.widthRangeMm[0], inferredWidth);
      row.widthRangeMm[1] = Math.max(row.widthRangeMm[1], inferredWidth);
      row.feedRangeMmS[0] = Math.min(row.feedRangeMmS[0], stroke.feedMmMin / 60);
      row.feedRangeMmS[1] = Math.max(row.feedRangeMmS[1], stroke.feedMmMin / 60);
      row.volumeMm3 += stroke.volumeMm3;
      const core =
        (coreLength(stroke, wallDepth, side - wallDepth) / stroke.lengthMm) * stroke.volumeMm3;
      row.coreVolumeMm3 += core;
      row.wallZoneVolumeMm3 += stroke.volumeMm3 - core;
      fans.add(stroke.fanPwm);
      const hint = stroke.roleHint ?? "unlabelled";
      row.hints[hint] = (row.hints[hint] ?? 0) + stroke.volumeMm3;
      // This signature is commanded feed/fan, independent of a role label.
      if (Math.abs(stroke.feedMmMin / 60 - 21) < EPS && stroke.fanPwm === 255)
        row.bridgeProcessVolumeMm3 += stroke.volumeMm3;
      footprints.push({ ...stroke, radiusSquared: (inferredWidth / 2) ** 2 });
    }
    row.fanPwm = [...fans].sort((a, b) => a - b);
    // Count distinct boundary-parallel centerlines on three nonvertex probes
    // per side; infill outside the requested wall band cannot supply a wall.
    for (const axis of [0, 1])
      for (const probe of [3.137, 5.123, 7.179]) {
        const offsets = [[], []];
        for (const stroke of strokes) {
          if (Math.abs(stroke.from[axis] - stroke.to[axis]) > EPS) continue;
          const other = 1 - axis,
            a = stroke.from[other],
            b = stroke.to[other];
          if (!(Math.min(a, b) <= probe && Math.max(a, b) > probe)) continue;
          for (const sideIndex of [0, 1]) {
            const d = sideIndex === 0 ? stroke.from[axis] : side - stroke.from[axis];
            if (
              d > 0 &&
              d < (walls - 0.25) * width &&
              !offsets[sideIndex].some((v) => Math.abs(v - d) < EPS)
            )
              offsets[sideIndex].push(d);
          }
        }
        for (const sideIndex of [0, 1]) {
          offsets[sideIndex].sort((a, b) => a - b);
          if (offsets[sideIndex].length !== walls)
            problem(
              "wall_count",
              `Z${z}: axis ${axis}, side ${sideIndex}, probe ${probe}: ${offsets[sideIndex].length} walls instead of ${walls}`,
            );
        }
        row.wallProbes.push({ axis, probe, offsets });
      }
    const mandatorySolid = infill === 1 || lower < skin - 1e-6 || z > side - skin + 1e-6;
    let required = 0,
      missing = 0,
      coreSamples = 0,
      filledCore = 0;
    const samples = Math.round(side / GRID);
    for (let ix = 0; ix < samples; ix++)
      for (let iy = 0; iy < samples; iy++) {
        const point = [(ix + 0.5) * GRID, (iy + 0.5) * GRID];
        const dx = Math.min(point[0], side - point[0]),
          dy = Math.min(point[1], side - point[1]);
        // Exact square corners need a separate bead/corner model; do not assert
        // them from round-capsule sampling. All straight wall bands remain owned.
        const corner = dx < walls * width && dy < walls * width;
        const mustCover = !corner && (mandatorySolid || Math.min(dx, dy) < wallDepth - GRID / 2);
        const inCore = dx > wallDepth + width && dy > wallDepth + width;
        if (!mustCover && !inCore) continue;
        const covered = footprints.some((stroke) => {
          if (++work > 100_000_000) throw new Error("analytic observer work limit");
          return distanceSquared(point, stroke.from, stroke.to) <= stroke.radiusSquared + 1e-8;
        });
        if (mustCover) {
          required++;
          if (!covered) missing++;
        }
        if (inCore) {
          coreSamples++;
          if (covered) filledCore++;
        }
      }
    row.coverage = {
      requiredSamples: required,
      missingSamples: missing,
      gridMm: GRID,
      nominalCoreFraction: coreSamples ? filledCore / coreSamples : null,
      mandatorySolid,
    };
    if (missing > 0)
      problem(
        "mandatory_coverage",
        `Z${z}: ${missing}/${required} required nominal footprint samples missing`,
      );
    if (!mandatorySolid && infill === 0 && row.coreVolumeMm3 > 1e-5)
      problem("unexpected_core", `Z${z}: hollow cube has interior extrusion`);
    for (const band of bands) {
      const overlap = Math.max(0, Math.min(z, band.upperMm) - Math.max(lower, band.lowerMm));
      if (!overlap) continue;
      const weight = overlap / height;
      band.volumeMm3 += row.volumeMm3 * weight;
      band.coreVolumeMm3 += row.coreVolumeMm3 * weight;
      band.wallZoneVolumeMm3 += row.wallZoneVolumeMm3 * weight;
    }
    layers.push(row);
    lower = z;
  }
  return {
    correctnessPassed: violations.length === 0,
    violations,
    layerCount: layers.length,
    modelVolumeMm3: layers.reduce((sum, l) => sum + l.volumeMm3, 0),
    bridgeProcessVolumeMm3: layers.reduce((sum, l) => sum + l.bridgeProcessVolumeMm3, 0),
    outsideModel: outside,
    controls: decoded.controls,
    commands: decoded.commands,
    layers,
    bands,
    assumptions:
      "XYZ3 reference tolerance; linear planar single-tool millimetre/E8 subset; nominal stadium-derived 0.1mm footprint samples, square corners excluded. No physical bead, collision, firmware or printability qualification.",
  };
}

export function compareAudits(spiling, reference, { scheduleProbe = null } = {}) {
  const differences = [];
  const add = (id, status, explanation, evidence) =>
    differences.push({ id, status, explanation, evidence });
  const sameSchedule =
    spiling.layers.length === reference.layers.length &&
    spiling.layers.every((l, i) => Math.abs(l.upperMm - reference.layers[i].upperMm) < 1e-6);
  const refinedTerminal =
    spiling.layers.length === 51 &&
    reference.layers.length === 50 &&
    spiling.layers
      .slice(0, 49)
      .every((l, i) => Math.abs(l.upperMm - reference.layers[i].upperMm) < 1e-6) &&
    Math.abs(spiling.layers[49].upperMm - 9.9) < 1e-6 &&
    Math.abs(spiling.layers[50].upperMm - 10) < 1e-6;
  const scheduleExplained =
    refinedTerminal &&
    scheduleProbe?.correctnessPassed === true &&
    scheduleProbe.baseline.requested_shape_error_mm === 0.1 &&
    scheduleProbe.diagnostic.requested_shape_error_mm === 0.101 &&
    scheduleProbe.diagnostic.achieved_shape_error_mm > 0.1 &&
    scheduleProbe.diagnostic.achieved_shape_error_mm <= 0.101 &&
    scheduleProbe.elevations.length === 50 &&
    scheduleProbe.elevations.every((z, i) => Math.abs(z - reference.layers[i].upperMm) < 1e-6);
  add(
    "layer_schedule",
    sameSchedule ? "match" : scheduleExplained ? "explained" : "unresolved",
    sameSchedule
      ? "Actual deposition elevations match within command precision."
      : scheduleExplained
        ? "Native closed-source shape certification refines the terminal interval under the 0.100mm bound. Changing only that bound to 0.101mm gives 50 independently audited layers, with achieved native error above 0.100mm. This explains the schedule without accepting a weaker bound for the baseline."
        : "Different actual physical intervals require a bounded scheduler explanation; counts alone do not establish a defect.",
    {
      controlledProbe: scheduleProbe,
      spiling: spiling.layers.map((l) => l.upperMm),
      reference: reference.layers.map((l) => l.upperMm),
    },
  );
  const offsetsFit = (audit, pitch) =>
    audit.layers.every((l) =>
      l.wallProbes.every((p) =>
        p.offsets.every((offsets) =>
          offsets.every((offset, i) => Math.abs(offset - (0.225 + i * pitch)) <= EPS),
        ),
      ),
    );
  const oneWall = spiling.layers.every((l) =>
    l.wallProbes.every((p) => p.offsets.every((o) => o.length === 1)),
  );
  const wallPolicyExplained =
    offsetsFit(spiling, 0.45) && offsetsFit(reference, 0.45 - 0.2 * (1 - Math.PI / 4));
  add(
    "wall_spacing",
    wallPolicyExplained ? (oneWall ? "match" : "explained") : "unresolved",
    "Three nonvertex probes on each side of every actual layer check commanded wall centers. Spiling uses nominal 0.45mm wall pitch; Prusa classic uses rounded-bead pitch width-height*(1-pi/4), about 0.40708mm at height0.2. This accounts for center placement, not physical fusion or the full volume difference.",
    {
      spilingNominalPitchMm: 0.45,
      referenceRoundedPitchMm: 0.45 - 0.2 * (1 - Math.PI / 4),
      allProbeOffsetsFit: wallPolicyExplained,
    },
  );
  add(
    "bridge_process",
    Math.abs(spiling.bridgeProcessVolumeMm3 - reference.bridgeProcessVolumeMm3) < 1e-5
      ? "match"
      : "unresolved",
    "Feed 21mm/s plus fan255 identifies the configured bridge process without trusting role comments. Compare actual lower support and production policy before accepting the difference.",
    {
      spilingVolumeMm3: spiling.bridgeProcessVolumeMm3,
      referenceVolumeMm3: reference.bridgeProcessVolumeMm3,
    },
  );
  add(
    "material_distribution",
    Math.abs(spiling.modelVolumeMm3 - reference.modelVolumeMm3) < 1e-5 &&
      spiling.bands.every((b, i) =>
        ["volumeMm3", "coreVolumeMm3", "wallZoneVolumeMm3"].every(
          (key) => Math.abs(b[key] - reference.bands[i][key]) < 1e-5,
        ),
      )
      ? "match"
      : "unresolved",
    "Actual E-derived volume is partitioned by analytic wall/core centerline zones and common physical height bands; the full per-band delta remains visible.",
    {
      spilingVolumeMm3: spiling.modelVolumeMm3,
      referenceVolumeMm3: reference.modelVolumeMm3,
      bands: spiling.bands.map((b, i) => ({
        lowerMm: b.lowerMm,
        upperMm: b.upperMm,
        volumeDeltaMm3: b.volumeMm3 - reference.bands[i].volumeMm3,
        coreDeltaMm3: b.coreVolumeMm3 - reference.bands[i].coreVolumeMm3,
        wallZoneDeltaMm3: b.wallZoneVolumeMm3 - reference.bands[i].wallZoneVolumeMm3,
      })),
    },
  );
  return {
    correctnessPassed: spiling.correctnessPassed && reference.correctnessPassed,
    differencesExplained: differences.every((d) => d.status !== "unresolved"),
    differences,
  };
}
