// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

import type { ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import printer from "./software-marlin.printer.ts";
import solidIntent from "./solid-fill.intent.ts";

const intent = {
  printer,
  recipe: {
    ...solidIntent.recipe,
    id: "software-sparse-skins",
    revision: "1",
    infill_fraction: 0.25,
    top_skin_thickness_mm: 0.8,
    bottom_skin_thickness_mm: 0.8,
  },
} satisfies ManufacturingIntent;

export default intent;
