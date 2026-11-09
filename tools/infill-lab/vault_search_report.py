# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Auditable search Pareto plot and nominal-road galleries, never strength claims."""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path

from vault_search import frontier, geometric_mean, native_rows, write


def report(archive, captures, output):
    data = json.loads(archive.read_text())
    native = {}
    for root in captures:
        _, rows = native_rows(root)
        for row in rows:
            native[row["id"]] = (root, row)
    rows = data["rows"]
    programs = {}
    for row in rows:
        root, construction = native[row["capture_id"]]
        if construction["candidate"] != row["candidate"]:
            raise ValueError("archive candidate differs from capture")
        if construction["status"] == "ok":
            programs.setdefault(construction["replay_sha256"], []).append(
                row["capture_id"]
            )
        if row["status"] == "ok" and (
            construction["status"] != "ok"
            or row["roads_sha256"] != construction["roads_sha256"]
            or row["volume_mm3"] != construction["volume_mm3"]
        ):
            raise ValueError("archive dose/roads differ from capture")
    valid = [r for r in rows if r["status"] == "ok"]
    solids = [r for r in valid if r["candidate"]["fraction"] == 1]
    baselines = [
        r
        for r in rows
        if r["candidate"]["pattern"] == "web_roof"
        and r["candidate"]["fraction"] == 0.1
        and "roof_spacing_mm" not in r["candidate"]
    ]
    if (
        len(solids) != 1
        or len(baselines) != 1
        or len({r["cohort"] for r in valid}) != 1
    ):
        raise ValueError(
            "one coherent cohort, solid and default Vault baseline required"
        )
    solid = solids[0]
    baseline = baselines[0] if baselines[0]["status"] == "ok" else None
    pareto_ids = {r["capture_id"] for r in frontier(rows)}
    for r in valid:
        r["relative_responses"] = {
            a: r["stiffness_n_mm"][a] / solid["stiffness_n_mm"][a]
            for a in r["stiffness_n_mm"]
        }
        r["geometric_mean_vs_solid"] = geometric_mean(
            list(r["relative_responses"].values())
        )
        r["material_vs_solid"] = r["volume_mm3"] / solid["volume_mm3"]
        r["motion_vs_solid"] = r["motion_s"] / solid["motion_s"]
        r["provisional_pareto"] = r["capture_id"] in pareto_ids
    by_id = {r["capture_id"]: r for r in rows}
    pairs = []
    for child in valid:
        parent = by_id.get(child.get("parent"))
        if parent is not None and parent["status"] == "ok":
            pairs.append(
                dict(
                    parent=parent["capture_id"],
                    child=child["capture_id"],
                    material_ratio=child["volume_mm3"] / parent["volume_mm3"],
                    motion_ratio=child["motion_s"] / parent["motion_s"],
                    xyz_ratio={
                        a: child["stiffness_n_mm"][a] / parent["stiffness_n_mm"][a]
                        if parent["stiffness_n_mm"][a]
                        else None
                        for a in child["stiffness_n_mm"]
                    },
                )
            )
    vault = [r for r in valid if r["candidate"]["pattern"] == "web_roof"]
    # Contrasting candidates: default, least material, highest efficiency, strongest
    # local response. This is gallery/validation selection, not a product preset.
    selected = list(
        {
            r["capture_id"]: r
            for r in [
                *([baseline] if baseline is not None else []),
                min(vault, key=lambda r: r["volume_mm3"]),
                max(
                    vault,
                    key=lambda r: r["geometric_mean_vs_solid"] / r["material_vs_solid"],
                ),
                max(vault, key=lambda r: r["geometric_mean_vs_solid"]),
            ]
        }.values()
    )
    output.mkdir(parents=True, exist_ok=False)
    write(
        output / "report.json",
        dict(
            scope=data["scope"],
            native_programs=programs,
            rows=rows,
            pairs=pairs,
            frontier=sorted(pareto_ids),
            gallery=[r["capture_id"] for r in selected],
            baseline=baselines[0]["capture_id"],
            baseline_status=baselines[0]["status"],
            solid=solid["capture_id"],
        ),
    )
    native_refusals = sum(r.get("stage") == "native" for r in rows)
    plot(
        valid,
        baseline,
        output,
        total=len(rows),
        native_refusals=native_refusals,
        program_count=len(programs),
    )
    gallery(selected, native, output)
    print(
        json.dumps(
            dict(
                rows=len(rows),
                distinct_native_programs=len(programs),
                admitted=len(valid),
                vault=len(vault),
                frontier=len(pareto_ids),
                pairs=len(pairs),
            )
        )
    )


