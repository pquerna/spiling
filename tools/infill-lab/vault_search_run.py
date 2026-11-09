# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Execute/resume the bounded 50-candidate Vault search with frozen evaluators.

Provision reviewed isolated prefixes first. This driver installs nothing and
never changes Git, solver sources, native binaries or production defaults.
Run with fixture dependencies inside external native memory/task/time bounds.
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
import json
import math
import shlex
from pathlib import Path
import subprocess
import time
from types import SimpleNamespace as Args

from campaign import capture, dispatch
from lab import bounded
from model import file_hash
from vault_search import collect, core_pack, join_core, pack, propose, write


@dataclass(frozen=True)
class Workers:
    hosts: list[str]
    roof_prefixes: list[str]
    core_prefixes: list[str]
    memory_gib: float
    cpus: int
    blas_threads: int = 1


def admitted_slots(machines, workers, count):
    if (
        not math.isfinite(workers.memory_gib)
        or workers.memory_gib <= 0
        or type(workers.cpus) is not int
        or workers.cpus <= 0
    ):
        raise ValueError("finite positive worker budgets required")
    for machine in machines:
        count = min(
            count,
            int(0.8 * machine["memory_available"] / (workers.memory_gib * 1024**3)),
            int(0.75 * machine["cpus"] / workers.cpus),
        )
    if count < 1:
        raise ValueError("no worker fits live host headroom")
    return count


def inventory(host, prefix):
    # Trusted reviewed launcher, read-only inventory; no package or daemon edits.
    code = f"import runpy,json; state=runpy.run_path({(prefix + '/tooling/launch.py')!r}); print(json.dumps(state['inventory']()))"
    return json.loads(
        subprocess.check_output(
            ["ssh", "-o", "BatchMode=yes", host, "python3 -c " + shlex.quote(code)],
            text=True,
            timeout=30,
        )
    )


def dispatch_panel(workers, kind, inputs, results, round_number, trial_seconds):
    prefixes = getattr(workers, kind + "_prefixes")
    if not prefixes or len(set(prefixes)) != len(prefixes):
        raise ValueError("distinct explicitly admitted worker prefixes required")

    machines = [inventory(host, prefixes[0]) for host in workers.hosts]
    slots = admitted_slots(machines, workers, len(prefixes))
    prefixes = prefixes[:slots]
    record = dict(
        kind=kind,
        round=round_number,
        machines=machines,
        slots=slots,
        memory_gib_per_job=workers.memory_gib,
        cpu_quota_per_job=workers.cpus,
    )
    with (results.parent / "admissions.jsonl").open("a") as stream:
        stream.write(json.dumps(record) + "\n")
    print(json.dumps(dict(admission=record)), flush=True)

    def shard(slot):
        dispatch(
            Args(
                input=inputs,
                output=results,
                exclude=None,
                hosts=workers.hosts,
                remote_root=prefixes[slot],
                run_id=f"v{kind[0]}{round_number}s{slot}",
                seconds=trial_seconds,
                backend="vault" if kind == "roof" else "quadratic",
                cpus=workers.cpus,
                memory_gib=workers.memory_gib,
                blas_threads=workers.blas_threads,
                cpu_profile=False,
                ccx_results_threads=None,
                ccx_stiffness_threads=None,
                ccx_equation_threads=None,
                shard_index=slot,
                shard_count=len(prefixes),
            )
        )

    with ThreadPoolExecutor(max_workers=len(prefixes)) as pool:
        for _ in pool.map(shard, range(len(prefixes))):
            pass


def run(args):
    bounded()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=True)
    workers = Workers(**json.loads(args.workers.read_text()))
    manifest = dict(
        controller=file_hash(Path(__file__)),
        search=file_hash(Path(__file__).with_name("vault_search.py")),
        workers=json.loads(args.workers.read_text()),
        context={p.name: file_hash(p) for p in args.context.iterdir() if p.is_file()},
        bin_dir=str(args.bin_dir.resolve()),
        specimen="cube48",
        roof=dict(width=8, depth=4, max_z=1, divisions=1, solver=args.solver),
        core=dict(width=40, depth=8, max_z=1, divisions=1),
        rounds=5,
    )
    config = root / "search.json"
    if config.exists():
        if json.loads(config.read_text()) != manifest:
            raise ValueError("frozen search configuration changed; use a new study")
    else:
        write(config, manifest)
    deadline = time.monotonic() + args.seconds
    for number in range(5):
        if time.monotonic() >= deadline:
            raise TimeoutError("bounded search deadline reached between rounds")
        archive = root / f"archive{number}.json"
        if archive.exists():
            continue
        previous = root / f"archive{number - 1}.json" if number else None
        proposals = root / f"round{number}"
        if not proposals.exists():
            propose(Args(round=number, history=previous, output=proposals))
        native = root / f"capture{number}"
        capture(
            Args(
                output=native,
                bin_dir=args.bin_dir,
                candidates=proposals / "queue.jsonl",
                specimens=["cube48"],
                seconds=min(1800, max(1, deadline - time.monotonic())),
                trial_seconds=180,
                reuse=None,
            )
        )
        roof_inputs, roof_results = (
            root / f"roof-inputs{number}",
            root / f"roof-results{number}",
        )
        if not roof_inputs.exists():
            pack(
                Args(
                    capture=native,
                    context=args.context,
                    output=roof_inputs,
                    selection=None,
                    width=8,
                    depth=4,
                    shift=0,
                    max_z=1,
                    divisions=1,
                    solver=args.solver,
                )
            )
        dispatch_panel(
            workers, "roof", roof_inputs, roof_results, number, args.trial_seconds
        )
        roof_archive = root / f"roof-archive{number}.json"
        if not roof_archive.exists():
            collect(
                Args(
                    input=roof_inputs,
                    results=roof_results,
                    proposals=proposals / "proposals.json",
                    history=root / f"roof-archive{number - 1}.json" if number else None,
                    output=roof_archive,
                )
            )
        core_inputs, core_results = (
            root / f"core-inputs{number}",
            root / f"core-results{number}",
        )
        if not core_inputs.exists():
            core_pack(
                Args(
                    capture=native,
                    context=args.context,
                    output=core_inputs,
                    width=40,
                    divisions=1,
                )
            )
        dispatch_panel(
            workers, "core", core_inputs, core_results, number, args.trial_seconds
        )
        join_core(
            Args(
                roof=roof_archive,
                input=core_inputs,
                results=core_results,
                history=previous,
                output=archive,
            )
        )
        print(json.dumps(dict(round=number, archive=str(archive))), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("output", "bin-dir", "context", "workers"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument(
        "--solver",
        choices=("superlu", "superlu_mmd", "p_amg", "amg"),
        default="superlu",
    )
    parser.add_argument("--seconds", type=int, default=10800)
    parser.add_argument("--trial-seconds", type=int, default=300)
    args = parser.parse_args()
    if not 0 < args.seconds <= 86400 or not 0 < args.trial_seconds <= 3600:
        raise ValueError("finite study/trial budgets required")
    run(args)


if __name__ == "__main__":
    main()
