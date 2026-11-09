# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Freeze independent paired Vault validation from existing native receipts.

No new construction, inferred success, or edits to the search archive. Prepare
one physical fixture at a time; collection uses the same numerical and identity
checks as the primary search. A changed fixture is always a separate cohort.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
from types import SimpleNamespace as Args

from vault_search import collect, native_rows, pack, write


def subset(captures, selected, output):
    if len(selected) != 2 or len(set(selected)) != 2:
        raise ValueError("two distinct native capture IDs required")
    found = {}
    for root in captures:
        spec, rows = native_rows(root)
        for row in rows:
            if row["id"] in selected:
                if row["id"] in found:
                    raise ValueError("duplicate selected capture")
                found[row["id"]] = (root, spec, row)
    if set(found) != set(selected):
        raise ValueError("unknown selected capture")
    entries = [found[key] for key in selected]
    first = entries[0][1]
    if any(spec["identity"] != first["identity"] for _, spec, _ in entries):
        raise ValueError("paired native producers differ")
    if any(r["status"] != "ok" for _, _, r in entries):
        raise ValueError("native failure cannot enter a mechanical pair")
    if len({(r["specimen"], r["source_sha256"]) for _, _, r in entries}) != 1:
        raise ValueError("one identical native source required")
    output.mkdir(parents=True, exist_ok=False)
    rows = [r for _, _, r in entries]
    write(
        output / "capture.json",
        dict(
            first,
            candidates=[r["candidate"] for r in rows],
            specimens=[rows[0]["specimen"]],
        ),
    )
    (output / "captures.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    for root, _, row in entries:
        shutil.copytree(root / row["native"], output / row["native"])
    native_rows(output)
    write(output / "pair.json", dict(parent=selected[0], child=selected[1]))
    write(
        output / "proposals.json",
        dict(
            proposals=[
                dict(
                    candidate=r["candidate"],
                    parent=selected[0] if n else None,
                    reason="independent_paired_validation",
                )
                for n, r in enumerate(rows)
            ]
        ),
    )


def compare(archive, pair):
    rows = {r["capture_id"]: r for r in archive["rows"]}
    parent, child = (rows[pair[key]] for key in ("parent", "child"))
    base = dict(
        parent=parent["capture_id"],
        child=child["capture_id"],
        parent_status=parent["status"],
        child_status=child["status"],
    )
    if parent["status"] != "ok" or child["status"] != "ok":
        return dict(base, status="incomplete")
    if parent["cohort"] != child["cohort"]:
        raise ValueError("cannot compare different physical cohorts")
    if set(parent["stiffness_n_mm"]) != set(child["stiffness_n_mm"]):
        raise ValueError("paired response inventories differ")
    ratios = {
        a: child["stiffness_n_mm"][a] / value if value > 0 else None
        for a, value in parent["stiffness_n_mm"].items()
    }
    return dict(
        base,
        status="ok",
        cohort=parent["cohort"],
        response_ratios=ratios,
        material_ratio=child["volume_mm3"] / parent["volume_mm3"],
        motion_ratio=child["motion_s"] / parent["motion_s"],
        scope="paired_nominal_elastic_sensitivity_not_strength",
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    q = sub.add_parser("prepare")
    q.add_argument("--captures", type=Path, nargs="+", required=True)
    q.add_argument("--ids", nargs=2, required=True, metavar=("PARENT", "CHILD"))
    q.add_argument("--output", type=Path, required=True)
    q = sub.add_parser("pack")
    for name in ("capture", "context", "output"):
        q.add_argument("--" + name, type=Path, required=True)
    q.add_argument("--width", type=float, default=8)
    q.add_argument("--depth", type=float, default=4)
    q.add_argument("--shift", type=float, default=0)
    q.add_argument("--max-z", type=float, default=1)
    q.add_argument("--divisions", type=int, choices=(1, 2, 4, 8), default=1)
    q.add_argument("--solver", choices=("superlu_mmd", "p_amg"), default="superlu_mmd")
    q = sub.add_parser("collect")
    for name in ("capture", "input", "results", "output"):
        q.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    if args.command == "prepare":
        subset(args.captures, args.ids, args.output)
    elif args.command == "pack":
        pack(Args(**{**vars(args), "selection": None}))
    else:
        args.output.mkdir(parents=True, exist_ok=False)
        collect(
            Args(
                input=args.input,
                results=args.results,
                proposals=args.capture / "proposals.json",
                history=None,
                output=args.output / "archive.json",
            )
        )
        comparison = compare(
            json.loads((args.output / "archive.json").read_text()),
            json.loads((args.capture / "pair.json").read_text()),
        )
        write(args.output / "comparison.json", comparison)
        print(json.dumps(comparison))


if __name__ == "__main__":
    main()
