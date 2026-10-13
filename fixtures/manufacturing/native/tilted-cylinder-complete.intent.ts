// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

import type { ManufacturingIntent } from "../../../packages/protocol/src/generated.ts";
import completeIntent from "../complete-sparse.intent.ts";

const intent = {
  printer: completeIntent.printer,
  recipe: {
    ...completeIntent.recipe,
    id: "native-deadline-tilt",
    revision: "3",
    layer_height_mm: 0.4,
    bead_width_mm: 0.6,
    perimeter_count: 1,
    infill_fraction: 1,
    print_speed_mm_s: 20,
    travel_speed_mm_s: 100,
    flow_multiplier: 1,
    process: {
      first_layer_speed_factor: 1,
      first_layer_flow_factor: 1,
      fan_first_layers: 1,
      fan_after_first_layers: 0,
      bridge_speed_factor: 0.5,
      bridge_flow_factor: 1,
      bridge_fan_pwm: 255,
      max_bridge_span_mm: 6,
      retraction: { kind: "disabled" },
    },
    supports: { kind: "disabled" },
  },
} satisfies ManufacturingIntent;

export default intent;
