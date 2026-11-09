# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Rust-only source-edit trials through native compile, replay and saved return.

Python coordinates and accounts for results; it never constructs candidate roads.
No git reset, source checkout, automatic promotion or verification bypass.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import shutil
import subprocess
import time
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
BUILD_ENV = (
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_BUILD_TARGET",
    "CARGO_PROFILE_DEV_OPT_LEVEL",
    "CARGO_INCREMENTAL",
)
EDITABLE = (
    "crates/manufacturing/src/planner/region/web_roof.rs",
    "crates/manufacturing/src/planner/region/web_roof/architecture.rs",
    "crates/manufacturing/src/planner/region/web_roof/joints.rs",
)


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def write(path, value):
    with path.open("x") as stream:
        stream.write(json.dumps(value, indent=2, allow_nan=False) + "\n")


def valid_name(name):
    if not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_-]{0,79}", name):
        raise ValueError("invalid trial name")
    return name


def source_inventory():
    listed = (
        subprocess.check_output(
            ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
            cwd=REPO,
        )
        .decode()
        .split("\0")
    )
    result = {}
    for name in listed:
        if not name:
            continue
        path = REPO / name
        if path.is_symlink():
            raise ValueError(
                f"source symlink is outside the frozen source contract: {name}"
            )
        if path.is_file():
            result[name] = sha(path)
    return result


def checked_sources(frozen):
    current = source_inventory()
    expected = frozen["fixed_sources"]
    fixed = {k: v for k, v in current.items() if k not in EDITABLE}
    if fixed != expected:
        changed = sorted(
            k for k in set(fixed) | set(expected) if fixed.get(k) != expected.get(k)
        )
        raise ValueError(
            f"frozen source/judge changed outside Rust generator: {changed[:12]}"
        )
    return {name: current[name] for name in EDITABLE}


def freeze(root, candidates, specimens):
    from model import Candidate
    from specimens import SIZES

    changed = (
        subprocess.check_output(["git", "diff", "--name-only", "-z", "HEAD"], cwd=REPO)
        .decode()
        .split("\0")
    )
    untracked = (
        subprocess.check_output(
            ["git", "ls-files", "--others", "--exclude-standard", "-z"], cwd=REPO
        )
        .decode()
        .split("\0")
    )
    if any(name and name not in EDITABLE for name in changed + untracked):
        raise ValueError(
            "freeze requires a committed baseline outside the Rust generator"
        )
    if (
        not specimens
        or any(name not in SIZES for name in specimens)
        or len(set(specimens)) != len(specimens)
    ):
        raise ValueError("unknown or duplicate native specimens")
    queue = [
        Candidate(**json.loads(line)).data()
        for line in candidates.read_text().splitlines()
        if line.strip()
    ]
    if not queue or len({json.dumps(row, sort_keys=True) for row in queue}) != len(
        queue
    ):
        raise ValueError("empty or duplicate frozen native candidate panel")
    inventory = source_inventory()
    if any(name not in inventory for name in EDITABLE):
        raise ValueError("Rust generator source missing")
    if (
        root.is_relative_to(REPO)
        and subprocess.run(
            ["git", "check-ignore", "-q", str(root)], cwd=REPO
        ).returncode
        != 0
    ):
        raise ValueError(
            "cohort must be outside the checkout or in an ignored directory"
        )
    root.mkdir(parents=True, exist_ok=False)
    (root / "trials").mkdir()
    (root / "events.jsonl").touch()
    (root / "candidates.jsonl").write_text(
        "".join(json.dumps(row) + "\n" for row in queue)
    )
    frozen = dict(
        schema=1,
        generation="native_rust_compile_fresh_replay_source_absent_return",
        editable_sources=list(EDITABLE),
        fixed_sources={k: v for k, v in inventory.items() if k not in EDITABLE},
        initial_sources={k: inventory[k] for k in EDITABLE},
        candidate_sha256=sha(root / "candidates.jsonl"),
        specimens=specimens,
        rustc=subprocess.check_output(["rustc", "--version", "--verbose"], text=True),
        build_environment={name: os.environ.get(name) for name in BUILD_ENV},
        base_commit=subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=REPO, text=True
        ).strip(),
        mechanical_scope="Native construction acceptance is not a mechanics score. Use a separately frozen evaluator cohort.",
    )
    write(root / "freeze.json", frozen)
    return frozen


def event(root, value):
    with (root / "events.jsonl").open("a") as stream:
        stream.write(json.dumps(value, allow_nan=False) + "\n")


def prepare(root, name, parent, hypothesis):
    valid_name(name)
    if parent is not None:
        valid_name(parent)
        if not (root / "trials" / parent / "case.json").is_file():
            raise ValueError("parent trial is missing")
    frozen = json.loads((root / "freeze.json").read_text())
    sources = checked_sources(frozen)
    folder = root / "trials" / name
    folder.mkdir(exist_ok=False)
    for path in EDITABLE:
        target = folder / "source" / path
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(REPO / path, target)
    case = dict(
        name=name,
        parent=parent,
        hypothesis=hypothesis,
        generation=frozen["generation"],
        freeze_sha256=sha(root / "freeze.json"),
        rust_sources=sources,
    )
    write(folder / "case.json", case)
    event(root, dict(event="prepared", **case))
    return case


