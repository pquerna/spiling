# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Snapshot algorithm edits, freeze their judge and dispatch inert research rows.

No automatic git reset, commit, or promotion. Only interior_algorithm.py is a
candidate. Every proposal has immutable code/input hashes and a named parent.
"""
from __future__ import annotations
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import re
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def write(path, data):
    path.write_text(json.dumps(data, indent=2, allow_nan=False) + "\n")


def ssh(host, args, **kw):
    return subprocess.run(
        [
            "ssh",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            host,
            shlex.join(list(map(str, args))),
        ],
        check=True,
        **kw,
    )


def freeze(
    root,
    baseline,
    context,
    scenario,
    kind="lower",
    roof_bounds=None,
    solver=None,
    *,
    hosts,
    remote_root,
    image,
):
    root.mkdir(parents=True, exist_ok=False)
    judge = root / "judge"
    judge.mkdir()
    shutil.copyfile(HERE / "algorithm_judge.py", judge / "algorithm_judge.py")
    shutil.copyfile(HERE / "algorithm_remote.py", judge / "algorithm_remote.py")
    if kind == "roof":
        shutil.copyfile(
            HERE / "roof_algorithm_judge.py", judge / "roof_algorithm_judge.py"
        )
    shutil.copyfile(baseline, root / "baseline.json")
    spec = json.loads(scenario.read_text())
    refinement = spec["scenario"]["refinement"]
    if refinement.get("cap_segments", 32) not in (4, 8, 16, 32):
        raise ValueError("unsupported explicit capsule refinement")
    hashes = {
        **{
            name: sha(context / name)
            for name in spec["judge"]
            if name not in ("algorithm_judge.py", "roof_algorithm_judge.py")
        },
        "algorithm_judge.py": sha(judge / "algorithm_judge.py"),
    }
    if kind == "roof":
        hashes["roof_algorithm_judge.py"] = sha(judge / "roof_algorithm_judge.py")
    write(
        root / "freeze.json",
        dict(
            judge=hashes,
            image=image,
            kind=kind,
            roof_scenario=dict(
                spec["scenario"],
                solver=solver or spec["scenario"]["solver"],
                bounds=(
                    [roof_bounds[:3], roof_bounds[3:]]
                    if roof_bounds
                    else [[25.466815, 41.75, 40.0], [46.25, 46.25, 48.0]]
                ),
            ),
            baseline_sha256=sha(root / "baseline.json"),
            scenario=dict(
                spec["scenario"], solver=solver or spec["scenario"]["solver"]
            ),
            editable_source="tools/infill-lab/interior_algorithm.py",
            promotion="requires_native_replay_not_research_admission",
        ),
    )
    (root / "trials").mkdir()
    (root / "results").mkdir()
    (root / ".auto").mkdir()
    (root / ".auto/log.jsonl").touch()
    cohort = sha(root / "freeze.json")[:16]
    prefix = remote_root.rstrip("/") + "/" + cohort
    write(root / "dispatch.json", dict(hosts=hosts, remote_root=prefix))
    for host in hosts:
        ssh(
            host,
            ["mkdir", "-p", prefix + "/judge", prefix + "/inputs", prefix + "/results"],
        )
        subprocess.run(
            [
                "scp",
                "-q",
                str(judge / "algorithm_judge.py"),
                str(judge / "algorithm_remote.py"),
                *([str(judge / "roof_algorithm_judge.py")] if kind == "roof" else []),
                host + ":" + prefix + "/judge/",
            ],
            check=True,
        )


def valid_name(name):
    if not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_-]{0,79}", name):
        raise ValueError("invalid trial name")
    return name


def prepare(root, name, parent, hypothesis):
    valid_name(name)
    if parent is not None:
        valid_name(parent)
    frozen = json.loads((root / "freeze.json").read_text())
    for judge_name in ("algorithm_judge.py", "roof_algorithm_judge.py"):
        if (
            judge_name in frozen["judge"]
            and sha(HERE / judge_name) != frozen["judge"][judge_name]
        ):
            raise ValueError("judge source edited after freeze")
    folder = root / "trials" / name
    folder.mkdir(exist_ok=False)
    shutil.copyfile(HERE / "interior_algorithm.py", folder / "algorithm.py")
    shutil.copyfile(root / "baseline.json", folder / "baseline.json")
    script = """import importlib.util,json,sys,time
