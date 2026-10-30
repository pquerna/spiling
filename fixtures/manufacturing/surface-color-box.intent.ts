// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

import type { ManufacturingIntent, SurfaceColorConstruction } from "../../packages/protocol/src/generated.ts";
import fixed from "./fixed-dual.intent.ts";

// Frozen box-mm.step at [20,20,0], with its returned native -Y face.
// Loaded B is explicitly requested as white; no measured optics are implied.
const construction: SurfaceColorConstruction = {
  occurrence_id: 1,
  definition_id: "ff6a13679b0d6c958f4bbd9716249290a72e4cec9c5df3a61cb4c4f3f93e2495",
  source_hash: "1b23042e12183748e12bc4e0195ae546c80296ce4170709eb3f836dce2f4ac98",
  faces: [{ face_id: "step:25", skin_depth_mm: 1.2, backing_depth_mm: 1.2 }],
  shell: { kind: "uniform", material_id: "software-pla-a" },
  white_material_id: "software-pla-b",
};

const intent = {
  ...fixed,
  printer: {
    ...fixed.printer,
    workflow: {
      kind: "fixed_dual_marlin",
      job: {
        ...fixed.printer.workflow.job,
        assignments: [{ ...fixed.printer.workflow.job.assignments[0], material: { kind: "uniform", material_id: "software-pla-b" } }],
        surface_constructions: [construction],
      },
    },
  },
  recipe: { ...fixed.recipe, id: "surface-color-box", infill_fraction: 0.25 },
} satisfies ManufacturingIntent;

export default intent;
