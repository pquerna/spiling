/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Independent actual-command accounting for the frozen analytic coupons.
// Spatial regions partition CENTERLINES, not physical bead intersections.
const EPS = 0.002;
const ROUND = 1e-6;
const metrics = [
  "volumeMm3",
  "lengthMm",
  "commonHeightVolumeMm3",
  "heightAdjustmentMm3",
  "doseResidualMm3",
];
const total = (rows, key) => rows.reduce((sum, row) => sum + row[key], 0);
const close = (a, b) => Number.isFinite(a) && Number.isFinite(b) && Math.abs(a - b) < ROUND;
const empty = () => Object.fromEntries(metrics.map((key) => [key, 0]));
const area = (w, h) => h * (w - h * (1 - Math.PI / 4));
const regions = (source) =>
  source.id === "one-ended"
    ? [
        ["left-post", 0, 2],
        ["span", 2, source.widthMm],
      ]
    : [
        ["left-post", 0, 2],
        ["span", 2, source.widthMm - 2],
        ["right-post", source.widthMm - 2, source.widthMm],
      ];
function sourceIntervals(source, z) {
  return z > 3 + ROUND
    ? [[0, source.widthMm]]
    : source.id === "one-ended"
      ? [[0, 2]]
      : [
          [0, 2],
          [source.widthMm - 2, source.widthMm],
        ];
}
function kind(s, source, width) {
  const dx = Math.abs(s.to[0] - s.from[0]),
    dy = Math.abs(s.to[1] - s.from[1]);
  const boundary = sourceIntervals(source, s.to[2]).some(
    ([lo, hi]) =>
      (dx < ROUND && [lo + width / 2, hi - width / 2].some((x) => Math.abs(s.from[0] - x) < EPS)) ||
      (dy < ROUND && [width / 2, 6 - width / 2].some((y) => Math.abs(s.from[1] - y) < EPS)),
  );
  return boundary
    ? "boundary-parallel"
    : dy < ROUND
      ? "x-aligned"
      : dx < ROUND
        ? "y-aligned"
        : "oblique";
}
function modelStrokes(decoded, prime) {
  return decoded.strokes.filter(
    (s) =>
      !(
        prime &&
        [s.from, s.to].every((p, i) =>
          p.every((v, j) => Math.abs(v - [prime.prime_start_mm, prime.prime_end_mm][i][j]) < ROUND),
        ) &&
        Math.abs(s.filamentMm - prime.prime_filament_length_mm) < 1e-7
      ),
  );
}
// Half-open x cells avoid charging an exactly vertical partition stroke twice.
function fraction(s, lo, hi, final) {
  const dx = s.to[0] - s.from[0];
  if (Math.abs(dx) < 1e-12)
    return s.from[0] >= lo && (s.from[0] < hi || (final && s.from[0] === hi)) ? 1 : 0;
  const a = (lo - s.from[0]) / dx,
    b = (hi - s.from[0]) / dx;
  return Math.max(0, Math.min(1, Math.max(a, b)) - Math.max(0, Math.min(a, b)));
}
export function bridgeMaterialLedger(
  decoded,
  audit,
  source,
  { prime = null, widthMm = 0.45, flowMultiplier = 1 } = {},
) {
  if (
    !(flowMultiplier > 0 && flowMultiplier <= 2) ||
    widthMm !== 0.45 ||
    ![6, 10, 14].includes(source.widthMm) ||
    !["span-2", "span-6", "span-10", "one-ended"].includes(source.id)
  )
    throw Error("unsupported bridge material accounting inputs");
  const strokes = modelStrokes(decoded, prime);
  if (strokes.length > 100_000 || audit.layers.length > 100)
    throw Error("bridge material accounting limit");
  const layers = audit.layers.map((l) => ({
    lowerMm: l.lowerMm,
    upperMm: l.upperMm,
    ...empty(),
    cells: [],
  }));
  const byZ = new Map(layers.map((l) => [l.upperMm.toFixed(6), l]));
  const rs = regions(source);
  // Two endpoints rounded to 1e-6 mm plus two absolute E coordinates rounded
  // to 1e-8 mm: a 2e-7 mm3 bound encloses command quantization for this
  // 0.45 mm / at most 0.202 mm native stadium model. Reference fitted flow
  // is observed against this countermodel, never required to match it.
  const doseModel = {
    perStrokeVolumeAllowanceMm3: 2e-7,
    maximumAbsoluteResidualMm3: 0,
    mismatchedStrokeCount: 0,
    examples: [],
  };
  for (const s of strokes) {
    const layer = byZ.get(s.to[2].toFixed(6));
    if (!layer || Math.abs(s.from[2] - s.to[2]) > ROUND || !(s.lengthMm > 0) || !(s.volumeMm3 > 0))
      throw Error("unaccounted or invalid model deposition");
    const h = layer.upperMm - layer.lowerMm;
    if (!(h > 0 && h <= 0.202)) throw Error("invalid material interval");
    const residual = s.volumeMm3 - s.lengthMm * area(widthMm, h) * flowMultiplier;
    doseModel.maximumAbsoluteResidualMm3 = Math.max(
      doseModel.maximumAbsoluteResidualMm3,
      Math.abs(residual),
    );
    if (Math.abs(residual) > doseModel.perStrokeVolumeAllowanceMm3) {
      doseModel.mismatchedStrokeCount++;
      if (doseModel.examples.length < 4)
        doseModel.examples.push({ line: s.line, from: s.from, to: s.to, residualMm3: residual });
    }
    const classification = kind(s, source, widthMm);
    let assigned = 0;
    for (const [i, [region, lo, hi]] of rs.entries()) {
      const f = fraction(s, lo, hi, i === rs.length - 1);
      if (!f) continue;
      assigned += f;
      let cell = layer.cells.find((c) => c.region === region && c.kind === classification);
      if (!cell) layer.cells.push((cell = { region, kind: classification, ...empty() }));
      const length = s.lengthMm * f,
        common = length * area(widthMm, 0.2) * flowMultiplier;
      const values = {
        volumeMm3: s.volumeMm3 * f,
        lengthMm: length,
        commonHeightVolumeMm3: common,
        heightAdjustmentMm3: length * (area(widthMm, h) - area(widthMm, 0.2)) * flowMultiplier,
        doseResidualMm3: (s.volumeMm3 - s.lengthMm * area(widthMm, h) * flowMultiplier) * f,
      };
      for (const k of metrics) {
        cell[k] += values[k];
        layer[k] += values[k];
      }
    }
    if (!close(assigned, 1)) throw Error("deposition outside material ledger regions");
  }
  const bands = Array.from({ length: 20 }, (_, i) => {
    const lowerMm = i / 5,
      upperMm = (i + 1) / 5,
      cells = [];
    for (const l of layers) {
      const f =
        Math.max(0, Math.min(upperMm, l.upperMm) - Math.max(lowerMm, l.lowerMm)) /
        (l.upperMm - l.lowerMm);
      if (!f) continue;
      for (const c of l.cells) {
        let cell = cells.find((x) => x.region === c.region && x.kind === c.kind);
        if (!cell) cells.push((cell = { region: c.region, kind: c.kind, ...empty() }));
        for (const k of metrics) cell[k] += c[k] * f;
      }
    }
    const sourceVolumeMm3 = rs.reduce(
      (sum, [region, lo, hi]) =>
        sum + (region === "span" && upperMm <= 3 + ROUND ? 0 : (hi - lo) * 6 * (upperMm - lowerMm)),
      0,
    );
    return {
      lowerMm,
      upperMm,
      sourceVolumeMm3,
      ...Object.fromEntries(metrics.map((k) => [k, total(cells, k)])),
      cells,
    };
  });
  const ledger = {
    ...Object.fromEntries(metrics.map((k) => [k, total(layers, k)])),
    sourceVolumeMm3: total(bands, "sourceVolumeMm3"),
    doseModel,
    layers,
    bands,
    scope:
      "Common 0.2 mm bands, actual interval-overlap resampling and x-projected centerline regions. Boundary-parallel/x/y/oblique are observed geometry, never role labels. Volume = common-height nominal stadium length contribution + actual-height correction + actual E dose residual. Neither nominal volume nor source volume proves physical fill; fractional length in resampled bands is accounting, not a new path.",
  };
  ledger.reconciled = bridgeLedgerReconciles(ledger, audit);
  return ledger;
}
export function bridgeLedgerReconciles(ledger, audit) {
  const valid = (r) =>
    metrics.every((k) => Number.isFinite(r[k])) &&
    close(r.volumeMm3, r.commonHeightVolumeMm3 + r.heightAdjustmentMm3 + r.doseResidualMm3);
  return (
    valid(ledger) &&
    close(ledger.volumeMm3, audit.modelVolumeMm3) &&
    ledger.layers.length === audit.layers.length &&
    ledger.layers.every(
      (l, i) =>
        valid(l) &&
        close(l.lowerMm, audit.layers[i].lowerMm) &&
        close(l.upperMm, audit.layers[i].upperMm) &&
        close(l.volumeMm3, audit.layers[i].volumeMm3) &&
        close(l.lengthMm, audit.layers[i].lengthMm) &&
        l.cells.every(valid) &&
        metrics.every((k) => close(l[k], total(l.cells, k))),
    ) &&
    ledger.bands.length === 20 &&
    ledger.bands.every(
      (b, i) =>
        valid(b) &&
        close(b.lowerMm, i / 5) &&
        close(b.upperMm, (i + 1) / 5) &&
        b.cells.every(
          (c) =>
            valid(c) &&
            metrics.every((k) =>
              close(
                c[k],
                ledger.layers.reduce(
                  (sum, l) =>
                    sum +
                    ((l.cells.find((x) => x.region === c.region && x.kind === c.kind)?.[k] ?? 0) *
                      Math.max(
                        0,
                        Math.min(b.upperMm, l.upperMm) - Math.max(b.lowerMm, l.lowerMm),
                      )) /
                      (l.upperMm - l.lowerMm),
                  0,
                ),
              ),
            ),
        ) &&
        metrics.every(
          (k) =>
            close(b[k], total(b.cells, k)) &&
            close(
              b[k],
              ledger.layers.reduce(
                (sum, l) =>
                  sum +
                  (l[k] *
                    Math.max(0, Math.min(b.upperMm, l.upperMm) - Math.max(b.lowerMm, l.lowerMm))) /
                    (l.upperMm - l.lowerMm),
                0,
              ),
            ),
        ),
    ) &&
    metrics.every(
      (k) => close(ledger[k], total(ledger.layers, k)) && close(ledger[k], total(ledger.bands, k)),
    )
  );
}
export function bridgeLedgerDelta(a, b) {
  const difference = (x, y) => Object.fromEntries(metrics.map((k) => [k, x[k] - y[k]]));
  return {
    ...difference(a, b),
    bands: a.bands.map((band, i) => {
      const other = b.bands[i];
      const keys = new Set([...band.cells, ...other.cells].map((c) => `${c.region}/${c.kind}`));
      return {
        lowerMm: band.lowerMm,
        upperMm: band.upperMm,
        ...difference(band, other),
        cells: [...keys].sort().map((key) => {
          const [region, kind] = key.split("/");
          return {
            region,
            kind,
            ...difference(
              band.cells.find((c) => c.region === region && c.kind === kind) ?? empty(),
              other.cells.find((c) => c.region === region && c.kind === kind) ?? empty(),
            ),
          };
        }),
      };
    }),
  };
}
function nearest(point, strokes, height, flow, source, charge) {
  let best = null,
    equivalent = null;
  for (const s of strokes) {
    charge();
    const dx = s.to[0] - s.from[0],
      dy = s.to[1] - s.from[1];
    const t = Math.max(
      0,
      Math.min(
        1,
        ((point[0] - s.from[0]) * dx + (point[1] - s.from[1]) * dy) / (dx * dx + dy * dy),
      ),
    );
    const d = Math.hypot(point[0] - s.from[0] - t * dx, point[1] - s.from[1] - t * dy);
    const dose = s.volumeMm3 / s.lengthMm,
      equivalentWidth = dose / flow / height + height * (1 - Math.PI / 4);
    const witness = {
      line: s.line,
      geometryKind: kind(s, source, 0.45),
      from: s.from,
      to: s.to,
      distanceMm: d,
      projection: t,
      doseMm2: dose,
      equivalentStadiumWidthMm: equivalentWidth,
    };
    if (!best || d < best.distanceMm) best = witness;
    // A wide, non-nearest stroke can cover the point under this countermodel.
    if (!equivalent || d - equivalentWidth / 2 < equivalent.clearanceMm)
      equivalent = { ...witness, clearanceMm: d - equivalentWidth / 2 };
  }
  return {
    nearest: best,
    nominalCovered: best !== null && best.distanceMm <= 0.225 + EPS,
    equivalentStadium: equivalent,
    equivalentStadiumCovered: equivalent !== null && equivalent.clearanceMm <= EPS,
  };
}
// Each baseline miss retains its actual coordinate and deciding control strokes.
// No missed point is discarded or turned into a baseline coverage pass.
export function bridgeCoverageWitnesses(source, variants) {
  let work = 0;
  const charge = () => {
    if (++work > 40_000_000) throw Error("bridge witness work limit");
  };
  if (!variants.baseline || Object.keys(variants).length > 8)
    throw Error("bridge coverage controls missing or excessive");
  if (
    Object.values(variants).some(
      (v) =>
        v.audit.layers.length > 100 ||
        v.decoded.strokes.length > 100_000 ||
        !((v.flowMultiplier ?? 1) > 0 && (v.flowMultiplier ?? 1) <= 2),
    )
  )
    throw Error("bridge coverage input limit");
  const groups = Object.fromEntries(
    Object.entries(variants).map(([id, v]) => {
      const map = new Map(v.audit.layers.map((l) => [l.upperMm.toFixed(6), { ...l, strokes: [] }]));
      for (const s of modelStrokes(v.decoded, v.prime)) {
        const l = map.get(s.to[2].toFixed(6));
        if (!l) throw Error("unaccounted coverage deposition");
        l.strokes.push(s);
      }
      return [id, map];
    }),
  );
  const witnesses = [],
    counts = {};
  for (const l of variants.baseline.audit.layers) {
    if (l.uncoveredPoints?.length !== l.uncoveredGridSamples)
      throw Error("complete baseline missed coordinates required");
    for (const point of l.uncoveredPoints) {
      if (witnesses.length >= 100_000) throw Error("bridge coverage sample limit");
      const observations = {};
      for (const [id, map] of Object.entries(groups)) {
        const layer = map.get(l.upperMm.toFixed(6));
        if (!layer || Math.abs(layer.lowerMm - l.lowerMm) > ROUND)
          throw Error("coverage controls require matching physical intervals");
        observations[id] = nearest(
          point,
          layer.strokes,
          layer.upperMm - layer.lowerMm,
          variants[id].flowMultiplier ?? 1,
          source,
          charge,
        );
      }
      if (observations.baseline.nominalCovered) throw Error("baseline missed point is covered");
      const coveringControls = Object.entries(observations)
        .filter(([id, o]) => id !== "baseline" && o.nominalCovered)
        .map(([id]) => id);
      const disposition = coveringControls.length
        ? "covered-by-controlled-construction"
        : observations.baseline.equivalentStadiumCovered
          ? "dose-model-sensitive"
          : "nominal-gap-retained";
      counts[disposition] = (counts[disposition] ?? 0) + 1;
      witnesses.push({
        lowerMm: l.lowerMm,
        upperMm: l.upperMm,
        point,
        disposition,
        coveringControls,
        observations,
      });
    }
  }
  const key = (l, point) => [l.upperMm, ...point].map((v) => v.toFixed(6)).join("/");
  const misses = (v) =>
    new Set(
      v.audit.layers.flatMap((l) => {
        if (l.uncoveredPoints?.length !== l.uncoveredGridSamples)
          throw Error("complete control missed coordinates required");
        return l.uncoveredPoints.map((p) => key(l, p));
      }),
    );
  const baselineMisses = misses(variants.baseline);
  const controlMisses = Object.fromEntries(
    Object.entries(variants).map(([name, v]) => {
      const points = misses(v);
      return [
        name,
        {
          total: points.size,
          baselineMissesRepaired: [...baselineMisses].filter((p) => !points.has(p)).length,
          introducedMisses: [...points].filter((p) => !baselineMisses.has(p)).length,
          baselineMissesRetained: [...points].filter((p) => baselineMisses.has(p)).length,
        },
      ];
    }),
  );
  return {
    sampleCount: witnesses.length,
    controlMisses,
    counts,
    witnesses,
    work,
    scope:
      "Exhaustive baseline grid misses, fixed 0.45 mm nominal capsules and unchanged 0.002 mm command allowance. A controlled change covering the same coordinate attributes a response, not baseline correctness. Each control also retains all introduced misses; the union of coverage across different programs is never a printable program. Inverted E/length/height stadium width is an explicit countermodel (flow multiplier removed), never physical bead authority, sag, strength, or a widened baseline tolerance.",
  };
}

