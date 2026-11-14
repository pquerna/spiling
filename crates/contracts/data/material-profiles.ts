// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
// Trusted authoring data; the engine consumes only the generated inert JSON.
//
// Generic process profiles for the common polymer families, one per nozzle
// class, keyed by compatibility (nozzle diameter, a generic brass hotend and
// part fan, polymer family) rather than by printer. Values follow the
// generic presets bundled with PrusaSlicer 2.9.4 (PrusaResearch.ini, config
// 2.4.0, and Creality.ini 1.1.0 for Bowden retraction), cited per family
// below; where its MK3S-era "Generic" and MK4S presets differ, the value
// sits between them. They are declared starting points, not calibrated for
// any spool. Speeds are the printer's (role speeds); a profile caps them only
// through its volumetric flow ceiling and the material print speed.
import type {
  MaterialProcessProfile,
  ProcessPolicy,
  RetractionPolicy,
} from "../../../packages/protocol/src/generated.ts";

const NOZZLES = [0.25, 0.4, 0.5, 0.6, 0.8];
const pwm = (percent: number) => Math.round((percent / 100) * 255);
const round6 = (v: number) => Math.round(v * 1e6) / 1e6;

// SHA-256 of the exact bundled profile files the values cite.
const PRUSA_RESEARCH_INI = "51f65fdc4c7d142a20b57c3e5914d2bafd23d0d16c1a30c0a620713617cf776c";

type Family = {
  family: string;
  label: string;
  cite: string;
  nozzle: number;
  firstNozzle: number;
  bed: number;
  firstBed: number;
  standby: number;
  heatedBed: boolean;
  flow: number;
  multiplier: number;
  printSpeed: number;
  fan: { first: number; min: number; max: number; bridge: number; shortLayer: number };
  cooling: { minimumLayerTime: number; minimumSpeed: number };
  direct: RetractionPolicy;
  bowden: RetractionPolicy;
};
const retract = (
  distance_mm: number,
  speed_mm_s: number,
  minimum_travel_mm: number,
  z_hop_mm: number,
): RetractionPolicy => ({ kind: "enabled", distance_mm, speed_mm_s, minimum_travel_mm, z_hop_mm });

const families: Family[] = [
  {
    family: "PLA",
    label: "Generic PLA",
    // [filament:Generic PLA] 210/215, [filament:Generic PLA @MK4S] 220/230;
    // bed 60; full fan after the first layer; slow below 6-12 s; 15 mm3/s.
    cite: "[filament:Generic PLA], [filament:Generic PLA @MK4S]",
    nozzle: 215,
    firstNozzle: 220,
    bed: 60,
    firstBed: 60,
    standby: 170,
    heatedBed: false,
    flow: 15,
    multiplier: 1,
    printSpeed: 600,
    fan: { first: 0, min: 100, max: 100, bridge: 100, shortLayer: 17 },
    cooling: { minimumLayerTime: 6, minimumSpeed: 15 },
    // MK4S printer retract 0.7 mm @ 35, lift 0.2, before travel 1.5;
    // Creality Ender-3 (Bowden) 5 mm, deretract 40.
    direct: retract(0.8, 35, 1.5, 0.2),
    bowden: retract(5, 40, 2, 0),
  },
  {
    family: "PETG",
    label: "Generic PETG",
    // [filament:Generic PETG] / @MK4S: 240/240, bed 85 first / 90; fan
    // 20-50 %, bridge 40-50 %; slow below 7-15 s; 8-12 mm3/s; retract 0.8-1.
    cite: "[filament:Generic PETG], [filament:Generic PETG @MK4S]",
    nozzle: 240,
    firstNozzle: 240,
    bed: 90,
    firstBed: 85,
    // PrusaSlicer's MK4S start G-code probes PET at 175 degrees.
    standby: 175,
    heatedBed: true,
    flow: 10,
    multiplier: 1,
    printSpeed: 600,
    fan: { first: 0, min: 30, max: 50, bridge: 50, shortLayer: 20 },
    cooling: { minimumLayerTime: 10, minimumSpeed: 15 },
    direct: retract(0.8, 35, 1.5, 0.2),
    bowden: retract(6, 40, 2, 0),
  },
  {
    family: "ABS",
    label: "Generic ABS",
    // [filament:Generic ABS] / @MK4S: 255-260, bed 100 first / 110; fan
    // 10-15 %, bridge 20-25 %; slow below 20 s; 11-15 mm3/s.
    cite: "[filament:Generic ABS], [filament:Generic ABS @MK4S]",
    nozzle: 255,
    firstNozzle: 255,
    bed: 110,
    firstBed: 100,
    standby: 170,
    heatedBed: true,
    flow: 12,
    multiplier: 1,
    printSpeed: 600,
    fan: { first: 0, min: 15, max: 15, bridge: 25, shortLayer: 30 },
    cooling: { minimumLayerTime: 20, minimumSpeed: 15 },
    direct: retract(0.8, 35, 1.5, 0.2),
    bowden: retract(5, 40, 2, 0),
  },
  {
    family: "ASA",
    label: "Generic ASA",
    // [filament:Prusament ASA] (the bundle has no generic ASA): 260/260,
    // bed 105 first / 110, fan 20 %, bridge 30 %, slow below 15 s, 11 mm3/s.
    cite: "[filament:Prusament ASA]",
    nozzle: 260,
    firstNozzle: 260,
    bed: 110,
    firstBed: 105,
    standby: 170,
    heatedBed: true,
    flow: 11,
    multiplier: 1,
    printSpeed: 600,
    fan: { first: 0, min: 20, max: 20, bridge: 30, shortLayer: 20 },
    cooling: { minimumLayerTime: 15, minimumSpeed: 15 },
    direct: retract(0.8, 35, 1.5, 0.2),
    bowden: retract(5, 40, 2, 0),
  },
  {
    family: "TPU",
    label: "Generic TPU",
    // [filament:Generic FLEX] / @MK4S: 230-240, bed 50; fan 30-50 % (MK4S),
    // bridge 70 %; slow below 10-15 s; 1.2-3 mm3/s; flow 1.08 (MK4S);
    // direct retract 2.5 mm (deretract 20 mm/s), none on Bowden.
    cite: "[filament:Generic FLEX], [filament:Generic FLEX @MK4S]",
    nozzle: 230,
    firstNozzle: 230,
    bed: 50,
    firstBed: 50,
    // PrusaSlicer's MK4S start G-code probes FLEX at 210 degrees.
    standby: 210,
    heatedBed: false,
    flow: 3,
    multiplier: 1.08,
    printSpeed: 50,
    fan: { first: 0, min: 30, max: 50, bridge: 70, shortLayer: 100 },
    cooling: { minimumLayerTime: 10, minimumSpeed: 10 },
    direct: retract(2.5, 20, 2, 0),
    bowden: { kind: "disabled" },
  },
];

