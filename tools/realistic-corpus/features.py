# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0

"""Independent analytic checks for the parameterized box-family inputs."""
import math

MODEL_KINDS = {"perimeter", "solid_fill", "bottom_skin", "top_skin", "sparse_fill", "bridge", "separator_wall"}


def required_checks(case, intent):
    names = {"complete_source_inventory", "analytic_source_sections", "model_material_in_every_component",
             "retained_height", "no_silent_thin_omission"}
    if case.get("parameters", {}).get("requires_part_landing"):
        if intent["recipe"]["supports"]["kind"] == "disabled":
            names |= {"explicit_off", "unresolved_demand_reported"}
        else:
            names |= {"actual_support_deposited", "part_landing_not_bed_or_nominal_occupancy", "designed_roof_supported"}
    return names


def section_area(boxes, height):
    rectangles = [box for box in boxes if box["min"][2] <= height < box["max"][2]]
    stops = sorted({box[bound][0] for box in rectangles for bound in ("min", "max")})
    area = 0.0
    for left, right in zip(stops, stops[1:]):
        intervals = sorted((box["min"][1], box["max"][1]) for box in rectangles
                           if box["min"][0] < right and box["max"][0] > left)
        end = -math.inf
        length = 0.0
        for lower, upper in intervals:
            length += max(0.0, upper - max(lower, end))
            end = max(end, upper)
        area += (right - left) * length
    return area


def distance(point, start, end):
    delta = [end[axis] - start[axis] for axis in range(2)]
    norm = sum(value * value for value in delta)
    along = max(0.0, min(1.0, sum((point[axis] - start[axis]) * delta[axis] for axis in range(2)) / norm)) if norm else 0.0
    return math.hypot(*(point[axis] - start[axis] - along * delta[axis] for axis in range(2)))


def probe_covered(layers, point, height, allowance):
    for layer in layers:
        for path in layer["paths"]:
            if path["kind"] not in MODEL_KINDS:
                continue
            lower, upper = path["physical_z_mm"]
            if not lower <= height <= upper:
                continue
            width = path["width"]
            for index, (start, end) in enumerate(zip(path["points_mm"], path["points_mm"][1:])):
                diameter = width["width_mm"] if width["kind"] == "constant" else width["widths_mm"][index]
                if distance(point, start, end) <= diameter * 0.5 + allowance:
                    return True
    return False


def check(case, plan, verification, intent, imported):
    boxes = case["oracle"]["boxes"]
    layers = plan["layers"]
    minimum = [min(box["min"][axis] for box in boxes) for axis in range(3)]
    maximum = [max(box["max"][axis] for box in boxes) for axis in range(3)]
    envelope = intent["printer"]["build_envelope"]
    translation = [(envelope["min"][axis] + envelope["max"][axis] - minimum[axis] - maximum[axis]) * 0.5 for axis in range(2)]
    translation.append(-minimum[2])
    xy_allowance = plan["sampling_tolerance_mm"] + plan["offset_tolerance_mm"]
    z_allowance = plan["surface_tolerance_mm"] + plan["layer_quantization_mm"]
    perimeter = sum(2 * (box["max"][0] - box["min"][0] + box["max"][1] - box["min"][1]) for box in boxes)
    area_allowance = perimeter * plan["sampling_tolerance_mm"] + len(boxes) * math.pi * plan["sampling_tolerance_mm"] ** 2
    sections_ok = bool(layers)
    for layer in layers:
        height = layer["section_z_mm"] - translation[2]
        expected = [section_area(boxes, sample) for sample in (height - z_allowance, height, height + z_allowance)]
        observed = layer["region_coverage"]["source_section_area_mm2"]
        sections_ok &= math.isfinite(observed) and min(expected) - area_allowance <= observed <= max(expected) + area_allowance
    probes_ok = True
    for box in boxes:
        size = [box["max"][axis] - box["min"][axis] for axis in range(3)]
        narrow = 0 if size[0] <= size[1] else 1
        height = (box["min"][2] + box["max"][2]) * 0.5 + translation[2]
        for fraction in (0.25, 0.5, 0.75):
            point = [box["min"][axis] + size[axis] * fraction for axis in range(2)]
            point[narrow] = box["min"][narrow] + min(size[narrow] * 0.5, intent["recipe"]["bead_width_mm"] * 0.5)
            point = [value + translation[axis] for axis, value in enumerate(point)]
            probes_ok &= probe_covered(layers, point, height, xy_allowance)
    model_tops = [path["physical_z_mm"][1] for layer in layers for path in layer["paths"]
                  if path["kind"] in MODEL_KINDS]
    checks = {
        "complete_source_inventory": len(imported["occurrences"]) == case["oracle"]["occurrences"],
        "analytic_source_sections": bool(sections_ok), "model_material_in_every_component": bool(probes_ok),
        "retained_height": bool(model_tops) and abs(max(model_tops) - maximum[2] - translation[2]) <= z_allowance,
        "no_silent_thin_omission": all(layer["region_coverage"]["thin_omitted_area_mm2"] == 0 for layer in layers),
    }
    if case.get("parameters", {}).get("requires_part_landing"):
        support = [path for layer in layers for path in layer["paths"] if path["kind"] in ("support", "support_interface")]
        policy = intent["recipe"]["supports"]
        if policy["kind"] == "disabled":
            checks["explicit_off"] = not support and verification["support_deposited_volume_mm3"] == 0
            checks["unresolved_demand_reported"] = any(layer["region_coverage"]["unsupported_overhang_mm"] > 0 for layer in layers)
        else:
            floor = boxes[0]["max"][2] + translation[2]
            checks["actual_support_deposited"] = bool(support) and verification["support_deposited_volume_mm3"] > 0
            checks["part_landing_not_bed_or_nominal_occupancy"] = bool(support) and all(path["physical_z_mm"][0] >= floor + policy["z_gap_mm"] - z_allowance for path in support)
            checks["designed_roof_supported"] = all(layer["region_coverage"]["unsupported_overhang_mm"] == 0 for layer in layers)
    return checks