export function evaluateBridgeAccounting(ledgers, audits, coverage) {
  const names = ["native", "nativeSkin", "baseline", "overlap", "overlap25", "fullSkin", "half"];
  if (names.some((name) => !ledgers[name] || !audits[name]))
    return { accepted: false, reason: "complete native and reference controls required" };
  const sameBand = (a, b) =>
    metrics.every((k) => close(a[k], b[k])) &&
    a.cells.length === b.cells.length &&
    a.cells.every((c) => {
      const other = b.cells.find((x) => x.region === c.region && x.kind === c.kind);
      return other && metrics.every((k) => close(c[k], other[k]));
    });
  const misses = total(audits.baseline.layers, "uncoveredGridSamples");
  const checks = {
    everyPhysicalBandAndRegionReconciles: names.every((name) =>
      bridgeLedgerReconciles(ledgers[name], audits[name]),
    ),
    nativeDoseMatchesDeclaredStadiumModel: ["native", "nativeSkin"].every(
      (name) => ledgers[name].doseModel.mismatchedStrokeCount === 0,
    ),
    nativeSkinPreservesEveryBandAndRegion: ledgers.native.bands.every((b, i) =>
      sameBand(b, ledgers.nativeSkin.bands[i]),
    ),
    referenceSkinLeavesLaterRoofBandsUnchanged: ledgers.baseline.bands
      .filter((b) => b.lowerMm >= 3.2 - ROUND)
      .every((b) =>
        sameBand(
          b,
          ledgers.fullSkin.bands.find((l) => close(l.upperMm, b.upperMm)),
        ),
      ),
    referenceHalfFlowLeavesOtherBandsUnchanged: ledgers.baseline.bands
      .filter((b) => b.upperMm <= 3 + ROUND || b.lowerMm >= 3.2 - ROUND)
      .every((b) =>
        sameBand(
          b,
          ledgers.half.bands.find((l) => close(l.upperMm, b.upperMm)),
        ),
      ),
    everyControlMissCountReconciles: ["baseline", "overlap", "overlap25", "fullSkin", "half"].every(
      (name) => {
        const c = coverage.controlMisses?.[name];
        return (
          c &&
          c.total === total(audits[name].layers, "uncoveredGridSamples") &&
          c.baselineMissesRepaired + c.baselineMissesRetained === misses &&
          c.introducedMisses + c.baselineMissesRetained === c.total
        );
      },
    ),
    overlapControlDoesNotImplyMonotoneCoverage:
      coverage.controlMisses?.overlap.baselineMissesRepaired > 0 &&
      coverage.controlMisses?.overlap25.baselineMissesRepaired > 0 &&
      coverage.controlMisses?.overlap25.introducedMisses >
        coverage.controlMisses?.overlap.introducedMisses,
    everyBaselineMissHasActualControlWitnesses:
      coverage.sampleCount === misses &&
      coverage.witnesses.length === misses &&
      total(
        Object.entries(coverage.counts).map(([, n]) => ({ n })),
        "n",
      ) === misses &&
      coverage.witnesses.every(
        (w) =>
          ["baseline", "overlap", "overlap25", "fullSkin", "half"].every(
            (name) => w.observations[name]?.nearest && w.observations[name].equivalentStadium,
          ) && !w.observations.baseline.nominalCovered,
      ),
  };
  return { accepted: Object.values(checks).every((v) => v === true), checks };
}
