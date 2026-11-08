# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0
"""Bounded, resumable research trials; candidates never edit the judge or Git."""

from __future__ import annotations
import argparse
from dataclasses import asdict
import fcntl
import json
import os
from pathlib import Path
import random
import resource
import sys
import time

from model import Candidate, Material, digest, file_hash
from native import Native, ROOT, run
from specimens import SIZES, generate

HERE = Path(__file__).resolve().parent
JUDGE_FILES = [
    "model.py",
    "native.py",
    "geometry.py",
    "mechanics.py",
    "lab.py",
    "export-replay.mjs",
    "specimens.py",
    "uv.lock",
]


def dump(path, value):
    with path.open("x") as f:
        json.dump(value, f, sort_keys=True, allow_nan=False)
        f.write("\n")


def bounded():
    """Linux research runner requires external whole-tree limits, not thread hints."""
    group = Path("/proc/self/cgroup").read_text().strip().split("::")[-1]
    base = Path("/sys/fs/cgroup") / group.lstrip("/")
    for name in ("memory.max", "pids.max"):
        value = (base / name).read_text().strip()
        if value == "max" or int(value) <= 0:
            raise RuntimeError(f"external finite {name} required (see runbook)")


def identity(bin_dir):
    return {
        "judge": {p: file_hash(HERE / p) for p in JUDGE_FILES},
        "binaries": {
            p: file_hash(bin_dir / p) for p in ("spiling-cli", "spiling-engine")
        },
        "intent": file_hash(
            ROOT / "fixtures/manufacturing/complete-sparse.intent.json"
        ),
        "decoder": {
            p: file_hash(ROOT / "packages/protocol/src" / p)
            for p in ("replay.ts", "display.ts", "generated.ts")
        },
    }


def pareto(rows):
    """Minimize actual material and motion lower bound; maximize declared stiffness."""
    valid = [r for r in rows if r.get("status") == "ok"]

    def values(r):
        return (r["volume_mm3"], r["motion_s"], -r["mechanics"]["stiffness_n_mm"])

    def dominates(a, b):
        return all(x <= y for x, y in zip(a, b)) and any(x < y for x, y in zip(a, b))

    return [r for r in valid if not any(dominates(values(s), values(r)) for s in valid)]


def seeds():
    # Tuned ordinary controls include solid; no weak single baseline.
    for fraction in (0.1, 0.2, 0.35, 0.5, 0.75, 1.0):
        for axis in ("alternating_xy", "alternating_yx", "x", "y"):
            yield Candidate(fraction=fraction, axis=axis)
    for fraction in (0.1, 0.35):
        for axis in ("alternating_xy", "x"):
            for combined in (0.0, 0.3):
                yield Candidate(
                    fraction=fraction,
                    axis=axis,
                    layer_height=0.1,
                    combine_height=combined,
                )
    for axis in ("alternating_xy", "x"):
        for cut in (0.5, 0.7):
            yield Candidate(
                family="crown", fraction=0.1, axis=axis, cut=cut, region_fraction=0.65
            )
    yield Candidate(family="flanges", fraction=0.1, cut=0.7, region_fraction=0.8)
    for axis in ("x", "y"):
        yield Candidate(
            family="separator", fraction=0.1, axis=axis, cut=0.5, region_fraction=0.4
        )


def mutations(parent, rng):
    params = parent.data()
    field = rng.choice(["fraction", "axis", "family", "cut", "region_fraction"])
    choices = {
        "fraction": [0.05, 0.1, 0.15, 0.25, 0.4, 0.6, 0.9],
        "axis": ["x", "y", "alternating_xy", "alternating_yx"],
        "family": ["uniform", "crown", "separator"],
        "cut": [0.4, 0.5, 0.6, 0.7, 0.8],
        "region_fraction": [0.2, 0.4, 0.65, 0.9, 1.0],
    }
    params[field] = rng.choice(choices[field])
    try:
        return Candidate(**params)
    except ValueError:
        return parent


