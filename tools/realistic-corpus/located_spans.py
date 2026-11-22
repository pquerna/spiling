"""Common nominal lower-road span observations; not a manufacturing verifier."""

from bisect import bisect_left, bisect_right
from collections import defaultdict
from dataclasses import dataclass
import json
import math
import os
from pathlib import Path
import re
from tempfile import NamedTemporaryFile

import audit
import reference


METRIC = "nominal-lower-road-centerline-v1"
Z_EPSILON_MM = 0.001
EPS = 1e-9
CELL_MM = 4.0
PRUSA_SUPPORT = {"Support material", "Support material interface"}
PRUSA_BRIDGE = {"Bridge infill", "Internal bridge infill"}
PRUSA_FLOW_HEIGHT = PRUSA_BRIDGE | {"Overhang perimeter"}
PRUSA_COURSE_ROADS = {"External perimeter", "Perimeter", "Solid infill", "Top solid infill"}
NATIVE_MODEL = {"perimeter", "solid_fill", "bottom_skin", "top_skin", "sparse_fill",
                "bridge", "separator_wall"}
NATIVE_SUPPORT = {"support", "support_interface"}
PARAMETER = re.compile(r"([XYZE])([-+]?(?:\d+(?:\.\d*)?|\.\d+))")


@dataclass(slots=True)
class Road:
    layer: int
    ordinal: int
    role: str
    support: bool
    bridge: bool
    z: tuple
    points: list
    widths: list


def physical(lower, upper):
    if not math.isfinite(lower) or not math.isfinite(upper) or lower >= upper:
        raise ValueError("invalid physical road interval")
    return lower, upper


def native_roads(plan):
    for layer in plan["layers"]:
        for ordinal, path in enumerate(layer["paths"]):
            role = path["kind"]
            if role in {"skirt", "brim"}:
                continue
            if role not in NATIVE_MODEL | NATIVE_SUPPORT:
                raise ValueError(f"uncategorized Spiling road: {role}")
            points = [(x, y) for x, y, _ in path["points_mm"]]
            width = path["width"]
            if width["kind"] == "constant":
                widths = [width["width_mm"]] * (len(points) - 1)
            elif width["kind"] == "per_edge":
                widths = width["widths_mm"]
            else:
                raise ValueError("unknown Spiling road width profile")
            yield Road(layer["index"], ordinal, role, role in NATIVE_SUPPORT, role == "bridge",
                       physical(*path["physical_z_mm"]), points, widths)


