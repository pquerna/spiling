/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
import assert from "node:assert/strict";
import { mkdir, readFile, readdir, stat, unlink } from "node:fs/promises";
import { join, relative } from "node:path";
import { cases, referenceProfile, sha256 } from "./corpus.mjs";
import { referenceValues, verifyReferenceSettings } from "./reference-settings.mjs";
import { decodeGcode } from "./decode.mjs";
import {
  housing,
  auditHousing,
  checkHousingFaults,
  compareHousingRevision,
  housingOverlay,
  housingSourceOverlay,
} from "./mechanical.mjs";
import { reportHtml } from "./report.mjs";

const artifacts = ["program.gcode", "plan.json", "verification.json", "provenance.json"];
const settings = {
  layer_height: 0.1,
  first_layer_height: 0.1,
  top_solid_layers: 8,
  bottom_solid_layers: 8,
  retract_length: 0.8,
  retract_lift: 0.2,
};
async function fileText(path) {
  if ((await stat(path)).size > 16 * 1024 * 1024) throw Error("mechanical evidence file limit");
  return readFile(path, "utf8");
}
function nativeStatus(result, intent, identity) {
  assert.deepEqual(
    result.manufacturing.intent,
    intent,
    "captured intent differs from requested input",
  );
  assert.equal(
    result.project.saved_revision,
    result.project.revision,
    "job is not completely saved",
  );
  assert.equal(result.manufacturing.verification.verified, true);
  assert.equal(result.manufacturing.verified_artifact_hash, result.manufacturing.artifact.hash);
  const summary = result.manufacturing.artifact.summary;
  assert.equal(summary.native_geometry.certified_layer_count, summary.layers);
  assert(summary.native_geometry.achieved_shape_error_mm <= intent.recipe.native_shape_error_mm);
  assert.equal(
    summary.support_deposition_segments,
    0,
    "this comparison requires the actual no-support result",
  );
  assert.equal(summary.support_deposited_volume_mm3, 0);
  const pose = result.occurrences[0].pose;
  assert.deepEqual(pose.rotation_xyzw, [0, 0, 0, 1]);
  assert.equal(result.occurrences.length, 1);
  if (identity) {
    assert.equal(result.hello.engine_build, identity.engineBuild);
    assert.equal(result.occurrences[0].definition_id, identity.definitionId);
    assert.deepEqual(pose, identity.pose);
  }
  return {
    engineBuild: result.hello.engine_build,
    definitionId: result.occurrences[0].definition_id,
    pose,
  };
}
function comparison(a, b, terminalRetractionMm) {
  const sameSchedule =
    a.layers.length === b.layers.length &&
    a.layers.every((l, i) => Math.abs(l.upperMm - b.layers[i].upperMm) < 2e-6);
  const floorEvent = a.layers.some((l) => Math.abs(l.upperMm - housing.floorMm) < 2e-6);
  const referenceFixed = b.layers.every((l, i) => Math.abs(l.upperMm - (i + 1) * 0.1) < 2e-6);
  const difference = b.modelVolumeMm3 - a.modelVolumeMm3;
  const differences = [
    {
      id: "source_observations",
      status: a.correctnessPassed && b.correctnessPassed ? "matched" : "failed",
      explanation:
        "Both actual programs undergo the same original analytic material, protected pocket, finite coverage-distance and interval-accounting observations. These are bounded software observations, not continuous or physical coverage.",
    },
    {
      id: "layer_schedule",
      status: sameSchedule ? "matched" : floorEvent && referenceFixed ? "explained" : "unresolved",
      spilingLayers: a.layerCount,
      referenceLayers: b.layerCount,
      explanation:
        "The reference uses the requested fixed 0.1 mm schedule. Native scheduling may refine and retains the source pocket-floor event at Z=4.13 under its declared native error bound. Compare common physical bands; this does not assert identical construction or explain total material.",
    },
    {
      id: "support_policy",
      status: "explained",
      explanation:
        "Canonical native breakaway support remains enabled and must actually emit zero support. Reference supports are disabled. This establishes the same no-support output on this case only; admission policies are explicitly different. The separate disabled-native-support variant remains a required open refusal.",
    },
    {
      id: "terminal_retraction",
      status: "explained",
      referenceRetainedFilamentMm: terminalRetractionMm,
      explanation:
        "Reference terminal retraction is explicitly bounded by its configured 0.8 mm and retained in the debt ledger. Native replay closes its own debt. Retraction is never counted as model extrusion or silently forgiven.",
    },
    {
      id: "model_material",
      status: Math.abs(difference) < 1e-7 ? "matched" : "unresolved",
      referenceMinusSpilingMm3: difference,
      relativeToSpiling: difference / a.modelVolumeMm3,
      materialAccounting: {
        spiling: a.materialAccounting,
        reference: b.materialAccounting,
        referenceMinusSpiling: Object.fromEntries(
          ["fixedNominalVolumeMm3", "actualIntervalAdjustmentMm3", "commandedDoseResidualMm3"].map(
            (key) => [key, b.materialAccounting[key] - a.materialAccounting[key]],
          ),
        ),
      },
      explanation:
        "Physical-band accounting locates all commanded material differences. Nominal widths, internal fitting/linking, paths and actual-height policies may differ. Accounting and passing finite probes do not by themselves establish a causal policy explanation or physical equivalence.",
      bands: a.bands.map((band, i) => ({
        lowerMm: band.lowerMm,
        upperMm: band.upperMm,
        sourceVolumeMm3: band.sourceVolumeMm3,
        volumeDifferenceMm3: b.bands[i].volumeMm3 - band.volumeMm3,
        coreVolumeDifferenceMm3: b.bands[i].coreVolumeMm3 - band.coreVolumeMm3,
      })),
    },
  ];
  return {
    correctnessPassed: a.correctnessPassed && b.correctnessPassed,
    differencesExplained: differences.every(
      (d) => d.status === "matched" || d.status === "explained",
    ),
    differences,
  };
}
async function hashEvidence(directory, manifest, output) {
  for (const name of await readdir(directory)) {
    const path = join(directory, name),
      entry = await stat(path);
    if (entry.isDirectory()) {
      if (!name.endsWith(".spiling")) await hashEvidence(path, manifest, output);
    } else if (entry.isFile() && entry.size <= 16 * 1024 * 1024)
      manifest.files[relative(output, path)] = sha256(await readFile(path));
  }
}

