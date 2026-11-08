# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Collect frozen TetP2 panels; distinguish parity, refinement and cost evidence."""

from __future__ import annotations
import argparse
import json
import math
from pathlib import Path
from campaign import write
from model import digest, file_hash


def collect(inputs, outputs):
    manifest = json.loads((inputs / "jobs.json").read_text())
    rows = []
    missing = []
    images = set()
    threads = set()
    for job in manifest["jobs"]:
        folder = outputs / job["id"]
        if not (folder / "trials.jsonl").exists():
            missing.append(job["id"])
            continue
        spec = json.loads((inputs / job["id"] / "case.json").read_text())
        if digest(spec) != job["id"] or spec["judge"] != manifest["judge"]:
            raise ValueError("quadratic input identity differs")
        trials = [
            json.loads(line)
            for line in (folder / "trials.jsonl").read_text().splitlines()
        ]
        launch = json.loads((folder / "launch.json").read_text())
        dispatch = json.loads((folder / "dispatch.json").read_text())
        if (
            len(trials) != 1
            or not launch["removed"]
            or launch["backend"] != "quadratic"
        ):
            raise ValueError("incomplete or unclean quadratic run")
        row = trials[0]
        if (
            row["input_sha256"] != file_hash(inputs / job["id"] / "case.json")
            or row["judge"] != spec["judge"]
            or row["scenario"] != job["case"]
            or row["role"] != spec["role"]
            or row["solver"] != spec["solver"]
        ):
            raise ValueError("quadratic result does not bind input")
        target_path = inputs / job["id"] / "reference-case.json"
        if file_hash(target_path) != spec["reference_case_sha256"]:
            raise ValueError("frozen target differs")
        target = json.loads(target_path.read_text())
        if target["scenarios"] != [row["scenario"]]:
            raise ValueError("target scenario differs")
        if row["status"] == "ok" and spec["role"] == "parity":
            expected_path = inputs / job["id"] / "reference-result.json"
            if file_hash(expected_path) != spec["reference_result_sha256"]:
                raise ValueError("frozen expected response differs")
            expected = json.loads(expected_path.read_text())
            if "stiffness_n_mm" in row:
                actual_error = abs(
                    row["stiffness_n_mm"] / expected["stiffness_n_mm"] - 1
                )
                if not math.isclose(
                    actual_error, row["reference_relative_error"], abs_tol=1e-14
                ):
                    raise ValueError("reported parity does not match retained response")
        images.add(launch["image"]["id"])
        threads.add(launch["blas_threads"])
        if row["status"] == "ok":
            if not row["components"]:
                raise ValueError("empty accepted quadratic response")
            for component in row["components"]:
                for key, limit in (
                    ("relative_residual", 2e-8),
                    ("force_balance", 1e-6),
                    ("energy_error", 1e-6),
                ):
                    if (
                        not math.isfinite(component[key])
                        or not 0 <= component[key] <= limit
                    ):
                        raise ValueError("quadratic numerical gate failed")
            if (
                spec["role"] == "parity"
                and not 0 <= row["reference_relative_error"] <= 1e-5
            ):
                raise ValueError("external response parity failed")
        rows.append(
            {
                **row,
                "job": job["id"],
                "specimen": job["specimen"],
                "reference_case_sha256": spec["reference_case_sha256"],
                "launch": launch,
                "host": dispatch["host"],
            }
        )
    if len(images) > 1 or len(threads) > 1:
        raise ValueError("separate images/thread profiles require separate reports")
    checks = []
    for name in ("cube32", "beam80"):
        required = sorted(
            {
                j["case"]["refinement"]["divisions"]
                for j in manifest["jobs"]
                if j["specimen"] == name and j["solver"] == "amg"
            }
        )
        found = {
            r["scenario"]["refinement"]["divisions"]: r
            for r in rows
            if r["specimen"] == name and r["solver"] == "amg" and r["status"] == "ok"
        }
        check = dict(specimen=name, passed=False)
        if len(required) >= 2:
            a, b = required[-2:]
            check.update(coarse=a, fine=b)
            if a in found and b in found:
                change = abs(
                    found[b]["stiffness_n_mm"] / found[a]["stiffness_n_mm"] - 1
                )
                check.update(relative_change=change, limit=0.05, passed=change <= 0.05)
            else:
                check["reason"] = "missing_or_failed_required_mesh"
        else:
            check["reason"] = "missing_refinement_pair"
        checks.append(check)
    pairs = []
    for row in rows:
        if (
            row["solver"] != "amg"
            or row["status"] != "ok"
            or "stiffness_n_mm" not in row
        ):
            continue
        for base in rows:
            if (
                base["solver"] != "superlu_mmd"
                or base["status"] != "ok"
                or base["reference_case_sha256"] != row["reference_case_sha256"]
            ):
                continue
            matched = (
                base["host"] == row["host"]
                and base["launch"]["limits"] == row["launch"]["limits"]
            )
            error = abs(row["stiffness_n_mm"] / base["stiffness_n_mm"] - 1)
            pairs.append(
                dict(
                    specimen=row["specimen"],
                    case_sha256=row["reference_case_sha256"],
                    matched_host_limits=matched,
                    response_relative_error=error,
                    numerical_agreement=error <= 1e-7,
                    wall_speedup=base["wall_s"] / row["wall_s"]
                    if matched and error <= 1e-7
                    else None,
                    direct_wall_s=base["wall_s"],
                    amg_wall_s=row["wall_s"],
                    direct_peak_rss_kib=base["peak_rss_kib"],
                    amg_peak_rss_kib=row["peak_rss_kib"],
                )
            )
    failed = [r["job"] for r in rows if r["status"] != "ok"]
    return dict(
        scope="quadratic_reference_parity_not_whole_part_or_strength",
        status="numerical_panel_complete"
        if not missing and not failed
        else "numerical_panel_open",
        required=len(manifest["jobs"]),
        completed=len(rows),
        accepted=len(rows) - len(failed),
        missing=missing,
        failed=failed,
        image_ids=sorted(images),
        blas_threads=sorted(threads),
        refinement_checks=checks,
        controlled_solver_pairs=pairs,
        rows=rows,
    )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("input", "results", "output"):
        p.add_argument("--" + name, type=Path, required=True)
    a = p.parse_args()
    report = collect(a.input, a.results)
    write(a.output, report)
    print(
        json.dumps(
            {
                k: report[k]
                for k in (
                    "status",
                    "required",
                    "completed",
                    "accepted",
                    "refinement_checks",
                    "controlled_solver_pairs",
                )
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
