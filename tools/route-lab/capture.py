# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Capture frozen route fixtures from actual slicer layers.

`slicer` (default) builds project-authored parametric shapes, slices them with an
installed PrusaSlicer, and converts selected layers' extrusion moves. `native`
converts retained Spiling captures (for example Cathedral infill campaigns):
paths come from the exported plan, ordered and checked against the exported
program's own extrusion runs, so the control is Spiling's emitted order.

Builds project-authored parametric shapes, slices them with an installed
PrusaSlicer, and converts selected layers' extrusion moves into stroke text
for the Rust matrix. Stroke order is the slicer's own print order, so the
frozen input-order control approximates that slicer's sequencing. This tool
proposes no route and is not run during trials; regenerate only to start a
new cohort. Captured G-code is nominal slicer output, not Spiling deposition.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import random
import re
import subprocess
import tempfile

OUT = Path(__file__).resolve().parents[2] / 'crates/deposition-routing/examples/matrix/captured'
LOCKED = ('Perimeter', 'External perimeter', 'Overhang perimeter')
SLICER = ['--export-gcode', '--layer-height', '0.2', '--first-layer-height', '0.2',
          '--nozzle-diameter', '0.4', '--fill-density', '20%', '--fill-pattern', 'grid',
          '--perimeters', '2', '--skirts', '0', '--use-relative-e-distances', '--center', '60,60']


def heightmap_stl(heights, cell):
    """Watertight extrusion of a cell grid; every wall splits at every level."""
    nx, ny = len(heights), len(heights[0])
    levels = sorted({0.} | {h for column in heights for h in column})
    at = lambda i, j: heights[i][j] if 0 <= i < nx and 0 <= j < ny else 0.
    triangles = []

    def quad(a, b, c, d):
        triangles.extend([(a, b, c), (a, c, d)])
    for i in range(nx):
        for j in range(ny):
            h = at(i, j)
            if h <= 0:
                continue
            x0, x1, y0, y1 = i*cell, (i+1)*cell, j*cell, (j+1)*cell
            quad((x0, y0, h), (x1, y0, h), (x1, y1, h), (x0, y1, h))
            quad((x0, y0, 0.), (x0, y1, 0.), (x1, y1, 0.), (x1, y0, 0.))
            for (di, dj, p, q) in ((1, 0, (x1, y0), (x1, y1)), (-1, 0, (x0, y1), (x0, y0)),
                                   (0, 1, (x1, y1), (x0, y1)), (0, -1, (x0, y0), (x1, y0))):
                other = at(i+di, j+dj)
                for low, high in zip(levels, levels[1:]):
                    if low >= other and high <= h:
                        quad((*p, low), (*q, low), (*q, high), (*p, high))
    lines = ['solid shape']
    for triangle in triangles:
        lines.append('facet normal 0 0 0\nouter loop')
        lines.extend('vertex %.4f %.4f %.4f' % vertex for vertex in triangle)
        lines.append('endloop\nendfacet')
    return '\n'.join(lines) + '\nendsolid shape\n'


def plate(nx, ny, height, holes=()):
    grid = [[height]*ny for _ in range(nx)]
    for (i0, j0, i1, j1) in holes:
        for i in range(i0, i1):
            for j in range(j0, j1):
                grid[i][j] = 0.
    return grid


def grille(columns, rows, hole, rib):
    pitch = hole + rib
    return plate(columns*pitch + rib, rows*pitch + rib, 4.,
                 [(rib + c*pitch, rib + r*pitch, rib + c*pitch + hole, rib + r*pitch + hole)
                  for c in range(columns) for r in range(rows)])


def pins(columns, rows, size, gap, height):
    grid = plate(columns*(size+gap), rows*(size+gap), 0.)
    for c in range(columns):
        for r in range(rows):
            for i in range(size):
                for j in range(size):
                    grid[c*(size+gap)+i][r*(size+gap)+j] = height
    return grid


def code_plate(modules, seed):
    rng = random.Random(seed)
    return [[1.6 if 0 < i < modules-1 and 0 < j < modules-1 and rng.random() < 0.45 else 1.0
             for j in range(modules)] for i in range(modules)]


def fins(count, length, base):
    grid = plate(count*4 + 2, length, base)
    for fin in range(count):
        for j in range(1, length-1):
            grid[2 + fin*4][j] = base + 2.4
            grid[3 + fin*4][j] = base + 2.4
    return grid