def prusa_roads(lines):
    layer = -1
    top = height = width = role = current = None
    position = {}
    previous_e = debt = 0.0
    relative_e = False
    roads = []
    course_heights = {}
    for ordinal, raw in enumerate(lines, 1):
        line = raw.strip()
        if line.startswith(";Z:"):
            layer += 1
            top, height, current = float(line[3:]), None, None
        elif line.startswith(";HEIGHT:") and top is not None:
            height, current = float(line[8:]), None
            course_heights.setdefault(layer, height)
        elif line.startswith(";WIDTH:"):
            width = float(line[7:])
        elif line.startswith(";TYPE:"):
            role, current = line[6:].strip(), None
        words = line.partition(";")[0].split()
        if not words:
            continue
        command = words[0]
        if (command in {"G10", "G11", "G20", "G53", "G91", "M200", "M218"}
                or re.fullmatch(r"T[1-9]\d*", command)):
            raise ValueError(f"unsupported Prusa road mode at line {ordinal}: {command}")
        if command in {"M82", "M83"}:
            relative_e = command == "M83"
        if command not in {"G0", "G1", "G2", "G3", "G92"}:
            if command.startswith("G") and any(word.startswith("E") for word in words[1:]):
                raise ValueError(f"unsupported Prusa extrusion command at line {ordinal}")
            continue
        values = {}
        for word in words[1:]:
            if word.startswith(("X", "Y", "Z", "E")):
                match = PARAMETER.fullmatch(word)
                if not match or match[1] in values:
                    raise ValueError(f"ambiguous Prusa coordinate at line {ordinal}")
                values[match[1]] = float(match[2])
        coords = {axis: values[axis] for axis in "XYZ" if axis in values}
        if command == "G92":
            if coords:
                raise ValueError(f"Prusa XYZ reset at line {ordinal}")
            previous_e, current = values.get("E", previous_e), None
            continue
        before = tuple(position.get(axis) for axis in "XYZ")
        position.update(coords)
        after = tuple(position.get(axis) for axis in "XYZ")
        moving = before != after or command in {"G2", "G3"}
        delta = values.get("E", 0.0) if relative_e else values.get("E", previous_e) - previous_e
        previous_e += delta
        fresh = max(delta - debt, 0.0)
        debt = max(debt - delta, 0.0)
        if not moving or fresh <= EPS or role in {"Custom", "Skirt/Brim", "Ironing", "Wipe tower"}:
            if moving or abs(delta) > EPS:
                current = None
            continue
        if role not in reference.MODEL_TYPES | PRUSA_BRIDGE | PRUSA_SUPPORT:
            raise ValueError(f"uncategorized Prusa road at line {ordinal}: {role}")
        if (command not in {"G0", "G1"} or None in before + after or top is None or height is None
                or width is None or not (abs(before[2] - top) <= Z_EPSILON_MM
                                         and abs(after[2] - top) <= Z_EPSILON_MM)
                or before[:2] == after[:2] or fresh + EPS < delta):
            raise ValueError(f"unresolved Prusa road geometry at line {ordinal}")
        if current is None or current.points[-1] != before[:2]:
            current = Road(layer, ordinal, role, role in PRUSA_SUPPORT, role in PRUSA_BRIDGE,
                           physical(top - height, top), [before[:2]], [])
            roads.append(current)
        current.points.append(after[:2])
        current.widths.append(width)
    nominal = defaultdict(list)
    model_courses = {}
    for road in roads:
        if not road.support:
            model_courses[road.layer] = road.z[1]
        if road.role in PRUSA_COURSE_ROADS:
            nominal[road.layer].append(road.z[0])
    for road in roads:
        if road.role not in PRUSA_FLOW_HEIGHT:
            continue
        top = road.z[1]
        floors = nominal[road.layer]
        if floors and max(floors) - min(floors) <= Z_EPSILON_MM:
            road.z = physical(min(floors), top)
        elif not floors and (road.layer == 0 or road.layer - 1 in model_courses):
            road.z = physical(top - course_heights[road.layer], top)
        else:
            raise ValueError(f"unresolved Prusa model course at flow-height road line {road.ordinal}")
    return roads


def clip_axis(interval, coordinate, slope, low, high, strict=False):
    if slope == 0:
        inside = low < coordinate < high if strict else low <= coordinate <= high
        return interval if inside else None
    start, end = sorted(((low - coordinate) / slope, (high - coordinate) / slope))
    start, end = max(start, interval[0]), min(end, interval[1])
    return (start, end) if start < end else None


def capsule_intervals(start, unit, length, lower_start, lower_end, radius):
    for center in (lower_start, lower_end):
        dx, dy = center[0] - start[0], center[1] - start[1]
        perp = abs(dx * unit[1] - dy * unit[0])
        if perp < radius:
            half = math.sqrt((radius - perp) * (radius + perp))
            middle = dx * unit[0] + dy * unit[1]
            low, high = max(0.0, middle - half), min(length, middle + half)
            if low < high:
                yield low, high
    dx, dy = lower_end[0] - lower_start[0], lower_end[1] - lower_start[1]
    full = math.hypot(dx, dy)
    if full > 0:
        axis_x, axis_y = dx / full, dy / full
        dx, dy = start[0] - lower_start[0], start[1] - lower_start[1]
        interval = clip_axis((0, length), dx * axis_x + dy * axis_y,
                             unit[0] * axis_x + unit[1] * axis_y, 0, full)
        if interval is not None:
            interval = clip_axis(interval, dx * axis_y - dy * axis_x,
                                 unit[0] * axis_y - unit[1] * axis_x, -radius, radius, strict=True)
            if interval is not None:
                yield interval


def footprint_box(start, end, pad=0):
    return (min(start[0], end[0]) - pad, min(start[1], end[1]) - pad,
            max(start[0], end[0]) + pad, max(start[1], end[1]) + pad)


def cells(bounds):
    left, bottom, right, top = (math.floor(value / CELL_MM) for value in bounds)
    if (right - left + 1) * (top - bottom + 1) > 512:
        return None
    return ((x, y) for x in range(left, right + 1) for y in range(bottom, top + 1))


