/*!
 * SPDX-FileCopyrightText: 2026 Spiling contributors
 * SPDX-License-Identifier: OSL-3.0
 */
import { createHash } from "node:crypto";
export const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
export const cases = [1, 2].flatMap((walls) =>
  [0, 0.25, 1].map((infillFraction) => ({
    id: `cube-10-w${walls}-infill${infillFraction * 100}`,
    sideMm: 10,
    layerHeightMm: 0.2,
    lineWidthMm: 0.45,
    walls,
    infillFraction,
    skinThicknessMm: 0.8,
  })),
);

// Constrained analytic derivative of the original licensed source. This is not
// a general STEP transformer; refusal prevents a changed fixture being guessed.
export function cubeSource(source) {
  const scale = [0.5, 1, 1.25];
  if (
    (source.match(/= LINE\(/g) ?? []).length !== 12 ||
    (source.match(/= PLANE\(/g) ?? []).length !== 6
  )
    throw new Error("cube derivative requires the analytic 12-line/6-plane source");
  const directions = new Map();
  for (const m of source.matchAll(/#(\d+) = DIRECTION\('', \(([^)]+)\)\);/g)) {
    const values = m[2].split(",").map(Number);
    if (
      values.length !== 3 ||
      values.filter((v) => Math.abs(v) === 1).length !== 1 ||
      values.some((v) => v !== 0 && Math.abs(v) !== 1)
    )
      throw new Error("cube derivative requires axial directions");
    directions.set(m[1], values);
  }
  let points = 0,
    vectors = 0;
  const result = source
    .replace(/CARTESIAN_POINT\('', \(([^)]+)\)\)/g, (_, text) => {
      points++;
      const values = text.split(",").map(Number);
      if (values.length !== 3 || values.some((v, i) => ![0, [20, 10, 8][i]].includes(v)))
        throw new Error("unexpected analytic source point");
      return `CARTESIAN_POINT('', (${values.map((v, i) => (v * scale[i]).toFixed(1)).join(", ")}))`;
    })
    .replace(/VECTOR\('', #(\d+), ([^)]+)\)/g, (_, id, length) => {
      vectors++;
      const direction = directions.get(id);
      if (!direction || ![8, 10, 20].includes(Number(length)))
        throw new Error("unexpected analytic vector");
      return `VECTOR('', #${id}, ${(Number(length) * Math.hypot(...direction.map((v, i) => v * scale[i]))).toFixed(1)})`;
    })
    .replace("FILE_NAME('box-mm.step'", "FILE_NAME('cube-10-derived.step'");
  if (points !== 26 || vectors !== 12)
    throw new Error(`unexpected source structure: ${points} points/${vectors} vectors`);
  return result;
}
export function intentFor(base, c) {
  const intent = structuredClone(base);
  Object.assign(intent.recipe, { perimeter_count: c.walls, infill_fraction: c.infillFraction });
  intent.recipe.process.retraction = { kind: "disabled" };
  intent.recipe.supports = { kind: "disabled" };
  return intent;
}
export function referenceProfile(
  c,
  { topSolidLayers = 4, bridgeFlowRatio = 1, verticalShell = "enabled" } = {},
) {
  if (
    ![3, 4, 50].includes(topSolidLayers) ||
    ![0.5, 1].includes(bridgeFlowRatio) ||
    !["enabled", "disabled"].includes(verticalShell)
  )
    throw new Error("unsupported reference diagnostic settings");
  const values = {
    layer_height: 0.2,
    first_layer_height: 0.2,
    nozzle_diameter: 0.4,
    filament_diameter: 1.75,
    perimeters: c.walls,
    perimeter_generator: "classic",
    fill_density: `${c.infillFraction * 100}%`,
    fill_pattern: "rectilinear",
    fill_angle: 0,
    top_fill_pattern: "rectilinear",
    bottom_fill_pattern: "rectilinear",
    top_solid_layers: topSolidLayers,
    bottom_solid_layers: 4,
    solid_infill_below_area: 0,
    infill_every_layers: 1,
    infill_anchor: 0,
    infill_anchor_max: 0,
    ensure_vertical_shell_thickness: verticalShell,
    skirts: 0,
    brim_width: 0,
    support_material: 0,
    raft_layers: 0,
    extra_perimeters: 0,
    extra_perimeters_on_overhangs: 0,
    gap_fill_enabled: 0,
    infill_overlap: 0,
    seam_position: "nearest",
    gcode_flavor: "marlin2",
    use_relative_e_distances: 0,
    use_volumetric_e: 0,
    extrusion_multiplier: 1,
    retract_length: 0,
    retract_lift: 0,
    bridge_flow_ratio: bridgeFlowRatio,
    thick_bridges: 0,
    temperature: 200,
    first_layer_temperature: 200,
    bed_temperature: 60,
    first_layer_bed_temperature: 60,
    slowdown_below_layer_time: 0,
    fan_always_on: 1,
    min_fan_speed: 50,
    max_fan_speed: 50,
    bridge_fan_speed: 100,
    disable_fan_first_layers: 1,
    full_fan_speed_layer: 2,
    small_perimeter_speed: 35,
    seam_gap_distance: 0,
    filament_seam_gap_distance: 0,
    perimeter_speed: 35,
    external_perimeter_speed: 35,
    infill_speed: 35,
    solid_infill_speed: 35,
    top_solid_infill_speed: 35,
    first_layer_speed: "50%",
    travel_speed: 100,
    bridge_speed: 21,
    bed_shape: "0x0,220x0,220x220,0x220",
    start_gcode: "G21\\nG90\\nM82\\nG92 E0",
    end_gcode: "M104 S0\\nM140 S0",
  };
  for (const key of [
    "extrusion_width",
    "external_perimeter_extrusion_width",
    "perimeter_extrusion_width",
    "infill_extrusion_width",
    "solid_infill_extrusion_width",
    "top_infill_extrusion_width",
    "first_layer_extrusion_width",
  ])
    values[key] = 0.45;
  return (
    Object.entries(values)
      .map(([k, v]) => `${k} = ${v}`)
      .join("\n") + "\n"
  );
}

// These perturb the existing cube only; they are not alternative baseline settings.
export const referenceProbeSettings = [
  { id: "roof-shift", settings: { topSolidLayers: 3 } },
  { id: "half-bridge-flow", settings: { bridgeFlowRatio: 0.5 } },
  { id: "shell-disabled", settings: { verticalShell: "disabled" } },
  { id: "solid-all", settings: { topSolidLayers: 50 } },
];
export const referenceProbesFor = (c) =>
  referenceProbeSettings.filter((p) => (p.id === "solid-all") === (c.infillFraction === 1));
