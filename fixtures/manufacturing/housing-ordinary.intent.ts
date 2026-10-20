// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

import type { ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import baseline from "./complete-sparse.intent.ts";

// An ordinary fine job, independent of the fixed-axis H1 separator control.
export default {
  ...baseline,
  recipe: {
    ...baseline.recipe,
    id: "software-ordinary-housing",
    layer_height_mm: 0.1,
    rectilinear_axis: "alternating_xy",
    top_skin_axis: "alternating_xy",
    sparse_infill: { kind: "fine" },
  },
} satisfies ManufacturingIntent;
