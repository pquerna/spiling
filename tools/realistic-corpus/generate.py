# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0

"""Generate small native STEP families; run them through run.py --manifest.

This is an input recipe over the original planar box, not a geometry fallback.
Dimensions, source inventory and analytic oracles accompany each generated file.
"""
import argparse
import hashlib
import itertools
import json
from pathlib import Path
import re

import audit

TEMPLATE = Path(__file__).resolve().parents[2] / "fixtures/geometry/box-mm.step"
TOKEN = re.compile(r"'(?:[^']|'')*'|#(\d+)")
POINT = re.compile(r"CARTESIAN_POINT\('', \(([^)]+)\)\)")
DIRECTION = re.compile(r"DIRECTION\('', \(([^)]+)\)\)")
VECTOR = re.compile(r"(VECTOR\('', #\d+, )([^)]+)(\))")


def box(size, origin=(0, 0, 0), quarter_turn=False):
    if len(size) != 3 or any(not audit.positive(value) for value in size):
        raise ValueError("box dimensions must be finite and positive")
    source = TEMPLATE.read_text().split("DATA;", 1)[1].rsplit("ENDSEC;", 1)[0]

    def rotate(point):
        return [-point[1], point[0], point[2]] if quarter_turn else point

    def point(match):
        values = [float(value) for value in match[1].split(",")]
        scaled = rotate([value * length / base for value, length, base in zip(values, size, (20, 10, 8))])
        return "CARTESIAN_POINT('', (" + ", ".join(str(value + offset) for value, offset in zip(scaled, origin)) + "))"

    def direction(match):
        values = rotate([float(value) for value in match[1].split(",")])
        return "DIRECTION('', (" + ", ".join(map(str, values)) + "))"

    lengths = dict(zip((20.0, 10.0, 8.0), size))
    source = POINT.sub(point, source)
    source = DIRECTION.sub(direction, source)
    source = VECTOR.sub(lambda match: match[1] + str(lengths[float(match[2])]) + match[3], source)
    corners = [rotate(corner) for corner in itertools.product(*[(0, value) for value in size])]
    bounds = {"min": [min(c[axis] for c in corners) + origin[axis] for axis in range(3)],
              "max": [max(c[axis] for c in corners) + origin[axis] for axis in range(3)]}
    return source, bounds


def step(bodies):
    output = []
    offset = 0
    boxes = []
    for source, bounds in bodies:
        identifiers = [int(match[1]) for match in TOKEN.finditer(source) if match[1]]
        output.append(TOKEN.sub(lambda match: f"#{int(match[1]) + offset}" if match[1] else match[0], source))
        offset += max(identifiers)
        boxes.append(bounds)
    header = """ISO-10303-21;
HEADER;
FILE_DESCRIPTION(('Spiling parameterized native corpus'), '2;1');
FILE_NAME('family.step', '2026-01-01T00:00:00', ('Spiling contributors'), ('Spiling'), 'Spiling input recipe', 'Derived from original box-mm.step', 'OSL-3.0');
FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));
ENDSEC;
DATA;
"""
    return (header + "\n".join(output) + "ENDSEC;\nEND-ISO-10303-21;\n").encode(), boxes


def cases(family, scales):
    if family == "thin":
        for length, width, turned in ((8, 0.4, False), (16, 0.8, False), (12, 0.6, True)):
            yield f"thin-{length}-{width}-{'y' if turned else 'x'}.step", [box((length, width, 0.8), quarter_turn=turned)], {"family": "thin", "length_mm": length, "width_mm": width}
    elif family == "roots":
        for shifted in (False, True):
            bodies = [box((12, 8, 0.8)), box((2, 8, 2.4), (0, 0, 0.8)), box((10, 8, 0.8), (0, 0, 3.2))]
            if shifted:
                bodies.append(box((3, 3, 0.8), (18.25, 1.75, 0)))
            yield f"roots-{'island' if shifted else 'ordinary'}.step", bodies, {"family": "roots", "requires_part_landing": True}
    elif family == "bodies":
        for count in scales:
            if count < 1:
                raise ValueError("body counts must be positive")
            bodies = [box((2, 2, 0.8), ((index % 8) * 4, (index // 8) * 4, 0)) for index in range(count)]
            yield f"bodies-{count}.step", bodies, {"family": "bodies", "scale_axis": "independent_bodies", "scale": count}
    elif family == "height":
        for count in scales:
            if count < 1:
                raise ValueError("height scales must be positive")
            yield f"height-{count}.step", [box((4, 4, count * 0.8))], {"family": "height", "scale_axis": "height_mm", "scale": count * 0.8}
    else:
        raise ValueError(f"unknown family: {family}")


def generate(destination, families, scales):
    if not families or len(families) != len(set(families)) or not scales or len(scales) != len(set(scales)) or any(scale < 1 for scale in scales):
        raise ValueError("families and positive scales must be nonempty and unique")
    root = Path(destination)
    root.mkdir()
    (root / "step").mkdir()
    parts = []
    for family in families:
        for name, bodies, parameters in cases(family, scales):
            data, boxes = step(bodies)
            (root / "step" / name).write_bytes(data)
            parts.append({"file": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
                          "expectation": {"kind": "complete"}, "parameters": parameters,
                          "oracle": {"boxes": boxes, "occurrences": len(boxes)}})
    manifest = {"recipe": "axis-aligned box dimensions, optional quarter turn, and disjoint STEP entity namespaces",
                "template": {"path": "fixtures/geometry/box-mm.step", "sha256": audit.digest(TEMPLATE)},
                "license": "OSL-3.0", "parts": parts}
    (root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination")
    parser.add_argument("--family", choices=("thin", "roots", "bodies", "height"), action="append")
    parser.add_argument("--scales", type=int, nargs="+", default=[1, 4, 16])
    args = parser.parse_args()
    manifest = generate(args.destination, args.family or ["thin", "roots"], args.scales)
    print(json.dumps({"manifest": str(Path(args.destination, "manifest.json")), "cases": len(manifest["parts"])}))


if __name__ == "__main__":
    main()
