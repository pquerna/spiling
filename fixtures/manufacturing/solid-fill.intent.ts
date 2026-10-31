// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

import type { ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import printer from "./software-cartesian.printer.ts";

const intent = {
  material_profile: null,
  printer,
  recipe: {
    id: "software-solid-fill",
    revision: "6",
    material_id: "nominal-software-filament",
    material_revision: "1",
    filament_diameter_mm: 1.75,
    layer_height_mm: 0.2,
    native_shape_error_mm: 0.1,
    bead_width_mm: 0.45,
    perimeter_count: 2,
    infill_fraction: 1,
    sparse_infill: { kind: "fine" },
    top_skin_thickness_mm: 0.8,
    bottom_skin_thickness_mm: 0.8,
    print_speed_mm_s: 35,
    travel_speed_mm_s: 100,
    flow_multiplier: 1,
    process: {
      first_layer_speed_factor: 0.5,
      first_layer_flow_factor: 1,
      fan_first_layers: 0,
      fan_after_first_layers: 128,
      bridge_speed_factor: 0.6,
      bridge_flow_factor: 1,
      bridge_fan_pwm: 255,
      max_bridge_span_mm: 10,
      retraction: {
        kind: "enabled",
        distance_mm: 0.8,
        speed_mm_s: 25,
        minimum_travel_mm: 2,
        z_hop_mm: 0.2,
      },
    },
    supports: { kind: "disabled" },
    seam_policy: { kind: "nearest" },
    rectilinear_axis: "alternating_xy",
    top_skin_axis: "alternating_xy",
  },
} satisfies ManufacturingIntent;

export default intent;
