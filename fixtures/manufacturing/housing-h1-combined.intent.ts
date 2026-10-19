// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

import type { ManufacturingIntent } from "../../packages/protocol/src/generated.ts";
import fine from "./housing-h1-fine.intent.ts";

// The paired inputs differ only in sparse-infill scheduling policy.
export default {
  ...fine,
  recipe: {
    ...fine.recipe,
    sparse_infill: { kind: "combine_eligible", max_height_mm: 0.3 },
  },
} satisfies ManufacturingIntent;
