// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
import baseline from "./complete-sparse.intent.ts";
import type { ManufacturingIntent } from "../../packages/protocol/src/generated.ts";

// The real housing operator changes only the explicitly authored hatch axis.
export default {
  ...baseline,
  recipe: { ...baseline.recipe, rectilinear_axis: "x", top_skin_axis: "x" },
} satisfies ManufacturingIntent;
