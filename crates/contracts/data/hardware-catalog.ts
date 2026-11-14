// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Trusted authoring data; the engine consumes only the generated inert JSON.
// The two software references are declared specifications (machine: null);
// every other setup is derived from machine-descriptions.ts.
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
const box = (x: number, y: number, z: number) => ({ min: [0, 0, 0], max: [x, y, z] });
// Models for machine-description setups (machine-descriptions.ts); the engine
// derives each setup and its stock sheet from the machine facts. Vendor/model
// text matches printer-references.ts so a reference finds its setup.
const described = [
  {
    id: "prusa.mk4s",
    vendor: "Prusa Research",
    model: "Original Prusa MK4S",
    source: "https://www.prusa3d.com/product/original-prusa-mk4s-3d-printer-5/",
    mounting_interface: "prusa.mk4s.sheet",
    declared_volume_mm: box(250, 210, 220),
  },
  {
    id: "bambulab.a1",
    vendor: "Bambu Lab",
    model: "A1",
    source: "https://bambulab.com/en/a1/tech-specs",
    mounting_interface: "bambulab.a1.plate",
    declared_volume_mm: box(256, 256, 256),
  },
  {
    id: "bambulab.a1-mini",
    vendor: "Bambu Lab",
    model: "A1 mini",
    source: "https://bambulab.com/en/a1-mini/tech-specs",
    mounting_interface: "bambulab.a1-mini.plate",
    declared_volume_mm: box(180, 180, 180),
  },
  {
    id: "creality.ender-3-v3-se",
    vendor: "Creality",
    model: "Ender-3 V3 SE",
    source: "https://www.creality.com/products/creality-ender-3-v3-se",
    mounting_interface: "creality.ender-3-v3-se.plate",
    declared_volume_mm: box(220, 220, 250),
  },
  {
    id: "generic.klipper-cartesian",
    vendor: "Generic",
    model: "Klipper Cartesian",
    source: "Spiling generic family defaults sized for modern printers; edit to match the machine",
    mounting_interface: "generic.klipper-cartesian.plate",
    declared_volume_mm: box(350, 350, 350),
  },
  {
    id: "generic.marlin-bedslinger",
    vendor: "Generic",
    model: "Marlin bed-slinger",
    source: "Spiling generic family defaults sized for modern printers; edit to match the machine",
    mounting_interface: "generic.marlin-bedslinger.plate",
    declared_volume_mm: box(300, 300, 300),
  },
] satisfies HardwareModelDefinition[];
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
    ...described,
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
      machine: null,
      printer: cartesian,
    },
    {
      id: "spiling.reference.marlin.04",
      label: "Marlin software reference · 0.4 mm",
      model,
      machine: null,
      printer: marlin,
    },
  ],
  plates: [plate],
} satisfies HardwareCatalog;
