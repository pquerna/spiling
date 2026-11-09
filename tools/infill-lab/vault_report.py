# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Audit and plot matched production constructions without inventing strength scores."""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path

from model import digest, file_hash
from remote.coupon import costs, interior_roads


def segment_key(road):
    return tuple(sorted((tuple(road["start"][:2]), tuple(road["end"][:2]))))


NAMES = {"rectilinear": "Rectilinear", "grid": "Grid", "web_roof": "Vault"}
COLORS = {"rectilinear": "#9da9ba", "grid": "#579fea", "web_roof": "#f4b454"}


def audit(root):
    capture = json.loads((root / "capture.json").read_text())
    if capture["schema"] != 1:
        raise ValueError("unknown native capture schema")
    expected = {
        digest(dict(candidate=c, specimen=s, identity=capture["identity"]))
        for s in capture["specimens"]
        for c in capture["candidates"]
    }
    rows, seen = [], set()
    for line in (root / "captures.jsonl").read_text().splitlines():
        row = json.loads(line)
        key = digest(
            dict(
                candidate=row["candidate"],
                specimen=row["specimen"],
                identity=capture["identity"],
            )
        )
        if key != row["id"] or key not in expected or key in seen:
            raise ValueError("unexpected, changed or duplicate native capture")
        seen.add(key)
        if row["status"] != "ok":
            rows.append(row)
            continue
        folder = (root / row["native"]).resolve()
        if not folder.is_relative_to(root.resolve() / "native"):
            raise ValueError("native capture escapes study")
        manifest = json.loads((folder / "manifest.json").read_text())
        if set(manifest) != {
            "roads.json",
            "actual.splr",
            "receipt.json",
            "project.spil",
        }:
            raise ValueError("incomplete native closure")
        if any(file_hash(folder / p) != h for p, h in manifest.items()):
            raise ValueError("native capture hash differs")
        data = json.loads((folder / "roads.json").read_text())
        if (
            not data["software_only"]
            or not data["report"]["verified"]
            or data["candidate"] != row["candidate"]
            or data["source_sha256"] != row["source_sha256"]
            or data["bin_sha256"] != capture["identity"]["binaries"]
            or manifest["roads.json"] != row["roads_sha256"]
            or manifest["actual.splr"] != row["replay_sha256"]
            or data["report"]["deposited_volume_mm3"] != row["volume_mm3"]
        ):
            raise ValueError("native receipt/source/producer differs")
        dose = costs(data)
        motion = data["motion_seconds_lower_bound"]
        if not math.isfinite(motion) or motion <= 0:
            raise ValueError("positive finite motion lower bound required")
        rows.append(
            dict(row, roles=dose, motion_s=motion, compile_s=data["compile_wall_s"])
        )
    if seen != expected:
        raise ValueError(f"native panel incomplete: {len(seen)}/{len(expected)}")
    for s in capture["specimens"]:
        group = [r for r in rows if r["specimen"] == s and r["status"] == "ok"]
        solids = [r for r in group if r["candidate"]["fraction"] == 1]
        if len(solids) != 1:
            raise ValueError("one admitted solid control per source required")
        for row in group:
            row["material_vs_solid"] = row["volume_mm3"] / solids[0]["volume_mm3"]
            row["motion_vs_solid"] = row["motion_s"] / solids[0]["motion_s"]
            if row["candidate"]["pattern"] == "web_roof":
                peers = [
                    r
                    for r in group
                    if r["candidate"]["pattern"] == "grid"
                    and r["candidate"]["fraction"] == row["candidate"]["fraction"]
                ]
                if len(peers) != 1:
                    raise ValueError("matched admitted Grid control required")
                row["material_vs_grid"] = row["volume_mm3"] / peers[0]["volume_mm3"]
                row["motion_vs_grid"] = row["motion_s"] / peers[0]["motion_s"]
    return dict(
        scope="verified_native_construction_costs_not_mechanical_frontier",
        capture_sha256=file_hash(root / "capture.json"),
        identity=capture["identity"],
        specimens=capture["specimens"],
        rows=rows,
        mechanical_ranking="blocked_pending_numerical_and_whole_part_qualification",
        time_scope="decoded_motion_lower_bound_excludes_thermal_acceleration_and_firmware",
    )


