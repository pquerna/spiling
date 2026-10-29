// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

import type { FixedDualTool, ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import single from "./solid-fill.intent.ts";

// Software-only setup: two unchanged 20×10×8 mm box occurrences translated to
// [20,20,0] and [120,20,0]. No actual printer, measured color or adhesion claim.
const tool = (index: 0 | 1): FixedDualTool => ({
  id: `fixed-hotend-${index}`,
  feeder_id: `direct-feeder-${index}`,
  material: {
    id: index === 0 ? "software-pla-a" : "software-pla-b",
    revision: "1",
    declaration_hash: index === 0
      ? "883b1eaa92941d652997e2092ad5f5cf9aabeba3991c8654782a281dbfbf0111"
      : "fabb4a0c776b0f4d2c0448b562bd106abe5cf10aee2a8e31fca120a1d2cc2f89",
    polymer_family: "PLA",
  },
  offset_xy_mm: index === 0 ? [0, 0] : [25, 0],
  nozzle_diameter_mm: 0.4,
  filament_diameter_mm: 1.75,
  deposition_limits: single.printer.deposition_limits,
  max_feed_mm_s: 150,
  max_volumetric_flow_mm3_s: 12,
  nozzle_temperature_c: 200,
  standby_temperature_c: 150,
  minimum_extrusion_temperature_c: 170,
  maximum_nozzle_temperature_c: 250,
  retract_filament_mm: 0.6,
  retract_speed_mm_s: 25,
  minimum_reprime_volume_mm3: 0.1,
  waste_line_xy_mm: index === 0 ? [[60, 160], [80, 160]] : [[160, 160], [180, 160]],
});

const intent = {
  printer: {
    ...single.printer,
    id: "software-fixed-dual-marlin-reference",
    revision: "1",
    name: "Fixed-dual Marlin reference — software only, physically unqualified",
    capabilities: {
      ...single.printer.capabilities,
      extruders: 2,
      output_dialect: "marlin2_fixed_dual_gcode_v1",
    },
    workflow: {
      kind: "fixed_dual_marlin",
      job: {
        machine_id: "software-fixed-dual-marlin-reference",
        configuration: "marlin2125_native_fixed_hotends",
        firmware_revision: "e5a167aaa0a671ea6d9b9d98dc0518c88d5caed7",
        configuration_hash: "7baa388a82815c8388f786b0c03e7d73fc8df1a1a9e859989e42174ba188b654",
        calibration_record_hash: "cfa5864bc575b22bd8131af8bc480d983f67e2cda796a90f5f7f5b7890a776b4",
        homing_contract: "marlin_native_cartesian",
        home_position_mm: [0, 0, 0],
        bed_temperature_c: 60,
        maximum_bed_temperature_c: 100,
        tools: [tool(0), tool(1)],
        assignments: [
          { occurrence_id: 1, material_id: "software-pla-a", work_envelope_mm: { min: [19, 19, -1], max: [41, 31, 9] } },
          { occurrence_id: 2, material_id: "software-pla-b", work_envelope_mm: { min: [119, 19, -1], max: [141, 31, 9] } },
        ],
        transition_lift_mm: 0.5,
        carriage_travel_bounds_mm: { min: [-30, -5, 0], max: [220, 220, 220] },
        carriage_body_bounds_mm: { min: [-2, -2, 8], max: [27, 2, 25] },
        inactive_nozzle_radius_mm: 0.3,
        park_position_mm: [0, 210, 20],
      },
    },
  },
  recipe: {
    ...single.recipe,
    id: "fixed-dual-whole-occurrences",
    revision: "1",
    material_id: "software-pla-a",
    process: { ...single.recipe.process, retraction: { kind: "disabled" } },
  },
} satisfies ManufacturingIntent;

export default intent;
