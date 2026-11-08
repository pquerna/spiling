# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Validate complete independent element panels without rewarding failed cases."""

from __future__ import annotations
import argparse
import json
import math
from pathlib import Path
from campaign import write
from model import digest, file_hash


def comparison(coarse, fine, limit=0.05):
    if not all(math.isfinite(v) and v > 0 for v in (coarse, fine)):
        raise ValueError("finite positive complete responses required")
    change = abs(fine / coarse - 1)
    return dict(relative_change=change, limit=limit, passed=change <= limit)


def analytical_gate(checks):
    expected = {
        (element, case)
        for element in ("C3D6", "C3D15", "C3D10", "C3D8I")
        for case in ("pure_bending", "compression_x", "compression_y", "compression_z")
    }
    present = {(c["element"], c["case"]) for c in checks}
    coarse_linear_control = any(
        c["element"] == "C3D6"
        and c["case"] == "pure_bending"
        and c["refinement"] == dict(nx=2, ny=1, nz=1)
        and c["relative_error"] > 1
        for c in checks
    )
    return (
        expected <= present
        and coarse_linear_control
        and all(c["passed"] for c in checks if c["required"])
    )


def collect(inputs, outputs):
    manifest = json.loads((inputs / "jobs.json").read_text())
    rows = []
    images = set()
    thread_profiles = set()
    missing = []
    for job in manifest["jobs"]:
        folder = outputs / job["id"]
        if not (folder / "trials.jsonl").exists():
            missing.append(job["id"])
            continue
        spec = json.loads((inputs / job["id"] / "case.json").read_text())
        if digest(spec) != job["id"] or spec["judge"] != manifest["judge"]:
            raise ValueError("independent input differs from frozen cohort")
        entries = [
            json.loads(line)
            for line in (folder / "trials.jsonl").read_text().splitlines()
        ]
        launch = json.loads((folder / "launch.json").read_text())
        if not launch.get("removed"):
            raise ValueError("independent result lacks owned-container cleanup")
        if launch.get("backend") != "reference":
            raise ValueError("independent result has a different backend")
        threads = launch.get("blas_threads")
        if type(threads) is not int or threads < 1:
            raise ValueError("independent result lacks explicit thread profile")
        profile = [threads]
        for field in (
            "ccx_nproc_results",
            "ccx_nproc_stiffness",
            "ccx_nproc_equation_solver",
        ):
            count = launch.get(field, threads)
            if type(count) is not int or count < 1:
                raise ValueError("invalid independent solver thread profile")
            profile.append(count)
        thread_profiles.add(tuple(profile))
        images.add(launch["image"]["id"])
        if len(entries) != len(spec["scenarios"]):
            raise ValueError("incomplete/duplicate independent scenario result")
        for row, scenario in zip(entries, spec["scenarios"]):
            if (
                row["scenario"] != scenario
                or row["input_sha256"] != file_hash(inputs / job["id"] / "case.json")
                or row["judge"] != spec["judge"]
            ):
                raise ValueError("independent result does not bind frozen input")
            if row["status"] == "ok":
                if not row["components"]:
                    raise ValueError("independent response lacks accepted components")
                for part in row["components"]:
                    for key in (
                        "force_balance",
                        "energy_error",
                        "volume_relative_error",
                    ):
                        if not math.isfinite(part[key]) or not 0 <= part[key] <= 1e-5:
                            raise ValueError("independent result physical gate failed")
                if spec["kind_source"] == "analytical_beam" and scenario[
                    "case"
                ].startswith("compression_"):
                    axis = "xyz".index(scenario["case"][-1])
                    lo, hi = scenario["bounds"]
                    size = [b - a for a, b in zip(lo, hi)]
                    e = scenario["material"]["young_mpa"] * (
                        scenario["material"]["z_ratio"] if axis == 2 else 1
                    )
                    expected = e * math.prod(size) / size[axis] ** 2
                    row["analytical_axial_relative_error"] = abs(
                        row["stiffness_n_mm"] / expected - 1
                    )
            row.update(
                job=job["id"],
                source_kind=spec["kind_source"],
                specimen=spec.get("specimen", "analytical_beam"),
                launch=launch,
                image=launch["image"]["id"],
            )
            rows.append(row)
    if len(images) > 1:
        raise ValueError("different evaluator images require separate reports")
    if len(thread_profiles) > 1:
        raise ValueError("different thread profiles require separate reports")
    beam_checks = []
    for row in rows:
        if row["source_kind"] != "analytical_beam" or row["status"] != "ok":
            continue
        # Linear wedges are deliberately a known-error control, not an admitted
        # higher-order formulation which must pass the bending expectation.
        s = row["scenario"]
        error = (
            row["components"][0]["analytical_relative_error"]
            if s["case"] == "pure_bending"
            else row["analytical_axial_relative_error"]
        )
        beam_checks.append(
            dict(
                element=s["element"],
                case=s["case"],
                refinement=s["refinement"],
                relative_error=error,
                required=s["element"] != "C3D6" or s["case"] != "pure_bending",
                passed=error <= 0.005,
            )
        )
    checks = []
    for name in ("cube32", "beam80"):
        found = {
            (
                r["scenario"]["element"],
                r["scenario"]["refinement"]["divisions"],
                r["scenario"]["refinement"]["max_z"],
            ): r
            for r in rows
            if r["specimen"] == name and r["status"] == "ok"
        }
        pairs = [("quadratic_wedge_thickness", ("C3D15", 1, 1.0), ("C3D15", 1, 0.1))]
        finest = {}
        for element, label in (
            ("C3D15", "quadratic_wedge_planar"),
            ("C3D10", "quadratic_tet_planar"),
        ):
            # Select from the required input panel, never from the successful
            # subset. A rejected finest solve must not promote a coarser pair.
            divisions = sorted(
                {
                    job["case"]["refinement"]["divisions"]
                    for job in manifest["jobs"]
                    if job["specimen"] == name
                    and job["case"]["element"] == element
                    and job["case"]["refinement"]["max_z"] == 1.0
                }
            )
            if len(divisions) >= 2:
                coarse, fine = divisions[-2:]
                pairs.append((label, (element, coarse, 1.0), (element, fine, 1.0)))
                finest[element] = (element, fine, 1.0)
            else:
                checks.append(
                    dict(
                        specimen=name,
                        check=label,
                        passed=False,
                        reason="missing_refinement_pair",
                    )
                )
        if len(finest) == 2:
            pairs.append(
                ("independent_quadratic_formulations", finest["C3D15"], finest["C3D10"])
            )
        for label, a, b in pairs:
            result = (
                comparison(found[a]["stiffness_n_mm"], found[b]["stiffness_n_mm"])
                if a in found and b in found
                else dict(passed=False, reason="missing_or_rejected_response")
            )
            checks.append(dict(specimen=name, check=label, coarse=a, fine=b, **result))
    failed = [
        dict(job=r["job"], scenario=r["scenario"], error=r.get("error"))
        for r in rows
        if r["status"] != "ok"
    ]
    qualified = (
        not missing
        and not failed
        and analytical_gate(beam_checks)
        and all(c["passed"] for c in checks)
    )
    return dict(
        scope="independent_element_qualification_not_whole_part_strength",
        image_ids=sorted(images),
        thread_profiles=[
            dict(omp_blas=t, ccx_results=r, ccx_stiffness=s, ccx_equation_solver=e)
            for t, r, s, e in sorted(thread_profiles)
        ],
        status="element_gate_passed" if qualified else "element_gate_open",
        required_cases=len(manifest["jobs"]),
        completed=len(rows),
        accepted=sum(r["status"] == "ok" for r in rows),
        missing=missing,
        failed=failed,
        analytical_checks=beam_checks,
        response_checks=checks,
        rows=rows,
    )


