/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */

import type { FixedDualTool, ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import profiled from "./fixed-dual-profiles.intent.ts";

const job = profiled.printer.workflow.job;
const bulk = job.tools[1];
const limits = { ...bulk.deposition_limits, min_bead_width_mm: 0.45 };
const tool: FixedDualTool = {
  ...bulk,
  nozzle_diameter_mm: 0.6,
  waste_bead_width_mm: 0.6,
  deposition_limits: limits,
  material_profile: {
    ...bulk.material_profile!,
    selection: {
      ...bulk.material_profile!.selection,
      profile: {
        ...bulk.material_profile!.selection.profile,
        id: "software-dual-bulk-06",
        nozzle_diameter_mm: 0.6,
        deposition_limits: limits,
        print_speed_mm_s: 38,
      },
    },
  },
};
export default {
  ...profiled,
  printer: {
    ...profiled.printer,
    workflow: {
      kind: "fixed_dual_marlin",
      job: {
        ...job,
        timing_model: { id: "software-rest-to-rest", revision: "1", linear_acceleration_mm_s2: 1000, filament_acceleration_mm_s2: 500, tool_activation_seconds: [0.5, 0.5], command_overhead_seconds: 0.0002 },
        interior_goal: { minimum_modeled_saving_seconds: 1, max_extra_model_fraction: 0.05, max_extra_waste_mm3: 0 },
        tools: [job.tools[0], tool],
        assignments: [
          {
            ...job.assignments[0],
            work_envelope_mm: { min: [19, 19, -1], max: [41, 41, 9] },
            interior: { tool_id: tool.id, bead_width_mm: 0.6, policy_override: null },
          },
        ],
      },
    },
  },
  recipe: { ...profiled.recipe, id: "mixed-width-interior" },
} satisfies ManufacturingIntent;
