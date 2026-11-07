/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */

import type {
  FixedDualTool,
  ManufacturingIntent,
  MaterialProfileSelection,
} from "../../packages/protocol/src/generated.ts";
import shared from "./fixed-dual-shared-material.intent.ts";

const job = shared.printer.workflow.job;
const selection = (tool: FixedDualTool, index: number): MaterialProfileSelection => ({
  profile: {
    id: `software-dual-process-${index}`,
    revision: "1",
    material: {
      id: tool.material.id,
      revision: tool.material.revision,
      polymer_family: tool.material.polymer_family,
      manufacturer: "Spiling synthetic test declarations",
      product: "Shared PLA reference",
      filler: null,
      filament_diameter_mm: tool.filament_diameter_mm,
      requires_abrasion_resistant_nozzle: false,
    },
    printer_id: shared.printer.id,
    printer_revision: shared.printer.revision,
    nozzle_diameter_mm: tool.nozzle_diameter_mm,
    hotend_id: tool.id,
    nozzle_material: "brass",
    cooling_configuration_id: "shared-reference-fan",
    deposition_limits: tool.deposition_limits,
    nozzle_temperature_c: index === 0 ? 200 : 205,
    bed_temperature_c: job.bed_temperature_c,
    print_speed_mm_s: index === 0 ? 30 : 18,
    flow_multiplier: 1,
    max_volumetric_flow_mm3_s: 6,
    process: {
      ...shared.recipe.process,
      first_layer_speed_factor: index === 0 ? 0.5 : 0.75,
      fan_after_first_layers: index === 0 ? 128 : 160,
    },
    source: {
      kind: "synthetic_test",
      source_id: "material-profile-test-source.txt",
      source_revision: "1",
      source_hash: "95c80eb53239470982522fdb2bcf56168a6fa279790d057fe2d62fdf1978d865",
    },
  },
  setup: {
    hotend_id: tool.id,
    nozzle_material: "brass",
    abrasion_resistant_nozzle: false,
    cooling_configuration_id: "shared-reference-fan",
  },
  overrides: {
    nozzle_temperature_c: null,
    bed_temperature_c: null,
    print_speed_mm_s: null,
    flow_multiplier: null,
    process: null,
  },
});
const tools = job.tools.map((tool, index) => ({
  ...tool,
  nozzle_temperature_c: index === 0 ? 200 : 205,
  material_profile: { resolver_version: 1, selection: selection(tool, index) },
})) as [FixedDualTool, FixedDualTool];
export default {
  ...shared,
  printer: { ...shared.printer, workflow: { kind: "fixed_dual_marlin", job: { ...job, tools } } },
  recipe: { ...shared.recipe, id: "fixed-dual-profiles" },
} satisfies ManufacturingIntent;
