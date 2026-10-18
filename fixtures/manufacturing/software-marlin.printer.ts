// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

import type { PrinterSpecification } from "../../packages/protocol/src/generated.ts";
import diagnosticPrinter from "./software-cartesian.printer.ts";

// This is software test data, not a real printer or a calibrated process.
const printer = {
  ...diagnosticPrinter,
  id: "software-marlin2-reference",
  revision: "6",
  name: "Complete Marlin 2 software reference — physically unqualified",
  capabilities: {
    ...diagnosticPrinter.capabilities,
    output_dialect: "marlin2_single_extruder_gcode_v1",
    generated_supports: true,
  },
  workflow: {
    kind: "complete_marlin",
    job: {
      machine_id: "software-marlin2-reference",
      firmware_version: "2.1.2.5-software-contract-fixture",
      // SHA-256 of the exact adjacent configuration and calibration record bytes.
      configuration_hash: "c46cf9a35f2916b3109dbbae2a06dc0cb212d4fe611b511bf1c72220bbcc147c",
      calibration_record_hash: "322df893a20557507671a732334464bebf7164d9c547e6ef09eea356e99b3a8b",
      homing_contract: "marlin_native_cartesian",
      home_position_mm: [0, 0, 0],
      nozzle_temperature_c: 200,
      bed_temperature_c: 60,
      minimum_extrusion_temperature_c: 170,
      maximum_nozzle_temperature_c: 250,
      maximum_bed_temperature_c: 100,
      prime_start_mm: [10, 210, 0.2],
      prime_end_mm: [30, 210, 0.2],
      prime_filament_length_mm: 1.5,
      prime_speed_mm_s: 10,
      park_position_mm: [0, 210, 20],
    },
  },
} satisfies PrinterSpecification;

export default printer;
