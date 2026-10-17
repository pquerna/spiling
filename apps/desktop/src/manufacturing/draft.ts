/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import type { ManufacturingIntent, ManufacturingArtifactRecord } from "@spiling/protocol";
export const scalarFields = [
  ["layer_height_mm", "Layer height (mm)"],
  ["native_shape_error_mm", "Native shape error budget (mm)"],
  ["bead_width_mm", "Bead width (mm)"],
  ["perimeter_count", "Perimeters"],
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
}
export const intentIdentity = (intent: ManufacturingIntent | null) => JSON.stringify(intent);
export function makeDraft(
  intent: ManufacturingIntent,
  base: ManufacturingIntent | null,
): IntentDraft {
  return {
    intent: structuredClone(intent),
    base: intentIdentity(base),
    policyValues: Object.fromEntries(policyScalarFields(intent).map(([key]) => [key, String(policyValue(intent, key))])),
    values: Object.fromEntries(
      scalarFields.map(([key]) => [key, String(intent.recipe[key])]),
    ) as Record<ScalarField, string>,
  };
}
export function draftIntent(draft: IntentDraft): ManufacturingIntent {
  const intent = structuredClone(draft.intent);
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
