// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
// Originally authored model declarations. No imported slicer profiles or scripts.
import type { CatalogItem } from "../../../packages/protocol/src/generated.ts";
type Printer = Extract<CatalogItem, { kind: "printer" }>;
export default [
  {
    id: "bambulab.a1",
    item: {
      kind: "printer",
      manufacturer: "Bambu Lab",
      model: "A1",
      aliases: ["Bambu A1"],
      volume_mm: [256, 256, 256],
      source_url: "https://bambulab.com/en/a1/tech-specs",
    },
  },
  {
    id: "bambulab.a1-mini",
    item: {
      kind: "printer",
      manufacturer: "Bambu Lab",
      model: "A1 mini",
      aliases: ["Bambu A1 mini"],
      volume_mm: [180, 180, 180],
      source_url: "https://bambulab.com/en/a1-mini/tech-specs",
    },
  },
  {
    id: "bambulab.p1s",
    item: {
      kind: "printer",
      manufacturer: "Bambu Lab",
      model: "P1S",
      aliases: ["Bambu P1S"],
      volume_mm: [256, 256, 256],
      source_url: "https://us.store.bambulab.com/products/p1s",
    },
  },
  {
    id: "bambulab.p1p",
    item: {
      kind: "printer",
      manufacturer: "Bambu Lab",
      model: "P1P",
      aliases: ["Bambu P1P"],
      volume_mm: [256, 256, 256],
      source_url: "https://public-cdn.bambulab.com/store/bambulab-P1P-tech-specs.pdf",
    },
  },
  {
    id: "bambulab.x1c",
    item: {
      kind: "printer",
      manufacturer: "Bambu Lab",
      model: "X1 Carbon",
      aliases: ["Bambu X1C", "X1C", "Bambu Lab X1C"],
      volume_mm: [256, 256, 256],
      source_url: "https://public-cdn.bambulab.com/store/bambulab-X1-carbon-tech-specs.pdf",
    },
  },
  {
    id: "bambulab.x1e",
    item: {
      kind: "printer",
      manufacturer: "Bambu Lab",
      model: "X1E",
      aliases: ["Bambu X1E"],
      volume_mm: [256, 256, 256],
      source_url: "https://cdn1.bambulab.com/x1e/spec/X1E%20Spec(EN).pdf",
    },
  },
  {
    id: "prusa.mk4s",
    item: {
      kind: "printer",
      manufacturer: "Prusa Research",
      model: "Original Prusa MK4S",
      aliases: ["Prusa MK4S"],
      volume_mm: [250, 210, 220],
      source_url: "https://www.prusa3d.com/product/original-prusa-mk4s-3d-printer-5/",
    },
  },
  {
    id: "creality.ender-3-v3-se",
    item: {
      kind: "printer",
      manufacturer: "Creality",
      model: "Ender-3 V3 SE",
      aliases: ["Ender 3 V3 SE"],
      volume_mm: [220, 220, 250],
      source_url: "https://www.creality.com/products/creality-ender-3-v3-se",
    },
  },
] satisfies Array<{ id: string; item: Printer }>;
