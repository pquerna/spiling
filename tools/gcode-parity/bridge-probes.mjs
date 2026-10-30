/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { decodeGcode } from "./decode.mjs";
import { auditBridge } from "./bridges.mjs";
import {
  bridgeMaterialLedger,
  bridgeLedgerDelta,
  bridgeCoverageWitnesses,
  evaluateBridgeAccounting,
} from "./bridge-ledger.mjs";

export function referenceValues(ini, values) {
  for (const [key, value] of Object.entries(values)) {
    if (!/^[a-z_]+$/.test(key) || /[\r\n]/.test(String(value)))
      throw Error("invalid reference setting");
    const pattern = new RegExp(`^${key} = .*`, "gm");
    if ([...ini.matchAll(pattern)].length !== 1)
      throw Error(`reference setting ${key} is absent or ambiguous`);
    ini = ini.replace(pattern, `${key} = ${value}`);
  }
  return ini;
}
function bridgeRows(decoded, controls) {
  const strokes = decoded.strokes.filter(
    (s) =>
      s.to[2] > 3.002 &&
      s.to[2] <= 3.202 &&
      Math.abs(s.feedMmMin / 60 - controls.bridgeSpeedMmS) < 1e-6 &&
      s.fanPwm === controls.bridgeFanPwm,
  );
  const rows = strokes.filter(
    (s) =>
      Math.abs(s.from[1] - s.to[1]) < 1e-6 &&
      Math.min(s.from[0], s.to[0]) < 2 &&
      Math.max(s.from[0], s.to[0]) > 8,
  );
  const ys = [...new Set(rows.map((s) => s.to[1]))].sort((a, b) => a - b);
  const length = rows.reduce((sum, s) => sum + s.lengthMm, 0);
  const dose = rows.reduce((sum, s) => sum + s.volumeMm3, 0) / length;
  const pitch = ys.length > 1 ? (ys.at(-1) - ys[0]) / (ys.length - 1) : null;
  return {
    rows: rows.length,
    pitchMm: pitch,
    doseMm2: dose,
    doseOverPitchMm: pitch ? dose / pitch : null,
    processLengthMm: strokes.reduce((sum, s) => sum + s.lengthMm, 0),
    processVolumeMm3: strokes.reduce((sum, s) => sum + s.volumeMm3, 0),
  };
}
// E is derived from two rounded absolute coordinates. Reordering the same
// strokes can move one 1e-8 mm rounding unit between them; geometry/process
// signatures stay exact at the observed command precision.
export function compareSolidPrograms(baselineProgram, changedProgram) {
  const a = decodeGcode(baselineProgram).strokes,
    b = decodeGcode(changedProgram).strokes;
  const directedKey = (s) => JSON.stringify([s.from, s.to, s.feedMmMin, s.fanPwm]);
  const key = (s) => JSON.stringify([[s.from, s.to].sort(), s.feedMmMin, s.fanPwm]);
  const groups = (strokes) => {
    const map = new Map();
    for (const s of strokes) {
      const k = key(s);
      if (!map.has(k)) map.set(k, []);
      map.get(k).push(s.filamentMm);
    }
    return map;
  };
  const aa = groups(a),
    bb = groups(b);
  const equal =
    aa.size === bb.size &&
    [...aa].every(([k, values]) => {
      const other = bb.get(k);
      return (
        other?.length === values.length && values.every((v, i) => Math.abs(v - other[i]) <= 2.1e-8)
      );
    });
  const heights = [...new Set([...a, ...b].map((s) => s.to[2]))].sort((a, b) => a - b);
  const changedOrder = heights.filter(
    (z) =>
      JSON.stringify(a.filter((s) => s.to[2] === z).map(directedKey)) !==
      JSON.stringify(b.filter((s) => s.to[2] === z).map(directedKey)),
  );
  const nearestSeam = (strokes) => {
    const last = strokes.filter((s) => Math.abs(s.to[2] - 3.2) < 1e-6).at(-1)?.to;
    const first = strokes.find((s) => Math.abs(s.to[2] - 3.4) < 1e-6)?.from;
    if (!last || !first) return { matched: false };
    const corners = [
      [0.225, 0.225],
      [9.775, 0.225],
      [9.775, 5.775],
      [0.225, 5.775],
    ];
    const distance = (p) => Math.hypot(p[0] - last[0], p[1] - last[1]);
    const minimum = Math.min(...corners.map(distance));
    return {
      matched:
        corners.some((p) => p.every((v, i) => Math.abs(v - first[i]) < 1e-6)) &&
        Math.abs(distance(first) - minimum) < 1e-6,
      previousEnd: last,
      nextStart: first,
      distanceMm: distance(first),
      minimumCornerDistanceMm: minimum,
    };
  };
  const seams = { baseline: nearestSeam(a), fullSkin: nearestSeam(b) };
  return {
    byteIdentical: baselineProgram === changedProgram,
    depositionMultisetIdentical: equal,
    changedDepositionOrderHeightsMm: changedOrder,
    totalFilamentDifferenceMm:
      b.reduce((v, s) => v + s.filamentMm, 0) - a.reduce((v, s) => v + s.filamentMm, 0),
    orderDifferenceConfinedToRoofAndSuccessor: changedOrder.every(
      (z) => Math.abs(z - 3.2) < 1e-6 || Math.abs(z - 3.4) < 1e-6,
    ),
    nearestSeam: seams,
  };
}
export function evaluateBridgeProbes({
  baseline,
  overlap,
  skin,
  half,
  baselineDecoded,
  halfDecoded,
  controls,
  nativeSkinComparison,
}) {
  const postMisses = (a) =>
    a.layers.filter((l) => l.upperMm <= 3.002).reduce((sum, l) => sum + l.uncoveredGridSamples, 0);
  const safe = (a) => a.violations.every((v) => v.code === "mandatory_nominal_coverage");
  const left = bridgeRows(baselineDecoded, controls),
    right = bridgeRows(halfDecoded, controls);
  const unchangedNonRoof =
    baseline.layers.length === half.layers.length &&
    baseline.layers.every(
      (l, i) =>
        (l.upperMm > 3.002 && l.upperMm <= 3.202) ||
        Math.abs(l.volumeMm3 - half.layers[i].volumeMm3) < 1e-6,
    );
  const checks = {
    nativeSolidSkinDepositionAndDoseInvariant:
      nativeSkinComparison?.depositionMultisetIdentical === true &&
      Math.abs(nativeSkinComparison.totalFilamentDifferenceMm) < 1e-7,
    nativeSolidSkinOrderHasLocalWitness:
      nativeSkinComparison?.orderDifferenceConfinedToRoofAndSuccessor === true &&
      nativeSkinComparison.nearestSeam.baseline.matched &&
      nativeSkinComparison.nearestSeam.fullSkin.matched &&
      nativeSkinComparison.freshVerified === true,
    referencePostCoverageDependsOnOverlap:
      postMisses(baseline) > 0 && postMisses(overlap) === 0 && safe(overlap),
    referencePostCoverageDependsOnSolidClassification:
      postMisses(baseline) > postMisses(skin) &&
      safe(skin) &&
      Math.abs(skin.modelVolumeMm3 - baseline.modelVolumeMm3) > 1,
    halfBridgeControlLeavesOtherIntervalsUnchanged: unchangedNonRoof,
    referenceBridgeDoseAndPitchChangeTogether:
      left.rows > 2 &&
      right.rows > left.rows * 1.8 &&
      right.doseMm2 / left.doseMm2 > 0.45 &&
      right.doseMm2 / left.doseMm2 < 0.55 &&
      Math.abs(left.doseOverPitchMm - 0.2) < 0.0002 &&
      Math.abs(right.doseOverPitchMm - 0.2) < 0.0002,
    referenceBridgeProcessVolumeIsNotSimplyHalved:
      right.processVolumeMm3 / left.processVolumeMm3 > 0.98 &&
      right.processVolumeMm3 / left.processVolumeMm3 < 1.02,
  };
  return {
    accepted: Object.values(checks).every((v) => v === true),
    checks,
    nativeSkinComparison,
    postUncoveredSamples: {
      baseline: postMisses(baseline),
      overlap: postMisses(overlap),
      fullSkin: postMisses(skin),
    },
    bridgeRows: { baseline: left, halfFlow: right },
    scope:
      "Controlled one-setting diagnostics on the unchanged solid span-6 source. Geometric observations and dose/pitch evidence explain distinct contributions; baseline misses and remaining whole-job material deltas remain binding. Diagnostic pitch/volume bounds are not baseline correctness tolerances.",
  };
}

