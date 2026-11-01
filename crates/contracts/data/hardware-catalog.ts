// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Trusted authoring data; the engine consumes only the generated inert JSON.
import type {
  HardwareCatalog,
  HardwareModelDefinition,
  PlateDefinition,
} from "../../../packages/protocol/src/generated.ts";
import cartesian from "../../../fixtures/manufacturing/software-cartesian.printer.ts";
import marlin from "../../../fixtures/manufacturing/software-marlin.printer.ts";
const model = {
  id: "spiling.software-cartesian",
  vendor: "Spiling",
  model: "Cartesian software reference",
  source: "Spiling software declarations; no physical qualification",
  mounting_interface: "spiling.reference.220",
  declared_volume_mm: cartesian.build_envelope,
} satisfies HardwareModelDefinition;
const plate = {
  id: "spiling.reference.220.plate",
  label: "220 mm reference sheet",
  mounting_interface: model.mounting_interface,
  thickness_mm: 1,
  surface_z_mm: 0,
  source: "Spiling software declaration",
  faces: [
    {
      key: "textured",
      label: "Textured face",
      surface: "declared textured",
      usable_mm: cartesian.build_envelope,
      keepouts_mm: [],
      max_bed_celsius: 110,
    },
    {
      key: "smooth",
      label: "Smooth face",
      surface: "declared smooth",
      usable_mm: cartesian.build_envelope,
      keepouts_mm: [],
      max_bed_celsius: 100,
    },
  ],
} satisfies PlateDefinition;
export default {
  models: [
    model,
    {
      id: "bambulab.h2c",
      vendor: "Bambu Lab",
      model: "H2C",
      source: "Catalog identity only; no Spiling backend or qualified machine setup",
      mounting_interface: "bambulab.h2-series",
      declared_volume_mm: null,
    },
  ],
  setups: [
    {
      id: "spiling.reference.cartesian.04",
      label: "Diagnostic Cartesian · 0.4 mm",
      model,
      printer: cartesian,
    },
    {
      id: "spiling.reference.marlin.04",
      label: "Marlin software reference · 0.4 mm",
      model,
      printer: marlin,
    },
  ],
  plates: [plate],
} satisfies HardwareCatalog;