def style():
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    plt.rcParams.update(
        {
            "figure.facecolor": "#141c27",
            "axes.facecolor": "#141c27",
            "text.color": "#edf2f8",
            "axes.labelcolor": "#b0bfd0",
            "axes.edgecolor": "#536277",
            "xtick.color": "#b0bfd0",
            "ytick.color": "#b0bfd0",
            "font.size": 10,
        }
    )
    return plt


def label(r):
    c = r["candidate"]
    spacing = c.get("roof_spacing_mm")
    roof = f"{spacing:g} mm roof" if spacing is not None else "Auto roof"
    return f"{100 * c['fraction']:g}% webs · {roof}"


def plot(rows, baseline, output, *, total, native_refusals, program_count):
    plt = style()
    fig, axes = plt.subplots(1, 2, figsize=(14, 6), layout="constrained")
    for pattern, title, color, marker in [
        ("rectilinear", "Rectilinear", "#8aa1c0", "s"),
        ("grid", "Grid", "#72d3a2", "D"),
        ("web_roof", "Vault", "#eda85b", "o"),
    ]:
        group = [
            r
            for r in rows
            if r["candidate"]["pattern"] == pattern and r["candidate"]["fraction"] != 1
        ]
        for ax, y, ylabel in [
            (axes[0], "material_vs_solid", "Whole-cube material / solid (%)"),
            (axes[1], "motion_vs_solid", "Whole-cube motion bound / solid (%)"),
        ]:
            ax.scatter(
                [100 * r["geometric_mean_vs_solid"] for r in group],
                [100 * r[y] for r in group],
                label=title,
                c=color,
                marker=marker,
                alpha=0.75,
                s=[70 if r["provisional_pareto"] else 28 for r in group],
                edgecolors=[
                    "white" if r["provisional_pareto"] else color for r in group
                ],
                linewidths=0.6,
            )
            # Only connect the fixed ordinary density controls. Different Vault
            # roof settings are separate constructions, not one interpolated curve.
            if pattern != "web_roof":
                ordered = sorted(group, key=lambda r: r["candidate"]["fraction"])
                ax.plot(
                    [100 * r["geometric_mean_vs_solid"] for r in ordered],
                    [100 * r[y] for r in ordered],
                    color=color,
                    alpha=0.4,
                    linewidth=1,
                )
                for r in ordered:
                    ax.annotate(
                        f"{100 * r['candidate']['fraction']:g}%",
                        (100 * r["geometric_mean_vs_solid"], 100 * r[y]),
                        xytext=(4, 4),
                        textcoords="offset points",
                        fontsize=7,
                        color=color,
                    )
            ax.set(
                xlabel="Roof + core elastic response / solid · geometric mean (%)",
                ylabel=ylabel,
            )
            ax.grid(alpha=0.12)
    for ax, y in zip(axes, ("material_vs_solid", "motion_vs_solid"), strict=True):
        if baseline is not None:
            ax.scatter(
                [100 * baseline["geometric_mean_vs_solid"]],
                [100 * baseline[y]],
                marker="*",
                color="#faecbb",
                s=190,
                label="Vault 10% / Auto",
            )
        else:
            ax.text(
                0.02,
                0.98,
                "Vault 10% / Auto: incomplete mechanics",
                transform=ax.transAxes,
                va="top",
                fontsize=8,
            )
        ax.legend(fontsize=8)
    fig.suptitle(
        "Vault autoresearch · 48 mm cube / finite roof + core patches",
        fontsize=17,
        weight="bold",
    )
    fig.supxlabel(
        "Right and down is better within this fixed local elasticity model. White outline: provisional multi-axis Pareto among scored settings.\n"
        f"{total} settings / {program_count} distinct native programs; {len(rows)} scored (solid off-plot); {native_refusals} native refusal(s); "
        f"{total - len(rows) - native_refusals} incomplete mechanics.\n"
        "Whole-object costs; nominal bonded footprints, weak-Z 0.25. Not strength or full-part qualification.",
        fontsize=10,
    )
    fig.savefig(output / "pareto.png", dpi=170)
    plt.close(fig)


