"""Frozen analytic support of immutable-road subsets; not native admission or strength."""
from __future__ import annotations

import math

EPS = 0.000002
MAX_ROADS = 200_000
MAX_PAIRS = 12_000_000


def dot(first, second):
    return sum(a * b for a, b in zip(first, second))


def sub(first, second):
    return [a - b for a, b in zip(first, second)]


def disk_interval(start, delta, center, radius):
    offset = sub(start, center)
    square, linear = dot(delta, delta), dot(offset, delta)
    discriminant = linear * linear - square * (dot(offset, offset) - radius * radius)
    if discriminant <= 0 or square == 0:
        return None
    half = math.sqrt(discriminant)
    low, high = max(0.0, (-linear - half) / square), min(1.0, (-linear + half) / square)
    return [low, high] if low < high else None


def linear_interval(origin, slope, low, high):
    if slope == 0:
        return [0.0, 1.0] if low <= origin <= high else None
    first, second = (low - origin) / slope, (high - origin) / slope
    result = [max(0.0, min(first, second)), min(1.0, max(first, second))]
    return result if result[0] < result[1] else None


def capsule_intervals(road, previous):
    start, stop = road["start"][:2], road["end"][:2]
    first, second = previous["start"][:2], previous["end"][:2]
    radius = previous["width"] / 2 - EPS
    delta, direction = sub(stop, start), sub(second, first)
    square, result = dot(direction, direction), []
    for center in (first, second):
        interval = disk_interval(start, delta, center, radius)
        if interval:
            result.append(interval)
    if square > 0:
        offset = sub(start, first)
        projection = linear_interval(dot(offset, direction), dot(delta, direction), 0.0, square)
        cross = offset[0] * direction[1] - offset[1] * direction[0]
        slope = delta[0] * direction[1] - delta[1] * direction[0]
        extent = radius * math.sqrt(square)
        strip = linear_interval(cross, slope, -extent, extent)
        if projection and strip:
            interval = [max(projection[0], strip[0]), min(projection[1], strip[1])]
            if interval[0] < interval[1]:
                result.append(interval)
    return result


def point_distance(point, road):
    first, second = road["start"][:2], road["end"][:2]
    direction = sub(second, first)
    square = dot(direction, direction)
    parameter = min(1.0, max(0.0, dot(sub(point, first), direction) / square)) if square else 0.0
    return math.dist(point, [first[axis] + parameter * direction[axis] for axis in (0, 1)])


def gaps(proof, active):
    intervals = sorted((low, high) for owner, low, high in proof["intervals"] if active[owner])
    cursor, result = 0.0, []
    for low, high in intervals:
        if low > cursor:
            result.append([cursor, low])
        cursor = max(cursor, high)
    if cursor < 1:
        result.append([cursor, 1.0])
    endpoints = [False, False]
    for owner, first, second in proof["endpoints"]:
        if active[owner]:
            endpoints[0] |= first
            endpoints[1] |= second
    return [(low, high, low > 0 or endpoints[0], high < 1 or endpoints[1]) for low, high in result]


def violations(road, proof, active, bridge_limit):
    result = []
    for low, high, left, right in gaps(proof, active):
        span = (high - low) * road["length"]
        supported = (left or right) and (span <= 0.01 or (not (left and right) and span <= road["width"] / 2 + 0.01))
        bridge = left and right and road["kind"] == 8 and span <= bridge_limit
        if not supported and not bridge:
            result.append([low, high])
    return result


def baseline_covers(proof, interval, length):
    return any(low <= interval[0] + EPS / length and interval[1] <= high + EPS / length for low, high in proof["baseline_bad"])


