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

SPILING_CORPUS_DEFAULTS=1 keeps the intent's own defaults (layer heights,
infill, speeds) and only switches supports; PrusaSlicer then follows the same
values. SPILING_CORPUS_PRUSA_INI=FILE slices PrusaSlicer with that matched
configuration (`--load`; for example a vendor printer+print+filament preset
flattened to one file) instead of command-line overrides; `from-intent`
derives one from the intent's printer, material and recipe. Each record keeps
both slicers' estimated print time and filament length and their ratio.
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
JOBS = int(os.environ.get("SPILING_CORPUS_JOBS", "4"))
LOCK = threading.Lock()
CATHEDRAL = os.environ.get("SPILING_CORPUS_INFILL") == "cathedral"
FRACTION = float(os.environ.get("SPILING_CORPUS_FRACTION", "0.15"))
# "disabled" (PrusaSlicer's default) or "breakaway" (the intent fixture's supports).
SUPPORTS = os.environ.get("SPILING_CORPUS_SUPPORTS", "disabled")
DEFAULTS = os.environ.get("SPILING_CORPUS_DEFAULTS") == "1"
PRUSA_INI = os.environ.get("SPILING_CORPUS_PRUSA_INI")
WORK = os.path.join(ROOT, "work")
INTENT = json.load(open(INTENT_SRC))


def intent_file():
    d = json.loads(json.dumps(INTENT))
    if not DEFAULTS:
        d["recipe"]["infill_fraction"] = FRACTION
        d["recipe"]["layer_height_mm"] = 0.2
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


def duration_seconds(text):
    """PrusaSlicer's '1d 2h 3m 4s' estimate in seconds."""
    units = {"d": 86400, "h": 3600, "m": 60, "s": 1}
    return sum(int(n) * units[u] for n, u in re.findall(r"(\d+)([dhms])", text))


def bed_center():
    envelope = INTENT["printer"]["build_envelope"]
    return [round((envelope["min"][i] + envelope["max"][i]) / 2, 3) for i in range(2)]


def prusa_ini_from_intent(path):
    """A PrusaSlicer configuration matching the intent's printer, material and
    recipe: bed, nozzle, temperatures, role speeds, layer heights, walls,
    skins, infill, cooling, retraction and the declared motion limits."""
    printer, recipe = INTENT["printer"], INTENT["recipe"]
    job = printer["workflow"]["job"]
    env = printer["build_envelope"]
    x, y, z = (env["max"][i] - env["min"][i] for i in range(3))
    flow = printer["max_volumetric_flow_mm3_s"]
    if INTENT.get("material_profile"):
        flow = min(flow, INTENT["material_profile"]["selection"]["profile"]["max_volumetric_flow_mm3_s"])
    speeds = recipe.get("role_speeds") or {}
    uniform = recipe["print_speed_mm_s"]
    process = recipe["process"]
    pct = lambda pwm: round(pwm / 255 * 100)
    accel = (job.get("timing_model") or {}).get("acceleration_mm_s2", 1500)
    jd = (job.get("timing_model") or {}).get("junction_deviation_mm", 0.013)
    width = recipe["bead_width_mm"]
    values = {
        "bed_shape": f"0x0,{x:g}x0,{x:g}x{y:g},0x{y:g}", "max_print_height": z,
        "nozzle_diameter": printer["nozzle_diameter_mm"], "filament_diameter": recipe["filament_diameter_mm"],
        "gcode_flavor": "marlin2", "use_relative_e_distances": 1, "binary_gcode": 0,
        "layer_gcode": "G92 E0",
        "layer_height": recipe["layer_height_mm"],
        "first_layer_height": recipe.get("first_layer_height_mm") or recipe["layer_height_mm"],
        "perimeters": recipe["perimeter_count"],
        # Solid layer counts from the skin thicknesses; no small-area solid
        # infill (as PrusaSlicer's own MK4S presets).
        "top_solid_layers": max(1, round(recipe["top_skin_thickness_mm"] / recipe["layer_height_mm"])),
        "bottom_solid_layers": max(1, round(recipe["bottom_skin_thickness_mm"] / recipe["layer_height_mm"])),
        "solid_infill_below_area": 0,
        "top_solid_min_thickness": recipe["top_skin_thickness_mm"],
        "bottom_solid_min_thickness": recipe["bottom_skin_thickness_mm"],
        "fill_density": f"{round(recipe['infill_fraction'] * 100)}%", "fill_pattern": "rectilinear",
        "top_fill_pattern": "rectilinear", "bottom_fill_pattern": "rectilinear",
        "extrusion_multiplier": recipe["flow_multiplier"],
        "temperature": job["nozzle_temperature_c"],
        "first_layer_temperature": job.get("first_layer_nozzle_temperature_c", job["nozzle_temperature_c"]),
        "bed_temperature": job["bed_temperature_c"],
        "first_layer_bed_temperature": job.get("first_layer_bed_temperature_c", job["bed_temperature_c"]),
        "first_layer_speed": speeds.get("first_layer_mm_s", uniform * process["first_layer_speed_factor"]),
        "first_layer_infill_speed": speeds.get("first_layer_infill_mm_s", uniform * process["first_layer_speed_factor"]),
        "external_perimeter_speed": speeds.get("outer_wall_mm_s", uniform),
        "perimeter_speed": speeds.get("inner_wall_mm_s", uniform),
        "small_perimeter_speed": speeds.get("outer_wall_mm_s", uniform),
        "gap_fill_speed": speeds.get("inner_wall_mm_s", uniform),
        "infill_speed": speeds.get("sparse_infill_mm_s", uniform),
        "solid_infill_speed": speeds.get("solid_infill_mm_s", uniform),
        "top_solid_infill_speed": speeds.get("top_skin_mm_s", uniform),
        "support_material_speed": speeds.get("support_mm_s", uniform),
        "support_material_interface_speed": speeds.get("support_mm_s", uniform),
        "bridge_speed": speeds.get("bridge_mm_s", uniform * process["bridge_speed_factor"]),
        "travel_speed": recipe["travel_speed_mm_s"], "max_print_speed": printer["max_print_feed_mm_s"],
        "max_volumetric_speed": 0, "filament_max_volumetric_speed": flow,
        "cooling": 1, "fan_always_on": 1, "disable_fan_first_layers": 1, "full_fan_speed_layer": 0,
        "slowdown_below_layer_time": process["minimum_layer_time_s"],
        "min_print_speed": process["minimum_print_speed_mm_s"],
        "fan_below_layer_time": process["fan_short_layer_time_s"],
        "min_fan_speed": pct(process["fan_after_first_layers"]),
        "max_fan_speed": pct(process["fan_short_layer_pwm"]),
        "bridge_fan_speed": pct(process["bridge_fan_pwm"]),
        "skirts": 0, "brim_width": 0, "seam_position": "nearest", "wipe": 0, "retract_layer_change": 0,
        "machine_limits_usage": "time_estimate_only",
        "machine_max_acceleration_extruding": accel, "machine_max_acceleration_retracting": accel,
        "machine_max_acceleration_travel": accel, "machine_max_acceleration_x": accel,
        "machine_max_acceleration_y": accel, "default_acceleration": 0,
        "machine_max_feedrate_x": printer["max_travel_feed_mm_s"],
        "machine_max_feedrate_y": printer["max_travel_feed_mm_s"],
        "machine_max_junction_deviation": jd,
    }
    for key in ("extrusion_width", "perimeter_extrusion_width", "external_perimeter_extrusion_width",
                "infill_extrusion_width", "solid_infill_extrusion_width", "top_infill_extrusion_width",
                "first_layer_extrusion_width", "support_material_extrusion_width"):
        values[key] = width
    retraction = process["retraction"]
    if retraction["kind"] == "enabled":
        values.update({"retract_length": retraction["distance_mm"], "retract_speed": retraction["speed_mm_s"],
                       "deretract_speed": retraction["speed_mm_s"],
                       "retract_before_travel": retraction["minimum_travel_mm"],
                       "retract_lift": retraction["z_hop_mm"]})
    else:
        values["retract_length"] = 0
    with open(path, "w") as f:
        for k, v in values.items():
            f.write(f"{k} = {v}\n")
    return path


