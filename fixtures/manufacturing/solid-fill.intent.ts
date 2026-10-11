// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

import type { ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import printer from "./software-cartesian.printer.ts";

const intent = {
  printer,
  recipe: {
    id: "software-solid-fill",
    revision: "1",
    material_id: "nominal-software-filament",
    material_revision: "1",
    filament_diameter_mm: 1.75,
    layer_height_mm: 0.2,
    bead_width_mm: 0.45,
    perimeter_count: 2,
    infill_fraction: 1,
    print_speed_mm_s: 35,
    travel_speed_mm_s: 100,
    flow_multiplier: 1,
  },
} satisfies ManufacturingIntent;

export default intent;
