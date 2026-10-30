/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { observeGaps } from "./anchors.mjs";
import { decodeGcode } from "./decode.mjs";
import { auditCube, compareAudits } from "./analyze.mjs";
import { cases, cubeSource, intentFor, referenceProfile } from "./corpus.mjs";
const preamble = "G21\nG90\nM83\nG92 E0\nG1 X0 Y0 Z0.2 F2100\n";
const spec = cases.find((c) => c.walls === 2 && c.infillFraction === 0);
function synthetic({
  missingWall = false,
  missingRoof = false,
  flow = 1,
  refine = false,
  hint = "Wall",
} = {}) {
  const lines = [
    "G21",
    "G90",
    "M83",
    "G92 E0",
    "G1 X0 Y0 Z0.2 F2100",
    `M221 S${flow * 100}`,
    `;TYPE:${hint}`,
  ];
  const elevations = Array.from({ length: 50 }, (_, i) => (i + 1) * 0.2);
  if (refine) elevations.splice(49, 0, 9.9);
  let lower = 0;
  for (const z of elevations) {
    lines.push(`G1 F${lower === 0 ? 1050 : 2100}`, `M106 S${lower === 0 ? 0 : 128}`);
    const height = z - lower,
      area = height * (0.45 - height * (1 - Math.PI / 4));
    const stroke = (a, b) => {
      lines.push(`G1 X${a[0].toFixed(6)} Y${a[1].toFixed(6)} Z${z.toFixed(6)}`);
      lines.push(
        `G1 X${b[0].toFixed(6)} Y${b[1].toFixed(6)} E${((Math.hypot(b[0] - a[0], b[1] - a[1]) * area) / (Math.PI * (1.75 / 2) ** 2)).toFixed(8)}`,
      );
    };
    for (const inset of missingWall ? [0.225] : [0.225, 0.675]) {
      const p = [
        [inset, inset],
        [10 - inset, inset],
        [10 - inset, 10 - inset],
        [inset, 10 - inset],
      ];
      for (let i = 0; i < 4; i++) stroke(p[i], p[(i + 1) % 4]);
    }
    if (lower < 0.8 - 1e-6 || (!missingRoof && z > 9.2 + 1e-6))
      for (let y = 0.9; y <= 9.1 + 1e-6; y += 0.4) stroke([0.9, y], [9.1, y]);
    lower = z;
  }
  return lines.join("\n");
}
const audit = (program) => auditCube(decodeGcode(program), spec);
test("analytic hollow cube is covered and comments cannot supply geometry", () => {
  const a = audit(synthetic()),
    b = audit(synthetic({ hint: "untrusted wrong role" }));
  assert.equal(a.correctnessPassed, true, JSON.stringify(a.violations));
  assert.equal(b.correctnessPassed, true);
  assert.equal(a.modelVolumeMm3, b.modelVolumeMm3);
});
test("missing wall, missing roof, halved flow and outside extrusion fail independently", () => {
  for (const [option, code] of [
    [{ missingWall: true }, "wall_count"],
    [{ missingRoof: true }, "mandatory_coverage"],
    [{ flow: 0.5 }, "bead_width"],
  ]) {
    const a = audit(synthetic(option));
    assert.equal(a.correctnessPassed, false);
    assert(
      a.violations.some((v) => v.code === code),
      JSON.stringify(a.violations),
    );
  }
  const a = audit(synthetic() + "\nG1 X11 Y5 E1");
  assert(a.violations.some((v) => v.code === "unauthorized_extrusion"));
});
test("physical band resampling preserves material through two half-height intervals", () => {
  const a = audit(synthetic()),
    b = audit(synthetic({ refine: true }));
  assert.equal(b.correctnessPassed, true, JSON.stringify(b.violations));
  assert.equal(b.layerCount, 51);
  assert(Math.abs(b.bands.reduce((sum, b) => sum + b.volumeMm3, 0) - b.modelVolumeMm3) < 1e-9);
  // Different stadium cross-section at half height changes volume; resampling
  // must conserve each actual program, never invent agreement between them.
  assert.equal(
    compareAudits(a, b).differences.find((d) => d.id === "layer_schedule").status,
    "unresolved",
  );
});
test("decimal E resets do not erase physical retraction debt", () => {
  const d = decodeGcode(preamble + "G1 E-0.8\nG92 E0\nG1 E0.3\nG1 E0.5\nG1 X1 E0.1");
  assert.equal(d.retractedFilamentMm, 0.8);
  assert.equal(d.recoveredFilamentMm, 0.8);
  assert.equal(d.strokes.length, 1);
  assert.equal(d.strokes[0].filamentMm, 0.1);
  assert.equal(d.stationaryFilamentMm, 0);
  assert.throws(() => decodeGcode(preamble + "G1 E-0.8\nG92 E0\nG1 X1 E0.8"), /moving recovery/);
  assert.throws(() => decodeGcode(preamble + "G1 E-0.8\nG92 E0"), /unrecovered/);
});
test("absolute extrusion resets, relative positioning and scale controls are decoded", () => {
  const d = decodeGcode(
    "G21\nG90\nM82\nG92 E100\nG1 X0 Y0 Z.2 F2100\nG1 X1 E100.1\nG92 E0\nM220 S50\nM221 S50\nG91\nM83\nG1 X1 E.2",
  );
  assert.equal(d.strokes.length, 2);
  assert.equal(d.strokes[1].to[0], 2);
  assert.equal(d.strokes[1].filamentMm, 0.1);
  assert.equal(d.strokes[1].feedMmMin, 1050);
});
test("unsupported or ambiguous motion, units and malformed decimals refuse", () => {
  for (const text of [
    "G20",
    "G2 X1 Y1 I0 J1",
    "T1",
    "G1 X1 X2",
    "G1 XNaN",
    "G1 X1e3",
    "G1X1",
    "G1 X1 E.000000001",
    "G91\nG1 E1",
  ])
    assert.throws(() => decodeGcode(preamble + text), /line/);
  assert.throws(() => decodeGcode("G90\nM83\nG1 X0 Y0 Z.2 F2100"), /units/);
  assert.throws(() => decodeGcode("G21\nG90\nM83\nG1 X0 Y0 Z.2 E1 F2100"), /unknown position/);
  assert.throws(() => decodeGcode(preamble + "G1 Z.4 X1 E1"), /nonplanar/);
});
test("homing invalidates positions and unknown fields cannot hide motion", () => {
  assert.throws(() => decodeGcode(preamble + "G28\nG1 X1 E.1"), /unknown position/);
  assert.throws(() => decodeGcode(preamble + "M104 X20 S200"), /unexpected/);
  assert.throws(() => decodeGcode(preamble + "M200 D1.75"), /volumetric/);
});
test("fractional reference fan PWM is retained and tool/fan selectors are bounded", () => {
  const d = decodeGcode(preamble + "M106 S127.5\nG1 X1 E.1");
  assert.equal(d.strokes[0].fanPwm, 127.5);
  for (const command of ["M106 S256", "M106 P1", "M104 T1 S200", "M149 F"])
    assert.throws(() => decodeGcode(preamble + command));
});
test("corpus derivative is coherent and refuses changed analytic inputs", () => {
  const source = readFileSync(
    new URL("../../fixtures/geometry/box-mm.step", import.meta.url),
    "utf8",
  );
  const cube = cubeSource(source);
  assert(cube.includes("FILE_NAME('cube-10-derived.step'"));
  assert(!cube.includes("(20.0, 10.0, 8.0)"));
  assert(cube.includes("(10.0, 10.0, 10.0)"));
  assert.throws(() =>
    cubeSource(source.replace("PLANE('', #81)", "CYLINDRICAL_SURFACE('', #81, 1.0)")),
  );
  assert.throws(() => cubeSource(source.replace("(-1.0, 0.0, 0.0)", "(-0.5, 0.5, 0.0)")));
  const base = JSON.parse(
    readFileSync(
      new URL("../../fixtures/manufacturing/complete-sparse.intent.json", import.meta.url),
    ),
  );
  assert.equal(intentFor(base, spec).recipe.infill_fraction, 0);
  assert.equal(base.recipe.infill_fraction, 0.25);
  assert(referenceProfile(spec).includes("fill_density = 0%"));
});

