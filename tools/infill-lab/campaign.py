# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Capture production constructions, freeze coupon jobs, and evaluate via SSH.

The engine/CLI always execute locally and sequentially. Sparks receive inert
hashed deposition, never a remote project-writer or an exposed Docker daemon.
"""

from __future__ import annotations
import argparse
from concurrent.futures import ThreadPoolExecutor
from dataclasses import asdict
import fcntl
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time

from model import Candidate, Material, digest, file_hash
from native import Native, ROOT
from specimens import SIZES, generate

from remote.coupon import JUDGE
from remote.launch import CONTEXT, image_tag

HERE = Path(__file__).resolve().parent

SHAPES = ("cube32", "beam80", "column48", "panel48", "tube32", "angle40")


def write(path, data):
    path.write_text(json.dumps(data, indent=2, sort_keys=True, allow_nan=False) + "\n")


def candidates():
    """Prespecified standard curves and admitted one-interface reinforcement."""
    yield Candidate(fraction=1)
    for fraction in (0.1, 0.2, 0.35, 0.5):
        yield Candidate(fraction=fraction)
        yield Candidate(fraction=fraction, pattern="grid")
        yield Candidate(fraction=fraction, axis="x")
    for high in (0.35, 0.5):
        for axis in ("x", "y"):
            yield Candidate(
                family="separator",
                fraction=0.1,
                axis=axis,
                cut=0.5,
                region_fraction=high,
            )
    for high in (0.5, 1.0):
        yield Candidate(family="crown", fraction=0.1, cut=0.75, region_fraction=high)


def bounds(name, *, shift=0):
    x, y, z = SIZES[name]
    # Finite cut interior; exclude all exterior walls/skins by replay role,
    # including internal hole walls. This is not a periodic or full-part test.
    middle = z / 2 + shift
    return [[22.0, 22.0, middle - 4], [20 + x - 2, 20 + y - 2, middle + 4]]


def context(destination):
    """Only reviewed evaluator sources enter the Docker context."""
    destination.mkdir(parents=True, exist_ok=False)
    for name in (*CONTEXT, "launch.py"):
        origin = (
            HERE / name
            if name
            in {
                "model.py",
                "geometry.py",
                "mechanics.py",
                "reference.py",
                "quadratic.py",
                "quadratic_preconditioner.py",
                "quadratic_fields.py",
                "vault_mechanics.py",
            }
            else HERE / "remote" / name
        )
        shutil.copyfile(origin, destination / name)
    return image_tag(destination)


def capture(args):
    from lab import bounded, identity

    bounded()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=True)
    with (root / "writer.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        generate(root / "specimens")
        queue = (
            list(candidates())
            if args.candidates is None
            else [
                Candidate(**json.loads(line))
                for line in args.candidates.read_text().splitlines()
                if line.strip()
            ]
        )
        producer = identity(args.bin_dir.resolve())
        spec = {
            "schema": 1,
            "identity": producer,
            "specimens": args.specimens,
            "candidates": [c.data() for c in queue],
        }
        manifest = root / "capture.json"
        if manifest.exists() and json.loads(manifest.read_text()) != spec:
            raise ValueError("capture inputs changed; use a new root")
        if not manifest.exists():
            write(manifest, spec)
        native = Native(
            args.bin_dir.resolve(),
            ROOT / "fixtures/manufacturing/complete-sparse.intent.json",
            timeout=args.trial_seconds,
        )
        log = root / "captures.jsonl"
        rows = (
            [json.loads(line) for line in log.read_text().splitlines()]
            if log.exists()
            else []
        )
        known = {r["id"] for r in rows}
        reuse = {}
        if args.reuse:
            old_root = args.reuse.resolve()
            old_spec = json.loads((old_root / "capture.json").read_text())
            if any(
                old_spec["identity"][k] != producer[k]
                for k in ("binaries", "intent", "decoder")
            ):
                raise ValueError("native cache producer/intent/decoder differs")
            for line in (old_root / "captures.jsonl").read_text().splitlines():
                prior = json.loads(line)
                if (
                    prior["status"] == "ok"
                    and prior["candidate"]["family"] == "uniform"
                ):
                    # Uniform generation is unchanged by the regional grammar.
                    # Verify the retained complete native closure before copy.
                    old = old_root / prior["native"]
                    hashes = json.loads((old / "manifest.json").read_text())
                    if set(hashes) != {
                        "roads.json",
                        "actual.splr",
                        "receipt.json",
                        "project.spil",
                    } or any(file_hash(old / p) != h for p, h in hashes.items()):
                        raise ValueError("retained native manifest differs")
                    data = json.loads((old / "roads.json").read_text())
                    if (
                        data["candidate"] != prior["candidate"]
                        or data["bin_sha256"] != producer["binaries"]
                        or data["process_edits"]
                    ):
                        raise ValueError("retained uniform construction differs")
                    reuse[(prior["specimen"], digest(prior["candidate"]))] = (
                        old,
                        prior,
                    )

        deadline = time.monotonic() + args.seconds
        for name in args.specimens:
            for c in queue:
                key = digest(
                    {"candidate": c.data(), "specimen": name, "identity": producer}
                )
                if key in known:
                    continue
                if time.monotonic() >= deadline:
                    return
                if identity(args.bin_dir.resolve()) != producer:
                    raise ValueError("producer/judge changed during capture")
                source = root / "specimens" / (name + ".step")
                folder = root / "native" / key[:24]
                row = {
                    "id": key,
                    "specimen": name,
                    "candidate": c.data(),
                    "native": str(folder.relative_to(root)),
                }
                try:
                    retained = reuse.get((name, digest(c.data())))
                    if retained:
                        old, prior = retained
                        if file_hash(source) != prior["source_sha256"]:
                            raise ValueError("retained native source differs")
                        shutil.copytree(old, folder)
                        data = json.loads((folder / "roads.json").read_text())
                    else:
                        data = native.compile(source, SIZES[name], c, folder)
                    write(
                        folder / "manifest.json",
                        {
                            p: file_hash(folder / p)
                            for p in (
                                "roads.json",
                                "actual.splr",
                                "receipt.json",
                                "project.spil",
                            )
                        },
                    )
                    row.update(
                        status="ok",
                        source_sha256=data["source_sha256"],
                        roads_sha256=file_hash(folder / "roads.json"),
                        replay_sha256=file_hash(folder / "actual.splr"),
                        volume_mm3=data["report"]["deposited_volume_mm3"],
                    )
                except Exception as error:
                    row.update(status="refused_or_failed", error=str(error))
                with log.open("a") as stream:
                    stream.write(json.dumps(row, allow_nan=False) + "\n")
                    stream.flush()
                    os.fsync(stream.fileno())
                print(json.dumps(row), flush=True)


def pack(args):
    root = args.study_root.resolve()
    ctx = args.context.resolve()
    judge = {p: file_hash(ctx / p) for p in JUDGE}
    rows = [
        json.loads(line) for line in (root / "captures.jsonl").read_text().splitlines()
    ]
    if args.candidates:
        allowed = {
            digest(Candidate(**json.loads(line)).data())
            for line in args.candidates.read_text().splitlines()
            if line.strip()
        }
        rows = [r for r in rows if digest(r["candidate"]) in allowed]
    inputs = root / args.panel
    inputs.mkdir(exist_ok=False)
    jobs = []
    for row in rows:
        if row["status"] != "ok":
            continue
        folder = root / row["native"]
        manifest = json.loads((folder / "manifest.json").read_text())
        if any(file_hash(folder / p) != sha for p, sha in manifest.items()):
            raise ValueError("native capture manifest differs")
        scenarios = []
        for z_ratio in args.z_ratios:
            for division in args.divisions:
                for shift in args.shifts:
                    scenarios.append(
                        {
                            "material": asdict(Material(z_ratio=z_ratio)),
                            "bounds": bounds(row["specimen"], shift=shift),
                            "solver": args.solver,
                            "max_xy": args.dx,
                            "max_z": args.dz,
                            "divisions": division,
                        }
                    )
        spec = {
            "schema": 2,
            "kind": "actual_interior_coupon",
            "judge": judge,
            **{
                k: row[k]
                for k in (
                    "candidate",
                    "specimen",
                    "source_sha256",
                    "roads_sha256",
                    "replay_sha256",
                    "native",
                )
            },
            "scenarios": scenarios,
        }
        key = digest(spec)
        dest = inputs / key
        dest.mkdir()
        shutil.copyfile(folder / "roads.json", dest / "roads.json")
        write(dest / "case.json", spec)
        jobs.append(
            {"id": key, "specimen": row["specimen"], "candidate": row["candidate"]}
        )
    write(
        inputs / "jobs.json",
        {
            "schema": 1,
            "judge": judge,
            "jobs": jobs,
            "capture_sha256": file_hash(root / "capture.json"),
            "failed_captures": [r for r in rows if r["status"] != "ok"],
        },
    )
    print(
        json.dumps(
            {
                "inputs": str(inputs),
                "jobs": len(jobs),
                "failed_captures": len(rows) - len(jobs),
            }
        )
    )


def dispatch(args):
    inputs = args.input.resolve()
    manifest = json.loads((inputs / "jobs.json").read_text())
    if args.exclude:
        excluded = {
            job["id"]
            for source in args.exclude
            for job in json.loads((source / "jobs.json").read_text())["jobs"]
        }
        manifest["jobs"] = [
            job for job in manifest["jobs"] if job["id"] not in excluded
        ]
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    # Host and path are deliberately a small controlled SSH interface. Neither
    # geometry nor input JSON can inject shell commands or select an image.
    for host in args.hosts:
        if not re.fullmatch(r"[a-z0-9_.@-]+", host):
            raise ValueError("invalid SSH host")
    if (
        not re.fullmatch(r"/home/[a-z0-9_-]+/[a-zA-Z0-9_./-]+", args.remote_root)
        or ".." in Path(args.remote_root).parts
    ):
        raise ValueError("absolute dedicated remote prefix required")
    run_id = args.run_id
    if not re.fullmatch(r"[a-z][a-z0-9-]{0,15}", run_id):
        raise ValueError("short lowercase campaign ID required")

    images = []
    for host in args.hosts:
        preflight = subprocess.run(
            [
                "ssh",
                "-o",
                "BatchMode=yes",
                host,
                f"test -f {args.remote_root}/tooling/launch.py && test -d {args.remote_root}/inputs && cat {args.remote_root}/image.json",
            ],
            capture_output=True,
            text=True,
            check=True,
            timeout=30,
        )
        images.append(json.loads(preflight.stdout)["id"])
    if len(set(images)) != 1:
        raise ValueError("hosts have different frozen evaluator images")

    def on_host(slot):
        host = args.hosts[slot]
        assigned = manifest["jobs"][slot :: len(args.hosts)]
        receipts = []
        for job in assigned:
            key = job["id"]
            destination = output / key
            if (destination / "dispatch.json").exists():
                receipts.append(json.loads((destination / "dispatch.json").read_text()))
                continue
            destination.mkdir(exist_ok=True)
            remote_id = run_id + "-" + key[:16]
            remote = args.remote_root
            copy = subprocess.run(
                [
                    "scp",
                    "-q",
                    "-r",
                    str(inputs / key),
                    host + ":" + remote + "/inputs/",
                ],
                capture_output=True,
                text=True,
                timeout=180,
            )
            if copy.returncode:
                raise RuntimeError(copy.stderr)
            cmd = f"python3 {remote}/tooling/launch.py --root {remote} run --input {remote}/inputs/{key} --run-id {remote_id} --backend {args.backend} --seconds {args.seconds}"
            if getattr(args, "cpu_profile", False):
                cmd += " --cpu-profile"
            if args.blas_threads is not None:
                cmd += f" --blas-threads {args.blas_threads}"
            for phase in ("results", "stiffness", "equation"):
                count = getattr(args, f"ccx_{phase}_threads")
                if count is not None:
                    cmd += f" --ccx-{phase}-threads {count}"
            if args.cpus is not None:
                cmd += f" --cpus {args.cpus}"
            if args.memory_gib is not None:
                cmd += f" --memory-gib {args.memory_gib}"
            with (destination / "ssh.log").open("w") as log:
                result = subprocess.run(
                    [
                        "ssh",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "ServerAliveInterval=15",
                        host,
                        cmd,
                    ],
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    timeout=args.seconds + 120,
                )
            fetch = subprocess.run(
                [
                    "scp",
                    "-q",
                    "-r",
                    host + ":" + remote + "/runs/" + remote_id + "/.",
                    str(destination),
                ],
                capture_output=True,
                text=True,
                timeout=180,
            )
            if fetch.returncode:
                raise RuntimeError(
                    "retrieve failed; reconnect before retry: " + fetch.stderr
                )
            receipt = {
                "job": key,
                "host": host,
                "exit_code": result.returncode,
                "remote_run": remote_id,
            }
            write(destination / "dispatch.json", receipt)
            receipts.append(receipt)
            print(json.dumps(receipt), flush=True)
        return receipts

    with ThreadPoolExecutor(max_workers=len(args.hosts)) as pool:
        for result in pool.map(on_host, range(len(args.hosts))):
            print(json.dumps({"host_jobs_complete": len(result)}), flush=True)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="command", required=True)
    c = sub.add_parser("capture")
    c.add_argument("--output", type=Path, required=True)
    c.add_argument("--bin-dir", type=Path, required=True)
    c.add_argument("--specimens", nargs="+", choices=SIZES, default=list(SHAPES))
    c.add_argument("--candidates", type=Path)
    c.add_argument("--reuse", type=Path)
    c.add_argument("--seconds", type=float, default=7200)
    c.add_argument("--trial-seconds", type=int, default=120)
    b = sub.add_parser("context")
    b.add_argument("--output", type=Path, required=True)
    b = sub.add_parser("pack")
    b.add_argument("--study-root", type=Path, required=True)
    b.add_argument("--context", type=Path, required=True)
    b.add_argument("--panel", default="inputs")
    b.add_argument("--candidates", type=Path)
    b.add_argument(
        "--solver", choices=("superlu", "superlu_mmd", "amg"), default="superlu"
    )
    b.add_argument("--dx", type=float, default=4)
    b.add_argument("--dz", type=float, default=1)
    b.add_argument("--z-ratios", type=float, nargs="+", default=[0.25])
    b.add_argument(
        "--divisions", type=int, nargs="+", choices=(1, 2, 4, 8), default=[1]
    )
    b.add_argument("--shifts", type=float, nargs="+", default=[0])
    d = sub.add_parser("dispatch")
    d.add_argument("--input", type=Path, required=True)
    d.add_argument("--output", type=Path, required=True)
    d.add_argument("--exclude", type=Path, nargs="*")
    d.add_argument("--hosts", nargs="+", required=True)
    d.add_argument("--remote-root", required=True)
    d.add_argument("--run-id", required=True)
    d.add_argument("--seconds", type=int, default=600)
    d.add_argument(
        "--backend",
        choices=("coupon", "reference", "quadratic", "vault"),
        default="coupon",
    )
    d.add_argument("--cpus", type=int)
    d.add_argument("--blas-threads", type=int)
    d.add_argument("--cpu-profile", action="store_true")
    for phase in ("results", "stiffness", "equation"):
        d.add_argument(f"--ccx-{phase}-threads", type=int)
    d.add_argument("--memory-gib", type=float)
    args = p.parse_args()
    if args.command == "context":
        print(context(args.output))
    else:
        {"capture": capture, "pack": pack, "dispatch": dispatch}[args.command](args)


if __name__ == "__main__":
    main()
