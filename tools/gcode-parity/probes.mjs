/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
// Evaluate actual independently decoded diagnostic programs. No role hints.
const close = (a, b) => Number.isFinite(a) && Number.isFinite(b) && Math.abs(a - b) < 0.001;
const layer = (audit, z) => audit?.layers.find((l) => Math.abs(l.upperMm - z) < 1e-6);
export function evaluateReferenceProbes(baseline, probes) {
  const shifted = probes?.find((p) => p.id === "roof-shift")?.audit;
  const half = probes?.find((p) => p.id === "half-bridge-flow")?.audit;
  const disabled = probes?.find((p) => p.id === "shell-disabled")?.audit;
  const baseRing = layer(baseline, 9.2),
    shiftedRing = layer(shifted, 9.4);
  const baseBridge = layer(baseline, 9.4),
    shiftedBridge = layer(shifted, 9.6);
  const baseEarlier = layer(baseline, 9),
    shiftedEarlier = layer(shifted, 9.2);
  const baseModel = layer(baseline.constructionObservations, 9.4)?.bridgeRows;
  const halfModel = layer(half?.constructionObservations, 9.4)?.bridgeRows;
  const roofPreparation = Boolean(
    baseRing &&
    shiftedRing &&
    baseBridge &&
    shiftedBridge &&
    close(baseEarlier?.volumeMm3, shiftedEarlier?.volumeMm3) &&
    close(baseRing.volumeMm3, shiftedRing.volumeMm3) &&
    close(baseBridge.bridgeProcessVolumeMm3, shiftedBridge.bridgeProcessVolumeMm3) &&
    baseRing.coreVolumeMm3 > baseEarlier.coreVolumeMm3 + 1 &&
    close(baseRing.volumeMm3, layer(disabled, 9.2)?.volumeMm3) &&
    close(baseBridge.volumeMm3, layer(disabled, 9.4)?.volumeMm3),
  );
  const circularBridge = Boolean(
    baseModel?.circularAgreement &&
    halfModel?.circularAgreement &&
    !close(baseModel.areaRangeMm2[0], halfModel.areaRangeMm2[0]) &&
    Math.abs(baseModel.stadiumResidualMm3) > 1 &&
    halfModel.fittedCircularDiameterMm < baseModel.fittedCircularDiameterMm &&
    baseline.layers
      .filter((l) => l.upperMm < 9.4)
      .every((l) => close(l.volumeMm3, layer(half, l.upperMm)?.volumeMm3)),
  );
  return {
    roofPreparation,
    circularBridge,
    observations: {
      originalPreparation: baseRing && {
        z: 9.2,
        coreVolumeMm3: baseRing.coreVolumeMm3,
        volumeMm3: baseRing.volumeMm3,
      },
      shiftedPreparation: shiftedRing && {
        z: 9.4,
        coreVolumeMm3: shiftedRing.coreVolumeMm3,
        volumeMm3: shiftedRing.volumeMm3,
      },
      originalRoofBridgeVolumeMm3: baseBridge?.bridgeProcessVolumeMm3,
      shiftedRoofBridgeVolumeMm3: shiftedBridge?.bridgeProcessVolumeMm3,
      originalBridgeModel: baseModel,
      halfFlowBridgeModel: halfModel,
    },
    limits:
      "Controlled reference perturbations and nominal geometry/E fits explain a construction policy; baseline audit violations are retained. This is not physical validation or proof that either slicer is superior.",
  };
}
