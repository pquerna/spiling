/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import type {
  ManufacturingIntent,
  ManufacturingArtifactRecord,
  SeamPolicy,
} from "@spiling/protocol";
export const scalarFields = [
  ["layer_height_mm", "Layer height (mm)"],
  ["native_shape_error_mm", "Native shape error budget (mm)"],
  ["bead_width_mm", "Preferred wall width (mm)"],
  ["perimeter_count", "Nominal contour depth"],
  ["top_skin_thickness_mm", "Top skin thickness (mm)"],
  ["bottom_skin_thickness_mm", "Bottom skin thickness (mm)"],
  ["infill_fraction", "Interior infill fraction (0–1]"],
  ["filament_diameter_mm", "Filament diameter (mm)"],
  ["print_speed_mm_s", "Deposition speed (mm/s)"],
  ["travel_speed_mm_s", "Travel speed (mm/s)"],
  ["flow_multiplier", "Flow multiplier"],
] as const;
const processScalars = [
  ["process.first_layer_speed_factor", "First-layer speed factor"],
  ["process.first_layer_flow_factor", "First-layer flow factor"],
  ["process.fan_first_layers", "First-layer fan PWM (0–255)"],
  ["process.fan_after_first_layers", "Later-layer fan PWM (0–255)"],
  ["process.bridge_speed_factor", "Bridge speed factor"],
  ["process.bridge_flow_factor", "Bridge flow factor"],
  ["process.bridge_fan_pwm", "Bridge fan PWM (0–255)"],
  ["process.max_bridge_span_mm", "Maximum bridge span (mm)"],
] as const;
const retractionScalars = [
  ["process.retraction.distance_mm", "Retraction distance (mm)"],
  ["process.retraction.speed_mm_s", "Retraction speed (mm/s)"],
  ["process.retraction.minimum_travel_mm", "Retraction minimum travel (mm)"],
  ["process.retraction.z_hop_mm", "Retraction Z hop (mm)"],
] as const;
const supportScalars = [
  ["supports.density", "Breakaway support density (0–1]"],
  ["supports.xy_clearance_mm", "Support XY clearance (mm)"],
  ["supports.z_gap_mm", "Support Z gap (mm)"],
  ["supports.interface_thickness_mm", "Support interface thickness (mm)"],
] as const;
export function policyScalarFields(intent: ManufacturingIntent) {
  return [
    ...processScalars,
    ...(intent.recipe.process.retraction.kind === "enabled" ? retractionScalars : []),
    ...(intent.recipe.supports.kind === "breakaway" ? supportScalars : []),
  ];
}
function policyValue(intent: ManufacturingIntent, path: string): unknown {
  return path.split(".").reduce((value, key) => (value as Record<string, unknown>)[key],
    intent.recipe as unknown);
}
export type ScalarField = (typeof scalarFields)[number][0];
export interface IntentDraft {
  intent: ManufacturingIntent;
  values: Record<ScalarField, string>;
  base: string;
  policyValues: Record<string, string>;
  seamPolicyKind: SeamPolicy["kind"];
  preferredDirection: [string, string];
  sparseInfillKind: "fine" | "combine_eligible";
  sparseMaxHeight: string;
  widthLimits: { minimum: string; maximum: string };
}
export const intentIdentity = (intent: ManufacturingIntent | null) => JSON.stringify(intent);
export function makeDraft(
  intent: ManufacturingIntent,
  base: ManufacturingIntent | null,
): IntentDraft {
  return {
    intent: structuredClone(intent),
    base: intentIdentity(base),
    widthLimits: {
      minimum: String(intent.printer.deposition_limits.min_bead_width_mm),
      maximum: String(intent.printer.deposition_limits.max_bead_width_mm),
    },
    seamPolicyKind: intent.recipe.seam_policy.kind,
    sparseInfillKind: intent.recipe.sparse_infill.kind,
    sparseMaxHeight: String(intent.recipe.sparse_infill.kind === "combine_eligible"
      ? intent.recipe.sparse_infill.max_height_mm : Math.min(0.3, intent.printer.deposition_limits.max_bead_height_mm)),
    preferredDirection: intent.recipe.seam_policy.kind === "preferred_build_direction"
      ? intent.recipe.seam_policy.xy.map(String) as [string, string]
      : ["", ""],
    policyValues: Object.fromEntries(policyScalarFields(intent).map(([key]) => [key, String(policyValue(intent, key))])),
    values: Object.fromEntries(
      scalarFields.map(([key]) => [key, String(intent.recipe[key])]),
    ) as Record<ScalarField, string>,
  };
}
function preferredBuildDirection(draft: IntentDraft): [number, number] {
  const [xText, yText] = draft.preferredDirection;
  const x = Number(xText), y = Number(yText);
  const scale = Math.max(Math.abs(x), Math.abs(y));
  if (!xText.trim() || !yText.trim() || !Number.isFinite(x) || !Number.isFinite(y) || scale === 0)
    throw new Error("Preferred build direction XY must contain two finite numbers and be nonzero.");
  const original = draft.intent.recipe.seam_policy;
  // Preserve admitted unit vectors exactly through reload and unrelated scalar edits.
  if (original.kind === "preferred_build_direction" && x === original.xy[0] && y === original.xy[1])
    return [x === 0 ? 0 : x, y === 0 ? 0 : y];
  // Scale before taking the norm so both huge and subnormal authored inputs remain usable.
  const scaledX = x / scale, scaledY = y / scale;
  const length = Math.hypot(scaledX, scaledY);
  const normalizedX = scaledX / length, normalizedY = scaledY / length;
  return [normalizedX === 0 ? 0 : normalizedX, normalizedY === 0 ? 0 : normalizedY];
}
export function draftIntent(draft: IntentDraft): ManufacturingIntent {
  const intent = structuredClone(draft.intent);
  const minimumWidth = Number(draft.widthLimits.minimum), maximumWidth = Number(draft.widthLimits.maximum);
  if (!draft.widthLimits.minimum.trim() || !draft.widthLimits.maximum.trim()
      || !Number.isFinite(minimumWidth) || !Number.isFinite(maximumWidth)
      || minimumWidth <= 0 || maximumWidth < minimumWidth)
    throw new Error("Software bead width limits must be finite, positive and ordered.");
  intent.printer.deposition_limits.min_bead_width_mm = minimumWidth;
  intent.printer.deposition_limits.max_bead_width_mm = maximumWidth;
  for (const [key, label] of scalarFields) {
    const text = draft.values[key],
      value = Number(text);
    if (
      !text.trim() ||
      !Number.isFinite(value) ||
      value <= 0 ||
      (key === "perimeter_count" && !Number.isInteger(value)) ||
      (key === "infill_fraction" && value > 1)
    ) {
      throw new Error(
        key === "infill_fraction"
          ? `${label} must be finite and greater than zero, at most one.`
          : `${label} must be a finite positive ${key === "perimeter_count" ? "integer" : "number"}.`,
      );
    }
    intent.recipe[key] = value;
  }
  for (const [path, label] of policyScalarFields(intent)) {
    const text = draft.policyValues[path] ?? "", value = Number(text);
    const fan = path.includes("fan_");
    const nonnegative = path.endsWith("z_hop_mm") || path.endsWith("z_gap_mm") || fan;
    const fraction = path.endsWith("speed_factor") || path.endsWith("density");
    if (!text.trim() || !Number.isFinite(value) || (nonnegative ? value < 0 : value <= 0) ||
        (fan && (!Number.isInteger(value) || value > 255)) || (fraction && value > 1))
      throw new Error(`${label} is outside its explicit finite policy range.`);
    const keys = path.split(".");
    const last = keys.pop()!;
    const target = keys.reduce((value, key) => value[key] as Record<string, unknown>,
      intent.recipe as unknown as Record<string, unknown>);
    target[last] = value;
  }
  const maxHeight = Number(draft.sparseMaxHeight);
  if (draft.sparseInfillKind === "combine_eligible" &&
      (!draft.sparseMaxHeight.trim() || !Number.isFinite(maxHeight) || maxHeight <= 0 ||
       maxHeight > intent.printer.deposition_limits.max_bead_height_mm))
    throw new Error("Combined sparse maximum height must be positive and within the declared software deposition envelope.");
  intent.recipe.sparse_infill = draft.sparseInfillKind === "fine"
    ? { kind: "fine" }
    : { kind: "combine_eligible", max_height_mm: maxHeight };
  intent.recipe.seam_policy = draft.seamPolicyKind === "nearest"
    ? { kind: "nearest" }
    : { kind: "preferred_build_direction", xy: preferredBuildDirection(draft) };
  return intent;
}
export function draftChanged(draft: IntentDraft | null): boolean {
  if (!draft) return false;
  try {
    return intentIdentity(draftIntent(draft)) !== draft.base;
  } catch {
    return true;
  }
}
export function sameArtifact(
  a: ManufacturingArtifactRecord | null | undefined,
  b: ManufacturingArtifactRecord | null | undefined,
): boolean {
  return !!a && !!b && a.hash === b.hash && a.input_hash === b.input_hash;
}

export interface InspectionBinding {
  session: string;
  authority: string;
  projectId: string;
  invalidated: boolean;
  record: ManufacturingArtifactRecord;
}
export interface CurrentBinding {
  session: string | null | undefined;
  authority: string;
  projectId: string | undefined;
  statusProjectId: string | undefined;
  currentRevision: number | undefined;
  statusRevision: number | undefined;
  artifact: ManufacturingArtifactRecord | null | undefined;
}
export function inspectionIsFresh(
  inspection: InspectionBinding | null,
  current: CurrentBinding,
): boolean {
  return (
    !!inspection &&
    !inspection.invalidated &&
    inspection.session === current.session &&
    inspection.authority === current.authority &&
    inspection.projectId === current.projectId &&
    current.statusProjectId === current.projectId &&
    current.statusRevision === current.currentRevision &&
    sameArtifact(current.artifact, inspection.record)
  );
}