export async function runMechanicalComparisons({
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
      "Frozen original rounded housing, then 25% to 50% sparse revision on the same saved native job. Independent actual-command observations, fresh emitted-byte verification, Undo/Redo and source-independent reopen; software only, NOT MACHINE READY.",
    cases: [],
    unqualifiedVariants: [
      {
        id: "native_support_disabled",
        status: "not_exercised_by_this_runner",
        stage: "wall source-local overlap qualification",
        source: housing.source,
        explanation:
          "The same housing with supports and retraction disabled reaches a source-local wall excess refusal. Do not treat the canonical ordinary passing job as qualification of that variant.",
      },
    ],
  };
  const overlays = {},
    nativeAudits = [],
    referenceAudits = [];
  const input = JSON.parse(
    await fileText(join(root, "fixtures/manufacturing/housing-ordinary.intent.json")),
  );
  const recipe = input.recipe;
  assert.equal(recipe.layer_height_mm, 0.1);
  assert.equal(recipe.bead_width_mm, 0.45);
  assert.equal(recipe.infill_fraction, 0.25);
  assert.equal(recipe.perimeter_count, 2);
  assert.equal(recipe.native_shape_error_mm, 0.1);
  assert.equal(recipe.flow_multiplier, 1);
  assert.equal(recipe.top_skin_thickness_mm, 0.8);
  assert.equal(recipe.bottom_skin_thickness_mm, 0.8);
  assert.equal(recipe.rectilinear_axis, "alternating_xy");
  assert.equal(recipe.top_skin_axis, "alternating_xy");
  assert.equal(recipe.sparse_infill.kind, "fine");
  assert.equal(recipe.supports.kind, "breakaway");
  assert.equal(recipe.process.retraction.kind, "enabled");
  assert.equal(recipe.process.retraction.distance_mm, 0.8);
  const source = await readFile(join(root, housing.source));
  assert.equal(sha256(source), housing.sha256);
  await save("source.step", source.toString());
  await save("external-source.step", source.toString());
  await save("contract.json", {
    ...housing,
    nativeSupportPolicy: recipe.supports,
    referenceSupportPolicy: "disabled; actual native zero support required",
    referenceTerminalRetractionLimitMm: 0.8,
    referenceSettings: settings,
    revisionInfillFraction: [0.25, 0.5],
    nativeIntentSha256: sha256(
      await readFile(join(root, "fixtures/manufacturing/housing-ordinary.intent.json")),
    ),
    costScope:
      "Per-command elapsed milliseconds; complete-workflow RSS must be retained by the documented external resource wrapper. No speed or memory target is claimed.",
  });
  const project = join(output, "job.spiling");
  let identity, baselineHash;
  for (const fraction of [0.25, 0.5]) {
    const id = fraction === 0.25 ? "housing-25" : "housing-50",
      prefix = join(output, id);
    await mkdir(prefix);
    const record = { id, specification: { infillFraction: fraction } };
    report.cases.push(record);
    try {
      const intent = structuredClone(input);
      intent.recipe.infill_fraction = fraction;
      await save(`${id}/intent.json`, intent);
      await save(`${id}/external-intent.json`, intent);
      let ini = referenceValues(
        referenceProfile(cases.find((c) => c.walls === 2 && c.infillFraction === 0.25)),
        { ...settings, fill_density: `${fraction * 100}%` },
      );
      ini += "retract_speed = 25\nderetract_speed = 25\nretract_before_travel = 2\n";
      await save(`${id}/reference.ini`, ini);
      await run(
        reference,
        [
          "--load",
          join(prefix, "reference.ini"),
          "--export-gcode",
          "--threads",
          "1",
          "--center",
          "12,9",
          "--save",
          join(prefix, "reference-resolved.ini"),
          "--output",
          join(prefix, "reference.gcode"),
          join(output, "source.step"),
        ],
        `${id}/reference`,
      );
      record.referenceSettings = verifyReferenceSettings(
        ini,
        await fileText(join(prefix, "reference-resolved.ini")),
      );
      const referenceDecoded = decodeGcode(await fileText(join(prefix, "reference.gcode")), {
        maximumTerminalRetractionMm: 0.8,
      });
      record.reference = auditHousing(referenceDecoded);
      referenceAudits.push(record.reference);
      await save(`${id}/reference-audit.json`, record.reference);
      const operations =
        fraction === 0.25
          ? [
              "create",
              project,
              "--import",
              join(output, "external-source.step"),
              "--intent",
              join(prefix, "external-intent.json"),
              "--center-on-bed",
              "1",
            ]
          : ["edit", project, "--intent", join(prefix, "external-intent.json"), "--undo", "--redo"];
      const stdout = await run(
        cli,
        [
          "project",
          ...operations,
          "--compile",
          "--verify",
          "--manufacturing-out",
          join(prefix, "export"),
          "--save",
          "--engine",
          engine,
        ],
        `${id}/spiling`,
      );
      const result = JSON.parse(stdout);
      identity = nativeStatus(result, intent, identity);
      manifest.engineBuild = identity.engineBuild;
      record.nativeSummary = result.manufacturing.artifact.summary;
      if (fraction === 0.25) baselineHash = result.manufacturing.artifact.hash;
      else {
        const lookup = (name) => result.operations.find((o) => o.operation === name);
        assert.equal(
          lookup("intent").manufacturing.artifact,
          null,
          "intent change must invalidate publication",
        );
        assert.equal(
          lookup("undo").manufacturing.artifact.hash,
          baselineHash,
          "Undo must restore the baseline job",
        );
        assert.equal(
          lookup("redo").manufacturing.artifact,
          null,
          "Redo must invalidate the baseline publication",
        );
        assert.notEqual(
          result.manufacturing.artifact.hash,
          baselineHash,
          "the consuming revision must change output",
        );
        record.undoRedo = {
          baselineArtifactRestored: true,
          redoInvalidatesBaseline: true,
          revisedArtifactChanged: true,
        };
      }
      const job = intent.printer.workflow.job;
      const observation = {
        translation: identity.pose.translation_mm,
        prime: {
          from: job.prime_start_mm,
          to: job.prime_end_mm,
          filamentMm: job.prime_filament_length_mm,
        },
        doseLimits: intent.printer.deposition_limits,
      };
      const decoded = decodeGcode(await fileText(join(prefix, "export/program.gcode")));
      record.spiling = auditHousing(decoded, observation);
      record.faultControls = checkHousingFaults(decoded, observation);
      await save(`${id}/fault-controls.json`, record.faultControls);
      nativeAudits.push(record.spiling);
      await save(`${id}/spiling-audit.json`, record.spiling);
      await unlink(join(prefix, "external-intent.json"));
      if (fraction === 0.25) await unlink(join(output, "external-source.step"));
      const reopened = JSON.parse(
        await run(
          cli,
          [
            "project",
            "inspect",
            project,
            "--verify",
            "--manufacturing-out",
            join(prefix, "reopened-export"),
            "--replay-out",
            join(prefix, "reopened.splr"),
            "--engine",
            engine,
          ],
          `${id}/reopened`,
        ),
      );
      nativeStatus(reopened, intent, identity);
      assert.equal(reopened.project.read_only, true);
      assert.equal(reopened.manufacturing.artifact.hash, result.manufacturing.artifact.hash);
      for (const name of artifacts)
        assert(
          (await readFile(join(prefix, "export", name))).equals(
            await readFile(join(prefix, "reopened-export", name)),
          ),
          `fresh ${name} differs`,
        );
      record.recovery = {
        freshReadOnlyVerification: true,
        identicalFourArtifacts: true,
        externalSourceRemoved: true,
        externalIntentRemoved: true,
        artifact: result.manufacturing.artifact.hash,
      };
      record.comparison = comparison(
        record.spiling,
        record.reference,
        referenceDecoded.terminalRetractedFilamentMm,
      );
      await save(`${id}/comparison.json`, record.comparison);
      overlays[id] = {
        spiling: housingOverlay(decoded, record.spiling, observation),
        reference: housingOverlay(referenceDecoded, record.reference),
        source: housingSourceOverlay(),
      };
    } catch (error) {
      record.error = error.message;
      console.error(`${id}: ${error.message}`);
    }
  }
  if (nativeAudits.length === 2 && referenceAudits.length === 2) {
    report.nativeRevision = compareHousingRevision(...nativeAudits);
    report.referenceRevision = compareHousingRevision(...referenceAudits);
  }
  report.nativeSoftwarePassed =
    report.cases.length === 2 &&
    report.cases.every(
      (c) =>
        !c.error &&
        c.spiling?.correctnessPassed &&
        c.faultControls?.passed &&
        c.recovery?.identicalFourArtifacts,
    ) &&
    report.nativeRevision?.passed === true;
  report.referenceAuditsPassed =
    report.cases.every((c) => c.reference?.correctnessPassed) &&
    report.referenceRevision?.passed === true;
  report.correctnessPassed = report.nativeSoftwarePassed && report.referenceAuditsPassed;
  report.differencesExplained =
    report.correctnessPassed && report.cases.every((c) => c.comparison?.differencesExplained);
  await save("report.html", reportHtml(report, overlays, { widthMm: 24, depthMm: 18 }));
  await hashEvidence(output, manifest, output);
  for (const name of [
    "mechanical.mjs",
    "mechanical-run.mjs",
    "decode.mjs",
    "reference-settings.mjs",
    "report.mjs",
    "corpus.mjs",
  ])
    manifest.files[`tools/gcode-parity/${name}`] = sha256(
      await readFile(join(root, "tools/gcode-parity", name)),
    );
  return report;
}
