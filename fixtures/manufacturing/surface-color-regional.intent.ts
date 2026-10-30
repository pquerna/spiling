// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

import type { ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import full from "./surface-color-box.intent.ts";

// Two source-face windows share persistent white backing/core and have opposite
// physical phases. The 0.03 mm origin creates real partial native intervals.
const intent = {
  ...full,
  printer: {
    ...full.printer,
    workflow: {
      ...full.printer.workflow,
      job: {
        ...full.printer.workflow.job,
        surface_constructions: [0, 1].map((index) => ({
          ...full.printer.workflow.job.surface_constructions[0],
          faces: full.printer.workflow.job.surface_constructions[0].faces.map((face) => ({
            ...face, mask: { min_u_mm: index === 0 ? 2 : 12, max_u_mm: index === 0 ? 8 : 18 },
          })),
          shell: {
            kind: "layer_cycle" as const,
            material_ids: ["software-pla-a", "software-pla-b"] as [string, string],
            recipe: { slots: [{ material: 0, height_mm: 0.2 }, { material: 1, height_mm: 0.2 }], phase_slot: index, origin_z_mm: 0.03 },
          },
        })),
      },
    },
  },
  recipe: { ...full.recipe, id: "surface-color-regional" },
} satisfies ManufacturingIntent;

export default intent;