def gallery(selected, native, output):
    from matplotlib.collections import PatchCollection
    from matplotlib.patches import Rectangle
    from shapely.geometry import LineString

    plt = style()
    fig, axes = plt.subplots(
        len(selected),
        2,
        figsize=(13, 4.5 * len(selected)),
        squeeze=False,
        layout="constrained",
    )
    for pair, row in zip(axes, selected, strict=True):
        root, construction = native[row["capture_id"]]
        data = json.loads((root / construction["native"] / "roads.json").read_text())
        roads = data["roads"]
        z = data["size_mm"][2]
        # A fixed slightly off-center section is shared by every candidate.
        y = 44.3
        section = LineString([(19, y), (69, y)])
        ax = pair[0]
        rectangles, colors = [], []
        base = {
            tuple(sorted((tuple(r["start"][:2]), tuple(r["end"][:2]))))
            for r in roads
            if r["kind"] == 5 and r["layer"] == 4
        }
        for road in roads:
            if road["upper"] < z - 12:
                continue
            footprint = LineString([road["start"][:2], road["end"][:2]]).buffer(
                road["width"] / 2, quad_segs=32
            )
            cut = footprint.intersection(section)
            if cut.is_empty:
                continue
            key = tuple(sorted((tuple(road["start"][:2]), tuple(road["end"][:2]))))
            regional = type(road["owner"]) is int and road["kind"] in (5, 8)
            color = ("#579fea" if key in base else "#f4b454") if regional else "#8091a5"
            for piece in cut.geoms if cut.geom_type == "MultiLineString" else [cut]:
                lo, _, hi, _ = piece.bounds
                rectangles.append(
                    Rectangle(
                        (lo, road["lower"]), hi - lo, road["upper"] - road["lower"]
                    )
                )
                colors.append(color)
        ax.add_collection(
            PatchCollection(rectangles, facecolors=colors, edgecolors="none")
        )
        ax.set(
            xlim=(19.5, 68.5),
            ylim=(z - 12, z + 0.5),
            aspect="equal",
            xlabel="X (mm)",
            ylabel="Z (mm)",
            title=f"Vault {label(row)} · actual top section / Y {y:g}",
        )
        ax = pair[1]
        near = min({r["upper"] for r in roads}, key=lambda h: abs(h - (z - 1.8)))
        polygons, colors = [], []
        from matplotlib.patches import Polygon

        for road in roads:
            if not math.isclose(road["upper"], near, abs_tol=1e-8):
                continue
            footprint = LineString([road["start"][:2], road["end"][:2]]).buffer(
                road["width"] / 2, quad_segs=32
            )
            polygons.append(Polygon(list(footprint.exterior.coords)))
            regional = type(road["owner"]) is int and road["kind"] in (5, 8)
            key = tuple(sorted((tuple(road["start"][:2]), tuple(road["end"][:2]))))
            colors.append(
                ("#579fea" if key in base else "#f4b454") if regional else "#8091a5"
            )
        ax.add_collection(
            PatchCollection(polygons, facecolors=colors, edgecolors="none")
        )
        ax.add_patch(
            Rectangle(
                tuple(row["scenario"]["bounds"][0][:2]),
                row["scenario"]["bounds"][1][0] - row["scenario"]["bounds"][0][0],
                row["scenario"]["bounds"][1][1] - row["scenario"]["bounds"][0][1],
                fill=False,
                edgecolor="#eff2f9",
                linewidth=1,
                linestyle="--",
            )
        )
        ax.set(
            xlim=(19, 69),
            ylim=(19, 69),
            aspect="equal",
            xlabel="X (mm)",
            ylabel="Y (mm)",
            title=f"Z {near:g} mm · {row['volume_mm3']:,.0f} mm³ whole-cube dose",
        )
    fig.suptitle(
        "Verified Vault deposition · contrasting research candidates",
        fontsize=17,
        weight="bold",
    )
    fig.supxlabel(
        "Blue: persistent web lanes · Gold: roof growth · Gray: walls/skins · Dashed: test patch\nNominal bead envelopes from actual replay; no simulated deformation or strength claim.",
        fontsize=10,
    )
    fig.savefig(output / "sections.png", dpi=150)
    plt.close(fig)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--archive", type=Path, required=True)
    p.add_argument("--captures", type=Path, nargs="+", required=True)
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    report(args.archive, args.captures, args.output)


if __name__ == "__main__":
    main()