def prepare(roads, bridge_limit):
    from shapely.geometry import box
    from shapely.strtree import STRtree

    if not roads or len(roads) > MAX_ROADS:
        raise ValueError("empty or unbounded road inventory")
    layers = {}
    for index, road in enumerate(roads):
        road["id"] = index
        road["length"] = math.dist(road["start"], road["end"])
        values = (*road["start"], *road["end"], road["width"], road["lower"], road["upper"], road["feed_mm_s"], road["extruded_filament_mm"])
        if not all(math.isfinite(value) for value in values) or road["length"] <= 0 or road["width"] <= 2 * EPS or road["upper"] <= road["lower"] or road["feed_mm_s"] <= 0 or road["extruded_filament_mm"] <= 0:
            raise ValueError("invalid actual deposition")
        layers.setdefault(road["upper"], []).append(index)
    indices = {}
    for height, owners in layers.items():
        boxes = [box(min(roads[index]["start"][0], roads[index]["end"][0]) - roads[index]["width"] / 2,
                     min(roads[index]["start"][1], roads[index]["end"][1]) - roads[index]["width"] / 2,
                     max(roads[index]["start"][0], roads[index]["end"][0]) + roads[index]["width"] / 2,
                     max(roads[index]["start"][1], roads[index]["end"][1]) + roads[index]["width"] / 2) for index in owners]
        indices[height] = owners, STRtree(boxes)
    proofs, pair_work = [], 0
    floor = min(road["lower"] for road in roads)
    for road in roads:
        proof = {"intervals": [], "endpoints": [], "bed": road["lower"] == floor}
        if not proof["bed"] and road["lower"] in indices:
            owners, tree = indices[road["lower"]]
            radius = road["width"] / 2
            bounds = box(min(road["start"][0], road["end"][0]) - radius,
                         min(road["start"][1], road["end"][1]) - radius,
                         max(road["start"][0], road["end"][0]) + radius,
                         max(road["start"][1], road["end"][1]) + radius)
            for found in tree.query(bounds):
                pair_work += 1
                if pair_work > MAX_PAIRS:
                    raise ValueError("frozen support pair work exceeded")
                owner = owners[int(found)]
                previous = roads[owner]
                for low, high in capsule_intervals(road, previous):
                    proof["intervals"].append([owner, low, high])
                allowance = (road["width"] + previous["width"]) / 2 - EPS
                contacts = [point_distance(point[:2], previous) < allowance for point in (road["start"], road["end"])]
                if any(contacts):
                    proof["endpoints"].append([owner, *contacts])
        proofs.append(proof)
    active = [True] * len(roads)
    for road, proof in zip(roads, proofs):
        proof["baseline_bad"] = [] if proof["bed"] else violations(road, proof, active, bridge_limit)
    return proofs, pair_work


def judge(context, selected):
    roads, proofs = context["roads"], context["proofs"]
    if not isinstance(selected, list) or any(type(index) is not int or index < 0 or index >= len(roads) for index in selected) or selected != sorted(set(selected)):
        raise ValueError("candidate IDs must be unique, sorted baseline indices")
    active = [False] * len(roads)
    for index in selected:
        active[index] = True
    if any(not active[index] and not road["optional"] for index, road in enumerate(roads)):
        raise ValueError("removed protected web/seed/joint/skin/shell/bridge")
    worst_roof_gap, inherited, count = 0.0, 0, 0
    for index in selected:
        road, proof = roads[index], proofs[index]
        if proof["bed"]:
            continue
        bad = violations(road, proof, active, context["bridge_limit_mm"])
        if any(not baseline_covers(proof, interval, road["length"]) for interval in bad):
            raise ValueError(f"new support/process violation at road {index}, layer {road['layer']}: {bad[:3]}")
        inherited += bool(bad)
        if road["owner"] == "mandatory_skin":
            count += 1
            for low, high, left, right in gaps(proof, active):
                if left and right and road["kind"] == 8:
                    worst_roof_gap = max(worst_roof_gap, (high - low) * road["length"])
    total, optional, deposition_time, travel, starts = 0.0, 0.0, 0.0, 0.0, 0
    previous = None
    filament_area = math.pi * (context["filament_diameter_mm"] / 2) ** 2
    for index in selected:
        road = roads[index]
        volume = road["extruded_filament_mm"] * filament_area
        total += volume
        optional += volume if road["optional"] else 0.0
        deposition_time += road["length"] / road["feed_mm_s"]
        distance = math.dist(previous, road["start"]) if previous else 0.0
        if previous is None or distance > EPS:
            starts += 1
        travel += distance
        previous = road["end"]
    return {"volume_mm3": total, "optional_corbel_mm3": optional, "deposition_feed_s": deposition_time,
            "filtered_travel_mm": travel, "ideal_filtered_motion_s": deposition_time + travel / context["travel_speed_mm_s"],
            "starts": starts, "deposition_commands": len(selected), "roof_strokes_checked": count,
            "max_roof_bridge_mm": worst_roof_gap, "bridge_margin_mm": context["bridge_limit_mm"] - worst_roof_gap,
            "inherited_warning_roads": inherited, "native_replayed": False}