export async function runBridgeProbes({ record, output, cli, engine, reference, run, save }) {
  const prefix = record.id,
    directory = join(output, prefix);
  const source = record.source;
  const original = await readFile(join(directory, "prusa.ini"), "utf8");
  const observations = {};
  for (const [id, settings] of [
    ["overlap", { infill_overlap: "15%" }],
    ["overlap-25", { infill_overlap: "25%" }],
    ["full-skin", { top_solid_layers: 20 }],
    ["half-bridge", { bridge_flow_ratio: 0.5 }],
  ]) {
    const stem = `probe-${id}`;
    await save(`${prefix}/${stem}.ini`, referenceValues(original, settings));
    await run(
      reference,
      [
        "--load",
        join(directory, `${stem}.ini`),
        "--export-gcode",
        "--threads",
        "1",
        "--center",
        "5,3",
        "--save",
        join(directory, `${stem}-resolved.ini`),
        "--output",
        join(directory, `${stem}.gcode`),
        join(directory, "source.step"),
      ],
      `${prefix}/${stem}`,
    );
    const decoded = decodeGcode(await readFile(join(directory, `${stem}.gcode`), "utf8"));
    const audit = auditBridge(decoded, source);
    await save(`${prefix}/${stem}-audit.json`, audit);
    observations[id] = { decoded, audit };
  }
  const intent = JSON.parse(await readFile(join(directory, "spiling.intent.json"), "utf8"));
  intent.recipe.top_skin_thickness_mm = 4;
  await save(`${prefix}/probe-full-skin.intent.json`, intent);
  await run(
    cli,
    [
      "project",
      "create",
      join(directory, "probe-full-skin.spiling"),
      "--import",
      join(directory, "source.step"),
      "--intent",
      join(directory, "probe-full-skin.intent.json"),
      "--material-profile",
      join(directory, "material-profile.json"),
      "--compile",
      "--verify",
      "--manufacturing-out",
      join(directory, "probe-full-skin-export"),
      "--engine",
      engine,
    ],
    `${prefix}/probe-full-skin-native`,
  );
  const baselineProgram = await readFile(join(directory, "spiling-export/program.gcode"), "utf8");
  const nativeSkinProgram = await readFile(
    join(directory, "probe-full-skin-export/program.gcode"),
    "utf8",
  );
  const verification = JSON.parse(
    await readFile(join(directory, "probe-full-skin-export/verification.json"), "utf8"),
  );
  const query = {
    baseline: record.reference,
    overlap: observations.overlap.audit,
    skin: observations["full-skin"].audit,
    half: observations["half-bridge"].audit,
    baselineDecoded: decodeGcode(await readFile(join(directory, "prusa.gcode"), "utf8")),
    halfDecoded: observations["half-bridge"].decoded,
    controls: record.resolvedControls,
    nativeSkinComparison: {
      ...compareSolidPrograms(baselineProgram, nativeSkinProgram),
      freshVerified: verification.verified === true,
    },
  };
  const result = evaluateBridgeProbes(query);
  const outside = structuredClone(query.overlap);
  outside.violations.push({ code: "outside_source_bounds", witness: "injected unrelated failure" });
  const moved = structuredClone(query.half);
  moved.layers[0].volumeMm3 += 0.01;
  moved.layers.at(-1).volumeMm3 -= 0.01;
  result.faultChecks = [
    [
      "native changed deposited material",
      {
        ...query,
        nativeSkinComparison: { ...query.nativeSkinComparison, depositionMultisetIdentical: false },
      },
    ],
    ["unrelated reference geometry failure", { ...query, overlap: outside }],
    ["missing half-flow control", { ...query, halfDecoded: query.baselineDecoded }],
    ["shifted material with conserved total", { ...query, half: moved }],
  ].map(([name, changed]) => ({ name, rejected: !evaluateBridgeProbes(changed).accepted }));
  result.accepted = result.accepted && result.faultChecks.every((f) => f.rejected);
  const prime = intent.printer.workflow.job;
  const variants = {
    baseline: { decoded: query.baselineDecoded, audit: record.reference },
    overlap: observations.overlap,
    overlap25: observations["overlap-25"],
    fullSkin: observations["full-skin"],
    half: observations["half-bridge"],
    native: { decoded: decodeGcode(baselineProgram), audit: record.spiling, prime },
    nativeSkin: { decoded: decodeGcode(nativeSkinProgram), prime },
  };
  variants.nativeSkin.audit = auditBridge(variants.nativeSkin.decoded, source, { prime });
  const flowMultiplier = record.resolvedControls.flowMultiplier;
  const ledgers = {},
    audits = {};
  for (const [name, variant] of Object.entries(variants)) {
    variant.flowMultiplier = flowMultiplier;
    ledgers[name] = bridgeMaterialLedger(variant.decoded, variant.audit, source, {
      prime: variant.prime,
      flowMultiplier,
    });
    audits[name] = variant.audit;
  }
  const coverage = bridgeCoverageWitnesses(
    source,
    Object.fromEntries(
      ["baseline", "overlap", "overlap25", "fullSkin", "half"].map((name) => [
        name,
        variants[name],
      ]),
    ),
  );
  const accounting = evaluateBridgeAccounting(ledgers, audits, coverage);
  const movedRegion = structuredClone(ledgers);
  for (const key of [
    "volumeMm3",
    "lengthMm",
    "commonHeightVolumeMm3",
    "heightAdjustmentMm3",
    "doseResidualMm3",
  ]) {
    const shift = movedRegion.native.bands[0].cells[0][key] * 0.1;
    movedRegion.native.bands[0].cells[0][key] -= shift;
    movedRegion.native.bands[0].cells[1][key] += shift;
  }
  const firstLayer = variants.native.decoded.strokes.filter(
    (s) => Math.abs(s.to[2] - 0.2) < 1e-6 && s.to[1] <= 6.002,
  );
  if (!baselineProgram.split("\n").includes("M82") || firstLayer.length < 2)
    throw Error("balanced dose fault requires the captured absolute-E native program");
  // Alter one absolute E coordinate. The next stroke compensates exactly,
  // preserving layer and job totals while changing two local doses.
  const changedLines = baselineProgram.split("\n");
  const changedLine = firstLayer[0].line;
  const eField = /\bE(-?\d+\.\d{8})\b/g;
  if ([...changedLines[changedLine - 1].matchAll(eField)].length !== 1)
    throw Error("balanced dose fault requires one eight-decimal E coordinate");
  changedLines[changedLine - 1] = changedLines[changedLine - 1].replace(
    eField,
    (_, value) => `E${(Number(value) + 0.001).toFixed(8)}`,
  );
  const balancedProgram = changedLines.join("\n");
  await save(`${prefix}/fault-balanced-dose.gcode`, balancedProgram);
  const balancedDose = decodeGcode(balancedProgram);
  const alteredDoseLedgers = {
    ...ledgers,
    native: bridgeMaterialLedger(balancedDose, audits.native, source, { prime, flowMultiplier }),
  };
  accounting.balancedDoseWitness = {
    changedLine,
    filamentCoordinateShiftMm: 0.001,
    nominalGeometryUnchanged: auditBridge(balancedDose, source, { prime }).correctnessPassed,
    materialTotalsReconcile: alteredDoseLedgers.native.reconciled,
    rejectedStrokeCount: alteredDoseLedgers.native.doseModel.mismatchedStrokeCount,
  };
  const incompleteCoverage = structuredClone(coverage);
  incompleteCoverage.witnesses.pop();
  const missingControl = { ...ledgers };
  delete missingControl.half;
  accounting.faultChecks = [
    {
      name: "balanced stroke dose defect with conserved layer and job material",
      rejected:
        accounting.balancedDoseWitness.nominalGeometryUnchanged &&
        accounting.balancedDoseWitness.materialTotalsReconcile &&
        accounting.balancedDoseWitness.rejectedStrokeCount === 2 &&
        !evaluateBridgeAccounting(alteredDoseLedgers, audits, coverage).accepted,
    },
    {
      name: "shifted spatial material with conserved band total",
      rejected: !evaluateBridgeAccounting(movedRegion, audits, coverage).accepted,
    },
    {
      name: "omitted baseline coverage witness",
      rejected: !evaluateBridgeAccounting(ledgers, audits, incompleteCoverage).accepted,
    },
    {
      name: "missing accounting control",
      rejected: !evaluateBridgeAccounting(missingControl, audits, coverage).accepted,
    },
  ];
  accounting.accepted = accounting.accepted && accounting.faultChecks.every((f) => f.rejected);
  await save(`${prefix}/material-accounting.json`, {
    ...accounting,
    ledgers,
    differencesFromReference: Object.fromEntries(
      ["native", "overlap", "overlap25", "fullSkin", "half"].map((name) => [
        name,
        bridgeLedgerDelta(ledgers[name], ledgers.baseline),
      ]),
    ),
  });
  await save(`${prefix}/coverage-witnesses.json`, coverage);
  result.materialAccounting = {
    ...accounting,
    baselineMisses: coverage.sampleCount,
    coverageResponses: coverage.counts,
    controlMisses: coverage.controlMisses,
  };
  result.accepted = result.accepted && accounting.accepted;
  await save(`${prefix}/probe-witnesses.json`, result);
  return result;
}