# name -> (heightmap, cell mm, selected layers). One layer is a single band;
# several consecutive layers form a physical stack with derived prerequisites.
SHAPES = {
    'block': (plate(20, 20, 4.), 1., [10]),
    'grille': (grille(5, 5, 4, 3), 1., [10]),
    'grille-wide': (grille(7, 4, 3, 4), 1., [10]),
    'grille-top': (grille(4, 4, 4, 3), 1., [19]),
    'grille-top-wide': (grille(5, 3, 3, 4), 1., [19]),
    'bracket': (plate(44, 30, 4., [(5, 5, 10, 10), (34, 5, 39, 10), (5, 20, 10, 25),
                                   (34, 20, 39, 25), (16, 12, 28, 18)]), 1., [10]),
    'bracket-long': (plate(56, 24, 4., [(4, 4, 8, 8), (48, 4, 52, 8), (4, 16, 8, 20), (48, 16, 52, 20),
                                        (14, 9, 22, 15), (26, 9, 30, 15), (34, 9, 42, 15)]), 1., [10]),
    'pins': (pins(6, 6, 2, 3, 4.), 1., [10]),
    'pins-wide': (pins(8, 4, 3, 3, 4.), 1., [10]),
    'pin-stack': (pins(4, 4, 2, 3, 4.), 1., [8, 9, 10, 11]),
    'pin-stack-wide': (pins(5, 3, 3, 2, 4.), 1., [8, 9, 10, 11]),
    'code': (code_plate(17, 7), 1.5, [6]),
    'code-other': (code_plate(15, 23), 1.5, [6]),
    'fins': (fins(8, 30, 0.6), 1., [6]),
    'fins-many': (fins(12, 20, 0.6), 1., [6]),
}


def strokes(gcode, layers):
    """Maximal uninterrupted extrusion runs of one role/height; widths stay per edge."""
    result, layer, x, y, z = [], -1, None, None, 0.
    role, width, height, current = '', 0., 0., None
    for line in gcode.splitlines():
        if line.startswith(';LAYER_CHANGE'):
            layer += 1
            current = None
        elif line.startswith(';TYPE:'):
            role, current = line[6:].strip(), None
        elif line.startswith(';WIDTH:'):
            width = float(line[7:])
        elif line.startswith(';HEIGHT:'):
            height, current = float(line[8:]), None
        elif line.startswith(';Z:'):
            z = float(line[3:])
        elif line.startswith('G1 '):
            words = {w[0]: float(w[1:]) for w in line.split(';')[0].split()[1:] if re.fullmatch(r'[XYZEF]-?[\d.]+', w)}
            nx, ny = words.get('X', x), words.get('Y', y)
            moved = (nx, ny) != (x, y)
            if moved and words.get('E', 0.) > 0 and x is not None and layer in layers:
                if current is None:
                    current = dict(z=z, height=height, widths=[], role=role, points=[(x, y)])
                    result.append(current)
                current['points'].append((nx, ny))
                current['widths'].append(width)
            elif moved or 'E' in words:
                current = None
            x, y = nx, ny
    if any(not s['role'] or min(s['widths']) <= s['height'] or s['height'] <= 0 for s in result):
        raise ValueError('captured extrusion outside the declared stroke model')
    return result


def bounds(stroke):
    xs, ys = zip(*stroke['points'])
    half = max(stroke['widths'])/2
    return min(xs)-half, min(ys)-half, max(xs)+half, max(ys)+half


def fixture(header, items, locked=LOCKED):
    lines = [f'# {header}',
             '# S id z_low z_high locked role | P x y width_of_arriving_edge | R id prerequisite...']
    for index, s in enumerate(items):
        role = s['role'].replace(' ', '_')
        lines.append('S %d %.3f %.3f %d %s' % (index, s['z']-s['height'], s['z'], s['role'] in locked, role))
        lines.extend('P %.3f %.3f %.6f' % (*p, w) for p, w in zip(s['points'], [0.] + s['widths']))
        # Declared dependency: overlapping material in the band directly below.
        a = bounds(s)
        below = [j for j, o in enumerate(items) if abs(o['z']-(s['z']-s['height'])) < 1e-6
                 and not (bounds(o)[2] <= a[0] or bounds(o)[0] >= a[2] or bounds(o)[3] <= a[1] or bounds(o)[1] >= a[3])]
        if below:
            lines.append('R %d %s' % (index, ' '.join(map(str, below))))
    return '\n'.join(lines) + '\n'


