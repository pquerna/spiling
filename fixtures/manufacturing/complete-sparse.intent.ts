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
    revision: "2",
    infill_fraction: 0.25,
    top_skin_thickness_mm: 0.8,
    bottom_skin_thickness_mm: 0.8,
    supports: { kind: "breakaway", density: 0.3, xy_clearance_mm: 0.3, z_gap_mm: 0.2, interface_thickness_mm: 0.4 },
  },
} satisfies ManufacturingIntent;

export default intent;
