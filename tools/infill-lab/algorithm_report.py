# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Report one frozen source-edit cohort; never pool incompatible judges."""
from __future__ import annotations

import argparse
import json
import math
from pathlib import Path


def records(root, *, exclude=()):
    frozen = json.loads((root / "freeze.json").read_text())
    rows = []
    for folder in sorted((root / "trials").iterdir()):
        case = json.loads((folder / "case.json").read_text())
        if any(case[k] != frozen[k] for k in ("judge", "baseline_sha256", "image")):
            raise ValueError("trial belongs to another frozen cohort")
        admission = json.loads((folder / "admission.json").read_text())
        destination = root / "results" / folder.name
        result = destination / "trial.json"
        failure = destination / "failure.json"
        measured = json.loads(result.read_text()) if result.exists() else {}
        if not measured and failure.exists():
            measured = json.loads(failure.read_text())
        rows.append(
            {
                **case,
                **measured,
                "status": measured.get("status", admission["status"]),
                "admission": admission,
                "excluded": folder.name in exclude,
            }
        )
    return rows


def scores(rows, control):
    baseline = next(r for r in rows if r["name"] == control)
    if baseline["status"] != "ok":
        raise ValueError("a successful paired control is required")
    axes = tuple(baseline["stiffness_n_mm"])
    for row in rows:
        if row["status"] != "ok" or row["excluded"]:
            continue
        values = row["stiffness_n_mm"]
        if set(values) != set(axes):
            raise ValueError("incompatible fixture inventory")
        ratios = {a: values[a] / baseline["stiffness_n_mm"][a] for a in axes}
        if any(not math.isfinite(v) or v <= 0 for v in ratios.values()):
            raise ValueError(
                "nonpositive or unknown stiffness cannot enter a log score"
            )
        row.update(
            stiffness_ratios=ratios,
            geometric_mean_stiffness_ratio=math.exp(
                sum(map(math.log, ratios.values())) / len(axes)
            ),
            worst_stiffness_ratio=min(ratios.values()),
            material_ratio=row["volume_mm3"] / baseline["volume_mm3"],
            motion_proxy_ratio=row["motion_proxy_s"] / baseline["motion_proxy_s"],
        )
    return rows


def pareto(rows):
    valid = [r for r in rows if "geometric_mean_stiffness_ratio" in r]

    def cost(r):
        return (
            r["material_ratio"],
            r["motion_proxy_ratio"],
            -r["geometric_mean_stiffness_ratio"],
        )

    def dominates(a, b):
        return all(x <= y for x, y in zip(a, b)) and any(x < y for x, y in zip(a, b))

    return [
        r["name"]
        for r in valid
        if not any(dominates(cost(other), cost(r)) for other in valid)
    ]