# Native Spiling roles. Walls keep their emitted direction and seam; bridges
# keep their anchor-to-anchor direction; fill and skins may reverse.
NATIVE_LOCKED = ('perimeter', 'bridge')
# name -> (capture directory below --native-root, consecutive plan layer indexes).
NATIVE = {
    'cathedral-web': ('four-holes-0.16-auto', [20]),
    'cathedral-web-round': ('round-0.08-auto', [20]),
    'cathedral-roof': ('four-holes-0.16-auto', [32]),
    'cathedral-roof-neck': ('narrow-neck-0.16-auto', [44]),
    'cathedral-roof-u': ('u-shape-0.16-auto', [29]),
    'cathedral-roof-l': ('l-shape-0.16-auto', [29]),
    'cathedral-roof-round': ('round-0.16-auto', [29]),
    'cathedral-roof-annulus': ('annulus-0.08-auto', [30]),
    'cathedral-stack': ('four-holes-0.16-auto', [30, 31, 32, 33]),
    'cathedral-stack-u': ('u-shape-0.16-auto', [28, 29, 30, 31]),
}


def program_runs(gcode, z):
    """Start points of uninterrupted extrusion runs at one commanded Z."""
    runs, x, y, cz, extruding = [], None, None, None, False
    for line in gcode.splitlines():
        if not line.startswith(('G1 ', 'G0 ')):
            continue
        words = {w[0]: float(w[1:]) for w in line.split(';')[0].split()[1:] if re.fullmatch(r'[XYZEF]-?[\d.]+', w)}
        nx, ny, cz = words.get('X', x), words.get('Y', y), words.get('Z', cz)
        if (nx, ny) != (x, y):
            now = words.get('E', 0.) > 0
            if now and not extruding and cz is not None and abs(cz - z) < 1e-6:
                runs.append((round(x, 3), round(y, 3)))
            extruding = now
        x, y = nx, ny
    return runs


def native(capture, layers):
    plan = json.loads((capture/'export/plan.json').read_text())
    gcode = (capture/'export/program.gcode').read_text()
    items = []
    for index in layers:
        layer = plan['layers'][index]
        paths = layer['paths']
        starts = [tuple(round(c, 3) for c in path['points_mm'][0][:2]) for path in paths]
        if program_runs(gcode, layer['z_mm']) != starts:
            raise ValueError(f'{capture.name} layer {index}: plan order differs from emitted program')
        for path in paths:
            points = [tuple(p[:2]) for p in path['points_mm']]
            width = path['width']
            widths = width['widths_mm'] if width['kind'] == 'per_edge' else [width['width_mm']]*(len(points)-1)
            low, high = path['physical_z_mm']
            items.append(dict(z=high, height=high-low, widths=widths, role=path['kind'], points=points))
    return items


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', nargs='?', choices=('slicer', 'native'), default='slicer')
    parser.add_argument('--slicer', default='prusa-slicer')
    parser.add_argument('--native-root', type=Path, help='retained Cathedral campaign pressure directory')
    parser.add_argument('--output', type=Path, default=OUT)
    args = parser.parse_args()
    if args.source == 'native':
        if args.native_root is None:
            parser.error('native capture requires --native-root')
        args.output.mkdir(parents=True, exist_ok=True)
        producer = json.loads((args.native_root/'producer.json').read_text())
        for name, (case, layers) in NATIVE.items():
            items = native(args.native_root/case, layers)
            header = f"{name}: Spiling {case} plan layers {layers}; engine sha256 {producer['spiling-engine']}"
            (args.output/f'{name}.txt').write_text(fixture(header, items, NATIVE_LOCKED))
            print(name, len(items), 'strokes', sum(len(s['points'])-1 for s in items), 'edges')
        return
    version = subprocess.run([args.slicer, '--help'], capture_output=True, text=True).stdout.splitlines()[0].split()[0].removeprefix('PrusaSlicer-')
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as temporary:
        for name, (heights, cell, layers) in SHAPES.items():
            stl, gcode = Path(temporary)/f'{name}.stl', Path(temporary)/f'{name}.gcode'
            stl.write_text(heightmap_stl(heights, cell))
            subprocess.run([args.slicer, *SLICER, '-o', str(gcode), str(stl)], check=True, capture_output=True, timeout=300)
            items = strokes(gcode.read_text(), layers)
            if not items:
                raise ValueError(f'{name}: no extrusion captured')
            (args.output/f'{name}.txt').write_text(fixture(f'{name}: PrusaSlicer {version}; ' + ' '.join(SLICER), items))
            print(name, len(items), 'strokes', sum(len(s['points'])-1 for s in items), 'edges')


if __name__ == '__main__':
    main()
