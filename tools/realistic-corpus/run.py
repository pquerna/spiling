#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Realistic-corpus journey: each STEP through the real Spiling CLI and PrusaSlicer.

Spiling: create + import + intent + center the whole source on the bed + compile + save, then
source-absent read-only reopen + verify + export. PrusaSlicer: slice the same
STEP with matched layer height and infill as the printability reference.
Usage: run.py CORPUS_DIR BIN_DIR INTENT_JSON [RESULTS] [PART_PREFIX...]
CORPUS_DIR holds step/ from fetch.py; results and work/ are written there.
Writes one JSON record per part to results.ndjson. Standard library only.
"""
import json, os, re, shutil, subprocess, sys, time, hashlib, threading
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.abspath(sys.argv[1])
R = os.path.abspath(sys.argv[2])  # directory holding spiling-cli and spiling-engine
INTENT_SRC = os.path.abspath(sys.argv[3])
OUT = os.path.join(ROOT, sys.argv[4] if len(sys.argv) > 4 else "results.ndjson")
ONLY = sys.argv[5:] if len(sys.argv) > 5 else None
# Seconds per CLI step before the part is recorded as a timeout.
TIMEOUT = int(os.environ.get("SPILING_CORPUS_TIMEOUT", "900"))
# Parts compiled concurrently; each engine is one process (mostly one core).
JOBS = int(os.environ.get("SPILING_CORPUS_JOBS", "8"))
LOCK = threading.Lock()
CATHEDRAL = os.environ.get("SPILING_CORPUS_INFILL") == "cathedral"
FRACTION = float(os.environ.get("SPILING_CORPUS_FRACTION", "0.15"))
# "disabled" (PrusaSlicer's default) or "breakaway" (the intent fixture's supports).
SUPPORTS = os.environ.get("SPILING_CORPUS_SUPPORTS", "disabled")
WORK = os.path.join(ROOT, "work")


def intent_file():
    d = json.load(open(INTENT_SRC))
    d["recipe"]["infill_fraction"] = FRACTION
    d["recipe"]["layer_height_mm"] = 0.2
    # Park above any part: the top of the printer's envelope.
    d["printer"]["workflow"]["job"]["park_position_mm"][2] = d["printer"]["build_envelope"]["max"][2]
    if SUPPORTS == "disabled":
        d["recipe"]["supports"] = {"kind": "disabled"}
    elif d["recipe"]["supports"].get("kind") != SUPPORTS:
        raise SystemExit(f"intent fixture has no {SUPPORTS} supports to keep")
    if CATHEDRAL:
        d["recipe"]["sparse_infill"] = {"kind": "web_roof", "roof_spacing_mm": None, "architecture": None}
    path = os.path.join(WORK, "intent.json")
    json.dump(d, open(path, "w"))
    return path


def engine_peak(proc):
    peak = 0
    deadline = time.time() + TIMEOUT
    while proc.poll() is None:
        if time.time() > deadline:
            raise subprocess.TimeoutExpired(proc.args, TIMEOUT)
        for pid in os.listdir("/proc"):
            if not pid.isdigit():
                continue
            try:
                # Other processes' names need not be UTF-8.
                if open(f"/proc/{pid}/comm", "rb").read().strip() != b"spiling-engine":
                    continue
                if int(open(f"/proc/{pid}/stat").read().split()[3]) != proc.pid:
                    continue
                for line in open(f"/proc/{pid}/status"):
                    if line.startswith("VmHWM:"):
                        peak = max(peak, int(line.split()[1]))
            except OSError:
                pass
        time.sleep(0.2)
    return peak // 1024


def run_cli(args, cwd):
    started = time.time()
    err_path = os.path.join(cwd, f"{args[0]}.stderr.txt")
    with open(os.path.join(cwd, f"{args[0]}.stdout.json"), "w") as out, open(err_path, "w") as err:
        proc = subprocess.Popen(
            [os.path.join(R, "spiling-cli"), "project", *args, "--engine", os.path.join(R, "spiling-engine")],
            cwd=cwd, stdout=out, stderr=err)
        try:
            peak = engine_peak(proc)
            proc.wait(timeout=TIMEOUT)
        except subprocess.TimeoutExpired:
            proc.kill()
            return {"ok": False, "seconds": TIMEOUT, "error": {"code": "timeout"}}
    failure = None
    for line in open(err_path):
        try:
            record = json.loads(line)
        except ValueError:
            continue
        if record.get("level") == "error":
            failure = {"code": (record.get("error") or {}).get("code") or record.get("event"),
                       "domain": record.get("domain"),
                       "message": (record.get("error") or {}).get("message") or record.get("message")}
    return {"ok": proc.returncode == 0, "seconds": round(time.time() - started, 2),
            "engine_mib": peak, "error": failure}


def prusa(step, cwd):
    started = time.time()
    gcode = os.path.join(cwd, "prusa.gcode")
    try:
        proc = subprocess.run(["prusa-slicer", "--export-gcode", "--layer-height", "0.2",
                               "--fill-density", f"{round(FRACTION * 100)}%",
                               *(["--support-material"] if SUPPORTS != "disabled" else []), "--center", "110,110", "-o", gcode, step],
                              cwd=cwd, capture_output=True, text=True, timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return {"ok": False, "seconds": TIMEOUT}
    ok = proc.returncode == 0 and os.path.exists(gcode)
    info = {"ok": ok, "seconds": round(time.time() - started, 2)}
    if ok:
        info["g1_lines"] = sum(1 for l in open(gcode) if l.startswith("G1 "))
    else:
        info["message"] = (proc.stderr or proc.stdout)[-300:]
    return info


def part(name, intent, results):
    src = os.path.join(ROOT, "step", name)
    cwd = os.path.join(WORK, name)
    shutil.rmtree(cwd, ignore_errors=True)
    os.makedirs(cwd)
    local = os.path.join(cwd, "source.step")
    shutil.copy(src, local)
    record = {"part": name, "bytes": os.path.getsize(src),
              "sha256": hashlib.sha256(open(src, "rb").read()).hexdigest()}
    record["prusa"] = prusa(local, cwd)
    create = run_cli(["create", "p.spil", "--import", "source.step", "--intent", intent,
                      "--center-all-on-bed", "--compile", "--save"], cwd)
    record["spiling_compile"] = create
    if create["ok"]:
        os.remove(local)
        ret = run_cli(["open", "p.spil", "--read-only", "--verify", "--manufacturing-out", "out"], cwd)
        record["spiling_return"] = ret
        if ret["ok"]:
            stdout = open(os.path.join(cwd, "open.stdout.json")).read()
            m = re.search(r'"deposition_segments":\s*(\d+)', stdout)
            record["deposition_segments"] = int(m.group(1)) if m else None
            m = re.search(r'"filament_length_mm":\s*([0-9.]+)', stdout)
            record["filament_mm"] = float(m.group(1)) if m else None
            plan = json.load(open(os.path.join(cwd, "out", "plan.json")))
            layers = plan["layers"]
            spans = [l["region_coverage"]["unsupported_overhang_mm"] for l in layers]
            record["layers"] = len(layers)
            record["overhang_layers"] = sum(1 for v in spans if v > 0)
            record["max_overhang_mm"] = round(max(spans, default=0), 3)
            record["ceiling_layers"] = sum(
                1 for l in layers
                if any(d["ceilings"] for d in l["region_coverage"]["web_roof_domains"]))
    with LOCK:
        results.write(json.dumps(record) + "\n")
        results.flush()
    c = record["spiling_compile"]
    print(name, "prusa", record["prusa"]["ok"], "| spiling", c["ok"],
          (c["error"] or {}).get("code"), "| return", (record.get("spiling_return") or {}).get("ok"),
          flush=True)



def main():
    os.makedirs(WORK, exist_ok=True)
    intent = intent_file()
    steps = sorted(os.listdir(os.path.join(ROOT, "step")))
    if ONLY:
        steps = [s for s in steps if any(s.startswith(o) for o in ONLY)]
    with open(OUT, "a") as results, ThreadPoolExecutor(JOBS) as pool:
        # Each part is single-threaded work in its own engine process; run
        # them side by side so the run takes the slowest part, not the sum.
        list(pool.map(lambda name: part(name, intent, results), steps))


main()