from pathlib import Path
sys.path.insert(0,sys.argv[1])
from algorithm_judge import context
p=Path(sys.argv[2]);s=importlib.util.spec_from_file_location('candidate',p/'algorithm.py');m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
b=json.loads((p/'baseline.json').read_text());t=time.monotonic();rows=m.generate(context(b))
(p/'sparse.json').write_text(json.dumps(rows,allow_nan=False))
(p/'generation.json').write_text(json.dumps(dict(wall_s=time.monotonic()-t,rows=len(rows))))
"""
    spec = {
        **frozen,
        "name": name,
        "parent": parent,
        "hypothesis": hypothesis,
        "algorithm_sha256": sha(folder / "algorithm.py"),
        "sparse_sha256": None,
    }
    write(folder / "case.json", spec)
    try:
        subprocess.run(
            [sys.executable, "-c", script, str(HERE), str(folder)],
            check=True,
            timeout=30,
        )
    except (subprocess.SubprocessError, OSError) as error:
        receipt = dict(status="failed", phase="generation", error=str(error))
        write(folder / "admission.json", receipt)
        with (root / ".auto/log.jsonl").open("a") as stream:
            stream.write(
                json.dumps(
                    dict(
                        event="proposed",
                        name=name,
                        parent=parent,
                        hypothesis=hypothesis,
                        algorithm_sha256=spec["algorithm_sha256"],
                        **receipt,
                    )
                )
                + "\n"
            )
        print(json.dumps(dict(name=name, **receipt)), flush=True)
        return receipt
    spec["sparse_sha256"] = sha(folder / "sparse.json")
    write(folder / "case.json", spec)
    # Admission first locally; refused algorithms are still immutable attempts.
    from algorithm_judge import admit, accounting

    data = json.loads((root / "baseline.json").read_text())
    rows = json.loads((folder / "sparse.json").read_text())
    try:
        if frozen.get("kind") == "roof":
            from roof_algorithm_judge import admit_roof

            admission = admit_roof(data, rows)
        else:
            admission = admit(data, rows)
        receipt = dict(status="admitted", admission=admission, **accounting(data, rows))
    except Exception as error:
        receipt = dict(status="refused", error=str(error))
    write(folder / "admission.json", receipt)
    with (root / ".auto/log.jsonl").open("a") as stream:
        stream.write(
            json.dumps(
                dict(
                    event="proposed",
                    name=name,
                    parent=parent,
                    hypothesis=hypothesis,
                    algorithm_sha256=spec["algorithm_sha256"],
                    **receipt,
                )
            )
            + "\n"
        )
    print(json.dumps(dict(name=name, **receipt)), flush=True)
    return receipt


def run_one(root, name, host, seconds):
    valid_name(name)
    if (root / "results" / name).exists():
        raise ValueError("trial already has a result; prepare a new named repetition")
    folder = root / "trials" / name
    prefix = json.loads((root / "dispatch.json").read_text())["remote_root"]
    image = json.loads((folder / "case.json").read_text())["image"]
    receipt = json.loads((folder / "admission.json").read_text())
    if receipt["status"] != "admitted":
        return dict(name=name, status="refused")
    subprocess.run(
        ["scp", "-q", "-r", str(folder), host + ":" + prefix + "/inputs/"],
        check=True,
        timeout=120,
    )
    result = subprocess.run(
        [
            "ssh",
            "-o",
            "BatchMode=yes",
            host,
            shlex.join(
                [
                    "python3",
                    prefix + "/judge/algorithm_remote.py",
                    "--root",
                    prefix,
                    "--name",
                    name,
                    "--image",
                    image,
                    "--seconds",
                    str(seconds),
                ]
            ),
        ],
        capture_output=True,
        text=True,
        timeout=seconds + 120,
    )
    (root / "results" / f"{name}.log").write_text(result.stdout + result.stderr)
    subprocess.run(
        [
            "scp",
            "-q",
            "-r",
            host + ":" + prefix + "/results/" + name,
            str(root / "results"),
        ],
        check=True,
        timeout=120,
    )
    output = root / "results" / name / "trial.json"
    if output.exists():
        row = json.loads(output.read_text())
    else:
        launch_path = root / "results" / name / "launch.json"
        launch = json.loads(launch_path.read_text()) if launch_path.exists() else {}
        state = launch.get("state", {})
        row = dict(
            name=name,
            status="failed",
            error=(
                "container OOMKilled"
                if state.get("OOMKilled")
                else launch.get(
                    "error",
                    f"container exit {state.get('ExitCode','unknown')}; SSH exit {result.returncode}",
                )
            ),
            launch=launch,
        )
        write(root / "results" / name / "failure.json", row)
    with (root / ".auto/log.jsonl").open("a") as stream:
        stream.write(
            json.dumps(
                dict(
                    event="evaluated",
                    host=host,
                    **{
                        k: v
                        for k, v in row.items()
                        if k not in ("components", "mesh", "judge")
                    },
                )
            )
            + "\n"
        )
    print(
        json.dumps(
            {
                k: v
                for k, v in row.items()
                if k not in ("components", "mesh", "judge", "admission")
            }
        ),
        flush=True,
    )
    return row


def capacity(available_bytes, cpus):
    # Reserve the complete allowed resident set of every simultaneous job,
    # rather than admitting two jobs against the same still-free RAM.
    return min(2, int(available_bytes * 0.8 // (40 * 1024**3)), int(cpus) // 3)


def run(root, names, seconds):
    # Two separate 40GiB jobs per host; live admission happens remotely before
    # every container. Avoid BLAS oversubscription and bound each entire process.
    hosts = json.loads((root / "dispatch.json").read_text())["hosts"]
    slots = []
    inventories = {}
    for host in hosts:
        script = "from pathlib import Path;import os,json;m={l.split(':')[0]:int(l.split()[1])*1024 for l in Path('/proc/meminfo').read_text().splitlines()};print(json.dumps([m['MemAvailable'],len(os.sched_getaffinity(0))]))"
        response = ssh(
            host, ["python3", "-c", script], capture_output=True, text=True, timeout=30
        )
        available, cpus = json.loads(response.stdout)
        count = capacity(available, cpus)
        inventories[host] = dict(available_bytes=available, cpus=cpus, slots=count)
        for index in range(count):
            slots.append((index, host))
    slots = [host for _, host in sorted(slots)]
    write(
        root / ".auto" / ("dispatch-" + str(time.time_ns()) + ".json"),
        dict(inventory=inventories, names=names, slots=slots),
    )
    if not slots:
        raise ValueError("no evaluator has reserved RAM/CPU headroom for a job")

    def worker(slot):
        return [
            run_one(root, name, slots[slot], seconds)
            for name in names[slot :: len(slots)]
        ]

    with ThreadPoolExecutor(max_workers=len(slots)) as pool:
        list(pool.map(worker, range(len(slots))))


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--root", type=Path, required=True)
    q = p.add_subparsers(dest="command", required=True)
    f = q.add_parser("freeze")
    f.add_argument("--hosts", nargs="+", required=True)
    f.add_argument("--remote-root", required=True)
    f.add_argument("--image", required=True)
    f.add_argument("--kind", choices=["lower", "roof"], default="lower")
    f.add_argument("--roof-bounds", type=float, nargs=6)
    f.add_argument("--solver", choices=["p_amg", "superlu_mmd"])
    f.add_argument("--baseline", type=Path, required=True)
    f.add_argument("--context", type=Path, required=True)
    f.add_argument("--scenario", type=Path, required=True)
    f = q.add_parser("prepare")
    f.add_argument("name")
    f.add_argument("--parent")
    f.add_argument("--hypothesis", required=True)
    f = q.add_parser("run")
    f.add_argument("names", nargs="+")
    f.add_argument("--seconds", type=int, default=600)
    a = p.parse_args()
    if a.command == "freeze":
        freeze(
            a.root,
            a.baseline,
            a.context,
            a.scenario,
            a.kind,
            a.roof_bounds,
            a.solver,
            hosts=a.hosts,
            remote_root=a.remote_root,
            image=a.image,
        )
    elif a.command == "prepare":
        prepare(a.root, a.name, a.parent, a.hypothesis)
    else:
        run(a.root, a.names, a.seconds)


if __name__ == "__main__":
    main()
