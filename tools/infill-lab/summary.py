# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0
"""Compare declared controls within cohorts, never pool different judges."""

from __future__ import annotations
import argparse
import json
from pathlib import Path
import numpy as np
import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
from model import digest


def read_study(root):
    cohorts = []
    judges = set()
    for p in sorted((root / "campaigns").glob("*/config.json")):
        config = json.loads(p.read_text())
        judges.add(digest(config["identity"]))
        rows = [
            json.loads(s) for s in (p.parent / "trials.jsonl").read_text().splitlines()
        ]
        cohorts.append((p.parent.name, config, rows))
    if len(judges) != 1:
        raise ValueError(
            "summary requires one frozen judge/producer; select a coherent study"
        )
    return cohorts


def assess(root, output):
    cohorts = read_study(root)
    comparisons = []
    budget = []

    def ordinary(c):
        return (
            c["family"] == "uniform"
            and c["fraction"] == 0.2
            and c["axis"] == "alternating_xy"
            and c.get("pattern", "rectilinear") == "rectilinear"
            and c.get("layer_height", 0.2) == 0.2
            and not c.get("combine_height")
        )

    def comparison(key, config, a, b):
        return {
            "cohort": key,
            "specimen": config["specimen"],
            "case": config["case"],
            "dx": config["dx"],
            "dz": config["dz"],
            "phase": config["phase"],
            "z_ratio": config["material"].get("z_ratio", 1),
            "reference": a,
            "candidate": b,
            "stiffness_gain_pct": 100
            * (b["mechanics"]["stiffness_n_mm"] / a["mechanics"]["stiffness_n_mm"] - 1),
            "object_material_change_pct": 100 * (b["volume_mm3"] / a["volume_mm3"] - 1),
            "motion_change_pct": 100 * (b["motion_s"] / a["motion_s"] - 1),
        }

    for key, config, rows in cohorts:
        good = [r for r in rows if r["status"] == "ok"]
        a = next((r for r in good if ordinary(r["candidate"])), None)
        if not a:
            continue
        grid = [
            r
            for r in good
            if r["candidate"].get("pattern") == "grid"
            and r["candidate"]["axis"] == "alternating_xy"
            and r["candidate"]["layer_height"] == 0.2
        ]
        b = next((r for r in grid if r["candidate"]["fraction"] == 0.2), None)
        if b:
            comparisons.append(comparison(key, config, a, b))
        eligible = [
            r
            for r in grid
            if r["volume_mm3"] <= a["volume_mm3"] and r["motion_s"] <= a["motion_s"]
        ]
        if eligible:
            best = max(eligible, key=lambda r: r["mechanics"]["stiffness_n_mm"])
            budget.append(comparison(key, config, a, best))
    rows = sorted(
        comparisons, key=lambda c: (c["specimen"], c["z_ratio"], -c["dx"], c["phase"])
    )
    rows += [r for r in budget if r["candidate"]["candidate"]["fraction"] != 0.2]
    if not rows:
        raise ValueError("no paired comparisons")
    output.mkdir(parents=True, exist_ok=True)
    summary = {
        "trials": sum(len(r) for _, _, r in cohorts),
        "successes": sum(r["status"] == "ok" for _, _, rows in cohorts for r in rows),
        "cohorts": len(cohorts),
        "same_density": comparisons,
        "within_reference_cost": budget,
        "failures": [r for _, _, rows in cohorts for r in rows if r["status"] != "ok"],
    }
    (output / "summary.json").write_text(
        json.dumps(summary, sort_keys=True, allow_nan=False)
    )
    labels = []
    for row in rows:
        label = f"{row['specimen']} · {row['dx']:g} mm"
        if row["phase"] != [0.0, 0.0]:
            label += " · shifted"
        if row["z_ratio"] != 1:
            label += f" · weak Z {row['z_ratio']:g}"
        if row["candidate"]["candidate"]["fraction"] != 0.2:
            label += " · budget repaired"
        labels.append(label)
    fig, axes = plt.subplots(
        1, 3, figsize=(13, max(4, len(rows) * 0.48)), sharey=True, layout="constrained"
    )
    y = np.arange(len(rows))
    for ax, key, title in zip(
        axes,
        ["stiffness_gain_pct", "object_material_change_pct", "motion_change_pct"],
        [
            "Simulated stiffness Δ (%)",
            "Object material Δ (%)",
            "Motion lower bound Δ (%)",
        ],
    ):
        values = [r[key] for r in rows]
        colors = (
            ["#008b8b" if v >= 0 else "#bd6c32" for v in values]
            if key == "stiffness_gain_pct"
            else ["#008b8b" if v <= 0 else "#bd6c32" for v in values]
        )
        ax.barh(y, values, color=colors, height=0.6)
        ax.axvline(0, color="#82909e", linewidth=0.8)
        ax.set_xlabel(title)
        ax.spines[["top", "right"]].set_visible(False)
        for i, v in enumerate(values):
            ax.text(
                v,
                i,
                f" {v:+.1f}",
                va="center",
                ha="left" if v >= 0 else "right",
                fontsize=9,
            )
        ax.margins(x=0.3)
    axes[0].set_yticks(y, labels)
    axes[0].invert_yaxis()
    fig.suptitle(
        "Grid versus alternating rectilinear · unchanged source and material within each row\nBending on beams; weakest of three independent compression directions on cube",
        fontsize=12,
    )
    fig.savefig(output / "validation.png", dpi=180)
    fig.savefig(output / "validation.svg")
    plt.close(fig)
    print(json.dumps({k: summary[k] for k in ("trials", "successes", "cohorts")}))


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--study-root", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    a = p.parse_args()
    assess(a.study_root.resolve(), a.output.resolve())
