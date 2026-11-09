# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Render verified actual Vault infill roads, not proposed geometry or strength.

Tooling-only numpy/matplotlib in an isolated prefix. Run after the construction
campaign; keep the original roads, producer receipt and result next to each plot.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.collections import LineCollection, PatchCollection
from matplotlib.patches import Rectangle
import numpy as np

BLUE, GOLD, WALL = "#579fea", "#f4b454", "#8091a5"


def is_web(road):
    return isinstance(road["owner"], int) and road["kind"] in (5, 8)


def segment_key(road):
    a, b = sorted((tuple(road["start"][:2]), tuple(road["end"][:2])))
    return a, b


def render(folder):
    replay = json.loads((folder / "roads.json").read_text())
    result = json.loads((folder / "result.json").read_text())
    receipt = json.loads((folder / "return.stdout.json").read_text())
    recipe = receipt["manufacturing"]["intent"]["recipe"]
    if recipe["sparse_infill"]["kind"] != "web_roof":
        raise ValueError("Vault infill report does not render ordinary-pattern controls")
    if not replay["report"]["verified"] or result["status"] != "accepted":
        raise ValueError("fresh verified replay and accepted journey required")
    roads = replay["roads"]
    interior = [road for road in roads if is_web(road)]
    if not interior:
        raise ValueError("no actual regional web roads")
    zs = sorted({road["upper"] for road in interior})
    base = {segment_key(road) for road in interior if road["upper"] == zs[0]}
    xy = np.array(
        [point[:2] for road in roads for point in (road["start"], road["end"])]
    )
    lo, hi = xy.min(axis=0), xy.max(axis=0)

    def color(road):
        if not is_web(road):
            return WALL
        return BLUE if segment_key(road) in base else GOLD

    fig = plt.figure(figsize=(13, 8), layout="constrained")
    grid = fig.add_gridspec(2, 3)
    for column, index, label in [
        (0, 0, "Continuous webs"),
        (1, len(zs) // 2, "Roof growth"),
        (2, -1, "Roof contacts"),
    ]:
        z = zs[index]
        layer = next(road["layer"] for road in interior if road["upper"] == z)
        group = [road for road in roads if road["layer"] == layer]
        ax = fig.add_subplot(grid[0, column])
        for regional in (False, True):
            selected = [road for road in group if is_web(road) == regional]
            ax.add_collection(
                LineCollection(
                    [[road["start"][:2], road["end"][:2]] for road in selected],
                    colors=[color(road) for road in selected],
                    linewidths=[max(0.5, road["width"] * 3) for road in selected],
                    alpha=1 if regional else 0.3,
                )
            )
        ax.set(
            xlim=(lo[0] - 1, hi[0] + 1),
            ylim=(lo[1] - 1, hi[1] + 1),
            aspect="equal",
            xlabel="X (mm)",
            ylabel="Y (mm)",
        )
        ax.set_title(f"{label} · Z {z:g} mm", loc="left", weight="bold")

    # Place the section on an actual non-base roof row, crossing a hole when one
    # is present. This keeps the chosen plane meaningful rather than sampling a
    # visually convenient proposed surface.
    horizontal = [
        road
        for road in interior
        if road["start"][1] == road["end"][1] and segment_key(road) not in base
    ]
    candidates = sorted({road["start"][1] for road in horizontal})
    section = (
        min(candidates, key=lambda y: abs(y - (lo[1] + hi[1]) / 2))
        if candidates
        else float((lo[1] + hi[1]) / 2)
    )
    ax = fig.add_subplot(grid[1, :2])
    patches, colors = [], []
    for road in roads:
        a, b, width = road["start"], road["end"], road["width"]
        if abs(a[1] - b[1]) < 1e-9:
            if abs(a[1] - section) > width / 2:
                continue
            left, right = sorted((a[0], b[0]))
            left, right = left - width / 2, right + width / 2
        elif min(a[1], b[1]) <= section <= max(a[1], b[1]):
            x = a[0] + (section - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
            left, right = x - width / 2, x + width / 2
        else:
            continue
        patches.append(
            Rectangle(
                (left, road["lower"]), right - left, road["upper"] - road["lower"]
            )
        )
        colors.append(color(road))
    ax.add_collection(
        PatchCollection(patches, facecolors=colors, edgecolors="none", alpha=0.9)
    )
    ax.set(
        xlim=(lo[0] - 1, hi[0] + 1),
        ylim=(0, max(road["upper"] for road in roads) + 0.2),
        aspect="equal",
        xlabel="X (mm)",
        ylabel="Z (mm)",
    )
    ax.set_title(f"Actual section · Y {section:g} mm", loc="left", weight="bold")
    ax = fig.add_subplot(grid[1, 2])
    ax.axis("off")
    spacing = result["roof_spacing_mm"]
    details = [
        f"Base webs   {100 * result['fraction']:g}%",
        f"Roof spacing   {'Auto' if spacing is None else f'{spacing:g} mm'}",
        f"Bead   {result['width_mm']:g} mm",
        f"Outer walls   {result['wall_count']}",
        f"Layer   {result['layer_height_mm']:g} mm",
        f"Material   {result['nominal_material_mm3']:,.0f} mm³",
        "",
        "Compile → fresh replay → save",
        "Remove external inputs → reopen",
        "Fresh replay → identical export",
        "",
        "Nominal bead envelopes",
        "Strength and physical print unqualified",
    ]
    ax.text(0, 0.98, "\n".join(details), va="top", linespacing=1.7)
    title = result["case"].replace("-", " ").title()
    fig.suptitle(f"Vault infill · {title}", fontsize=20, weight="bold", ha="left", x=0.02)
    fig.supxlabel(
        "Blue: continuous webs and local roots    Gold: retained roof growth    Gray: walls and skins",
        fontsize=11,
    )
    output = folder / "actual-web-roof.png"
    fig.savefig(output, dpi=160)
    plt.close(fig)
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("campaign", type=Path)
    args = parser.parse_args()
    plt.rcParams.update(
        {
            "font.family": "DejaVu Sans",
            "font.size": 10,
            "figure.facecolor": "#141c27",
            "axes.facecolor": "#141c27",
            "text.color": "#edf2f8",
            "axes.labelcolor": "#b0bfd0",
            "axes.edgecolor": "#536277",
            "xtick.color": "#b0bfd0",
            "ytick.color": "#b0bfd0",
        }
    )
    for folder in sorted(args.campaign.iterdir()):
        result = folder / "result.json"
        if result.is_file() and json.loads(result.read_text())["status"] == "accepted":
            print(render(folder))


if __name__ == "__main__":
    main()