def prusa(step, cwd):
    started = time.time()
    gcode = os.path.join(cwd, "prusa.gcode")
    recipe = INTENT["recipe"]
    layer = recipe["layer_height_mm"] if DEFAULTS else 0.2
    fraction = recipe["infill_fraction"] if DEFAULTS else FRACTION
    center = ",".join(str(v) for v in bed_center())
    if PRUSA_INI == "from-intent":
        args = ["--load", prusa_ini_from_intent(os.path.join(cwd, "prusa.ini"))]
    elif PRUSA_INI:
        args = ["--load", PRUSA_INI]
    else:
        args = ["--layer-height", str(layer), "--fill-density", f"{round(fraction * 100)}%"]
    # Spiling's generated supports stand on the build plate only.
    args += ["--support-material", "--support-material-buildplate-only"] if SUPPORTS != "disabled" else []
    try:
        proc = subprocess.run(["prusa-slicer", "--export-gcode", *args, "--center", center, "-o", gcode, step],
                              cwd=cwd, capture_output=True, text=True, timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return {"ok": False, "seconds": TIMEOUT}
    ok = proc.returncode == 0 and os.path.exists(gcode)
    info = {"ok": ok, "seconds": round(time.time() - started, 2)}
    if ok:
        info["g1_lines"] = 0
        for line in open(gcode, errors="replace"):
            if line.startswith("G1 "):
                info["g1_lines"] += 1
            elif line.startswith("; estimated printing time (normal mode) ="):
                info["estimated_seconds"] = duration_seconds(line.split("=", 1)[1])
            elif line.startswith("; filament used [mm] ="):
                info["filament_mm"] = float(line.split("=", 1)[1])
    else:
        info["message"] = (proc.stderr or proc.stdout)[-300:]
    return info


def spiling_report(stdout):
    """Verification totals and the estimate from the open/verify output."""
    def number(key):
        m = re.search(r'"%s":\s*(-?[0-9.eE+-]+)' % key, stdout)
        return float(m.group(1)) if m else None
    report = {k: number(k) for k in ("filament_length_mm", "support_filament_length_mm",
                                     "priming_filament_length_mm", "estimated_seconds")}
    parts = [report[k] for k in ("filament_length_mm", "support_filament_length_mm",
                                 "priming_filament_length_mm")]
    report["total_filament_mm"] = round(sum(v for v in parts if v is not None), 3)
    return report


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
            report = spiling_report(stdout)
            record["spiling"] = report
            reference = record["prusa"]
            if reference.get("filament_mm") and report["total_filament_mm"]:
                record["filament_ratio"] = round(report["total_filament_mm"] / reference["filament_mm"], 4)
            if reference.get("estimated_seconds") and report.get("estimated_seconds"):
                record["time_ratio"] = round(report["estimated_seconds"] / reference["estimated_seconds"], 4)
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
          "| filament", record.get("filament_ratio"), "time", record.get("time_ratio"),
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
