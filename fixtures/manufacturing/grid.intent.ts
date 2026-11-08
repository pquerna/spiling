// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

import type { ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import base from "./complete-sparse.intent.ts";

export default {
  ...base,
  recipe: {
    ...base.recipe,
    id: "software-grid-skins",
    revision: "1",
    infill_fraction: 0.3,
    sparse_infill: { kind: "grid" },
    supports: { kind: "disabled" },
  },
} satisfies ManufacturingIntent;
