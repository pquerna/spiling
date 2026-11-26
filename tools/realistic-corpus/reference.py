# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0

"""Observations of Prusa's annotated CLI output, not a manufacturing verifier."""
import math
import re

MODEL_TYPES = {"External perimeter", "Perimeter", "Overhang perimeter", "Internal infill",
               "Solid infill", "Top solid infill", "Bridge infill", "Gap fill", "Ironing"}


def rotation_args(rotations):
    """Set the per-axis rotation options in Prusa's imported frame."""
    flags = {"x": "--rotate-x", "y": "--rotate-y", "z": "--rotate"}
    if not isinstance(rotations, dict) or any(
            axis not in flags or type(degrees) is not int for axis, degrees in rotations.items()):
        raise ValueError("Prusa rotations require x, y or z axes with integer degrees")
    return [argument for axis, flag in flags.items() if axis in rotations
            for argument in (flag, str(rotations[axis]))]


def mesh_info(text):
    fields = dict(re.findall(r"(?m)^([a-z_]+)\s*=\s*(.+)$", text))
    result = {"manifold": {"yes": True, "no": False}.get(fields.get("manifold")),
              "open_edges": int(fields["open_edges"]) if "open_edges" in fields else None}
    for name in ("size_x", "size_y", "size_z", "min_x", "min_y", "min_z", "max_x", "max_y", "max_z", "volume"):
        result[name] = float(fields[name]) if name in fields else None
    for name in ("number_of_parts", "number_of_facets"):
        result[name] = int(fields[name]) if name in fields else None
    result["positive_geometry"] = all(isinstance(result[name], (int, float)) and
                                      math.isfinite(result[name]) and result[name] > 0
                                      for name in ("size_x", "size_y", "size_z", "volume"))
    return result


def program_info(lines, step_layer):
    result = {"g1_lines": 0, "model_extrusion_moves": 0, "filament_mm": None}
    layers = []
    role = None
    relative = False
    previous_e = 0.0
    position = {}
    for line in lines:
        if line.startswith(";TYPE:"):
            role = line[6:].strip()
        elif line.startswith(";Z:"):
            layers.append(float(line[3:]))
        elif line.startswith("; estimated printing time (normal mode) ="):
            units = {"d": 86400, "h": 3600, "m": 60, "s": 1}
            result["estimated_seconds"] = sum(int(value) * units[unit] for value, unit in
                                              re.findall(r"(\d+)([dhms])", line.split("=", 1)[1]))
        elif line.startswith("; filament used [mm] ="):
            result["filament_mm"] = sum(float(value) for value in line.split("=", 1)[1].split(","))
        command = line.split(";", 1)[0].split()
        if not command:
            continue
        code = command[0]
        if code in ("M82", "M83"):
            relative = code == "M83"
        if code not in ("G0", "G1", "G2", "G3", "G92"):
            continue
        values = {match[1]: float(match[2]) for word in command[1:]
                  if (match := re.fullmatch(r"([XYZE])([-+0-9.eE]+)", word))}
        if code == "G92":
            previous_e = values.get("E", previous_e)
            position.update({axis: values[axis] for axis in ("X", "Y") if axis in values})
            continue
        extrusion = values.get("E", 0.0) if relative else values.get("E", previous_e) - previous_e
        moving = code in ("G2", "G3") or any(axis in values and values[axis] != position.get(axis)
                                                             for axis in ("X", "Y"))
        if role in MODEL_TYPES and extrusion > 0 and moving:
            result["model_extrusion_moves"] += 1
        if "E" in values:
            previous_e = previous_e + values["E"] if relative else values["E"]
        position.update({axis: values[axis] for axis in ("X", "Y") if axis in values})
        result["g1_lines"] += code == "G1"
    result["layers"] = len(layers)
    result["skipped_layers"] = sum(max(0, round((upper - lower) / step_layer) - 1)
                                    for lower, upper in zip(layers, layers[1:]))
    result["nonempty_model"] = (result["model_extrusion_moves"] > 0 and bool(layers) and
                                result["filament_mm"] is not None and
                                math.isfinite(result["filament_mm"]) and result["filament_mm"] > 0)
    return result