test("local deficits, duplicated strokes and hidden speed defaults cannot pass on totals", () => {
  const thin = audit(synthetic({ flow: 0.9 }));
  assert(
    thin.violations.some((v) => v.code === "mandatory_coverage"),
    JSON.stringify(thin.violations),
  );
  const duplicate = audit(synthetic() + "\nG1 X.225 Y.225\nG1 X9.775 Y.225 E.3");
  assert(duplicate.violations.some((v) => v.code === "duplicate_stroke"));
  const slow = audit(synthetic().replaceAll("G1 F2100", "G1 F900"));
  assert(slow.violations.some((v) => v.code === "process_signature"));
  const a = audit(synthetic()),
    b = structuredClone(a);
  b.bands[0].volumeMm3 += 1;
  b.bands[1].volumeMm3 -= 1;
  assert.equal(
    compareAudits(a, b).differences.find((d) => d.id === "material_distribution").status,
    "unresolved",
  );
});

test("independent anchor observer separates interior gaps, endpoint overlap and missing anchors", () => {
  const candidate = { from: [0, 0, 0.4], to: [10, 0, 0.4] };
  const crossing = (x) => ({ from: [x, -1, 0.2], to: [x, 1, 0.2] });
  const observed = observeGaps(candidate, [crossing(0), crossing(5), crossing(10)], 0.4);
  assert.equal(observed.gaps.length, 2);
  assert(observed.gaps.every((g) => g.left && g.right && Math.abs(g.spanMm - 4.6) < 1e-10));
  assert(Math.abs(observed.coveredLengthMm - 0.8) < 1e-10);
  const missing = observeGaps(candidate, [crossing(5)], 0.4);
  assert(missing.gaps.every((g) => !(g.left && g.right)));
  const shifted = observeGaps(candidate, [crossing(-0.3), crossing(10.3)], 0.4);
  assert.equal(shifted.coveredLengthMm, 0);
  assert(shifted.gaps[0].left && shifted.gaps[0].right);
  const empty = observeGaps(candidate, [], 0.4);
  assert.equal(empty.gaps[0].spanMm, 10);
  assert(!empty.gaps[0].left && !empty.gaps[0].right);
  assert.throws(() => observeGaps(candidate, [], NaN));
});

