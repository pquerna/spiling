# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""One bounded code-search numerical job; Docker/prefix only, no host installs."""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import uuid


def run(a):
    root = a.root.resolve()
    source = root / "inputs" / a.name
    output = root / "results" / a.name
    if not a.name.replace("_", "").replace("-", "").isalnum():
        raise ValueError("invalid run name")
    output.mkdir(parents=True, exist_ok=False)
    memory = {
        line.split(":")[0]: int(line.split()[1]) * 1024
        for line in Path("/proc/meminfo").read_text().splitlines()
    }
    cpus = len(os.sched_getaffinity(0))
    limit = 40 * 1024**3
    if memory["MemAvailable"] < limit / 0.8 or cpus < 3:
        raise ValueError("job does not fit live headroom")
    name = "spiling-code-" + a.name + "-" + uuid.uuid4().hex[:8]
    args = [
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
        str(limit),
        "--memory-swap",
        str(limit),
        "--pids-limit",
        "256",
        "--cpus",
        "3",
        "--tmpfs",
        "/tmp:rw,nosuid,nodev,size=2g",
        "-e",
        "OPENBLAS_NUM_THREADS=1",
        "-e",
        "OMP_NUM_THREADS=1",
        "-e",
        "PYTHONPATH=/judge:/opt/spiling-eval",
        "-v",
        str(root / "judge") + ":/judge:ro",
        "-v",
        str(source) + ":/input:ro",
        "-v",
        str(output) + ":/output:rw",
        "--entrypoint",
        "python",
        a.image,
        "/judge/"
        + (
            "roof_algorithm_judge.py"
            if json.loads((source / "case.json").read_text()).get("kind") == "roof"
            else "algorithm_judge.py"
        ),
    ]
    began = time.monotonic()
    receipt = dict(
        image=a.image,
        seconds=a.seconds,
        memory_bytes=limit,
        cpus=3,
        blas_threads=1,
        memory_available=memory["MemAvailable"],
        host_cpus=cpus,
        container=name,
    )
    try:
        subprocess.run(args, check=True, capture_output=True, timeout=30)
        p = subprocess.run(
            ["docker", "start", "-a", name],
            capture_output=True,
            text=True,
            timeout=a.seconds,
        )
        (output / "stdout.log").write_text(p.stdout + p.stderr)
        receipt["exit_code"] = p.returncode
    except Exception as error:
        receipt.update(error=str(error), status="failed")
    finally:
        info = subprocess.run(
            ["docker", "inspect", name], capture_output=True, text=True, timeout=30
        )
        if info.returncode == 0:
            receipt["state"] = json.loads(info.stdout)[0]["State"]
        subprocess.run(["docker", "rm", "-f", name], capture_output=True, timeout=30)
        receipt["wall_s"] = time.monotonic() - began
        (output / "launch.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt), flush=True)


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--root", type=Path, required=True)
    p.add_argument("--name", required=True)
    p.add_argument("--image", required=True)
    p.add_argument("--seconds", type=int, default=600)
    run(p.parse_args())
