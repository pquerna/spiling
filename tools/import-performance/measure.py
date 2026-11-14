#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Paired real CLI import/placement/compile and source-absent return measurements."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import resource
import signal
import shutil
import subprocess
import time
import uuid


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def execute(binaries, arguments, directory, name):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    started = time.monotonic()
    peak_kib = 0
    with (directory / f"{name}.stdout.json").open("w") as output:
        with (directory / f"{name}.stderr.jsonl").open("w") as errors:
            process = subprocess.Popen(
                [str(binaries / "spiling-cli"), "project", *arguments,
                 "--engine", str(binaries / "spiling-engine")],
                cwd=directory, stdout=output, stderr=errors,
            )
            try:
                while process.poll() is None:
                    children = Path(f"/proc/{process.pid}/task/{process.pid}/children")
                    try:
                        for child in children.read_text().split():
                            status = Path(f"/proc/{child}/status").read_text()
                            for line in status.splitlines():
                                if line.startswith("VmHWM:"):
                                    peak_kib = max(peak_kib, int(line.split()[1]))
                    except (FileNotFoundError, ProcessLookupError):
                        pass
                    time.sleep(0.02)
            except BaseException:
                process.terminate()
                process.wait()
                raise
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    return {
        "ok": process.returncode == 0,
        "exit_code": process.returncode,
        "seconds": time.monotonic() - started,
        "cpu_seconds": after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime,
        "engine_mib": peak_kib / 1024,
    }


def journey(binaries, source, intent, directory):
    directory.mkdir()
    shutil.copyfile(source, directory / "source.step")
    result = {"part": source.name, "source_sha256": digest(source)}
    result["compile"] = execute(
        binaries,
        ["create", "part.spil", "--import", "source.step", "--intent", str(intent),
         "--center-all-on-bed", "--compile", "--save"],
        directory, "compile",
    )
    if result["compile"]["ok"]:
        result["compile"]["preview"] = scene_summary(directory / "compile.stdout.json")
        (directory / "source.step").unlink()
        result["return"] = execute(
            binaries,
            ["open", "part.spil", "--read-only", "--verify", "--manufacturing-out", "export"],
            directory, "return",
        )
        if result["return"]["ok"]:
            result["return"]["preview"] = scene_summary(directory / "return.stdout.json")
            result["export_hashes"] = {
                str(path.relative_to(directory / "export")): digest(path)
                for path in sorted((directory / "export").rglob("*")) if path.is_file()
            }
    result["ok"] = result["compile"]["ok"] and result.get("return", {}).get("ok", False)
    return result


def scene_summary(path):
    payload = json.loads(path.read_text())
    definitions = [entry["record"] for entry in payload["definitions"]]
    return {
        "definition_faces": {entry["definition_id"]: entry["face_count"] for entry in definitions},
        "face_tables": {entry["record"]["definition_id"]: entry["faces"]
                        for entry in payload["definitions"]},
        "displayed_definitions": sum(entry["display"]["kind"] == "mesh" for entry in definitions),
        "unavailable": {entry["definition_id"]: entry["display"] for entry in definitions
                        if entry["display"]["kind"] != "mesh"},
        "mesh_triangles": payload["unique_mesh_triangles"],
        "mesh_bytes": payload["unique_mesh_bytes"],
    }


def geometric_mean(values):
    return math.exp(sum(math.log(value) for value in values) / len(values))


