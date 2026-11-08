# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Run one disposable, bounded evaluator container on the current Linux host.

Invoke through ordinary SSH. No daemon configuration, package installation,
network listener, SSH key forwarding or Docker socket inside the container.
"""

from __future__ import annotations
import argparse
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import re
import signal
import subprocess
import time
import uuid

CONTEXT = (
    "Dockerfile",
    "requirements.in",
    "requirements.lock",
    "build-requirements.in",
    "build-requirements.lock",
    "pymetis.lock",
    "evaluate.py",
    "model.py",
    "geometry.py",
    "mechanics.py",
    "coupon.py",
)


def command(args, **kwargs):
    return subprocess.run(list(map(str, args)), check=True, **kwargs)


def inventory():
    memory = {}
    for line in Path("/proc/meminfo").read_text().splitlines():
        key, value = line.split(":", 1)
        memory[key] = int(value.split()[0]) * 1024
    return {
        "cpus": len(os.sched_getaffinity(0)),
        "memory_total": memory["MemTotal"],
        "memory_available": memory["MemAvailable"],
        "uid": os.getuid(),
        "gid": os.getgid(),
    }


def limits(machine, requested_memory=None, requested_cpus=None):
    available = min(machine["memory_available"] * 0.75, machine["memory_total"] * 0.6)
    memory = int(available if requested_memory is None else requested_memory * 1024**3)
    cpus = (
        max(1, math.floor(machine["cpus"] * 0.75))
        if requested_cpus is None
        else requested_cpus
    )
    if memory < 256 * 1024**2 or memory > available or not 0 < cpus <= machine["cpus"]:
        raise ValueError("requested resources exceed current host envelope")
    return {
        "memory_bytes": memory,
        "cpus": cpus,
        "pids": max(128, math.ceil(cpus) * 32),
    }


def image_tag(context):
    h = hashlib.sha256()
    for name in CONTEXT:
        h.update(name.encode())
        h.update((context / name).read_bytes())
    return "spiling/interior-evaluator:" + h.hexdigest()[:20]


def build(args, root):
    context = args.context.resolve()
    tag = image_tag(context)
    command(["docker", "build", "--pull", "--tag", tag, context], timeout=900)
    info = json.loads(
        command(
            ["docker", "image", "inspect", tag], capture_output=True, text=True
        ).stdout
    )[0]
    (root / "image.json").write_text(
        json.dumps({"tag": tag, "id": info["Id"], "context": str(context)}, indent=2)
        + "\n"
    )
    print(json.dumps({"image": tag, "id": info["Id"]}), flush=True)


def run(args, root):
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,63}", args.run_id):
        raise ValueError("run ID must be a short lowercase identifier")
    if not math.isfinite(args.seconds) or not 0 < args.seconds <= 86400:
        raise ValueError("finite job deadline of at most one day required")
    source = args.input.resolve(strict=True)
    if not source.is_relative_to(root / "inputs") or not source.is_dir():
        raise ValueError("input must be a directory under this prefix's inputs")
    image = json.loads((root / "image.json").read_text())
    current = json.loads(
        command(
            ["docker", "image", "inspect", image["id"]], capture_output=True, text=True
        ).stdout
    )[0]
    if (
        current["Config"].get("Labels", {}).get("org.spiling.purpose")
        != "offline-interior-evaluator"
    ):
        raise ValueError("image is not an owned evaluator")
    output = root / "runs" / args.run_id
    output.mkdir(parents=True, exist_ok=False)
    cache = root / "cache"
    cache.mkdir(exist_ok=True)
    machine = inventory()
    budget = limits(machine, args.memory_gib, args.cpus)
    name = "spiling-eval-" + args.run_id + "-" + uuid.uuid4().hex[:8]
    receipt = {
        "image": image,
        "launcher_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "input": str(source),
        "backend": args.backend,
        "machine": machine,
        "limits": budget,
        "seconds": args.seconds,
        "container": name,
    }
    (output / "launch.json").write_text(json.dumps(receipt, indent=2) + "\n")
    threads = str(max(1, math.floor(budget["cpus"])))
    create = [
        "docker",
        "create",
        "--name",
        name,
        "--init",
        "--label",
        "org.spiling.purpose=offline-interior-evaluator",
        "--network=none",
        "--read-only",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges",
        "--user",
        f"{os.getuid()}:{os.getgid()}",
        "--memory",
        str(budget["memory_bytes"]),
        "--memory-swap",
        str(budget["memory_bytes"]),
        "--pids-limit",
        str(budget["pids"]),
        "--cpus",
        str(budget["cpus"]),
        "--tmpfs",
        "/tmp:rw,nosuid,nodev,size=1073741824,mode=1777",
        "--mount",
        f"type=bind,src={source},dst=/input,readonly",
        "--mount",
        f"type=bind,src={output},dst=/output",
        "--mount",
        f"type=bind,src={cache},dst=/cache",
        "--env",
        f"OPENBLAS_NUM_THREADS={threads}",
        "--env",
        f"OMP_NUM_THREADS={threads}",
    ]
    if args.backend == "warp":
        create += ["--device=nvidia.com/gpu=all"]
    create += [image["id"], "--backend", args.backend]
    owned = False
    began = time.monotonic()
    try:
        owned = True
        command(create, capture_output=True, text=True, timeout=30)
        with (output / "container.log").open("w") as log:
            result = subprocess.run(
                ["docker", "start", "--attach", name],
                stdout=log,
                stderr=subprocess.STDOUT,
                timeout=args.seconds,
            )
        receipt["docker_exit"] = result.returncode
        with (output / "container.json").open("w") as inspection:
            command(["docker", "inspect", name], stdout=inspection, timeout=15)
        if result.returncode:
            raise RuntimeError(
                f"evaluator exited {result.returncode}; see {output}/container.log"
            )
    except BaseException as error:
        receipt["failure"] = str(error)
        raise
    finally:
        receipt["wall_s"] = time.monotonic() - began
        if owned:
            removal = subprocess.run(
                ["docker", "rm", "--force", name],
                capture_output=True,
                text=True,
                timeout=30,
            )
            receipt["removed"] = (
                removal.returncode == 0 or "No such container" in removal.stderr
            )
            if not receipt["removed"]:
                receipt["cleanup_error"] = removal.stderr
        (output / "launch.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps({"output": str(output), **receipt}), flush=True)
        if owned and not receipt.get("removed"):
            raise RuntimeError("owned container cleanup failed")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--root", type=Path, required=True)
    sub = p.add_subparsers(dest="action", required=True)
    b = sub.add_parser("build")
    b.add_argument("--context", type=Path, required=True)
    r = sub.add_parser("run")
    r.add_argument("--input", type=Path, required=True)
    r.add_argument("--run-id", required=True)
    r.add_argument("--backend", choices=("scipy", "warp", "coupon"), required=True)
    r.add_argument("--seconds", type=float, default=300)
    r.add_argument("--memory-gib", type=float)
    r.add_argument("--cpus", type=float)
    args = p.parse_args()
    root = args.root.expanduser().resolve()
    if not root.is_relative_to(Path.home()) or root == Path.home():
        raise ValueError("dedicated home-directory prefix required")
    root.mkdir(parents=True, exist_ok=True)

    def interrupted(signum, frame):
        raise KeyboardInterrupt(f"signal {signum}")

    for sig in (signal.SIGHUP, signal.SIGTERM, signal.SIGINT):
        signal.signal(sig, interrupted)
    with (root / "worker.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        build(args, root) if args.action == "build" else run(args, root)


if __name__ == "__main__":
    main()