def report(root, control, output, exclude):
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    frozen = json.loads((root / "freeze.json").read_text())
    specimen = (
        "local roof patch" if frozen["kind"] == "roof" else "lower chamber coupon"
    )
    rows = scores(records(root, exclude=exclude), control)
    frontier = pareto(rows)
    output.mkdir(parents=True, exist_ok=False)
    (output / "summary.json").write_text(
        json.dumps(
            dict(
                scope="finite_coupon_assumed_law_elastic_stiffness_not_strength_or_whole_part_capacity",
                control=control,
                frontier=frontier,
                rows=rows,
            ),
            indent=2,
            allow_nan=False,
        )
        + "\n"
    )
    fig, ax = plt.subplots(figsize=(11, 7), layout="constrained")
    good = [r for r in rows if "geometric_mean_stiffness_ratio" in r]
    points = ax.scatter(
        [100 * r["geometric_mean_stiffness_ratio"] for r in good],
        [100 * r["material_ratio"] for r in good],
        c=[100 * r["motion_proxy_ratio"] for r in good],
        cmap="viridis",
        s=85,
    )
    # Place nearby/equal-cost proposals separately; an ordering-only edit can
    # legitimately occupy the same stiffness/material coordinates as its parent.
    fig.canvas.draw()
    renderer = fig.canvas.get_renderer()
    occupied = []
    for row in good:
        point = (
            100 * row["geometric_mean_stiffness_ratio"],
            100 * row["material_ratio"],
        )
        label = ax.annotate(
            row["name"].replace("_", " "), point, textcoords="offset points", fontsize=9
        )
        for distance in (9, 24, 40, 58):
            for dx, dy in (
                (6, distance),
                (6, -distance),
                (-6, distance),
                (-6, -distance),
            ):
                label.set_position((dx, dy))
                label.set_ha("left" if dx > 0 else "right")
                bounds = label.get_window_extent(renderer).expanded(1.08, 1.2)
                if not any(bounds.overlaps(other) for other in occupied):
                    break
            else:
                continue
            break
        occupied.append(bounds)
        if distance > 9:
            ax.annotate(
                "",
                point,
                xytext=(dx, dy),
                textcoords="offset points",
                arrowprops=dict(arrowstyle="-", color="0.55", lw=0.6),
            )
    best = [r for r in good if r["name"] in frontier]
    ax.scatter(
        [100 * r["geometric_mean_stiffness_ratio"] for r in best],
        [100 * r["material_ratio"] for r in best],
        s=160,
        facecolors="none",
        edgecolors="#d26f28",
    )
    ax.axvline(100, color="0.6", linestyle=":")
    ax.axhline(100, color="0.6", linestyle=":")
    ax.set(
        xlabel="Geometric mean of normalized local elastic stiffness (%) → better",
        ylabel="Complete-object deposited material (%) → less is better",
        title=f"Source-edit search: {specimen}\nThree fixtures; weak-Z assumed PLA; orange rings = 3-objective frontier",
    )
    ax.grid(alpha=0.15)
    fig.colorbar(
        points, ax=ax, label="Motion proxy (% of control; not print-time prediction)"
    )
    fig.savefig(output / "pareto.png", dpi=180)
    fig.savefig(output / "pareto.svg")
    plt.close(fig)
    lines = [
        "| Candidate | Material | Motion proxy | Directional stiffness ratios | GM | Status |",
        "|---|---:|---:|---|---:|---|",
    ]
    for r in rows:
        if "geometric_mean_stiffness_ratio" in r:
            values = ", ".join(
                f"{k}: {v:.3f}" for k, v in r["stiffness_ratios"].items()
            )
            lines.append(
                f'| {r["name"]} | {r["material_ratio"]:.5f} | {r["motion_proxy_ratio"]:.5f} | {values} | {r["geometric_mean_stiffness_ratio"]:.4f} | scored |'
            )
        else:
            error = (
                "explicit report exclusion; raw result retained"
                if r["excluded"]
                else r.get("error", r["admission"].get("error", "not evaluated"))
            )
            lines.append(
                f'| {r["name"]} | — | — | — | — | {"excluded" if r["excluded"] else r["status"]}: {error} |'
            )
    (output / "results.md").write_text("\n".join(lines) + "\n")


def gallery(root, output, names):
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from cathedral_report import section, draw_shape, bay_preview

    fig, axes = plt.subplots(2, 3, figsize=(13, 8), layout="constrained")
    for ax, name in zip(axes.flat, names):
        rows = json.loads((root / "trials" / name / "sparse.json").read_text())
        draw_shape(ax, section(rows, 44.0))
        ax.set(
            xlim=(25, 47),
            ylim=(0, 14),
            title=name.replace("_", " "),
            xlabel="X (mm)",
            ylabel="Z (mm)",
        )
        ax.set_aspect("equal")
    fig.suptitle("Submitted deposition sections at Y=44mm — geometry, not stress plots")
    fig.savefig(output / "sections.png", dpi=180)
    plt.close(fig)
    fig = plt.figure(figsize=(13, 8), layout="constrained")
    for i, name in enumerate(names):
        rows = json.loads((root / "trials" / name / "sparse.json").read_text())
        ax = fig.add_subplot(2, 3, i + 1, projection="3d")
        bay_preview(
            ax, dict(roads=rows), [[25.466815, 41.75, 1.2], [46.25, 46.25, 14.0]]
        )
        ax.set_title(name.replace("_", " "))
    fig.suptitle("Submitted deposition — finite chamber with complete selected flanges")
    fig.savefig(output / "chambers.png", dpi=160)
    plt.close(fig)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cohort", type=Path, required=True)
    parser.add_argument("--control", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--exclude", nargs="*", default=[])
    parser.add_argument("--gallery", nargs="+")
    args = parser.parse_args()
    report(args.cohort, args.control, args.output, args.exclude)
    if args.gallery:
        gallery(args.cohort, args.output, args.gallery)
