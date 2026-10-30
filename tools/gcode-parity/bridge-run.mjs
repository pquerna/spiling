/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
import { readFile, mkdir, readdir, stat } from "node:fs/promises";
import { join } from "node:path";
import { bridgeSources } from "./bridge-corpus.mjs";
import { cases, intentFor, referenceProfile, sha256 } from "./corpus.mjs";
import { decodeGcode } from "./decode.mjs";
import { auditBridge } from "./bridges.mjs";
import { bridgeMaterialLedger, bridgeLedgerDelta } from "./bridge-ledger.mjs";
import { predictExternalRoof, observeExternalRoof } from "./bridge-policy.mjs";
import { referenceValues, runBridgeProbes } from "./bridge-probes.mjs";
const EPS = 0.002;

export async function runBridgeComparisons({
  root,
  output,
  cli,
  engine,
  reference,
  manifest,
  run,
  save,
}) {
  const report = {
    schema: 1,
    scope:
      "Frozen analytic two-post bridge and one-ended coupons; matched controls then captured synthetic profiles; software only.",
    cases: [],
  };
  const base = JSON.parse(
    await readFile(join(root, "fixtures/manufacturing/complete-sparse.intent.json"), "utf8"),
  );
  const contract = JSON.parse(
    await readFile(join(root, "fixtures/manufacturing/bridges/manifest.json"), "utf8"),
  );
  const a = JSON.parse(
    await readFile(join(root, "fixtures/manufacturing/material-profile-a.json"), "utf8"),
  );
  const b = JSON.parse(
    await readFile(join(root, "fixtures/manufacturing/material-profile-b.json"), "utf8"),
  );
  const cube = cases.find((c) => c.walls === 1 && c.infillFraction === 1);
  // Limit probes straddle the independently observed ~6.00156 mm perimeter gap.
  // Explicit overrides preserve the unchanged source profile's provenance.
  const scenarios = [
    ...bridgeSources.map((source) => ({
      id: `${source.id}-matched`,
      source,
      profile: a,
      limit: 8,
    })),
    { id: "span-6-limit-above", source: bridgeSources[1], profile: a, limit: 6.01 },
    { id: "span-6-limit-below", source: bridgeSources[1], profile: a, limit: 5.99 },
    { id: "span-6-profile-a", source: bridgeSources[1], profile: a },
    { id: "span-6-profile-b", source: bridgeSources[1], profile: b },
  ];
  for (const scenario of scenarios) {
    const { id, source } = scenario,
      directory = join(output, id);
    await mkdir(directory);
    const record = {
      id,
      source,
      expectedAdmission:
        source.id !== "one-ended" &&
        source.spanMm < (scenario.limit ?? scenario.profile.profile.process.max_bridge_span_mm),
      checks: {},
    };
    report.cases.push(record);
    console.error(`Comparing ${id}`);
    try {
      const sourceBytes = await readFile(
        join(root, "fixtures/manufacturing/bridges", `${source.id}.step`),
      );
      const frozen = contract.fixtures.find((f) => f.path === `${source.id}.step`);
      if (!frozen || sha256(sourceBytes) !== frozen.sha256)
        throw Error("frozen bridge source hash mismatch");
      await save(`${id}/source.step`, sourceBytes.toString());
      const intent = intentFor(base, cube),
        selection = structuredClone(scenario.profile);
      if (scenario.limit !== undefined)
        selection.overrides.process = {
          ...selection.profile.process,
          max_bridge_span_mm: scenario.limit,
        };
      const process = selection.overrides.process ?? selection.profile.process;
      await save(`${id}/spiling.intent.json`, intent);
      await save(`${id}/material-profile.json`, selection);
      record.resolvedControls = {
        declaredBridgeLimitMm: process.max_bridge_span_mm,
        bridgeSpeedMmS: selection.profile.print_speed_mm_s * process.bridge_speed_factor,
        bridgeFlowFactor: process.bridge_flow_factor,
        flowMultiplier: selection.profile.flow_multiplier,
        bridgeFanPwm: process.bridge_fan_pwm,
        nozzleTemperatureC: selection.profile.nozzle_temperature_c,
        bedTemperatureC: selection.profile.bed_temperature_c,
      };
      let referenceIni = referenceProfile(cube);
      const settings = {
        temperature: selection.profile.nozzle_temperature_c,
        first_layer_temperature: selection.profile.nozzle_temperature_c,
        bed_temperature: selection.profile.bed_temperature_c,
        first_layer_bed_temperature: selection.profile.bed_temperature_c,
        extrusion_multiplier: selection.profile.flow_multiplier,
        bridge_flow_ratio: process.bridge_flow_factor,
        bridge_speed: record.resolvedControls.bridgeSpeedMmS,
        bridge_fan_speed: Math.round((process.bridge_fan_pwm * 100) / 255),
        min_fan_speed: Math.round((process.fan_after_first_layers * 100) / 255),
        max_fan_speed: Math.round((process.fan_after_first_layers * 100) / 255),
      };
      for (const key of [
        "perimeter_speed",
        "external_perimeter_speed",
        "small_perimeter_speed",
        "infill_speed",
        "solid_infill_speed",
        "top_solid_infill_speed",
      ])
        settings[key] = selection.profile.print_speed_mm_s;
      referenceIni = referenceValues(referenceIni, settings);
      await save(`${id}/prusa.ini`, referenceIni);
      record.roofPolicyPrediction = predictExternalRoof(source, {
        flowMultiplier: record.resolvedControls.flowMultiplier,
      });
      await save(`${id}/roof-policy-prediction.json`, record.roofPolicyPrediction);
      await run(
        reference,
        [
          "--load",
          join(directory, "prusa.ini"),
          "--export-gcode",
          "--threads",
          "1",
          "--center",
          `${source.widthMm / 2},3`,
          "--save",
          join(directory, "prusa-resolved.ini"),
          "--output",
          join(directory, "prusa.gcode"),
          join(directory, "source.step"),
        ],
        `${id}/prusa`,
      );
      const referenceDecoded = decodeGcode(await readFile(join(directory, "prusa.gcode"), "utf8"));
      record.reference = auditBridge(referenceDecoded, source);
      record.roofPolicyObservation = observeExternalRoof(
        referenceDecoded,
        record.roofPolicyPrediction,
      );
      await save(`${id}/roof-policy-observation.json`, record.roofPolicyObservation);
      record.referenceMaterial = bridgeMaterialLedger(referenceDecoded, record.reference, source, {
        flowMultiplier: record.resolvedControls.flowMultiplier,
      });
      record.checks.referenceMaterialAccounted = record.referenceMaterial.reconciled;
      await save(`${id}/reference-material.json`, record.referenceMaterial);
      await save(`${id}/reference-audit.json`, record.reference);
      let failed = null,
        nativeResult;
      try {
        nativeResult = JSON.parse(
          await run(
            cli,
            [
              "project",
              "create",
              join(directory, "project.spiling"),
              "--import",
              join(directory, "source.step"),
              "--intent",
              join(directory, "spiling.intent.json"),
              "--material-profile",
              join(directory, "material-profile.json"),
              "--compile",
              "--verify",
              "--manufacturing-out",
              join(directory, "spiling-export"),
              "--save",
              "--engine",
              engine,
            ],
            `${id}/spiling`,
          ),
        );
      } catch (error) {
        failed = error;
      }
      if (failed) {
        const stderr = await readFile(join(directory, "spiling.stderr.txt"), "utf8");
        const diagnostic = JSON.parse(stderr.trim().split("\n").at(-1));
        record.refusal = diagnostic.error;
        record.checks.expectedRefusal =
          !record.expectedAdmission &&
          record.refusal?.code === "unsupported_geometry" &&
          record.refusal.message.includes("bounded deposited anchors");
        let published = true;
        try {
          await stat(join(directory, "spiling-export"));
        } catch (error) {
          if (error.code === "ENOENT") published = false;
          else throw error;
        }
        record.checks.noRejectedExport = !published;
        if (record.expectedAdmission || !record.checks.expectedRefusal || published) throw failed;
        const roof = record.reference.layers.filter(
          (r) => r.lowerMm >= 3 - EPS && r.upperMm <= 3.2 + EPS,
        );
        const witness =
          source.id === "one-ended"
            ? roof.some((r) => r.oneOrZeroEndedGapCount > 0)
            : roof.some((r) => r.maximumGapMm > process.max_bridge_span_mm);
        record.comparison = {
          kind: "declared_anchor_admission",
          explanationStatus: witness ? "explained" : "unresolved",
          explanation:
            source.id === "one-ended"
              ? "Reference emits a nominal one-ended overhang. Native requires two actual deposited anchors with supports disabled."
              : "Reference emits a nominal span beyond the selected native declared maximum. Reference has no equivalent maximum-span admission control.",
          observedReferenceRoof: roof,
        };
      } else {
        if (!record.expectedAdmission) throw Error("native admitted a required refusal");
        manifest.engineBuild = nativeResult.hello?.engine_build;
        const verification = JSON.parse(
          await readFile(join(directory, "spiling-export/verification.json"), "utf8"),
        );
        record.checks.freshExportVerification = verification.verified === true;
        const program = await readFile(join(directory, "spiling-export/program.gcode"), "utf8");
        const decoded = decodeGcode(program);
        record.spiling = auditBridge(decoded, source, { prime: intent.printer.workflow.job });
        await save(`${id}/spiling-audit.json`, record.spiling);
        record.nativeMaterial = bridgeMaterialLedger(decoded, record.spiling, source, {
          prime: intent.printer.workflow.job,
          flowMultiplier: record.resolvedControls.flowMultiplier,
        });
        record.checks.nativeMaterialAccounted = record.nativeMaterial.reconciled;
        record.checks.nativeDeclaredDose =
          process.bridge_flow_factor === 1 &&
          record.nativeMaterial.doseModel.mismatchedStrokeCount === 0;
        await save(`${id}/native-material.json`, record.nativeMaterial);
        await save(
          `${id}/material-difference.json`,
          bridgeLedgerDelta(record.nativeMaterial, record.referenceMaterial),
        );
        record.checks.nativeNominalGeometry = record.spiling.correctnessPassed;
        const gaps = record.spiling.layers.filter(
          (r) => r.upperMm > 3.002 && r.lowerMm < 3.202 && r.twoEndedGapCount > 0,
        );
        record.checks.actualTwoEndedBridge =
          gaps.length > 0 &&
          gaps.every(
            (r) =>
              r.maximumGapMm < process.max_bridge_span_mm + 0.002 && r.oneOrZeroEndedGapCount === 0,
          );
        const gapCommands = gaps.flatMap((r) => r.gapProcess);
        record.checks.bridgeCommands =
          gapCommands.length > 0 &&
          gapCommands.every(
            (p) =>
              Math.abs(p.feedMmS - record.resolvedControls.bridgeSpeedMmS) < 1e-5 &&
              p.fanPwm === process.bridge_fan_pwm,
          );
        record.checks.thermalCommands = [
          ["M109", selection.profile.nozzle_temperature_c],
          ["M190", selection.profile.bed_temperature_c],
        ].every(([op, t]) =>
          decoded.controls.some((c) => c.op === op && (c.fields.R ?? c.fields.S) === t),
        );
        await run(
          cli,
          [
            "project",
            "inspect",
            join(directory, "project.spiling"),
            "--verify",
            "--manufacturing-out",
            join(directory, "reopened-export"),
            "--engine",
            engine,
          ],
          `${id}/reopened`,
        );
        record.checks.reopenedBytes =
          program === (await readFile(join(directory, "reopened-export/program.gcode"), "utf8"));
        record.comparison = {
          modelVolumeDifferenceMm3: record.spiling.modelVolumeMm3 - record.reference.modelVolumeMm3,
          layerCounts: [record.spiling.layerCount, record.reference.layerCount],
          referenceNominalAuditPassed: record.reference.correctnessPassed,
          explanationStatus: "unresolved",
          knownConventions: [
            "Native deposited-anchor admission uses the selected declared span; reference has no equivalent maximum-span gate.",
            "Reference integer fan percent is rounded from the captured PWM; actual commands are retained.",
            "The terminal external solid roof has a forward fitted-row/link/dose witness; first-roof process, other construction policies and complete-comparison explanation remain distinct.",
          ],
        };
      }
      record.passed = Object.values(record.checks).every((v) => v === true);
    } catch (error) {
      record.error = error.message;
      record.passed = false;
    }
    await save(`${id}/comparison.json`, record);
  }
  const byId = (id) => report.cases.find((c) => c.id === id);
  const programFor = async (id) =>
    readFile(join(output, id, "spiling-export/program.gcode"), "utf8");
  const aCase = byId("span-6-profile-a"),
    bCase = byId("span-6-profile-b");
  report.causalChecks = {};
  if (aCase?.spiling && bCase?.spiling) {
    const [ap, bp] = await Promise.all([programFor(aCase.id), programFor(bCase.id)]);
    const model = (p) =>
      decodeGcode(p).strokes.filter((s) => [s.from, s.to].every((p) => p[1] <= 6.002));
    const aa = model(ap),
      bb = model(bp);
    report.causalChecks.syntheticProfileGeometryInvariant =
      aa.length === bb.length &&
      aa.every((s, i) =>
        [s.from, s.to].every((p, j) =>
          p.every((v, k) => Math.abs(v - [bb[i].from, bb[i].to][j][k]) < 1e-6),
        ),
      );
    report.causalChecks.syntheticProfileExtrusionRatio =
      Math.abs(
        bCase.spiling.modelVolumeMm3 / aCase.spiling.modelVolumeMm3 -
          bCase.resolvedControls.flowMultiplier / aCase.resolvedControls.flowMultiplier,
      ) < 1e-6;
    const hashes = await Promise.all(
      ["span-6-matched", "span-6-limit-above", "span-6-profile-a"].map(async (id) =>
        sha256(await programFor(id)),
      ),
    );
    report.causalChecks.admittedSpanLimitLeavesCommandsInvariant = new Set(hashes).size === 1;
    report.causalChecks.belowLimitRefusesWithoutExport =
      byId("span-6-limit-below")?.checks.noRejectedExport === true &&
      byId("span-6-limit-below")?.checks.expectedRefusal === true;
  }
  const baseline = byId("span-6-matched");
  if (baseline?.passed) {
    try {
      report.controlledProbes = await runBridgeProbes({
        record: baseline,
        output,
        cli,
        engine,
        reference,
        run,
        save,
      });
    } catch (error) {
      report.controlledProbes = { accepted: false, error: error.message };
    }
  }
  report.nativeSoftwarePassed =
    report.cases.length === scenarios.length &&
    report.cases.every((c) => c.passed) &&
    Object.keys(report.causalChecks).length === 4 &&
    Object.values(report.causalChecks).every((v) => v === true);
  report.referenceAuditsPassed =
    report.cases.length === scenarios.length &&
    report.cases.every((c) => c.reference?.correctnessPassed === true);
  report.correctnessPassed =
    report.nativeSoftwarePassed &&
    report.referenceAuditsPassed &&
    report.controlledProbes?.accepted === true;
  // Correct software gates do not silently close unexplained reference output.
  report.differencesExplained =
    report.nativeSoftwarePassed &&
    report.cases.every((c) => c.comparison?.explanationStatus === "explained");
  report.selectedPolicyExplained =
    report.nativeSoftwarePassed &&
    report.controlledProbes?.accepted === true &&
    report.controlledProbes.externalRoofPolicy?.accepted === true &&
    report.cases.length === scenarios.length &&
    report.cases.every(
      (c) =>
        c.roofPolicyObservation?.accepted === true && c.roofPolicyObservation.everyCommandAccounted,
    );
  report.selectedPolicyScope =
    "Frozen external terminal rectilinear roof: integer row fit, links/free ends and fitted dose, decided by a one-setting 90-degree reference control. This does not change strict nominal coverage or claim every comparison/physical difference explained.";
  report.referenceDifferences = report.cases.map((c) => ({ id: c.id, ...c.comparison }));
  const hashTree = async (relative) => {
    for (const item of await readdir(join(output, relative), { withFileTypes: true })) {
      const path = join(relative, item.name);
      if (item.isDirectory()) {
        if (!item.name.endsWith(".spiling")) await hashTree(path);
      } else manifest.files[path] = sha256(await readFile(join(output, path)));
    }
  };
  for (const c of report.cases) await hashTree(c.id);
  for (const name of [
    "bridge-run.mjs",
    "bridge-probes.mjs",
    "bridge-ledger.mjs",
    "bridge-policy.mjs",
    "bridge-corpus.mjs",
    "bridges.mjs",
    "corpus.mjs",
    "decode.mjs",
    "anchors.mjs",
  ])
    manifest.files[`tools/gcode-parity/${name}`] = sha256(
      await readFile(join(root, "tools/gcode-parity", name)),
    );
  manifest.files["tools/compare-slicers.mjs"] = sha256(
    await readFile(join(root, "tools/compare-slicers.mjs")),
  );
  return report;
}
