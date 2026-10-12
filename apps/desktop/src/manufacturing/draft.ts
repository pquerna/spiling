/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import type { ManufacturingIntent, ManufacturingArtifactRecord } from "@spiling/protocol";
export const scalarFields = [
  ["layer_height_mm", "Layer height (mm)"],
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
export type ScalarField = (typeof scalarFields)[number][0];
export interface IntentDraft {
  intent: ManufacturingIntent;
  values: Record<ScalarField, string>;
  base: string;
}
export const intentIdentity = (intent: ManufacturingIntent | null) => JSON.stringify(intent);
export function makeDraft(
  intent: ManufacturingIntent,
  base: ManufacturingIntent | null,
): IntentDraft {
  return {
    intent: structuredClone(intent),
    base: intentIdentity(base),
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