def plots(root, output, report):
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from matplotlib.collections import PatchCollection
    from matplotlib.patches import Rectangle
    from shapely.geometry import LineString

    plt.rcParams.update(
        {
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
    fig, axes = plt.subplots(2, 3, figsize=(15, 10), layout="constrained")
    for ax, name in zip(axes.flat, report["specimens"], strict=True):
        group = [
            r for r in report["rows"] if r["specimen"] == name and r["status"] == "ok"
        ]
        for pattern, label in NAMES.items():
            curve = sorted(
                [
                    r
                    for r in group
                    if r["candidate"]["pattern"] == pattern
                    and r["candidate"]["fraction"] != 1
                ],
                key=lambda r: r["candidate"]["fraction"],
            )
            ax.plot(
                [100 * r["material_vs_solid"] for r in curve],
                [100 * r["motion_vs_solid"] for r in curve],
                "o-",
                color=COLORS[pattern],
                label=label,
                alpha=0.85,
            )
            for r in curve:
                ax.annotate(
                    f"{100 * r['candidate']['fraction']:g}%",
                    (100 * r["material_vs_solid"], 100 * r["motion_vs_solid"]),
                    xytext=(4, 5),
                    textcoords="offset points",
                    fontsize=7,
                    color=COLORS[pattern],
                )
        ax.plot([100], [100], "s", color="#e8edf5", label="Solid")
        ax.set(
            title=name,
            xlabel="Whole-object material / solid (%)",
            ylabel="Motion lower bound / solid (%)",
        )
        ax.grid(alpha=0.15)
    axes[0, 0].legend()
    fig.suptitle("Vault infill · matched larger-object construction costs", fontsize=18)
    fig.supxlabel(
        "Measured settings joined for navigation; no interpolation or strength claim. Lower and left use fewer resources.",
        fontsize=10,
    )
    fig.savefig(output / "construction-costs.png", dpi=160)
    plt.close(fig)

    fig, axes = plt.subplots(
        len(report["specimens"]), 3, figsize=(15, 17), layout="constrained"
    )
    for row_axes, name in zip(axes, report["specimens"], strict=True):
        group = [
            r
            for r in report["rows"]
            if r["specimen"] == name
            and r["status"] == "ok"
            and r["candidate"]["fraction"] == 0.1
        ]
        vault = next(
            (r for r in group if r["candidate"]["pattern"] == "web_roof"), None
        )
        if vault is None:
            for ax in row_axes:
                ax.text(0.1, 0.5, f"{name}: Vault refused", transform=ax.transAxes)
            continue
        vault_data = json.loads((root / vault["native"] / "roads.json").read_text())
        interior = interior_roads(vault_data)
        first = min(r["upper"] for r in interior)
        base = {segment_key(r) for r in interior if r["upper"] == first}
        choices = [
            r["start"][1]
            for r in interior
            if r["start"][1] == r["end"][1] and segment_key(r) not in base
        ]
        size = vault_data["size_mm"]
        y = (
            min(choices, key=lambda v: abs(v - 20 - size[1] / 2))
            if choices
            else 20 + size[1] / 2
        )
        plane = LineString([(19, y), (21 + size[0], y)])
        for ax, pattern in zip(row_axes, NAMES, strict=True):
            found = next(
                (r for r in group if r["candidate"]["pattern"] == pattern), None
            )
            if found is None:
                ax.text(0.1, 0.5, f"{NAMES[pattern]} refused", transform=ax.transAxes)
                continue
            data = json.loads((root / found["native"] / "roads.json").read_text())
            patches, colors = [], []
            for r in data["roads"]:
                if (
                    not min(r["start"][1], r["end"][1]) - r["width"] / 2
                    <= y
                    <= max(r["start"][1], r["end"][1]) + r["width"] / 2
                ):
                    continue
                cross = (
                    LineString([r["start"][:2], r["end"][:2]])
                    .buffer(r["width"] / 2, quad_segs=32)
                    .intersection(plane)
                )
                for segment in getattr(cross, "geoms", [cross]):
                    if segment.is_empty or segment.geom_type != "LineString":
                        continue
                    lo, _, hi, _ = segment.bounds
                    patches.append(
                        Rectangle(
                            (lo - 20, r["lower"]), hi - lo, r["upper"] - r["lower"]
                        )
                    )
                    key = segment_key(r)
                    colors.append(
                        "#f4b454"
                        if pattern == "web_roof"
                        and type(r["owner"]) is int
                        and r["kind"] in (5, 8)
                        and key not in base
                        else "#579fea"
                        if r["kind"] in (2, 5, 13)
                        or (r["kind"] == 8 and type(r["owner"]) is int)
                        else "#8091a5"
                    )
            ax.add_collection(
                PatchCollection(patches, facecolors=colors, edgecolors="none")
            )
            ax.set(
                xlim=(-0.5, size[0] + 0.5),
                ylim=(0, size[2] + 0.3),
                aspect="equal",
                xlabel="X (mm)",
                ylabel="Z (mm)",
            )
            ax.set_title(
                f"{name} · {NAMES[pattern]} · Y {y - 20:.2f} mm",
                loc="left",
                fontsize=10,
            )
    fig.suptitle(
        "Vault infill · actual-program sections at 10% base setting", fontsize=18
    )
    fig.supxlabel(
        "Nominal footprint envelopes, not physical bead shapes. Blue: interior · Gold: added Vault roof roads · Gray: walls/skins.",
        fontsize=10,
    )
    fig.savefig(output / "actual-sections.png", dpi=160)
    plt.close(fig)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("study", type=Path)
    p.add_argument("output", type=Path)
    args = p.parse_args()
    report = audit(args.study.resolve())
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "construction.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    plots(args.study, args.output, report)
    print(
        json.dumps(
            dict(
                cases=len(report["rows"]),
                scope=report["scope"],
                output=str(args.output),
            )
        )
    )


if __name__ == "__main__":
    main()
