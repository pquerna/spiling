# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0
"""Propose grid densities under an actual-deposition budget, without seeing stiffness."""

from __future__ import annotations
import argparse
from dataclasses import replace
import json
import math
from pathlib import Path

from lab import bounded, dump
from model import Candidate, digest, file_hash
from native import Native, ROOT
from specimens import SIZES, generate


def refine_budget(measure, budget_mm3, *, rounds=4, ceiling=0.4):
    """Bounded cost-only bisection heuristic; retain every measured outcome.

    Row counts are discrete, and bridges can change dose. This does not assume
    a smooth/invertible dose law or claim exact mass matching or optimality.
    """
    if (
        not math.isfinite(budget_mm3)
        or budget_mm3 <= 0
        or not 1 <= rounds <= 16
        or not 0 < ceiling <= 1
    ):
        raise ValueError("invalid bounded material proposal request")
    low, high = 0.0, ceiling
    rows = []
    for _ in range(rounds):
        fraction = (low + high) / 2
        row = {"fraction": fraction}
        try:
            volume = float(measure(fraction))
            if not math.isfinite(volume) or volume <= 0:
                raise ValueError("invalid measured dose")
            row.update(
                status="measured", volume_mm3=volume, within_budget=volume <= budget_mm3
            )
            if volume <= budget_mm3:
                low = fraction
            else:
                high = fraction
        except RuntimeError as error:
            # A native refusal is evidence, not permission to fabricate a dose.
            # Stop: no monotone geometry/authority inference follows from refusal.
            row.update(status="refused", error=str(error))
            rows.append(row)
            break
        rows.append(row)
    return rows


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--study-root", type=Path, required=True)
    p.add_argument("--bin-dir", type=Path, required=True)
    p.add_argument("--specimen", choices=SIZES, required=True)
    p.add_argument("--budget-mm3", type=float, required=True)
    p.add_argument("--rounds", type=int, default=4)
    a = p.parse_args()
    bounded()
    if a.output.exists() or a.output.is_symlink():
        raise FileExistsError(a.output)
    root = a.study_root.resolve()
    generate(root / "specimens")
    source = root / "specimens" / f"{a.specimen}.step"
    config = {
        "specimen": a.specimen,
        "source_sha256": file_hash(source),
        "budget_mm3": a.budget_mm3,
        "rounds": a.rounds,
        "proposal_sha256": file_hash(Path(__file__)),
        "binaries": {
            name: file_hash(a.bin_dir / name)
            for name in ("spiling-cli", "spiling-engine")
        },
    }
    folder = root / "proposals" / digest(config)[:20]
    folder.mkdir(parents=True, exist_ok=False)
    dump(folder / "request.json", config)
    native = Native(
        a.bin_dir.resolve(), ROOT / "fixtures/manufacturing/complete-sparse.intent.json"
    )
    base = Candidate(pattern="grid")

    def measure(fraction):
        if file_hash(source) != config["source_sha256"] or any(
            file_hash(a.bin_dir / name) != sha
            for name, sha in config["binaries"].items()
        ):
            raise RuntimeError("proposal source or producer changed")
        c = replace(base, fraction=fraction)
        data = native.compile(source, SIZES[a.specimen], c, folder / c.identity)
        if (
            data["source_sha256"] != config["source_sha256"]
            or data["bin_sha256"] != config["binaries"]
        ):
            raise RuntimeError("proposal source or producer changed during capture")
        return data["report"]["deposited_volume_mm3"]

    rows = refine_budget(measure, a.budget_mm3, rounds=a.rounds)
    dump(folder / "costs.json", rows)
    with a.output.open("x") as stream:
        for row in rows:
            if row.get("within_budget"):
                stream.write(
                    json.dumps(replace(base, fraction=row["fraction"]).data()) + "\n"
                )
    print(
        json.dumps(
            {
                "costs": rows,
                "proposals": str(a.output),
                "stiffness_used_for_selection": False,
            }
        )
    )


if __name__ == "__main__":
    main()
