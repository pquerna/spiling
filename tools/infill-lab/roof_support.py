"""Native-backed, frozen Rust subset experiments with an independent support judge."""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import time

from model import Candidate, file_hash
from native import Native, run
from roof_support_judge import judge, prepare

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
EDITABLE = "crates/manufacturing/examples/roof_support/policy.rs"
CASES = [
    ("cube03", "box", (40, 40, 32), 0.03, 0.2, "train"),
    ("cube05", "box", (48, 48, 32), 0.05, 0.2, "train"),
    ("cube08", "box", (48, 48, 32), 0.08, 0.2, "train"),
    ("dense15", "box", (32, 32, 20), 0.15, 0.2, "train"),
    ("beam05", "box", (80, 30, 24), 0.05, 0.2, "train"),
    ("pocket05", "pocket", (48, 48, 24), 0.05, 0.2, "train"),
    ("shelf05", "shelf", (48, 40, 24), 0.05, 0.2, "train"),
    ("bore05", "bore", (48, 40, 24), 0.05, 0.2, "train"),
    ("panel03", "box", (56, 40, 12), 0.03, 0.2, "train"),
    ("hold-tube04", "tube", (44, 36, 28), 0.04, 0.2, "holdout"),
    ("hold-shelf04", "shelf", (62, 38, 23.15), 0.04, 0.2, "holdout"),
    ("hold-pocket06", "pocket", (52, 46, 20), 0.06, 0.1, "holdout"),
    ("hold-beam09", "box", (72, 26, 22), 0.09, 0.2, "holdout"),
]


def write(path, value):
    with Path(path).open("x") as stream:
        json.dump(value, stream, allow_nan=False, separators=(",", ":"))
        stream.write("\n")


def source_inventory():
    names = subprocess.check_output(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], cwd=REPO).decode().split("\0")
    return {name: file_hash(REPO / name) for name in names if name and name != EDITABLE and "__pycache__" not in name and not name.endswith(".pyc") and (REPO / name).is_file()}


def build_environment():
    return {name: os.environ.get(name) for name in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_PROFILE_DEV_OPT_LEVEL", "CARGO_INCREMENTAL")}


def shape_source(path, kind, size):
    from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox, BRepPrimAPI_MakeCylinder
    from OCP.BRepAlgoAPI import BRepAlgoAPI_Cut
    from OCP.gp import gp_Pnt, gp_Ax2, gp_Dir
    from OCP.BRepCheck import BRepCheck_Analyzer
    from OCP.STEPControl import STEPControl_Writer, STEPControl_AsIs
    from OCP.IFSelect import IFSelect_RetDone

    width, depth, height = size
    shape = BRepPrimAPI_MakeBox(width, depth, height).Shape()
    if kind == "shelf":
        tool = BRepPrimAPI_MakeBox(gp_Pnt(width * 0.51, -1, height * 0.53), width, depth + 2, height).Shape()
    elif kind == "pocket":
        tool = BRepPrimAPI_MakeBox(gp_Pnt(width * 0.31, depth * 0.29, height * 0.51), width * 0.41, depth * 0.43, height).Shape()
    elif kind == "tube":
        tool = BRepPrimAPI_MakeBox(gp_Pnt(width * 0.35, depth * 0.3, -1), width * 0.3, depth * 0.4, height + 2).Shape()
    elif kind == "bore":
        tool = BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(-1, depth * 0.47, height * 0.55), gp_Dir(1, 0, 0)), height * 0.18, width + 2).Shape()
    else:
        tool = None
    if tool is not None:
        shape = BRepAlgoAPI_Cut(shape, tool).Shape()
    if not BRepCheck_Analyzer(shape).IsValid():
        raise ValueError("invalid original research specimen")
    writer = STEPControl_Writer()
    writer.Transfer(shape, STEPControl_AsIs)
    if writer.Write(str(path)) != IFSelect_RetDone:
        raise ValueError("original STEP export failed")


def canonical(road):
    ends = sorted(tuple(round(value, 6) for value in point) for point in (road["start"], road["end"]))
    return (road["layer"], *ends)