def main():
    def terminate(signum, frame):
        raise InterruptedError(f"benchmark cancelled by signal {signum}")

    signal.signal(signal.SIGTERM, terminate)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, required=True, help="Directory containing STEP files")
    parser.add_argument("--baseline", type=Path, required=True, help="Frozen matching engine/CLI directory")
    parser.add_argument("--candidate", type=Path, required=True, help="Frozen matching engine/CLI directory")
    parser.add_argument("--intent", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="New directory on persistent disk")
    parser.add_argument("--parts", nargs="+", default=["28", "25"])
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--supports", choices=["disabled", "breakaway"], default="disabled")
    parser.add_argument("--retain-work", action="store_true")
    arguments = parser.parse_args()
    if arguments.repeats < 1:
        parser.error("repeats must be positive")
    selected = []
    for prefix in arguments.parts:
        matches = [path for path in arguments.corpus.iterdir()
                   if path.name.startswith(prefix + "-") and path.suffix.lower() == ".step"]
        if len(matches) != 1:
            parser.error(f"part {prefix}: expected one STEP, found {len(matches)}")
        selected.append(matches[0].resolve())
    binaries = {"baseline": arguments.baseline.resolve(), "candidate": arguments.candidate.resolve()}
    binary_hashes = {
        label: {name: digest(directory / name) for name in ["spiling-cli", "spiling-engine"]}
        for label, directory in binaries.items()
    }
    intent = json.loads(arguments.intent.read_text())
    intent["recipe"]["infill_fraction"] = 0.15
    intent["recipe"]["layer_height_mm"] = 0.2
    if arguments.supports == "disabled":
        intent["recipe"]["supports"] = {"kind": "disabled"}
    elif intent["recipe"]["supports"].get("kind") != "breakaway":
        parser.error("intent must provide explicit breakaway support settings")
    output = arguments.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    intent_path = output / "intent.json"
    intent_path.write_text(json.dumps(intent) + "\n")
    same_producer = binary_hashes["baseline"] == binary_hashes["candidate"]
    (output / "manifest.json").write_text(json.dumps({
        "schema": 1, "run_id": str(uuid.uuid4()), "supports": arguments.supports,
        "repeats": arguments.repeats, "binary_hashes": binary_hashes,
        "environment": {name: os.environ.get(name) for name in [
            "RAYON_NUM_THREADS", "TOKIO_WORKER_THREADS", "MALLOC_ARENA_MAX",
            "SPILING_MANUFACTURING_MEMORY_BYTES", "SPILING_SOURCE_MEMORY_BYTES",
        ]},
        "same_producer_calibration": same_producer,
        "intent_sha256": digest(intent_path), "parts": [source.name for source in selected],
        "host": os.uname().nodename,
        "metric": "paired geometric mean of compile and return process-CPU ratios",
        "wall_time_scope": "complete CLI with supervised engine, including import/placement and saved return",
        "cleanup": "successful work removed unless retained; failures and logs retained",
    }, indent=2) + "\n")
    ratios = {"cpu_seconds": [], "seconds": []}
    failed = False
    with (output / "results.jsonl").open("a", buffering=1) as records:
        for repeat in range(arguments.repeats):
            for source in selected:
                results = {}
                order = ["baseline", "candidate"] if repeat % 2 == 0 else ["candidate", "baseline"]
                for label in order:
                    directory = output / f"{repeat}-{source.stem}-{label}"
                    result = journey(binaries[label], source, intent_path, directory)
                    records.write(json.dumps({"repeat": repeat, "producer": label, **result}) + "\n")
                    print(json.dumps({
                        "repeat": repeat, "producer": label, "part": source.name,
                        "ok": result["ok"], "compile_seconds": result["compile"]["seconds"],
                        "return_seconds": result.get("return", {}).get("seconds"),
                        "displayed_definitions": result["compile"].get("preview", {}).get("displayed_definitions"),
                    }), flush=True)
                    results[label] = result
                    if result["ok"] and not arguments.retain_work:
                        for path in [directory / "part.spil", directory / "export"]:
                            if path.is_dir():
                                shutil.rmtree(path)
                            elif path.exists():
                                path.unlink()
                if not all(result["ok"] for result in results.values()):
                    failed = True
                    continue
                for stage in ["compile", "return"]:
                    baseline_preview = results["baseline"][stage]["preview"]
                    candidate_preview = results["candidate"][stage]["preview"]
                    if (baseline_preview["definition_faces"] != candidate_preview["definition_faces"]
                            or baseline_preview["face_tables"] != candidate_preview["face_tables"]
                            or candidate_preview["displayed_definitions"] < baseline_preview["displayed_definitions"]):
                        failed = True
                        print(f"CHECK_FAILED source/display coverage: {source.name}/{stage}", flush=True)
                    for metric in ratios:
                        baseline = results["baseline"][stage][metric]
                        candidate = results["candidate"][stage][metric]
                        if baseline <= 0 or candidate <= 0:
                            raise RuntimeError(f"nonpositive measured {stage}/{metric}")
                        ratios[metric].append(candidate / baseline)
    summary = {"ok": not failed, "same_producer_calibration": same_producer,
               "paired_stage_samples": len(ratios["seconds"]),
               "ratios": {metric: geometric_mean(values) if values else None
                          for metric, values in ratios.items()}}
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(f"METRIC successful_journeys={int(not failed)}")
    for metric, value in summary["ratios"].items():
        if value is not None:
            print(f"METRIC {metric}_ratio={value:.8f}")
    raise SystemExit(int(failed))


if __name__ == "__main__":
    main()
