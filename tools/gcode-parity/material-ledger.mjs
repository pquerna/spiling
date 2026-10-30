/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// A causal cross-case ledger for this frozen cube only. Existing audit failures
// remain failures; explaining a difference does not qualify deposited material.
const EPS = 1e-6;
const sum = (xs, f) => xs.reduce((n, x) => n + f(x), 0);
const close = (a, b, tolerance = 0.001) =>
  Number.isFinite(a) && Number.isFinite(b) && Math.abs(a - b) <= tolerance;
const at = (audit, z) =>
  audit?.constructionObservations?.layers.find((l) => Math.abs(l.upperMm - z) < EPS);
const nonWall = (l) => l.longFill.volumeMm3 + l.linksAndShortFill.volumeMm3;
const total = (l) => l.walls.volumeMm3 + nonWall(l);
function externalFit(audit) {
  return [0.2, 10].map((z) => {
    const l = at(audit, z),
      rows = l?.longFill;
    const fittedVolume = rows?.layout ? rows.lengthMm * l.heightMm * rows.layout.meanPitchMm : null;
    return {
      upperMm: z,
      actualVolumeMm3: rows?.volumeMm3,
      fittedVolumeMm3: fittedVolume,
      passed: Boolean(rows?.layout && close(rows.volumeMm3, fittedVolume, 0.005)),
    };
  });
}
function contributions(audit) {
  const result = Object.fromEntries(
    [
      "walls",
      "externalBottom",
      "internalBottom",
      "ordinaryInterior",
      "preRoofInterval",
      "firstRoof",
      "internalRoof",
      "terminalBand",
    ].map((k) => [k, 0]),
  );
  for (const l of audit.constructionObservations.layers) {
    result.walls += l.walls.volumeMm3;
    const key =
      l.upperMm <= 0.2 + EPS
        ? "externalBottom"
        : l.upperMm <= 0.8 + EPS
          ? "internalBottom"
          : l.upperMm <= 9 + EPS
            ? "ordinaryInterior"
            : l.upperMm <= 9.2 + EPS
              ? "preRoofInterval"
              : l.upperMm <= 9.4 + EPS
                ? "firstRoof"
                : l.upperMm <= 9.8 + EPS
                  ? "internalRoof"
                  : "terminalBand";
    result[key] += nonWall(l);
  }
  return result;
}
function reconciles(audit) {
  return (
    close(
      sum(audit.layers, (l) => l.volumeMm3),
      audit.modelVolumeMm3,
      1e-6,
    ) &&
    close(
      sum(audit.bands, (b) => b.volumeMm3),
      audit.modelVolumeMm3,
      1e-6,
    ) &&
    audit.bands.every((b) =>
      ["volumeMm3", "coreVolumeMm3", "wallZoneVolumeMm3"].every((key) =>
        close(
          b[key],
          sum(
            audit.layers,
            (l) =>
              (l[key] *
                Math.max(0, Math.min(l.upperMm, b.upperMm) - Math.max(l.lowerMm, b.lowerMm))) /
              (l.upperMm - l.lowerMm),
          ),
          1e-6,
        ),
      ),
    ) &&
    audit.constructionObservations.layers.every((l) => {
      const original = audit.layers.find((x) => Math.abs(x.upperMm - l.upperMm) < EPS);
      return (
        original &&
        close(total(l), original.volumeMm3, 1e-6) &&
        Math.abs(l.reconciliationResidualMm3) < 1e-6
      );
    })
  );
}
function referenceFlagsAdmitted(record) {
  return record.reference.violations.every(
    (v) =>
      record.specification.infillFraction === 0 &&
      ((v.code === "unexpected_core" && v.message.startsWith("Z9.2:")) ||
        (v.code === "mandatory_coverage" && v.message.startsWith("Z9.4:"))),
  );
}
export function explainCubeMaterialFamily(records, walls) {
  const family = [0, 0.25, 1].map((f) =>
    records.find((c) => c.specification?.walls === walls && c.specification?.infillFraction === f),
  );
  if (family.some((c) => !c?.spiling || !c.reference || c.error))
    return { explained: false, reason: "complete frozen cube family required" };
  const [hollow, sparse, dense] = family;
  const checks = {};
  checks.auditAndAccounting = family.every(
    (c) =>
      c.spiling.correctnessPassed &&
      referenceFlagsAdmitted(c) &&
      reconciles(c.spiling) &&
      reconciles(c.reference),
  );
  const explained = (c, id) =>
    ["match", "explained"].includes(c.comparison?.differences.find((d) => d.id === id)?.status);
  checks.decidingPolicies =
    family.every((c) =>
      ["layer_schedule", "wall_spacing", "bridge_process"].every((id) => explained(c, id)),
    ) &&
    [sparse, dense].every((c) => explained(c, "interior_hatch_fitting")) &&
    explained(sparse, "sparse_bridge_policy") &&
    [hollow, sparse].every(
      (c) =>
        explained(c, "reference_roof_preparation") &&
        explained(c, "reference_bridge_cross_section"),
    ) &&
    dense.solidProbeEvidence?.explained === true;
  if (!checks.auditAndAccounting || !checks.decidingPolicies)
    return {
      explained: false,
      walls,
      checks,
      reason:
        "independent audits, physical-band accounting and all controlled policy witnesses are required",
    };
  const nativeLayers = dense.spiling.constructionObservations.layers;
  checks.nativeChangesConfinedToInterior = family.every((c) =>
    nativeLayers.every((l) => {
      const observed = at(c.spiling, l.upperMm);
      return (
        observed &&
        close(observed.walls.volumeMm3, l.walls.volumeMm3, 1e-5) &&
        ((l.lowerMm >= 0.8 - EPS && l.upperMm <= 9.2 + EPS) ||
          close(nonWall(observed), nonWall(l), 1e-5))
      );
    }),
  );
  checks.nativeNominalExtrusion = family.every((c) =>
    c.spiling.constructionObservations.layers.every(
      (l) => Math.abs(l.allStadiumResidualMm3) < 1e-5,
    ),
  );
  checks.referenceWallsInvariant = family.every((c) =>
    dense.reference.constructionObservations.layers.every((l) =>
      close(at(c.reference, l.upperMm)?.walls.volumeMm3, l.walls.volumeMm3),
    ),
  );
  const ordinarySkinHeights = [0.2, 0.4, 0.6, 0.8, 9.6, 9.8, 10];
  checks.referenceOrdinarySkinsInvariant = family.every((c) =>
    ordinarySkinHeights.every((z) =>
      close(nonWall(at(c.reference, z)), nonWall(at(dense.reference, z))),
    ),
  );
  const template = at(dense.reference, 0.4);
  checks.referenceInternalSolidTemplate = family.every((c) =>
    [0.4, 0.6, 0.8, 9.6, 9.8].every((z) => {
      const l = at(c.reference, z);
      return (
        close(l.longFill.volumeMm3, template.longFill.volumeMm3) &&
        close(l.linksAndShortFill.volumeMm3, template.linksAndShortFill.volumeMm3) &&
        l.longFill.widthRangeMm[0] < 0.4 &&
        l.longFill.widthRangeMm[1] > 0.449
      );
    }),
  );
  checks.hollowInteriorEmpty =
    hollow.reference.constructionObservations.layers
      .filter((l) => l.lowerMm >= 0.8 - EPS && l.upperMm <= 9 + EPS)
      .every((l) => nonWall(l) === 0) &&
    hollow.spiling.constructionObservations.layers
      .filter((l) => l.lowerMm >= 0.8 - EPS && l.upperMm <= 9.2 + EPS)
      .every((l) => nonWall(l) === 0);
  checks.firstInteriorMatchesWitness = [sparse, dense].every((c) =>
    [c.spiling, c.reference].every((a) => close(nonWall(at(a, 1)), nonWall(at(a, 1.2)))),
  );
  const externalSkins = family.map((c) => ({ id: c.id, reference: externalFit(c.reference) }));
  checks.referenceExternalFittedFlow = externalSkins.every((c) =>
    c.reference.every((p) => p.passed),
  );
  const cases = family.map((c) => {
    const native = contributions(c.spiling),
      reference = contributions(c.reference);
    const components = Object.keys(native).map((component) => ({
      component,
      spilingVolumeMm3: native[component],
      referenceVolumeMm3: reference[component],
      deltaMm3: native[component] - reference[component],
    }));
    return {
      id: c.id,
      components,
      totalDeltaMm3: c.spiling.modelVolumeMm3 - c.reference.modelVolumeMm3,
      residualMm3:
        c.spiling.modelVolumeMm3 - c.reference.modelVolumeMm3 - sum(components, (x) => x.deltaMm3),
    };
  });
  checks.completeDelta = cases.every((c) => Math.abs(c.residualMm3) < 1e-6);
  return {
    explained: Object.values(checks).every(Boolean),
    walls,
    checks,
    externalSkins,
    cases,
    scope:
      "Specific policy explanation for the frozen10mm cube family, with cross-case invariance, controlled roof/flow/saturated-skin probes and independently accounted actual E. Every baseline audit flag remains binding. This is not volume equality, physical material/coverage qualification or a general slicer equivalence theorem.",
  };
}