def capture(root, binaries, only):
    root.mkdir(parents=True, exist_ok=True)
    sources, captures = root / "sources", root / "captures"
    sources.mkdir(exist_ok=True)
    captures.mkdir(exist_ok=True)
    base = REPO / "fixtures/manufacturing/complete-sparse.intent.json"
    for name, kind, size, fraction, height, split in CASES:
        if only and name not in only:
            continue
        folder = captures / name
        if (folder / "context.json").exists():
            continue
        source = sources / f"{name}.step"
        if not source.exists():
            shape_source(source, kind, size)
        candidate = Candidate(pattern="web_roof", fraction=fraction, layer_height=height)
        intent = json.loads(base.read_text())
        intent["recipe"].update(infill_fraction=fraction, layer_height_mm=height, sparse_infill=candidate.sparse_policy(), supports={"kind": "disabled"})
        owned_intent = sources / f"{name}.intent.json"
        if not owned_intent.exists():
            write(owned_intent, intent)
        began = time.monotonic()
        try:
            data = Native(binaries, base, timeout=240).compile(source, size, candidate, folder)
            run([binaries / "examples/roof_support", "classify", folder / "fresh-bundle/plan.json", owned_intent, folder / "optional.json"], timeout=90)
            inventory = json.loads((folder / "optional.json").read_text())
            optional = {canonical(road) for road in inventory["optional"]}
            roads = data["roads"]
            found = set()
            for road in roads:
                key = canonical(road)
                road["optional"] = road["kind"] == 5 and key in optional
                if road["optional"]:
                    if key in found:
                        raise ValueError("ambiguous optional-road identity")
                    found.add(key)
            if found != optional:
                raise ValueError(f"native optional inventory mismatch: {len(found)}/{len(optional)}")
            proofs, pairs = prepare(roads, intent["recipe"]["process"]["max_bridge_span_mm"])
            context = {"schema": 1, "name": name, "split": split, "roads": roads, "proofs": proofs,
                       "bridge_limit_mm": intent["recipe"]["process"]["max_bridge_span_mm"],
                       "travel_speed_mm_s": intent["recipe"]["travel_speed_mm_s"],
                       "filament_diameter_mm": intent["recipe"]["filament_diameter_mm"],
                       "source_sha256": file_hash(source), "capture_sha256": file_hash(folder / "roads.json"),
                       "plan_sha256": file_hash(folder / "fresh-bundle/plan.json"),
                       "optional_sha256": file_hash(folder / "optional.json"), "support_pairs": pairs,
                       "engine_build": data["engine_build"], "bin_sha256": data["bin_sha256"],
                       "baseline_native_replayed": data["report"]["verified"] and data["source_independent_reopen"],
                       "fresh_exports": data["fresh_export_sha256"], "columns": {k: inventory[k] for k in ("root_counts", "joint_columns")}}
            context["baseline"] = judge(context, list(range(len(roads))))
            write(folder / "context.json", context)
            print(json.dumps({"case": name, "status": "ok", "roads": len(roads), "optional": len(optional), "pairs": pairs,
                              "seconds": round(time.monotonic() - began, 2), "baseline": context["baseline"]}), flush=True)
        except Exception as error:
            write(root / f"capture-failure-{name}.json", {"name": name, "error": str(error)})
            raise


def freeze(root):
    contexts = {}
    for name, *_, split in CASES:
        path = root / "captures" / name / "context.json"
        data = json.loads(path.read_text())
        if not data["baseline_native_replayed"]:
            raise ValueError("unverified native control")
        contexts[name] = {"sha256": file_hash(path), "split": split}
    write(root / "freeze.json", {"schema": 1, "contexts": contexts, "sources": source_inventory(),
                                 "build_environment": build_environment(),
                                 "git_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO).decode().strip(),
                                 "rustc": subprocess.check_output(["rustc", "-V"], cwd=REPO).decode().strip(),
                                 "max_trials": 64, "plateau_after_all_families": 12,
                                 "required_families": ["material", "shortest", "axis_x", "axis_y", "phase", "random", "deadline", "bundle"],
                                 "maximum_algorithm_revisions": 4,
                                 "max_finalists": 3, "scope": "derived_nominal_subset_research_not_native_qualification",
                                 "metric_order": ["volume_ratio", "motion_ratio", "starts_ratio"],
                                 "hard_gates": ["unique_baseline_ids", "all_structural_columns_retained", "physical_lower_adjacency",
                                                "complete_actual_stroke_intervals", "no_new_unlicensed_bridge", "no_worsened_inherited_warning"]})
    (root / "trials").mkdir()


def checked(root):
    frozen = json.loads((root / "freeze.json").read_text())
    if source_inventory() != frozen["sources"]:
        raise ValueError("frozen judge or source changed outside candidate algorithm")
    if build_environment() != frozen["build_environment"]:
        raise ValueError("frozen build environment changed")
    for name, item in frozen["contexts"].items():
        if file_hash(root / "captures" / name / "context.json") != item["sha256"]:
            raise ValueError("frozen context changed")
    return frozen