def plot(report, destination):
    """Export a scientific diagnostic, never an infill-strength Pareto claim."""
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    fig, axes = plt.subplots(1, 2, figsize=(12, 4.8), layout="constrained")
    for ax, specimen, title in zip(
        axes,
        ("cube32", "beam80"),
        ("Cube · 28 × 28 × 8 mm cut", "Beam · 76 × 26 × 8 mm cut"),
    ):
        for element, label, color in (
            ("C3D6", "Linear prisms", "#64748b"),
            ("C3D15", "Quadratic prisms", "#0072b2"),
            ("C3D10", "Quadratic tetrahedra", "#d55e00"),
        ):
            selected = sorted(
                [
                    r
                    for r in report["rows"]
                    if r["specimen"] == specimen
                    and r["status"] == "ok"
                    and r["scenario"]["element"] == element
                    and r["scenario"]["refinement"]["max_z"] == 1.0
                ],
                key=lambda r: r["scenario"]["refinement"]["divisions"],
            )
            if selected:
                ax.plot(
                    [r["scenario"]["refinement"]["divisions"] for r in selected],
                    [r["stiffness_n_mm"] for r in selected],
                    marker="o",
                    color=color,
                    label=label,
                )
        ax.set(
            title=title,
            xlabel="Planar refinement factor",
            ylabel="Z stiffness (N/mm)",
            ylim=(0, None),
        )
        ax.set_xscale("log", base=2)
        ax.set_xticks([1, 2, 4, 8], ["1", "2", "4", "8"])
        ax.grid(alpha=0.2)
        ax.legend(frameon=False)
    fig.suptitle(
        "Crossing-road element qualification · "
        + ("gate passed" if report["status"] == "element_gate_passed" else "gate open"),
        weight="bold",
    )
    fig.supxlabel(
        "Same nominal deposition · assumed weak-Z elasticity · accepted numerical results only; missing/rejected cases remain in the report",
        fontsize=9,
    )
    destination.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(destination, dpi=180)
    plt.close(fig)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--results", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--plot", type=Path)
    args = parser.parse_args()
    report = collect(args.input, args.results)
    write(args.output, report)
    if args.plot:
        plot(report, args.plot)
    print(
        json.dumps(
            {
                k: report[k]
                for k in (
                    "status",
                    "required_cases",
                    "completed",
                    "accepted",
                    "thread_profiles",
                )
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