test("anchor observations preserve subdivision, reversal and zero-area tangency", () => {
  const s = { from: [0, 0, 0.4], to: [10, 0, 0.4] };
  const whole = [{ from: [5, -1, 0.2], to: [5, 1, 0.2] }];
  const split = [
    { from: [5, -1, 0.2], to: [5, 0, 0.2] },
    { from: [5, 0, 0.2], to: [5, 1, 0.2] },
  ];
  assert.deepEqual(observeGaps(s, whole, 0.4), observeGaps(s, split, 0.4));
  assert.deepEqual(observeGaps(s, whole, 0.4), observeGaps({ from: s.to, to: s.from }, whole, 0.4));
  const tangent = observeGaps(s, [{ from: [0, 0.4, 0.2], to: [10, 0.4, 0.2] }], 0.4);
  assert.equal(tangent.coveredLengthMm, 0);
  assert(!tangent.gaps[0].left && !tangent.gaps[0].right);
  let charged = 0;
  observeGaps(s, split, 0.4, () => charged++);
  assert.equal(charged, 2);
});

test("nominal circular bridge fit uses XYZ/E and rejects changed extrusion or pitch", async () => {
  const { observeCubeConstruction } = await import("./construction.mjs");
  const specimen = ({ flow = 1, pitch = 0.45, hint = "arbitrary" } = {}) => {
    const lines = [
      "G21",
      "G90",
      "M83",
      "G92 E0",
      "G1 X1 Y1 Z9.4 F1260",
      "M106 S255",
      `;TYPE:${hint}`,
    ];
    for (let i = 0; i < 8; i++) {
      lines.push(`G1 X1 Y${1 + i * pitch}`);
      lines.push(
        `G1 X9 E${((((8 * Math.PI * 0.4 ** 2) / 4) * flow) / (Math.PI * (1.75 / 2) ** 2)).toFixed(8)}`,
      );
    }
    return observeCubeConstruction(decodeGcode(lines.join("\n")), spec).layers[0].bridgeRows;
  };
  const a = specimen(),
    b = specimen({ hint: "External perimeter" });
  assert(a.circularAgreement);
  assert.deepEqual(a, b);
  assert(!specimen({ flow: 0.5 }).circularAgreement);
  assert(!specimen({ pitch: 0.5 }).circularAgreement);
});