def run(root, name, *, jobs=1, seconds=900):
    from lab import bounded

    bounded()
    valid_name(name)
    if jobs < 1 or seconds < 1:
        raise ValueError("positive external build/deadline reservation required")
    folder = root / "trials" / name
    if (folder / "result.json").exists():
        raise ValueError("trial already evaluated; use a new named trial")
    case = json.loads((folder / "case.json").read_text())
    frozen = json.loads((root / "freeze.json").read_text())
    began = time.monotonic()
    result = dict(
        name=name,
        parent=case["parent"],
        hypothesis=case["hypothesis"],
        generation=frozen["generation"],
        rust_sources=case["rust_sources"],
        native_replayed=False,
    )
    try:
        if (
            case["freeze_sha256"] != sha(root / "freeze.json")
            or checked_sources(frozen) != case["rust_sources"]
        ):
            raise ValueError("prepared source no longer matches this immutable trial")
        if sha(root / "candidates.jsonl") != frozen["candidate_sha256"]:
            raise ValueError("frozen candidate panel changed")
        for path, expected in case["rust_sources"].items():
            if sha(folder / "source" / path) != expected:
                raise ValueError("archived Rust snapshot changed")
        if (
            subprocess.check_output(["rustc", "--version", "--verbose"], text=True)
            != frozen["rustc"]
        ):
            raise ValueError("frozen Rust compiler changed")
        if {name: os.environ.get(name) for name in BUILD_ENV} != frozen[
            "build_environment"
        ]:
            raise ValueError("frozen Rust build settings changed")
        from native import run as execute

        execute(
            [
                "cargo",
                "build",
                "--locked",
                f"-j{jobs}",
                "-p",
                "spiling-engine",
                "-p",
                "spiling-cli",
            ],
            cwd=REPO,
            timeout=seconds,
            output=folder / "build",
        )
        if checked_sources(frozen) != case["rust_sources"]:
            raise ValueError("source changed during build")
        target = Path(os.environ.get("CARGO_TARGET_DIR", REPO / "target"))
        if not target.is_absolute():
            target = REPO / target
        bins = folder / "bin"
        bins.mkdir()
        for executable in ("spiling-engine", "spiling-cli"):
            shutil.copy2(target / "debug" / executable, bins / executable)
        result["binaries"] = {p.name: sha(p) for p in bins.iterdir()}
        from campaign import capture

        capture(
            SimpleNamespace(
                output=folder / "capture",
                bin_dir=bins,
                candidates=root / "candidates.jsonl",
                specimens=frozen["specimens"],
                seconds=seconds,
                trial_seconds=min(seconds, 120),
                reuse=None,
            )
        )
        rows = [
            json.loads(line)
            for line in (folder / "capture/captures.jsonl").read_text().splitlines()
        ]
        panel = (root / "candidates.jsonl").read_text().splitlines()
        if len(rows) != len(frozen["specimens"]) * len(panel):
            raise ValueError("native capture panel incomplete")
        if checked_sources(frozen) != case["rust_sources"]:
            raise ValueError("source/judge changed during capture")
        result["native_rows"] = rows
        complete = all(row["status"] == "ok" for row in rows)
        result.update(
            status="native_complete" if complete else "refused_or_failed",
            native_replayed=complete,
        )
    except (
        ValueError,
        RuntimeError,
        TimeoutError,
        OSError,
        subprocess.SubprocessError,
        KeyboardInterrupt,
    ) as error:
        result.update(
            status="failed",
            native_replayed=False,
            error_type=type(error).__name__,
            error=str(error),
        )
    result["wall_s"] = time.monotonic() - began
    write(folder / "result.json", result)
    event(root, dict(event="evaluated", **result))
    return result


def main():
    def interrupted(_signal, _frame):
        # Unwind the native runner so it kills/reaps its owned process group.
        raise KeyboardInterrupt("trial cancelled")

    signal.signal(signal.SIGTERM, interrupted)
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--root", type=Path, required=True)
    commands = p.add_subparsers(dest="command", required=True)
    f = commands.add_parser("freeze")
    f.add_argument("--candidates", type=Path, required=True)
    f.add_argument("--specimens", nargs="+", default=["cube48"])
    f = commands.add_parser("prepare")
    f.add_argument("name")
    f.add_argument("--parent")
    f.add_argument("--hypothesis", required=True)
    f = commands.add_parser("run")
    f.add_argument("name")
    f.add_argument("--jobs", type=int, default=1)
    f.add_argument("--seconds", type=int, default=900)
    a = p.parse_args()
    a.root = a.root.resolve()
    if a.command == "freeze":
        result = freeze(a.root, a.candidates, a.specimens)
    elif a.command == "prepare":
        result = prepare(a.root, a.name, a.parent, a.hypothesis)
    else:
        result = run(a.root, a.name, jobs=a.jobs, seconds=a.seconds)
    print(json.dumps(result, allow_nan=False))
    if result.get("status") in ("failed", "refused_or_failed"):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
