/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */
import type { FixedDualTool, ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import mixed from "./mixed-width-interior.intent.ts";

const job = mixed.printer.workflow.job;
const bulk = job.tools[1];
const limits = { ...bulk.deposition_limits, max_bead_height_mm: 0.5 };
const tool: FixedDualTool = {
  ...bulk,
  deposition_limits: limits,
  material_profile: {
    ...bulk.material_profile!,
    selection: {
      ...bulk.material_profile!.selection,
      profile: {
        ...bulk.material_profile!.selection.profile,
        id: "software-dual-bulk-combined-06",
        deposition_limits: limits,
        max_volumetric_flow_mm3_s: 9,
      },
    },
  },
};
export default {
  ...mixed,
  printer: {
    ...mixed.printer,
    deposition_limits: { ...mixed.printer.deposition_limits, max_bead_height_mm: 0.5 },
    workflow: { kind: "fixed_dual_marlin", job: { ...job, timing_model: { id: "software-rest-to-rest", revision: "1", linear_acceleration_mm_s2: 1000, filament_acceleration_mm_s2: 500, tool_activation_seconds: [0.5, 0.5], command_overhead_seconds: 0.0002 }, tools: [job.tools[0], tool] } },
  },
  recipe: {
    ...mixed.recipe,
    id: "mixed-width-combined",
    infill_fraction: 0.25,
    sparse_infill: { kind: "combine_eligible", max_height_mm: 0.45 },
  },
} satisfies ManufacturingIntent;
