// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

import type { ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import full from "./surface-color-box.intent.ts";

// A finite 4..16 mm source-face horizontal window; native height is unchanged.
const intent = {
  ...full,
  printer: {
    ...full.printer,
    workflow: {
      ...full.printer.workflow,
      job: {
        ...full.printer.workflow.job,
        surface_constructions: full.printer.workflow.job.surface_constructions.map((construction) => ({
          ...construction,
          faces: construction.faces.map((face) => ({ ...face, mask: { min_u_mm: 4, max_u_mm: 16 } })),
        })),
      },
    },
  },
  recipe: { ...full.recipe, id: "surface-color-window" },
} satisfies ManufacturingIntent;

export default intent;
