/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 * Licensed under the Open Software License version 3.0
 */

import type { ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import fixed from "./fixed-dual.intent.ts";

const job = fixed.printer.workflow.job;
const intent = {
  ...fixed,
  printer: {
    ...fixed.printer,
    workflow: {
      kind: "fixed_dual_marlin",
      job: {
        ...job,
        tools: [job.tools[0], { ...job.tools[1], material: job.tools[0].material }],
        assignments: job.assignments.map((assignment, index) => ({
          ...assignment,
          tool_id: job.tools[index].id,
          material: { kind: "uniform", material_id: job.tools[0].material.id },
        })),
      },
    },
  },
  recipe: { ...fixed.recipe, id: "fixed-dual-shared-material", revision: "1" },
} satisfies ManufacturingIntent;

export default intent;
