// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
// Trusted authoring data; the engine consumes only the generated inert JSON.
//
// Machine facts, layered: a setup inherits from its family/generic parent and
// overrides what its manufacturer publishes. Family defaults are sized for
// modern printers so a default limit is never the reason a job refuses or is
// throttled when the real printer could do it. Each limit records its source:
// a published specification URL or the declaring family. Unpublished values
// inherit the family default rather than an aggregator's guess.
import type {
  FactSource,
  MachineCatalog,
  MachineLayer,
  MachineLimit,
  SourcedLimit,
} from "../../../packages/protocol/src/generated.ts";

type Values = Partial<Record<MachineLimit, number>>;
const sourced = (values: Values, source: FactSource): SourcedLimit[] =>
  Object.entries(values).map(([key, value]) => ({
    key: key as MachineLimit,
    value: value as number,
    source,
  }));
const family = (id: string, values: Values): SourcedLimit[] =>
  sourced(values, { kind: "family_default", family: id });
const published = (reference: string, values: Values): SourcedLimit[] =>
  sourced(values, { kind: "published", reference });
const layer = (l: Partial<MachineLayer> & Pick<MachineLayer, "id" | "limits">): MachineLayer => ({
  parent: null,
  firmware: null,
  kinematics: null,
  levelling: null,
  heated_bed: null,
  arc_moves: null,
  ...l,
});

// Root of every FFF description. Deposition ratios: widths 0.75–1.875 × nozzle
// (0.30–0.75 mm at 0.4 mm), heights up to 0.75 × nozzle.
const fff = layer({
  id: "generic.fff",
  heated_bed: true,
  arc_moves: true,
  levelling: "probe_mesh",
  limits: family("generic.fff", {
    bed_x_mm: 350,
    bed_y_mm: 350,
    max_z_mm: 350,
    nozzle_diameter_mm: 0.4,
    filament_diameter_mm: 1.75,
    max_print_feed_mm_s: 600,
    max_travel_feed_mm_s: 800,
    max_acceleration_mm_s2: 20000,
    max_volumetric_flow_mm3_s: 32,
    max_nozzle_temperature_c: 300,
    min_extrusion_temperature_c: 170,
    max_bed_temperature_c: 120,
    min_bead_width_per_nozzle: 0.75,
    max_bead_width_per_nozzle: 1.875,
    max_bead_height_per_nozzle: 0.75,
    xy_resolution_mm: 0.0125,
    z_resolution_mm: 0.0025,
    toolhead_radius_mm: 10,
    toolhead_height_mm: 25,
  }),
});
const klipper = layer({
  id: "generic.klipper-cartesian",
  parent: fff.id,
  firmware: "klipper",
  kinematics: "core_xy",
  // Klipper interpolates G2/G3 only with an optional [gcode_arcs] section.
  arc_moves: false,
  limits: [],
});
const marlin = layer({
  id: "generic.marlin-bedslinger",
  parent: fff.id,
  firmware: "marlin2",
  kinematics: "bed_slinger",
  limits: family("generic.marlin-bedslinger", { bed_x_mm: 300, bed_y_mm: 300, max_z_mm: 300 }),
});
const mk4sUrl = "https://www.prusa3d.com/product/original-prusa-mk4s-3d-printer-5/";
const mk4s = layer({
  id: "prusa.mk4s",
  parent: fff.id,
  firmware: "prusa_buddy",
  kinematics: "bed_slinger",
  // Speed, acceleration and flow are not published; family defaults apply.
  limits: published(mk4sUrl, {
    bed_x_mm: 250,
    bed_y_mm: 210,
    max_z_mm: 220,
    nozzle_diameter_mm: 0.4,
    filament_diameter_mm: 1.75,
    max_nozzle_temperature_c: 290,
    max_bed_temperature_c: 120,
  }),
});
// bambulab.com tech-spec tables, as reproduced by an authorised reseller.
const a1Url = "https://bambulab.com/en/a1/tech-specs (via crcibernetica.com/bambu-lab-a1)";
const a1 = layer({
  id: "bambulab.a1",
  parent: fff.id,
  firmware: "bambu_lab",
  kinematics: "bed_slinger",
  limits: [
    ...published(a1Url, {
      bed_x_mm: 256,
      bed_y_mm: 256,
      max_z_mm: 256,
      nozzle_diameter_mm: 0.4,
      filament_diameter_mm: 1.75,
      max_print_feed_mm_s: 500,
      max_travel_feed_mm_s: 500,
      max_acceleration_mm_s2: 10000,
      max_nozzle_temperature_c: 300,
      max_bed_temperature_c: 100,
    }),
    ...published("https://wiki.bambulab.com/en/knowledge-sharing/volumetric-speed", {
      max_volumetric_flow_mm3_s: 28,
    }),
  ],
});
const a1MiniUrl =
  "https://bambulab.com/en/a1-mini/tech-specs (via crcibernetica.com/bambu-lab-a1-mini)";
const a1Mini = layer({
  id: "bambulab.a1-mini",
  parent: fff.id,
  firmware: "bambu_lab",
  kinematics: "bed_slinger",
  limits: published(a1MiniUrl, {
    bed_x_mm: 180,
    bed_y_mm: 180,
    max_z_mm: 180,
    nozzle_diameter_mm: 0.4,
    max_print_feed_mm_s: 500,
    max_travel_feed_mm_s: 500,
    max_acceleration_mm_s2: 10000,
    max_nozzle_temperature_c: 300,
    max_bed_temperature_c: 80,
  }),
});
const enderUrl = "https://www.creality.com/support/creality-ender-3-v3-se";
const ender = layer({
  id: "creality.ender-3-v3-se",
  parent: marlin.id,
  limits: [
    ...published("https://www.creality.com/products/creality-ender-3-v3-se", {
      bed_x_mm: 220,
      bed_y_mm: 220,
      max_z_mm: 250,
    }),
    ...published(enderUrl, {
      nozzle_diameter_mm: 0.4,
      max_print_feed_mm_s: 250,
      max_acceleration_mm_s2: 2500,
      max_nozzle_temperature_c: 260,
      max_bed_temperature_c: 100,
    }),
  ],
});

export default {
  layers: [fff, klipper, marlin, mk4s, a1, a1Mini, ender],
  setups: [
    { id: "prusa.mk4s.04", label: "Prusa MK4S · 0.4 mm", model: "prusa.mk4s", machine: mk4s.id },
    { id: "bambulab.a1.04", label: "Bambu Lab A1 · 0.4 mm", model: "bambulab.a1", machine: a1.id },
    {
      id: "bambulab.a1-mini.04",
      label: "Bambu Lab A1 mini · 0.4 mm",
      model: "bambulab.a1-mini",
      machine: a1Mini.id,
    },
    {
      id: "creality.ender-3-v3-se.04",
      label: "Creality Ender-3 V3 SE · 0.4 mm",
      model: "creality.ender-3-v3-se",
      machine: ender.id,
    },
    {
      id: "generic.klipper-cartesian.04",
      label: "Generic Klipper Cartesian · 0.4 mm",
      model: "generic.klipper-cartesian",
      machine: klipper.id,
    },
    {
      id: "generic.marlin-bedslinger.04",
      label: "Generic Marlin bed-slinger · 0.4 mm",
      model: "generic.marlin-bedslinger",
      machine: marlin.id,
    },
  ],
} satisfies MachineCatalog;
