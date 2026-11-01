// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

import type { ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import solid from "./solid-fill.intent.ts";

// Deliberately unattainable preference within explicit software-only declarations.
// The engine retains the request and selects a thicker admissible construction.
export default {
  ...solid,
  printer: {
    ...solid.printer,
    id: "software-goal-example",
    revision: "1",
    name: "Software layer-goal example, not a calibrated printer",
    deposition_limits: {
      ...solid.printer.deposition_limits,
      min_bead_height_mm: 0.1,
    },
  },
  recipe: {
    ...solid.recipe,
    id: "preferred-layers",
    revision: "1",
    layer_height_mm: 0.05,
    layer_height_policy: "preferred",
  },
} satisfies ManufacturingIntent;
