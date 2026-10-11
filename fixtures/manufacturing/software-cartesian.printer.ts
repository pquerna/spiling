// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

import type { PrinterSpecification } from "../../packages/protocol/src/generated.ts";

const printer = {
  schema_version: 1,
  id: "software-cartesian-reference",
  revision: "1",
  name: "Software Cartesian reference — not machine-ready",
  coordinate_frame: "right_handed_millimetres",
  build_envelope: { min: [0, 0, 0], max: [220, 220, 220] },
  components: [
    {
      id: "bed",
      role: "bed",
      pose: { translation_mm: [0, 0, 0], rotation_xyzw: [0, 0, 0, 1] },
      shape: { kind: "box", bounds_mm: { min: [0, 0, -4], max: [220, 220, 0] } },
    },
    {
      id: "toolhead-envelope",
      role: "toolhead",
      pose: { translation_mm: [0, 0, 0], rotation_xyzw: [0, 0, 0, 1] },
      shape: { kind: "cylinder", radius_mm: 10, height_mm: 25 },
    },
  ],
  capabilities: {
    motion_space: "cartesian_xyz",
    extruders: 1,
    output_dialect: "cartesian_absolute_gcode_v1",
    generated_supports: false,
  },
  nozzle_diameter_mm: 0.4,
  max_feed_mm_s: 150,
  max_volumetric_flow_mm3_s: 12,
} satisfies PrinterSpecification;

export default printer;
