// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

import type { ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import baseline from "./housing-two-zone.intent.ts";

// Keep the fixed-X separator hatch and source-zone phase of the housing baseline.
export default {
  ...baseline,
  recipe: {
    ...baseline.recipe,
    id: "software-h1-housing",
    revision: "1",
    layer_height_mm: 0.1,
    top_skin_axis: "alternating_xy",
    sparse_infill: { kind: "fine" },
  },
} satisfies ManufacturingIntent;