def trial(root, name, policy, parent, hypothesis, split):
    frozen = checked(root)
    previous = [json.loads(line) for line in (root / "log.jsonl").read_text().splitlines()] if (root / "log.jsonl").exists() else []
    if split == "train" and sum(row["split"] == "train" for row in previous) >= frozen["max_trials"]:
        raise ValueError("frozen training-trial budget exhausted")
    if not name.replace("-", "").replace("_", "").isalnum():
        raise ValueError("invalid trial name")
    folder = root / "trials" / name
    folder.mkdir()
    shutil.copyfile(REPO / EDITABLE, folder / "algorithm.rs")
    began = time.monotonic()
    algorithm = file_hash(REPO / EDITABLE)
    rows, status = [], "ok"
    try:
        run(["cargo", "build", "--locked", "-p", "spiling-manufacturing", "--example", "roof_support", "--features", "roof-research"], timeout=240, output=folder / "build")
        target = Path(os.environ.get("CARGO_TARGET_DIR", REPO / "target")) / "debug/examples/roof_support"
        binary = folder / "roof_support"
        shutil.copyfile(target, binary)
        write(folder / "policy.json", policy)
        for case, item in frozen["contexts"].items():
            if item["split"] != split:
                continue
            context_path = root / "captures" / case / "context.json"
            output = folder / f"{case}.selection.json"
            raw, wall = run([binary, "select", context_path, folder / "policy.json", output], timeout=120)
            context = json.loads(context_path.read_text())
            selected = json.loads(output.read_text())["selected"]
            result = judge(context, selected)
            baseline = context["baseline"]
            rows.append({"case": case, **result, "selection_wall_s": wall,
                         "volume_ratio": result["volume_mm3"] / baseline["volume_mm3"],
                         "motion_ratio": result["ideal_filtered_motion_s"] / baseline["ideal_filtered_motion_s"],
                         "starts_ratio": result["starts"] / baseline["starts"],
                         "selector": json.loads(raw), "selection_sha256": file_hash(output)})
        checked(root)
        if file_hash(REPO / EDITABLE) != algorithm:
            raise ValueError("algorithm changed during trial")
    except Exception as error:
        status = "checks_failed"
        rows.append({"error": str(error)})
    result = {"name": name, "parent": parent, "hypothesis": hypothesis, "policy": policy,
              "algorithm_sha256": algorithm, "status": status, "split": split, "cases": rows,
              "wall_s": time.monotonic() - began, "freeze_sha256": file_hash(root / "freeze.json"), "native_replayed": False}
    if status == "ok":
        for metric in ("volume_ratio", "motion_ratio", "starts_ratio"):
            result[metric] = math.exp(sum(math.log(row[metric]) for row in rows) / len(rows))
            result[f"worst_{metric}"] = max(row[metric] for row in rows)
    binary = folder / "roof_support"
    if binary.exists():
        result["binary_sha256"] = file_hash(binary)
        binary.unlink()
    if status == "ok":
        metrics = ("volume_ratio", "motion_ratio", "starts_ratio")
        successful = [row for row in previous if row["status"] == "ok" and row["split"] == split]
        dominated = any(all(row[key] <= result[key] + 1e-12 for key in metrics) for row in successful)
        result["decision"] = "dominated_discard" if dominated else "pareto_keep"
    else:
        result["decision"] = "checks_failed"
    write(folder / "result.json", result)
    with (root / "log.jsonl").open("a") as stream:
        stream.write(json.dumps(result, allow_nan=False) + "\n")
    print(json.dumps(result, allow_nan=False), flush=True)
    if status != "ok":
        raise RuntimeError("trial failed; retain receipt and diagnose before continuing")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    commands = parser.add_subparsers(dest="command", required=True)
    capture_parser = commands.add_parser("capture")
    capture_parser.add_argument("--bin", type=Path, required=True)
    capture_parser.add_argument("--only", nargs="*")
    commands.add_parser("freeze")
    trial_parser = commands.add_parser("trial")
    trial_parser.add_argument("name")
    trial_parser.add_argument("--policy", required=True)
    trial_parser.add_argument("--parent", default="baseline")
    trial_parser.add_argument("--hypothesis", required=True)
    trial_parser.add_argument("--split", choices=("train", "holdout"), default="train")
    args = parser.parse_args()
    args.root = args.root.resolve()
    if args.command == "capture":
        capture(args.root, args.bin.resolve(), args.only)
    elif args.command == "freeze":
        freeze(args.root)
    else:
        trial(args.root, args.name, json.loads(args.policy), args.parent, args.hypothesis, args.split)


if __name__ == "__main__":
    main()