const process = (f: Family): ProcessPolicy => ({
  first_layer_speed_factor: 1,
  first_layer_flow_factor: 1,
  fan_first_layers: pwm(f.fan.first),
  fan_after_first_layers: pwm(f.fan.min),
  bridge_speed_factor: 1,
  bridge_flow_factor: 1,
  bridge_fan_pwm: pwm(f.fan.bridge),
  max_bridge_span_mm: 10,
  retraction: f.direct,
  minimum_layer_time_s: f.cooling.minimumLayerTime,
  minimum_print_speed_mm_s: f.cooling.minimumSpeed,
  fan_short_layer_time_s: f.fan.shortLayer,
  fan_short_layer_pwm: pwm(f.fan.max),
});

const profiles: MaterialProcessProfile[] = families.flatMap((f) =>
  NOZZLES.map((nozzle) => ({
    id: `spiling.generic-${f.family.toLowerCase()}.${nozzle.toFixed(2)}`,
    revision: "1",
    material: {
      id: `spiling.generic-${f.family.toLowerCase()}`,
      revision: "1",
      manufacturer: "Generic",
      product: f.label,
      polymer_family: f.family,
      filler: null,
      filament_diameter_mm: 1.75,
      requires_abrasion_resistant_nozzle: false,
    },
    nozzle_diameter_mm: nozzle,
    hotend_id: "generic",
    nozzle_material: "brass",
    cooling_configuration_id: "part-fan",
    deposition_limits: {
      min_bead_width_mm: round6(0.75 * nozzle),
      max_bead_width_mm: round6(1.875 * nozzle),
      min_bead_height_mm: 0.000001,
      max_bead_height_mm: round6(0.75 * nozzle),
    },
    nozzle_temperature_c: f.nozzle,
    bed_temperature_c: f.bed,
    first_layer_nozzle_temperature_c: f.firstNozzle,
    first_layer_bed_temperature_c: f.firstBed,
    standby_nozzle_temperature_c: f.standby,
    requires_heated_bed: f.heatedBed,
    print_speed_mm_s: f.printSpeed,
    flow_multiplier: f.multiplier,
    max_volumetric_flow_mm3_s: f.flow,
    process: process(f),
    bowden_retraction: f.bowden,
    source: {
      kind: "imported_recommendation",
      source_id: `PrusaSlicer 2.9.4 PrusaResearch.ini ${f.cite}`,
      source_revision: "2.4.0",
      source_hash: PRUSA_RESEARCH_INI,
    },
  })),
);

export default profiles;