class LowerEdges:
    def __init__(self, donors):
        self.edges = []
        self.buckets = defaultdict(list)
        self.large = set()
        for road in donors:
            for start, end, width in zip(road.points, road.points[1:], road.widths):
                if start == end:
                    continue
                bounds = footprint_box(start, end, width / 2)
                ordinal = len(self.edges)
                self.edges.append((start, end, width / 2, road.z[1], bounds))
                footprint = cells(bounds)
                if footprint is None:
                    self.large.add(ordinal)
                else:
                    for cell in footprint:
                        self.buckets[cell].append(ordinal)

    def covered(self, start, end, unit, length, top):
        box = footprint_box(start, end, EPS)
        footprint = cells(box)
        if footprint is None:
            candidates = range(len(self.edges))
        else:
            candidates = self.large.copy()
            for cell in footprint:
                candidates.update(self.buckets.get(cell, ()))
        for ordinal in candidates:
            low, high, radius, lower_top, bounds = self.edges[ordinal]
            if (lower_top < top - EPS and box[0] <= bounds[2] and bounds[0] <= box[2]
                    and box[1] <= bounds[3] and bounds[1] <= box[3]):
                yield from capsule_intervals(start, unit, length, low, high, radius)


def missing_intervals(covered, length):
    cursor = 0.0
    for start, end in sorted(covered):
        if start > cursor + EPS:
            yield cursor, start
        cursor = max(cursor, end)
    if cursor + EPS < length:
        yield cursor, length


def describe_gap(road, points, length, two_ends, bridge_limit):
    return {"layer": road.layer, "road": road.ordinal, "role": road.role,
            "physical_z_mm": road.z, "points_mm": points,
            "bounds_xy_mm": [[min(point[axis] for point in points) for axis in (0, 1)],
                             [max(point[axis] for point in points) for axis in (0, 1)]],
            "length_mm": length, "two_ended": two_ends,
            "bridge_within_limit": road.bridge and two_ends and length <= bridge_limit}


def straight_runs(road):
    start = end = width = None
    for left, right, edge_width in zip(road.points, road.points[1:], road.widths):
        if left == right:
            continue
        if start is not None and edge_width == width:
            ux, uy = end[0] - start[0], end[1] - start[1]
            vx, vy = right[0] - left[0], right[1] - left[1]
            product, reverse = ux * vy, uy * vx
            roundoff = 8 * math.ulp(1.0) * (abs(product) + abs(reverse))
            if ux * vx + uy * vy > 0 and abs(product - reverse) <= roundoff:
                end = right
                continue
        if start is not None:
            yield start, end
        start, end, width = left, right, edge_width
    if start is not None:
        yield start, end


def road_gaps(road, lower, bridge_limit):
    for start, end in straight_runs(road):
        length = math.dist(start, end)
        if length <= EPS:
            continue
        unit = tuple((finish - begin) / length for begin, finish in zip(start, end))
        gaps = missing_intervals(lower.covered(start, end, unit, length, road.z[1]), length)
        for low, high in gaps:
            points = [tuple(axis + distance * step for axis, step in zip(start, unit))
                      for distance in (low, high)]
            yield describe_gap(road, points, high - low, low > EPS and high < length - EPS, bridge_limit)


def observe(roads, support_gap, bridge_limit, emit):
    roads = list(roads)
    upper = defaultdict(list)
    model_layers = set()
    for road in roads:
        if (road.z[0] < -Z_EPSILON_MM or not road.widths or len(road.widths) + 1 != len(road.points)
                or any(not math.isfinite(width) or width <= 0 for width in road.widths)
                or any(not math.isfinite(axis) for point in road.points for axis in point)):
            raise ValueError("invalid road geometry")
        if not road.support:
            model_layers.add(road.layer)
            if road.z[0] > Z_EPSILON_MM:
                upper[road.z[0]].append(road)
    if not model_layers:
        raise ValueError("no model roads to observe")
    floors = sorted(upper)
    lower = defaultdict(list)
    for road in roads:
        end = road.z[1]
        first = bisect_left(floors, end - Z_EPSILON_MM)
        last = bisect_right(floors, end + Z_EPSILON_MM + (support_gap if road.support else 0.0))
        for floor in floors[first:last]:
            lower[floor].append(road)
    count = bridges = 0
    by_layer = {}
    for floor, paths in upper.items():
        index = LowerEdges(lower[floor])
        for road in paths:
            for span in road_gaps(road, index, bridge_limit):
                emit(span)
                if span["bridge_within_limit"]:
                    bridges += 1
                else:
                    count += 1
                    if road.layer not in by_layer or span["length_mm"] > by_layer[road.layer]["length_mm"]:
                        by_layer[road.layer] = span
    worst = max(by_layer.values(), key=lambda span: span["length_mm"], default=None)
    return {"model_layers": len(model_layers), "non_bed_floors": len(floors),
            "unsupported_layers": len(by_layer), "unsupported_spans": count,
            "tagged_bounded_bridges": bridges, "max_span_mm": worst["length_mm"] if worst else 0.0,
            "worst": worst}