def evaluate(request):
    bounded()
    from geometry import voxelize
    from mechanics import solve

    began = time.monotonic()
    root = Path(request["root"])
    bin_dir = Path(request["bin_dir"])
    if identity(bin_dir) != request["identity"]:
        raise RuntimeError("judge/producer changed; start a new campaign")
    c = Candidate(**request["candidate"])
    specimen = request["specimen"]
    source = root / "specimens" / f"{specimen}.step"
    native_key = digest(
        {
            "candidate": c.data(),
            "source": file_hash(source),
            "native": {
                k: request["identity"][k] for k in ("binaries", "intent", "decoder")
            }
            | {
                "adapter": {
                    p: request["identity"]["judge"][p]
                    for p in ("model.py", "native.py", "export-replay.mjs")
                }
            },
        }
    )[:24]
    folder = root / "native" / native_key
    complete = sorted(
        p
        for p in (root / "native").glob(native_key + "*")
        if (p / "manifest.json").exists()
    )
    if complete:
        folder = complete[0]
    elif folder.exists():
        folder = folder.with_name(native_key + "-attempt-" + str(time.time_ns()))
    native = Native(
        bin_dir, ROOT / "fixtures/manufacturing/complete-sparse.intent.json"
    )
    if folder.exists():
        manifest = json.loads((folder / "manifest.json").read_text())
        if set(manifest) != {
            "roads.json",
            "actual.splr",
            "receipt.json",
            "project.spil",
        }:
            raise RuntimeError("invalid native cache manifest")
        for name, sha in manifest.items():
            if file_hash(folder / name) != sha:
                raise RuntimeError("native cache hash mismatch: " + name)
        data = json.loads((folder / "roads.json").read_text())
    else:
        data = native.compile(source, SIZES[specimen], c, folder)
        dump(
            folder / "manifest.json",
            {
                p: file_hash(folder / p)
                for p in ("roads.json", "actual.splr", "receipt.json", "project.spil")
            },
        )
    mesh_start = time.monotonic()
    mesh = voxelize(
        data["roads"], dx=request["dx"], dz=request["dz"], phase=tuple(request["phase"])
    )
    mesh_s = time.monotonic() - mesh_start
    result = solve(mesh, request["case"], Material(**request["material"]))
    return {
        "status": "ok",
        "candidate": c.data(),
        "specimen": specimen,
        "native": str(folder.relative_to(root)),
        "replay_sha256": file_hash(folder / "actual.splr"),
        "engine_build": data["engine_build"],
        "source_sha256": file_hash(source),
        "volume_mm3": data["report"]["deposited_volume_mm3"],
        "mass_g": data["report"]["deposited_volume_mm3"]
        * request["material"]["density_g_mm3"],
        "motion_s": data["motion_seconds_lower_bound"],
        "segments": len(data["roads"]),
        "compile_s": data["compile_wall_s"],
        "mesh_s": mesh_s,
        "mechanics": result,
        "wall_s": time.monotonic() - began,
        "peak_rss_kib": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
    }