test("construction wall accounting preserves subdivision, reversal and all extrusion", async () => {
  const { observeCubeConstruction } = await import("./construction.mjs");
  const prefix = "G21\nG90\nM83\nG92 E0\nG1 X0.225 Y0.225 Z0.2 F2100\n";
  const a = observeCubeConstruction(decodeGcode(prefix + "G1 Y9.775 E1"), spec).layers[0];
  const b = observeCubeConstruction(decodeGcode(prefix + "G1 Y5 E0.5\nG1 Y9.775 E0.5"), spec)
    .layers[0];
  const c = observeCubeConstruction(decodeGcode(prefix + "G1 Y9.775\nG1 Y0.225 E1"), spec)
    .layers[0];
  for (const x of [b, c]) {
    assert(Math.abs(x.walls.volumeMm3 - a.walls.volumeMm3) < 1e-12);
    assert(Math.abs(x.walls.lengthMm - a.walls.lengthMm) < 1e-12);
    assert.equal(x.linksAndShortFill.volumeMm3, 0);
    assert.equal(x.reconciliationResidualMm3, 0);
  }
});

test("reference probes cannot explain roofs from missing or unchanged controls", async () => {
  const { evaluateReferenceProbes } = await import("./probes.mjs");
  const a = audit(synthetic());
  assert.equal(evaluateReferenceProbes(a, []).roofPreparation, false);
  assert.equal(evaluateReferenceProbes(a, []).circularBridge, false);
  const unchanged = ["roof-shift", "half-bridge-flow", "shell-disabled"].map((id) => ({
    id,
    audit: a,
  }));
  assert.equal(evaluateReferenceProbes(a, unchanged).roofPreparation, false);
  assert.equal(evaluateReferenceProbes(a, unchanged).circularBridge, false);
});

test("internal-solid probe refuses unchanged reference and unproven native invariance", async () => {
  const { evaluateSolidProbe } = await import("./probes.mjs");
  const a = audit(synthetic());
  assert.equal(evaluateSolidProbe(a, a, a, a, true).explained, false);
  assert.equal(evaluateSolidProbe(a, a, a, a, false).explained, false);
});

test("whole-material explanation requires all real density controls and deciding policy witnesses", async () => {
  const { explainCubeMaterialFamily } = await import("./material-ledger.mjs");
  assert.equal(explainCubeMaterialFamily([], 2).explained, false);
  const a = audit(synthetic());
  const records = [0, 0.25, 1].map((infillFraction) => ({
    specification: { ...spec, infillFraction },
    spiling: a,
    reference: a,
  }));
  assert.equal(explainCubeMaterialFamily(records, 2).explained, false);
});

test("bridge observer rejects missing post, missing roof, void extrusion and duplicate commands", async () => {
  const { auditBridge } = await import("./bridges.mjs");
  const source = { id: "span-2", widthMm: 6 };
  const program = ({
    missingPost = false,
    missingRoof = false,
    voidStroke = false,
    duplicate = false,
  } = {}) => {
    const lines = ["G21", "G90", "M83", "G92 E0", "G1 X0 Y0 Z0.2 F2100"];
    for (let layer = 1; layer <= 20; layer++) {
      const z = layer * 0.2;
      if (missingRoof && layer > 15) continue;
      const intervals =
        layer <= 15
          ? missingPost
            ? [[0, 2]]
            : [
                [0, 2],
                [4, 6],
              ]
          : [[0, 6]];
      for (const [lo, hi] of intervals)
        for (let y = 0.225; y < 5.9; y += 0.25) {
          lines.push(`G1 X${lo + 0.225} Y${y.toFixed(6)} Z${z.toFixed(6)}`);
          lines.push(`G1 X${hi - 0.225} E0.04`);
          if (duplicate) lines.push(`G1 X${lo + 0.225} E0.04`);
        }
      if (voidStroke && layer === 5) lines.push(`G1 X1 Y3 Z${z}`, "G1 X5 E0.1");
    }
    return lines.join("\n");
  };
  const good = auditBridge(decodeGcode(program()), source);
  assert.equal(good.correctnessPassed, true, JSON.stringify(good.violations));
  assert(good.layers.some((r) => r.twoEndedGapCount > 0));
  for (const [fault, code] of [
    [{ missingPost: true }, "mandatory_nominal_coverage"],
    [{ missingRoof: true }, "model_height"],
    [{ voidStroke: true }, "extrusion_in_source_void"],
    [{ duplicate: true }, "duplicate_stroke"],
  ])
    assert(
      auditBridge(decodeGcode(program(fault)), source).violations.some((v) => v.code === code),
      code,
    );
});

