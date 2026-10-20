// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

import type { ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import baseline from "./housing-two-zone.intent.ts";

// Explicit fixed-X XY-separator H1 control, not the ordinary housing profile.
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