def study(args):
    bounded()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=True)
    with (root / "writer.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        generate(root / "specimens")
        spec = {
            "identity": identity(args.bin_dir.resolve()),
            "specimen": args.specimen,
            "dx": args.dx,
            "dz": args.dz,
            "phase": args.phase,
            "case": args.case,
            "material": asdict(Material(poisson=args.poisson)),
            "seed": args.seed,
            "search": "controls_then_pareto_mutation_v1",
        }
        judge = digest(spec)
        segment = root / "campaigns" / judge[:20]
        segment.mkdir(parents=True, exist_ok=True)
        if not (segment / "config.json").exists():
            dump(segment / "config.json", spec)
        rows = (
            [
                json.loads(line)
                for line in (segment / "trials.jsonl").read_text().splitlines()
            ]
            if (segment / "trials.jsonl").exists()
            else []
        )
        known = {digest(r["request"]["candidate"]) for r in rows}
        queue = (
            [
                Candidate(**json.loads(line))
                for line in args.candidates.read_text().splitlines()
            ]
            if args.candidates
            else list(seeds())
        )
        rng = random.Random(args.seed)
        end = time.monotonic() + args.seconds
        trials = 0
        attempts = 0
        while (
            trials < args.limit
            and time.monotonic() < end
            and attempts < args.limit * 100 + 1000
        ):
            if identity(args.bin_dir.resolve()) != spec["identity"]:
                raise RuntimeError(
                    "judge/producer changed during campaign; stopped before another trial"
                )
            attempts += 1
            parent = None
            if queue:
                c = queue.pop(0)
            else:
                archive = pareto(rows)
                if not archive:
                    break
                parent = rng.choice(archive)
                c = mutations(Candidate(**parent["candidate"]), rng)
            if digest(c.data()) in known:
                continue
            known.add(digest(c.data()))
            request = {
                **spec,
                "root": str(root),
                "bin_dir": str(args.bin_dir.resolve()),
                "candidate": c.data(),
            }
            key = digest(request)
            trial = segment / key[:20]
            trial.mkdir(exist_ok=False)
            dump(trial / "request.json", request)
            began = time.monotonic()
            try:
                run(
                    [
                        sys.executable,
                        HERE / "lab.py",
                        "worker",
                        trial / "request.json",
                        trial / "result.json",
                    ],
                    timeout=min(args.trial_seconds, max(0.1, end - time.monotonic())),
                    output=trial / "worker",
                )
                row = json.loads((trial / "result.json").read_text())
            except Exception as e:
                row = {
                    "status": "refused_or_failed",
                    "candidate": c.data(),
                    "error": str(e)[-6000:],
                }
            row.update(
                id=key,
                request=request,
                parent_id=parent["id"] if parent else None,
                wall_s=time.monotonic() - began,
            )
            with (segment / "trials.jsonl").open("a") as log:
                log.write(json.dumps(row, sort_keys=True, allow_nan=False) + "\n")
                log.flush()
                os.fsync(log.fileno())
            rows.append(row)
            trials += 1
            print(
                json.dumps(
                    {k: row[k] for k in ("id", "status", "candidate", "wall_s")}
                    | (
                        {
                            "volume": row["volume_mm3"],
                            "K": row["mechanics"]["stiffness_n_mm"],
                        }
                        if row["status"] == "ok"
                        else {"error": row["error"][:300]}
                    )
                ),
                flush=True,
            )
        print(
            json.dumps(
                {
                    "campaign": str(segment),
                    "total": len(rows),
                    "pareto": [r["id"] for r in pareto(rows)],
                }
            ),
            flush=True,
        )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="command", required=True)
    s = sub.add_parser("study")
    s.add_argument("--output", type=Path, required=True)
    s.add_argument("--bin-dir", type=Path, required=True)
    s.add_argument("--specimen", choices=SIZES, default="beam")
    s.add_argument(
        "--case",
        choices=["bend_z", "compression_x", "compression_y", "compression_z"],
        default="bend_z",
    )
    s.add_argument("--dx", type=float, default=0.225)
    s.add_argument("--dz", type=float, default=0.2)
    s.add_argument("--phase", type=float, nargs=2, default=[0.0, 0.0])
    s.add_argument("--poisson", type=float, default=0.35)
    s.add_argument("--limit", type=int, default=25)
    s.add_argument("--seconds", type=float, default=900)
    s.add_argument("--trial-seconds", type=float, default=120)
    s.add_argument("--seed", type=int, default=41)
    s.add_argument("--candidates", type=Path)
    w = sub.add_parser("worker")
    w.add_argument("request", type=Path)
    w.add_argument("result", type=Path)
    a = p.parse_args()
    if a.command == "study":
        study(a)
    else:
        dump(a.result, evaluate(json.loads(a.request.read_text())))


if __name__ == "__main__":
    main()