test("bridge anchor runs ignore straight command partitions but retain travel, turns and reversal", async () => {
  const { depositionRuns } = await import("./bridges.mjs");
  const a = { line: 10, from: [0, 0, 0.2], to: [3, 0, 0.2], lengthMm: 3 };
  const b = { line: 11, from: [3, 0, 0.2], to: [8, 0, 0.2], lengthMm: 5 };
  assert.equal(depositionRuns([a, b]).length, 1);
  assert.deepEqual(depositionRuns([a, b])[0].lines, [10, 11]);
  assert.equal(depositionRuns([a, { ...b, line: 12 }], [{ line: 11 }]).length, 2);
  assert.equal(depositionRuns([a, { ...b, to: [3, 3, 0.2] }]).length, 2);
  assert.equal(depositionRuns([a, { ...b, to: [0, 0, 0.2] }]).length, 2);
  assert.equal(depositionRuns([a, { ...b, from: [3.01, 0, 0.2] }]).length, 2);
});

test("reference probe settings refuse missing, duplicate and injected fields", async () => {
  const { referenceValues } = await import("./bridge-probes.mjs");
  assert.equal(
    referenceValues("infill_overlap = 0\n", { infill_overlap: "15%" }),
    "infill_overlap = 15%\n",
  );
  assert.throws(() => referenceValues("other = 0\n", { infill_overlap: 1 }), /absent or ambiguous/);
  assert.throws(
    () => referenceValues("infill_overlap = 0\ninfill_overlap = 1\n", { infill_overlap: 2 }),
    /absent or ambiguous/,
  );
  assert.throws(
    () => referenceValues("infill_overlap = 0\n", { infill_overlap: "0\nextra = 1" }),
    /invalid reference setting/,
  );
});

test("solid construction comparison retains byte/order differences and rejects changed dose", async () => {
  const { compareSolidPrograms } = await import("./bridge-probes.mjs");
  const pre = "G21\nG90\nM83\nG92 E0\nG1 X0 Y0 Z3.2 F2100\n";
  const a = pre + "G1 X2 E.1\nG1 Y1\nG1 X0 E.1\n";
  const b = pre + "G1 Y1\nG1 X2 E.1\nG1 Y0\nG1 X0 E.1\n";
  const comparison = compareSolidPrograms(a, b);
  assert.equal(comparison.byteIdentical, false);
  assert.equal(comparison.depositionMultisetIdentical, true);
  assert.deepEqual(comparison.changedDepositionOrderHeightsMm, [3.2]);
  assert.equal(
    compareSolidPrograms(a, b.replaceAll("E.1", "E.05")).depositionMultisetIdentical,
    false,
  );
});

