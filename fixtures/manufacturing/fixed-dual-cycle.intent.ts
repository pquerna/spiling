// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

import type { ManufacturingIntent, OccurrenceMaterialAssignment } from "../../packages/protocol/src/generated.ts";
import fixed from "./fixed-dual.intent.ts";

// Same software-only native occurrences and loaded materials; opposite physical
// phases resolve independently at common world Z. No measured color/bond claim.
const assignment = (index: 0 | 1): OccurrenceMaterialAssignment => ({
  ...fixed.printer.workflow.job.assignments[index],
  material: {
    kind: "layer_cycle",
    recipe: {
      slots: [{ material: 0, height_mm: 0.2 }, { material: 1, height_mm: 0.2 }],
      phase_slot: index,
      origin_z_mm: 0,
    },
    material_ids: ["software-pla-a", "software-pla-b"],
  },
});

const intent = {
  ...fixed,
  printer: {
    ...fixed.printer,
    workflow: {
      ...fixed.printer.workflow,
      job: {
        ...fixed.printer.workflow.job,
        assignments: [assignment(0), assignment(1)],
      },
    },
  },
  recipe: { ...fixed.recipe, id: "fixed-dual-whole-occurrence-cycles" },
} satisfies ManufacturingIntent;

export default intent;
