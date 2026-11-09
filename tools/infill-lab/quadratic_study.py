# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Freeze iterative/direct TetP2 studies against retained CalculiX cases."""

from __future__ import annotations
import argparse
import json
import math
from pathlib import Path
import shutil
from campaign import write
from model import digest, file_hash
from quadratic import JUDGE


def pack(
    inputs,
    results,
    context,
    output,
    *,
    solvers=("amg",),
    divisions=(1, 2, 4),
    field_bins=None,
    max_z=None,
):
    if field_bins is not None and (
        type(field_bins) is not int or not 2 <= field_bins <= 32
    ):
        raise ValueError("field bins must be an integer between 2 and 32")
    if max_z is not None and (not math.isfinite(max_z) or max_z <= 0):
        raise ValueError("positive thickness refinement required")
    output.mkdir(parents=True, exist_ok=False)
    judge = {name: file_hash(context / name) for name in JUDGE}
    manifest = json.loads((inputs / "jobs.json").read_text())
    jobs = []
    selected = []
    for job in manifest["jobs"]:
        case = job["case"]
        if case["element"] != "C3D10":
            continue
        if (
            job["specimen"] == "analytical_beam"
            or case["refinement"]["divisions"] in divisions
        ):
            selected.append((job, case))
    if 8 in divisions:
        for job in manifest["jobs"]:
            case = job["case"]
            if (
                job["specimen"] != "analytical_beam"
                and case["element"] == "C3D10"
                and case["refinement"]["divisions"] == 4
            ):
                selected.append(
                    (
                        job,
                        {**case, "refinement": {**case["refinement"], "divisions": 8}},
                    )
                )
    for job, scenario in selected:
        if max_z is not None and job["specimen"] != "analytical_beam":
            scenario = {
                **scenario,
                "refinement": {**scenario["refinement"], "max_z": max_z},
            }
        original = inputs / job["id"]
        target = json.loads((original / "case.json").read_text())
        if digest(target) != job["id"] or target["judge"] != manifest["judge"]:
            raise ValueError("retained case identity differs")
        reference_path = results / job["id"] / "trials.jsonl"
        ref = None
        if scenario == job["case"] and reference_path.exists():
            rows = [
                json.loads(line) for line in reference_path.read_text().splitlines()
            ]
            if len(rows) != 1:
                raise ValueError("exactly one reference result required")
            ref = rows[0]
            if (
                ref["status"] != "ok"
                or ref["scenario"] != scenario
                or ref["input_sha256"] != file_hash(original / "case.json")
                or ref["judge"] != target["judge"]
            ):
                raise ValueError("rejected/mismatched reference cannot become a target")
            launch = json.loads((results / job["id"] / "launch.json").read_text())
            if not launch["removed"] or launch["blas_threads"] != 1:
                raise ValueError("qualified single-thread reference/cleanup required")
            for c in ref["components"]:
                if any(
                    not 0 <= c[k] <= 1e-5
                    for k in ("force_balance", "energy_error", "volume_relative_error")
                ):
                    raise ValueError("reference numerical gate failed")
        for solver in solvers:
            case_text = (
                (original / "case.json").read_text()
                if scenario == job["case"]
                else json.dumps(
                    {**target, "scenarios": [scenario]}, indent=2, sort_keys=True
                )
                + "\n"
            )
            import hashlib

            spec = dict(
                schema=1,
                kind="quadratic_reference",
                judge=judge,
                reference_case_sha256=hashlib.sha256(case_text.encode()).hexdigest(),
                role="parity" if ref else "refinement",
                solver=solver,
                specimen=job["specimen"],
            )
            if field_bins is not None and job["specimen"] != "analytical_beam":
                spec["field_bins"] = field_bins
            ref_text = None
            if ref:
                ref_text = json.dumps(ref, sort_keys=True) + "\n"
                spec["reference_result_sha256"] = hashlib.sha256(
                    ref_text.encode()
                ).hexdigest()
            key = digest(spec)
            folder = output / key
            folder.mkdir()
            write(folder / "case.json", spec)
            (folder / "reference-case.json").write_text(case_text)
            if ref_text:
                (folder / "reference-result.json").write_text(ref_text)
            if (original / "roads.json").exists():
                shutil.copyfile(original / "roads.json", folder / "roads.json")
            jobs.append(
                dict(
                    id=key,
                    specimen=job["specimen"],
                    solver=solver,
                    role=spec["role"],
                    case=scenario,
                )
            )
    write(
        output / "jobs.json",
        dict(
            schema=1,
            judge=judge,
            jobs=jobs,
            scope="quadratic_reference_parity_and_refinement",
        ),
    )
    print(json.dumps(dict(jobs=len(jobs), output=str(output))))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("inputs", "results", "context", "output"):
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument(
        "--solvers", nargs="+", choices=("amg", "p_amg", "superlu_mmd"), default=["amg"]
    )
    p.add_argument(
        "--divisions", nargs="+", type=int, choices=(1, 2, 4, 8), default=[1, 2, 4]
    )
    p.add_argument("--field-bins", type=int)
    p.add_argument("--max-z", type=float)
    a = p.parse_args()
    pack(
        a.inputs,
        a.results,
        a.context,
        a.output,
        solvers=a.solvers,
        divisions=a.divisions,
        field_bins=a.field_bins,
        max_z=a.max_z,
    )


if __name__ == "__main__":
    main()