def compare(prusa_path, plan_path, output, support_gap, bridge_limit):
    if not all(math.isfinite(value) and value >= 0 for value in (support_gap, bridge_limit)):
        raise ValueError("invalid common span policy")
    paths = {"prusa": Path(prusa_path), "spiling": Path(plan_path)}
    header = {"event": "header", "metric": METRIC, "z_epsilon_mm": Z_EPSILON_MM,
              "support_gap_mm": support_gap, "bridge_limit_mm": bridge_limit,
              "inputs_sha256": {name: audit.digest(path) for name, path in paths.items()}}
    output = Path(output)
    temporary = None
    try:
        with NamedTemporaryFile(mode="w", encoding="utf-8", dir=output.parent, delete=False) as stream:
            temporary = Path(stream.name)

            def write(row):
                stream.write(json.dumps(row, allow_nan=False, separators=(",", ":")) + "\n")

            write(header)
            results = {}
            for name, path in paths.items():
                with path.open(encoding="utf-8") as source:
                    roads = prusa_roads(source) if name == "prusa" else native_roads(json.load(source))
                    results[name] = observe(roads, support_gap, bridge_limit,
                                            lambda span: write({"event": "span", "slicer": name, **span}))
            write({"event": "complete", **results})
        os.link(temporary, output)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    return {"status": "observed_unregistered", "metric": METRIC, **results,
            "part_max_span_delta_mm": results["spiling"]["max_span_mm"] - results["prusa"]["max_span_mm"],
            "evidence": str(output.resolve()), "evidence_sha256": audit.digest(output)}


def native_observation(plan, plan_path, evidence, support_gap, bridge_limit):
    if not all(math.isfinite(value) and value >= 0 for value in (support_gap, bridge_limit)):
        raise ValueError("invalid common span policy")
    if not evidence.exists():
        spans = []
        observed = observe(native_roads(plan), support_gap, bridge_limit,
                           lambda span: None if span["bridge_within_limit"] else spans.append(span))
        return spans, observed
    return retained_observation(evidence, "spiling", {"spiling": plan_path}, support_gap, bridge_limit)


def retained_observation(evidence, slicer, inputs, support_gap, bridge_limit):
    spans = []
    with evidence.open(encoding="utf-8") as stream:
        header = json.loads(next(stream, "null"))
        if (not isinstance(header, dict) or header.get("event") != "header"
                or header.get("metric") != METRIC or header.get("z_epsilon_mm") != Z_EPSILON_MM
                or header.get("support_gap_mm") != support_gap or header.get("bridge_limit_mm") != bridge_limit
                or any(header["inputs_sha256"][name] != audit.digest(path) for name, path in inputs.items())):
            raise ValueError(f"located evidence does not bind the current {slicer} observation")
        observed = None
        bounded = 0
        for line in stream:
            row = json.loads(line)
            if observed is not None:
                raise ValueError("data after located evidence completion")
            if row["event"] == "complete":
                observed = row[slicer]
                if not isinstance(observed, dict):
                    raise ValueError(f"invalid {slicer} located evidence footer")
            elif row["event"] != "span" or row["slicer"] not in {"spiling", "prusa"}:
                raise ValueError("unexpected located evidence row")
            elif row["slicer"] == slicer:
                lower, upper = row["physical_z_mm"]
                physical(lower, upper)
                (from_x, from_y), (to_x, to_y) = row["points_mm"]
                length = row["length_mm"]
                if (not all(math.isfinite(value) for value in (from_x, from_y, to_x, to_y, length))
                        or length <= EPS
                        or abs(math.hypot(to_x - from_x, to_y - from_y) - length) > EPS
                        or type(row["bridge_within_limit"]) is not bool):
                    raise ValueError("invalid located span")
                if row["bridge_within_limit"]:
                    bounded += 1
                else:
                    row.pop("event")
                    row.pop("slicer")
                    spans.append(row)
        if (observed is None or observed["unsupported_spans"] != len(spans)
                or observed["tagged_bounded_bridges"] != bounded
                or observed["unsupported_layers"] != len({span["layer"] for span in spans})
                or observed["max_span_mm"] != max((span["length_mm"] for span in spans), default=0)):
            raise ValueError(f"incomplete {slicer} located evidence")
    return spans, observed
