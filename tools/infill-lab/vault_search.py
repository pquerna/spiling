# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Bounded, resumable Vault parameter autoresearch; no minimum effect threshold.

Fixed native controls, immutable judge, parent/child provenance and a multi-axis
Pareto archive. Retention is provisional until paired refinement and held-outs;
no search result edits production defaults or confers physical strength.
"""

from __future__ import annotations

import argparse
from dataclasses import asdict
import json
import math
from pathlib import Path
import random
import shutil

from model import Candidate, Material, digest, file_hash
from specimens import SIZES


def write(path, data):
    with path.open("x") as stream:
        stream.write(json.dumps(data, indent=2, sort_keys=True, allow_nan=False) + "\n")


def frontier(rows):
    """Retain material/time and each axis; a geometric mean cannot hide weak axes."""
    valid = [r for r in rows if r["status"] == "ok"]

    axes = sorted(valid[0]["stiffness_n_mm"]) if valid else []
    if any(sorted(r["stiffness_n_mm"]) != axes for r in valid):
        raise ValueError("response inventories differ")

    def values(r):
        return (
            r["volume_mm3"],
            r["motion_s"],
            *(-r["stiffness_n_mm"][a] for a in axes),
        )

    def dominates(a, b):
        return all(x <= y for x, y in zip(a, b)) and any(x < y for x, y in zip(a, b))

    return [r for r in valid if not any(dominates(values(s), values(r)) for s in valid)]


def geometric_mean(values):
    if any(not math.isfinite(x) or x < 0 for x in values):
        raise ValueError("invalid response")
    return 0.0 if 0 in values else math.exp(sum(map(math.log, values)) / len(values))


def proposals(round_number, history):
    """10 seeds, 30 adaptive local changes, 10 exploration points; controls extra."""
    if not 0 <= round_number <= 4:
        raise ValueError("five bounded rounds required")
    seen = {digest(r["candidate"]) for r in history}
    result = []

    def add(candidate, parent, reason):
        key = digest(candidate.data())
        if key in seen:
            return False
        seen.add(key)
        result.append(dict(candidate=candidate.data(), parent=parent, reason=reason))
        return True

    if round_number == 0:
        if history:
            raise ValueError("initial round must have an empty archive")
        for fraction in (0.05, 0.1, 0.2):
            for pattern in ("rectilinear", "grid"):
                add(
                    Candidate(fraction=fraction, pattern=pattern), None, "fixed_control"
                )
        add(Candidate(fraction=1), None, "solid_control")
        for fraction, spacing in (
            (0.05, 7.5),
            (0.075, 7.5),
            (0.1, None),
            (0.15, 7.5),
            (0.2, 7.5),
            (0.05, 4),
            (0.075, 4),
            (0.1, 4),
            (0.05, 2),
            (0.1, 2),
        ):
            add(
                Candidate(
                    fraction=fraction, pattern="web_roof", roof_spacing_mm=spacing
                ),
                None,
                "seed",
            )
        return result
    rng = random.Random(48000 + round_number)
    if round_number == 4:
        for _ in range(10):
            while not add(
                Candidate(
                    fraction=round(rng.uniform(0.04, 0.3), 4),
                    pattern="web_roof",
                    roof_spacing_mm=round(rng.uniform(1.5, 7.5), 3),
                ),
                None,
                "exploration",
            ):
                pass
        return result
    parents = frontier([r for r in history if r["candidate"]["pattern"] == "web_roof"])
    if not parents:
        raise ValueError("local search requires admitted measured Vault parents")
    # Cycle low material, high response, and response/material niches. No single
    # scalar determines retention, and controls cannot become mutation parents.
    parents.sort(
        key=lambda r: geometric_mean(list(r["stiffness_n_mm"].values()))
        / r["volume_mm3"],
        reverse=True,
    )
    pool = [
        min(parents, key=lambda r: r["volume_mm3"]),
        parents[0],
        max(parents, key=lambda r: geometric_mean(list(r["stiffness_n_mm"].values()))),
        *parents,
    ]
    for n in range(10):
        parent = pool[n % len(pool)]
        base = Candidate(**parent["candidate"])
        for attempt in range(1000):
            spacing = base.roof_spacing_mm or 7.5
            data = base.data()
            if (n + attempt) % 2:
                data["fraction"] = round(
                    min(
                        0.4,
                        max(
                            0.025,
                            base.fraction
                            + rng.choice(
                                [
                                    -0.02,
                                    -0.015,
                                    -0.01,
                                    -0.005,
                                    -0.0025,
                                    0.0025,
                                    0.005,
                                    0.01,
                                    0.015,
                                    0.02,
                                ]
                            ),
                        ),
                    ),
                    4,
                )
            else:
                data["roof_spacing_mm"] = round(
                    min(
                        7.5,
                        max(
                            0.75,
                            spacing
                            + rng.choice(
                                [
                                    -1.0,
                                    -0.75,
                                    -0.5,
                                    -0.3,
                                    -0.2,
                                    -0.1,
                                    0.1,
                                    0.2,
                                    0.3,
                                    0.5,
                                    0.75,
                                    1.0,
                                ]
                            ),
                        ),
                    ),
                    3,
                )
            if attempt >= 32:
                data["fraction"] = round(
                    min(0.4, max(0.025, base.fraction + rng.uniform(-0.02, 0.02))), 4
                )
                data["roof_spacing_mm"] = round(
                    min(7.5, max(0.75, spacing + rng.uniform(-1, 1))), 3
                )
            if add(Candidate(**data), parent["capture_id"], "local_mutation"):
                break
        else:
            raise ValueError("bounded mutation proposals exhausted")
    return result


def propose(args):
    history = json.loads(args.history.read_text())["rows"] if args.history else []
    rows = proposals(args.round, history)
    args.output.mkdir(parents=True, exist_ok=False)
    write(
        args.output / "proposals.json",
        dict(
            round=args.round,
            history_sha256=file_hash(args.history) if args.history else None,
            proposals=rows,
        ),
    )
    (args.output / "queue.jsonl").write_text(
        "".join(json.dumps(r["candidate"]) + "\n" for r in rows)
    )
    print(json.dumps(dict(round=args.round, proposals=len(rows))))


def native_rows(root):
    spec = json.loads((root / "capture.json").read_text())
    rows = [
        json.loads(line) for line in (root / "captures.jsonl").read_text().splitlines()
    ]
    expected = {
        digest(dict(candidate=c, specimen=s, identity=spec["identity"]))
        for c in spec["candidates"]
        for s in spec["specimens"]
    }
    if len(rows) != len(expected) or {r["id"] for r in rows} != expected:
        raise ValueError("native capture incomplete/duplicated")
    for row in rows:
        if row["status"] != "ok":
            continue
        folder = root / row["native"]
        manifest = json.loads((folder / "manifest.json").read_text())
        if set(manifest) != {
            "roads.json",
            "actual.splr",
            "receipt.json",
            "project.spil",
        } or any(file_hash(folder / p) != h for p, h in manifest.items()):
            raise ValueError("native closure differs")
        data = json.loads((folder / "roads.json").read_text())
        if (
            not data["report"]["verified"]
            or not data["software_only"]
            or data["candidate"] != row["candidate"]
            or data["source_sha256"] != row["source_sha256"]
            or data["bin_sha256"] != spec["identity"]["binaries"]
            or file_hash(folder / "roads.json") != row["roads_sha256"]
            or file_hash(folder / "actual.splr") != row["replay_sha256"]
        ):
            raise ValueError("native identity differs")
    return spec, rows


def pack(args):
    from vault_mechanics import JUDGE

    spec, rows = native_rows(args.capture)
    if args.selection:
        selected = set(json.loads(args.selection.read_text()))
        if not selected or not selected <= {r["id"] for r in rows}:
            raise ValueError("selection contains unknown native captures")
        rows = [r for r in rows if r["id"] in selected]
    args.output.mkdir(parents=True, exist_ok=False)
    judge = {p: file_hash(args.context / p) for p in JUDGE}
    jobs = []
    for row in rows:
        if row["status"] != "ok":
            continue
        x, y, z = SIZES[row["specimen"]]
        width = args.width
        cx, cy = 20 + x / 2 + args.shift, 20 + y / 2 + args.shift
        scenario = dict(
            bounds=[
                [cx - width / 2, cy - width / 2, z - args.depth],
                [cx + width / 2, cy + width / 2, z],
            ],
            material=asdict(Material(z_ratio=0.25)),
            solver=args.solver,
            refinement=dict(max_xy=4, max_z=args.max_z, divisions=args.divisions),
        )
        case = dict(
            schema=1,
            kind="vault_roof_patch",
            judge=judge,
            candidate=row["candidate"],
            specimen=row["specimen"],
            binaries=spec["identity"]["binaries"],
            source_sha256=row["source_sha256"],
            roads_sha256=row["roads_sha256"],
            replay_sha256=row["replay_sha256"],
            scenario=scenario,
        )
        key = digest(case)
        folder = args.output / key
        folder.mkdir()
        shutil.copyfile(
            args.capture / row["native"] / "roads.json", folder / "roads.json"
        )
        write(folder / "case.json", case)
        jobs.append(
            dict(
                id=key,
                capture_id=row["id"],
                candidate=row["candidate"],
                specimen=row["specimen"],
            )
        )
    write(
        args.output / "jobs.json",
        dict(
            schema=1,
            judge=judge,
            jobs=jobs,
            refused=[r for r in rows if r["status"] != "ok"],
        ),
    )
    print(json.dumps(dict(jobs=len(jobs), refused=len(rows) - len(jobs))))


def collect(args):
    jobs = json.loads((args.input / "jobs.json").read_text())
    proposal_rows = json.loads(args.proposals.read_text())["proposals"]
    attribution = {digest(p["candidate"]): p for p in proposal_rows}
    rows = []
    cohorts = set()
    for job in jobs["jobs"]:
        key = job["id"]
        case_path = args.input / key / "case.json"
        case = json.loads(case_path.read_text())
        if digest(case) != key or case["judge"] != jobs["judge"]:
            raise ValueError("job identity differs")
        cohort = dict(
            judge=case["judge"],
            scenario=case["scenario"],
            source=case["source_sha256"],
            binaries=case["binaries"],
        )
        folder = args.results / key
        launch = json.loads((folder / "launch.json").read_text())
        dispatch = json.loads((folder / "dispatch.json").read_text())
        cohort["image"] = launch["image"]["id"]
        cohorts.add(digest(cohort))
        proposal = attribution[digest(job["candidate"])]
        lines = (
            (folder / "trials.jsonl").read_text().splitlines()
            if (folder / "trials.jsonl").exists()
            else []
        )
        if not launch["removed"]:
            raise ValueError("owned container not removed")
        if len(lines) != 1:
            rows.append(
                dict(
                    parent=proposal["parent"],
                    reason=proposal["reason"],
                    status="failed",
                    capture_id=job["capture_id"],
                    candidate=job["candidate"],
                    error="no complete mechanical result",
                    job=key,
                )
            )
            continue
        row = json.loads(lines[0])
        if (
            row["input_sha256"] != file_hash(case_path)
            or row["judge"] != jobs["judge"]
            or row["scenario"] != case["scenario"]
            or row["candidate"] != case["candidate"]
            or row["roads_sha256"] != case["roads_sha256"]
            or row["source_sha256"] != case["source_sha256"]
        ):
            raise ValueError("mechanical input/judge differs")
        if row["status"] == "ok":
            if dispatch["exit_code"] != 0 or launch["docker_exit"] != 0:
                raise ValueError("failed launch cannot be retained")
            for c in row["components"]:
                if any(
                    not math.isfinite(c[k]) or not 0 <= c[k] <= 1e-5
                    for k in ("relative_residual", "energy_error", "force_balance")
                ):
                    raise ValueError("mechanical numerical checks failed")
            geometric_mean([row["stiffness_n_mm"][a] for a in "xyz"])
        rows.append(
            dict(
                row,
                capture_id=job["capture_id"],
                job=key,
                cohort=digest(cohort),
                image=launch["image"]["id"],
                parent=proposal["parent"],
                reason=proposal["reason"],
            )
        )
    if len(cohorts) != 1:
        raise ValueError("one fixed specimen/fixture/judge cohort required")
    for refused in jobs["refused"]:
        proposal = attribution[digest(refused["candidate"])]
        rows.append(
            dict(
                status="failed",
                stage="native",
                capture_id=refused["id"],
                candidate=refused["candidate"],
                error=refused["error"],
                parent=proposal["parent"],
                reason=proposal["reason"],
            )
        )
    prior = json.loads(args.history.read_text())["rows"] if args.history else []
    if prior and any(
        r.get("cohort") not in cohorts for r in prior if r["status"] == "ok"
    ):
        raise ValueError("cannot pool different search cohorts")
    rows = prior + rows
    if len({r["capture_id"] for r in rows}) != len(rows):
        raise ValueError("duplicate search result")
    write(
        args.output,
        dict(
            scope="provisional_local_roof_elastic_search_not_strength",
            minimum_effect_threshold=None,
            rows=rows,
            frontier=[r["capture_id"] for r in frontier(rows)],
        ),
    )
    print(
        json.dumps(
            dict(
                rows=len(rows),
                ok=sum(r["status"] == "ok" for r in rows),
                frontier=len(frontier(rows)),
            )
        )
    )


def core_pack(args):
    """Use the existing independent quadratic backend on a broad mid-body crop."""
    from quadratic import JUDGE

    spec, rows = native_rows(args.capture)
    judge = {p: file_hash(args.context / p) for p in JUDGE}
    args.output.mkdir(parents=True, exist_ok=False)
    jobs = []
    for row in rows:
        if row["status"] != "ok":
            continue
        x, y, z = SIZES[row["specimen"]]
        width = args.width
        bounds = [
            [20 + x / 2 - width / 2, 20 + y / 2 - width / 2, z / 2 - 4],
            [20 + x / 2 + width / 2, 20 + y / 2 + width / 2, z / 2 + 4],
        ]
        for axis in "xyz":
            scenario = dict(
                case="compression_" + axis,
                element="C3D10",
                bounds=bounds,
                material=asdict(Material(z_ratio=0.25)),
                refinement=dict(max_xy=4, max_z=1, divisions=args.divisions),
            )
            target = dict(
                kind_source="actual_roads",
                scope="interior",
                candidate=row["candidate"],
                source_sha256=row["source_sha256"],
                roads_sha256=row["roads_sha256"],
                scenarios=[scenario],
            )
            text = json.dumps(target, sort_keys=True) + "\n"
            import hashlib

            case = dict(
                schema=1,
                kind="quadratic_reference",
                judge=judge,
                reference_case_sha256=hashlib.sha256(text.encode()).hexdigest(),
                role="refinement",
                solver="p_amg",
                specimen=row["specimen"],
            )
            key = digest(case)
            folder = args.output / key
            folder.mkdir()
            write(folder / "case.json", case)
            (folder / "reference-case.json").write_text(text)
            shutil.copyfile(
                args.capture / row["native"] / "roads.json", folder / "roads.json"
            )
            jobs.append(
                dict(
                    id=key,
                    capture_id=row["id"],
                    candidate=row["candidate"],
                    specimen=row["specimen"],
                    axis=axis,
                )
            )
    write(args.output / "jobs.json", dict(judge=judge, jobs=jobs))
    print(json.dumps(dict(jobs=len(jobs))))


def join_core(args):
    """Combine explicitly named fixtures, never pretend different cohorts are one."""
    roof = json.loads(args.roof.read_text())
    history = (
        {r["capture_id"]: r for r in json.loads(args.history.read_text())["rows"]}
        if args.history
        else {}
    )
    manifest = json.loads((args.input / "jobs.json").read_text())
    grouped, core_cohorts = {}, set()
    roof_by_id = {r["capture_id"]: r for r in roof["rows"]}
    for job in manifest["jobs"]:
        folder = args.input / job["id"]
        case = json.loads((folder / "case.json").read_text())
        target = json.loads((folder / "reference-case.json").read_text())
        if (
            digest(case) != job["id"]
            or file_hash(folder / "reference-case.json")
            != case["reference_case_sha256"]
            or target["candidate"] != job["candidate"]
            or case["judge"] != manifest["judge"]
            or target["candidate"] != roof_by_id[job["capture_id"]]["candidate"]
            or target["roads_sha256"] != roof_by_id[job["capture_id"]]["roads_sha256"]
            or target["source_sha256"] != roof_by_id[job["capture_id"]]["source_sha256"]
        ):
            raise ValueError("core input identity differs")
        result_folder = args.results / job["id"]
        launch = json.loads((result_folder / "launch.json").read_text())
        dispatch = json.loads((result_folder / "dispatch.json").read_text())
        if not launch["removed"]:
            raise ValueError("core container not removed")
        scenario = target["scenarios"][0]
        core_cohorts.add(
            digest(
                dict(
                    image=launch["image"]["id"],
                    judge=manifest["judge"],
                    source=target["source_sha256"],
                    scenario={k: v for k, v in scenario.items() if k != "case"},
                )
            )
        )
        lines = (
            (result_folder / "trials.jsonl").read_text().splitlines()
            if (result_folder / "trials.jsonl").exists()
            else []
        )
        if len(lines) != 1:
            row = dict(status="failed", error="no complete core result")
        else:
            row = json.loads(lines[0])
            if (
                row["input_sha256"] != file_hash(folder / "case.json")
                or row["judge"] != manifest["judge"]
                or row["scenario"] != scenario
            ):
                raise ValueError("core response identity differs")
            if row["status"] == "ok":
                if dispatch["exit_code"] != 0 or launch.get("docker_exit") != 0:
                    raise ValueError("failed core launch cannot be retained")
                for c in row["components"]:
                    if any(
                        not math.isfinite(c[k]) or not 0 <= c[k] <= 1e-5
                        for k in ("relative_residual", "energy_error", "force_balance")
                    ):
                        raise ValueError("core numerical checks failed")
        axes = grouped.setdefault(job["capture_id"], {})
        if job["axis"] in axes:
            raise ValueError("duplicate core axis")
        axes[job["axis"]] = dict(row, job=job["id"])
    if len(core_cohorts) != 1:
        raise ValueError("one fixed core cohort required")
    core_cohort = next(iter(core_cohorts))
    rows = []
    for original in roof["rows"]:
        key = original["capture_id"]
        if key in history:
            if history[key]["candidate"] != original["candidate"]:
                raise ValueError("history candidate differs")
            rows.append(history[key])
            continue
        row = dict(original)
        core = grouped.get(key)
        if row["status"] == "ok":
            if core is None or set(core) != set("xyz"):
                raise ValueError("complete core XYZ inventory required")
            row["roof_response"] = row["stiffness_n_mm"]
            row["core_response"] = core
            row["roof_cohort"] = row["cohort"]
            row["core_cohort"] = core_cohort
            row["cohort"] = digest(dict(roof=row["cohort"], core=core_cohort))
            if all(c["status"] == "ok" for c in core.values()):
                row["stiffness_n_mm"] = {
                    **{"roof_" + a: row["roof_response"][a] for a in "xyz"},
                    **{"core_" + a: core[a]["stiffness_n_mm"] for a in "xyz"},
                }
            else:
                row.update(
                    status="failed",
                    stage="core_mechanics",
                    error="one or more core cases failed",
                )
        rows.append(row)
    valid = [r for r in rows if r["status"] == "ok"]
    if len({r["cohort"] for r in valid}) != 1:
        raise ValueError("paired fixture cohorts differ")
    write(
        args.output,
        dict(
            scope="two_finite_coupon_nominal_elastic_search_not_strength",
            minimum_effect_threshold=None,
            rows=rows,
            frontier=[r["capture_id"] for r in frontier(rows)],
        ),
    )
    print(
        json.dumps(
            dict(rows=len(rows), admitted=len(valid), frontier=len(frontier(rows)))
        )
    )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="command", required=True)
    q = sub.add_parser("propose")
    q.add_argument("--round", type=int, required=True)
    q.add_argument("--history", type=Path)
    q.add_argument("--output", type=Path, required=True)
    q = sub.add_parser("pack")
    for field in ("capture", "context", "output"):
        q.add_argument("--" + field, type=Path, required=True)
    q.add_argument("--selection", type=Path)
    q.add_argument("--divisions", type=int, choices=(1, 2, 4, 8), default=1)
    q.add_argument("--width", type=float, default=16)
    q.add_argument("--max-z", type=float, default=1)
    q.add_argument("--depth", type=float, default=8)
    q.add_argument(
        "--solver",
        choices=("p_amg", "amg", "superlu", "superlu_mmd"),
        default="superlu_mmd",
    )
    q.add_argument("--shift", type=float, default=0)
    q = sub.add_parser("collect")
    for field in ("input", "results", "output"):
        q.add_argument("--" + field, type=Path, required=True)
    q.add_argument("--history", type=Path)
    q.add_argument("--proposals", type=Path, required=True)
    q = sub.add_parser("core-pack")
    for field in ("capture", "context", "output"):
        q.add_argument("--" + field, type=Path, required=True)
    q.add_argument("--width", type=float, default=40)
    q.add_argument("--divisions", type=int, choices=(1, 2, 4, 8), default=1)
    q = sub.add_parser("join-core")
    for field in ("roof", "input", "results", "output"):
        q.add_argument("--" + field, type=Path, required=True)
    q.add_argument("--history", type=Path)
    args = p.parse_args()
    {
        "propose": propose,
        "pack": pack,
        "collect": collect,
        "core-pack": core_pack,
        "join-core": join_core,
    }[args.command](args)


if __name__ == "__main__":
    main()
