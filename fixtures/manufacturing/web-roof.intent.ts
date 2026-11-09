// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

import type { ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import base from "./complete-sparse.intent.ts";

export default {
  ...base,
  recipe: {
    ...base.recipe,
    id: "software-web-roof",
    revision: "1",
    layer_height_mm: 0.2,
    bead_width_mm: 0.45,
    infill_fraction: 0.08,
    sparse_infill: { kind: "web_roof", roof_spacing_mm: null, architecture: null },
    supports: { kind: "disabled" },
    process: { ...base.recipe.process, max_bridge_span_mm: 2 },
  },
} satisfies ManufacturingIntent;