test("bridge material accounting conserves physical bands and spatial cells across command splits", async () => {
  const { bridgeMaterialLedger, bridgeLedgerReconciles, bridgeLedgerDelta } =
    await import("./bridge-ledger.mjs");
  const source = { id: "span-6", widthMm: 10 };
  const area = (h) => h * (0.45 - h * (1 - Math.PI / 4));
  const make = (split) => {
    const lines = ["G21", "G90", "M83", "G92 E0"];
    const layers = [];
    let lower = 0;
    for (const z of [3.8, 3.9, 4]) {
      // Only the terminal physical bands are exercised here, independently
      // of the complete coupon audit's earlier post/roof obligations.
      const lo = z === 3.8 ? 3.6 : lower;
      const length = 9,
        h = z - lo;
      lines.push(`G1 X0.5 Y3 Z${z} F2100`);
      if (split) lines.push(`G1 X5 E${((4.5 * area(h)) / (Math.PI * (1.75 / 2) ** 2)).toFixed(8)}`);
      lines.push(
        `G1 X9.5 E${(((split ? 4.5 : 9) * area(h)) / (Math.PI * (1.75 / 2) ** 2)).toFixed(8)}`,
      );
      layers.push({ lowerMm: lo, upperMm: z, lengthMm: length });
      lower = z;
    }
    const decoded = decodeGcode(lines.join("\n"));
    for (const l of layers)
      l.volumeMm3 = decoded.strokes
        .filter((s) => s.to[2] === l.upperMm)
        .reduce((v, s) => v + s.volumeMm3, 0);
    const audit = { layers, modelVolumeMm3: layers.reduce((v, l) => v + l.volumeMm3, 0) };
    return { decoded, audit, ledger: bridgeMaterialLedger(decoded, audit, source) };
  };
  const a = make(false),
    b = make(true);
  assert(a.ledger.reconciled && b.ledger.reconciled);
  assert(Math.abs(a.ledger.heightAdjustmentMm3) > 0.5);
  assert(Math.abs(bridgeLedgerDelta(a.ledger, b.ledger).volumeMm3) < 1e-6);
  const last = a.ledger.bands.at(-1);
  assert(Math.abs(last.lengthMm - 18) < 1e-6);
  assert.deepEqual(last.cells.map((c) => c.region).sort(), ["left-post", "right-post", "span"]);
  assert(Math.abs(last.cells.find((c) => c.region === "span").lengthMm - 12) < 1e-6);
  const moved = structuredClone(a.ledger);
  // Move an entire consistent contribution between regions: totals and
  // V=nominal+height+residual still hold, but spatial resampling must reject.
  for (const key of [
    "volumeMm3",
    "lengthMm",
    "commonHeightVolumeMm3",
    "heightAdjustmentMm3",
    "doseResidualMm3",
  ]) {
    const shift = moved.bands.at(-1).cells[0][key] * 0.1;
    moved.bands.at(-1).cells[0][key] -= shift;
    moved.bands.at(-1).cells[1][key] += shift;
  }
  assert.equal(bridgeLedgerReconciles(moved, a.audit), false);
  const balanced = structuredClone(b.decoded);
  balanced.strokes[0].volumeMm3 += 0.01;
  balanced.strokes[1].volumeMm3 -= 0.01;
  const broken = bridgeMaterialLedger(balanced, b.audit, source);
  assert.equal(broken.reconciled, true);
  assert.equal(broken.doseModel.mismatchedStrokeCount, 2);
  assert.equal(b.ledger.doseModel.mismatchedStrokeCount, 0);

  const unaccounted = structuredClone(a.decoded);
  unaccounted.strokes[0].to[2] = 3.7;
  assert.throws(() => bridgeMaterialLedger(unaccounted, a.audit, source), /unaccounted/);
});

test("bridge coverage witnesses retain every miss, control response and dose countermodel", async () => {
  const { bridgeCoverageWitnesses } = await import("./bridge-ledger.mjs");
  const variant = (x, dose) => {
    const p = [0.5, 3],
      z = 0.2;
    const lines = `${preamble}G1 X${x} Y1\nG1 X${x} Y5 E${((4 * dose) / (Math.PI * (1.75 / 2) ** 2)).toFixed(8)}`;
    return {
      decoded: decodeGcode(lines),
      audit: {
        layers: [
          {
            lowerMm: 0,
            upperMm: z,
            uncoveredGridSamples: Math.abs(x - p[0]) > 0.227 ? 1 : 0,
            uncoveredPoints: Math.abs(x - p[0]) > 0.227 ? [p] : [],
          },
        ],
      },
    };
  };
  const variants = {
    baseline: variant(0.75, 0.0814159),
    overlap: variant(0.7, 0.0814159),
    wide: variant(0.75, 0.11),
  };
  const result = bridgeCoverageWitnesses({ id: "span-6", widthMm: 10 }, variants);
  assert.equal(result.sampleCount, 1);
  assert.equal(result.controlMisses.overlap.baselineMissesRepaired, 1);
  assert.equal(result.controlMisses.wide.baselineMissesRetained, 1);
  const w = result.witnesses[0];
  assert.equal(w.disposition, "covered-by-controlled-construction");
  assert.deepEqual(w.coveringControls, ["overlap"]);
  assert.equal(w.observations.baseline.nominalCovered, false);
  assert.equal(w.observations.baseline.equivalentStadiumCovered, false);
  assert.equal(w.observations.wide.nominalCovered, false);
  assert.equal(w.observations.wide.equivalentStadiumCovered, true);
  assert(Math.abs(w.observations.baseline.nearest.distanceMm - 0.25) < 1e-8);
  const changed = structuredClone(variants);
  changed.baseline.audit.layers[0].uncoveredPoints = [];
  assert.throws(() => bridgeCoverageWitnesses({}, changed), /complete baseline/);
  const shifted = structuredClone(variants);
  shifted.overlap.audit.layers[0].lowerMm = 0.1;
  assert.throws(() => bridgeCoverageWitnesses({}, shifted), /matching physical intervals/);
});
