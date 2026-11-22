#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Realistic-corpus journey: each STEP through the real Spiling CLI and PrusaSlicer.

Spiling: create + import + printer/material/quality (or an explicit intent)
+ center the whole source on the bed + compile + save, then source-absent
read-only reopen + verify + export. PrusaSlicer: slice the same STEP with
matched settings as the printability reference.
Usage: run.py CORPUS_DIR BIN_DIR [RESULTS] [PART_PREFIX...] --profile PROFILE [--intent INTENT_JSON]
CORPUS_DIR holds step/ from fetch.py; results and work/ are written there.
Writes one JSON record per part to results.ndjson. Standard library only.
SPILING_CORPUS_INFILL selects rectilinear/grid/gyroid/cathedral; conventional
patterns are matched in PrusaSlicer, Cathedral keeps a labeled rectilinear control.

Default: no intent file. Each part runs `project create --import ... --printer
P --material M --quality Q --supports on|off` (SPILING_CORPUS_PRINTER, default
spiling.reference.marlin.04; SPILING_CORPUS_MATERIAL, default pla;
SPILING_CORPUS_QUALITY, default standard; SPILING_CORPUS_SUPPORTS disabled ->
off, breakaway -> on); the engine creates the intent and PrusaSlicer follows
its values (read from a part-free probe project).

--intent INTENT_JSON keeps the explicit-intent path: layer height 0.2 mm and
SPILING_CORPUS_FRACTION infill unless SPILING_CORPUS_DEFAULTS=1 keeps the
intent's own values; supports are switched as above. SPILING_CORPUS_PRUSA_INI=FILE slices PrusaSlicer with that matched
configuration (`--load`; for example a vendor printer+print+filament preset
flattened to one file) instead of command-line overrides; `from-intent`
derives one from the intent's printer, material and recipe. Each record keeps
both slicers' estimated print time and filament length and their ratio.

--prusa-off-results OFF.ndjson estimates Prusa support filament from a matched
supports-off reference. --coverage-control ON.ndjson CONTROL_CORPUS adds a
diagnostic native delta: one-to-one registration by role, physical Z and XY
within one grid cell plus half a bead. Native deltas, plan disagreement and
support filament drops above 5% are not coverage-gate failures.
Breakaway Prusa runs gate all native observer spans against Prusa's model
bead and unsupported footprints, using XY overlap on any single Prusa layer
within the larger local layer height. A fully registered longer native span
fails; partial registration and absent Prusa model geometry are reported
without a parity claim. Located evidence is kept in located-spans.ndjson.
The common lower-bead observer runs outside slice/reopen timings and does
not qualify physical printability.
"""
import argparse, json, os, re, shutil, signal, subprocess, sys, time, hashlib, threading
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from pathlib import Path
import audit
import features
import panel_coverage
import reference

PARSER = argparse.ArgumentParser(description=__doc__)
PARSER.add_argument("corpus")
PARSER.add_argument("binaries")
PARSER.add_argument("results", nargs="?", default="results.ndjson")
PARSER.add_argument("prefixes", nargs="*")
PARSER.add_argument("--intent")
PARSER.add_argument("--manifest", default=str(Path(__file__).with_name("manifest.json")))
PARSER.add_argument("--cohort", default="full")
PARSER.add_argument("--profile", choices=("dev", "profiling", "release"), required=True)
PARSER.add_argument("--reference", choices=("prusa", "none"), default="prusa")
PARSER.add_argument("--prusa-off-results")
PARSER.add_argument("--coverage-control", nargs=2, metavar=("RESULTS", "CORPUS_DIR"))
PARSER.add_argument("--list", action="store_true")
ARGS = PARSER.parse_intermixed_args()
INTENT_SRC = os.path.abspath(ARGS.intent) if ARGS.intent else None
ROOT = os.path.abspath(ARGS.corpus)
R = os.path.abspath(ARGS.binaries)
OUT = os.path.join(ROOT, ARGS.results)
PRINTER = os.environ.get("SPILING_CORPUS_PRINTER", "spiling.reference.marlin.04")
MATERIAL = os.environ.get("SPILING_CORPUS_MATERIAL", "pla")
QUALITY = os.environ.get("SPILING_CORPUS_QUALITY", "standard")
# Seconds per CLI step before the part is recorded as a timeout.
TIMEOUT = int(os.environ.get("SPILING_CORPUS_TIMEOUT", "900"))
# Parts compiled concurrently; each engine is one process (mostly one core).
JOBS = int(os.environ.get("SPILING_CORPUS_JOBS", "2"))
LOCK = threading.Lock()
INFILL = os.environ.get("SPILING_CORPUS_INFILL")
if INFILL not in (None, "rectilinear", "grid", "gyroid", "cathedral"):
    raise SystemExit("SPILING_CORPUS_INFILL must be rectilinear, grid, gyroid or cathedral")
FRACTION = float(os.environ.get("SPILING_CORPUS_FRACTION", "0.15"))
# "disabled" (PrusaSlicer's default) or "breakaway" (the intent fixture's supports).
SUPPORTS = os.environ.get("SPILING_CORPUS_SUPPORTS", "disabled")
if SUPPORTS not in ("disabled", "breakaway") or JOBS < 1 or TIMEOUT < 1:
    raise SystemExit("supports must be disabled|breakaway; jobs and timeout must be positive")
# Without an explicit intent the engine's defaults are the values.
DEFAULTS = INTENT_SRC is None or os.environ.get("SPILING_CORPUS_DEFAULTS") == "1"
PRUSA_INI = os.environ.get("SPILING_CORPUS_PRUSA_INI")
if PRUSA_INI and PRUSA_INI != "from-intent":
    PRUSA_INI = os.path.abspath(PRUSA_INI)
WORK = os.path.join(ROOT, "work")
INTENT = json.load(open(INTENT_SRC)) if INTENT_SRC else None


def defaults_args():
    """The one-step CLI flags replacing an intent file."""
    return ["--printer", PRINTER, "--material", MATERIAL, "--quality", QUALITY,
            "--supports", "off" if SUPPORTS == "disabled" else "on"] + (
                ["--infill", INFILL, "--infill-percent", str(FRACTION * 100)] if INFILL else [])


def probe_intent():
    """The engine-created intent for the chosen printer/material/quality,
    from a project without parts; PrusaSlicer's matched settings follow it."""
    cwd = os.path.join(WORK, "probe")
    shutil.rmtree(cwd, ignore_errors=True)
    os.makedirs(cwd)
    result = run_cli(["create", "probe.spil", *defaults_args()], cwd)
    if not result["ok"]:
        raise SystemExit(f"default settings refused: {result['error']}")
    return json.load(open(os.path.join(cwd, "create.stdout.json")))["manufacturing"]["intent"]


def intent_file():
    if INTENT_SRC is None:
        return None
    d = json.loads(json.dumps(INTENT))
    if not DEFAULTS:
        d["recipe"]["infill_fraction"] = FRACTION
        d["recipe"]["layer_height_mm"] = 0.2
    if SUPPORTS == "disabled":
        d["recipe"]["supports"] = {"kind": "disabled"}
    elif d["recipe"]["supports"].get("kind") != SUPPORTS:
        raise SystemExit(f"intent fixture has no {SUPPORTS} supports to keep")
    if INFILL:
        d["recipe"]["sparse_infill"] = ({"kind": "web_roof", "roof_spacing_mm": None}
            if INFILL == "cathedral" else {"kind": "fine" if INFILL == "rectilinear" else INFILL})
    path = os.path.join(WORK, "intent.json")
    json.dump(d, open(path, "w"))
    return path


def engine_peak(proc):
    peak = 0
    deadline = time.monotonic() + TIMEOUT
    while True:
        waited, status, usage = os.wait4(proc.pid, os.WNOHANG)
        if waited:
            proc.returncode = os.waitstatus_to_exitcode(status)
            return peak // 1024, round(usage.ru_utime + usage.ru_stime, 6)
        if time.monotonic() > deadline:
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


def run_cli(args, cwd):
    started = time.monotonic()
    err_path = os.path.join(cwd, f"{args[0]}.stderr.txt")
    with open(os.path.join(cwd, f"{args[0]}.stdout.json"), "w") as out, open(err_path, "w") as err:
        proc = subprocess.Popen(
            [os.path.join(R, "spiling-cli"), "project", *args, "--engine", os.path.join(R, "spiling-engine")],
            cwd=cwd, stdout=out, stderr=err, start_new_session=True)
        try:
            peak, cpu = engine_peak(proc)
            proc.wait(timeout=TIMEOUT)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            proc.wait()
            return {"ok": False, "seconds": TIMEOUT, "error": {"code": "timeout"}}
    failure = None
    for line in open(err_path):
        try:
            record = json.loads(line)
        except ValueError:
            continue
        if isinstance(record, dict) and record.get("level") == "error":
            failure = {"code": (record.get("error") or {}).get("code") or record.get("event"),
                       "domain": record.get("domain"),
                       "message": (record.get("error") or {}).get("message") or record.get("message"),
                       "details": record.get("error") or {}}
    return {"ok": proc.returncode == 0, "seconds": round(time.monotonic() - started, 2),
            "cpu_seconds": cpu, "engine_mib": peak or None, "error": failure}


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
        "fill_density": f"{recipe['infill_fraction'] * 100:.8g}%", "fill_pattern": prusa_pattern(recipe),
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


def adhesion_args(brimmed):
    """PrusaSlicer skirt/brim matching the recipe: one-layer skirt loops at
    the same distance and minimum (PrusaSlicer's minimum is filament length),
    an outer-only brim only where Spiling planned one (PrusaSlicer has no
    narrow/tall placement)."""
    recipe = INTENT["recipe"]
    skirt, brim = recipe.get("skirt"), recipe.get("brim")
    args = ["--skirts", str(skirt["loops"] if skirt else 0), "--skirt-height", "1"]
    if skirt:
        h = recipe.get("first_layer_height_mm") or recipe["layer_height_mm"]
        w = recipe["bead_width_mm"]
        bead = (w - h) * h + 3.141592653589793 * (h / 2) ** 2
        filament = 3.141592653589793 * (recipe["filament_diameter_mm"] / 2) ** 2
        args += ["--skirt-distance", str(skirt["distance_mm"]),
                 "--min-skirt-length", str(round(skirt["minimum_length_mm"] * bead / filament, 4))]
    if brim and brimmed:
        args += ["--brim-width", str(brim["width_mm"]), "--brim-separation", str(brim["separation_mm"]),
                 "--brim-type", "outer_only"]
    else:
        args += ["--brim-width", "0"]
    return args


def prusa_pattern(recipe):
    """Match native conventional patterns; Cathedral uses the labeled line control."""
    return {"gyroid": "gyroid", "grid": "grid"}.get(recipe["sparse_infill"]["kind"], "rectilinear")


def prusa(step, cwd, brimmed=False):
    started = time.monotonic()
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
        args = ["--layer-height", str(layer), "--fill-density", f"{fraction * 100:.8g}%",
                "--fill-pattern", prusa_pattern(recipe)]
    args += ["--support-material"] if SUPPORTS != "disabled" else []
    args += adhesion_args(brimmed)
    info = {"fill_pattern": prusa_pattern(recipe), "threads": 1}
    for stage, command in (("import", ["prusa-slicer", "--threads", "1", "--info", step]),
                           ("slice", ["prusa-slicer", "--threads", "1", "--export-gcode", *args,
                                      "--center", center, "-o", gcode, step])):
        info[stage] = {"command": command}
        try:
            proc = subprocess.run(command, cwd=cwd, capture_output=True, text=True, timeout=TIMEOUT)
            info[stage]["ok"] = proc.returncode == 0
            info[stage]["returncode"] = proc.returncode
            for stream in ("stdout", "stderr"):
                Path(cwd, f"prusa-{stage}.{stream}.txt").write_text(getattr(proc, stream))
            if stage == "import":
                info[stage].update(reference.mesh_info(proc.stdout))
            if proc.returncode:
                info[stage]["message"] = (proc.stderr or proc.stdout)[-300:]
        except subprocess.TimeoutExpired:
            info[stage].update({"ok": False, "message": "timeout"})
    info["seconds"] = round(time.monotonic() - started, 2)
    if info["slice"]["ok"] and os.path.exists(gcode):
        with open(gcode, errors="replace") as program:
            info.update(reference.program_info(program, layer))
    info["ok"] = bool(info["slice"]["ok"] and info.get("nonempty_model"))
    return info


def spiling_report(verification):
    """Verification totals and estimate from the fresh exported report."""
    report = {k: verification.get(k) for k in ("filament_length_mm", "support_filament_length_mm",
                                     "adhesion_filament_length_mm", "priming_filament_length_mm",
                                     "estimated_seconds")}
    parts = [report[k] for k in ("filament_length_mm", "support_filament_length_mm",
                                 "adhesion_filament_length_mm", "priming_filament_length_mm")]
    report["total_filament_mm"] = round(sum(v for v in parts if v is not None), 3)
    return report


def part(case, intent, results):
    name = case["file"]
    src = os.path.join(ROOT, "step", name)
    cwd = os.path.join(WORK, name)
    shutil.rmtree(cwd, ignore_errors=True)
    os.makedirs(cwd)
    local = os.path.join(cwd, "source.step")
    shutil.copy(src, local)
    record = {"part": name, "bytes": os.path.getsize(src),
              "sha256": hashlib.sha256(open(src, "rb").read()).hexdigest()}
    settings = ["--intent", intent] if intent else defaults_args()
    create = run_cli(["create", "p.spil", "--import", "source.step", *settings,
                      "--center-all-on-bed", "--compile", "--save"], cwd)
    record["spiling_compile"] = create
    record["settings"] = settings
    plan = None
    if create["ok"]:
        os.remove(local)
        record["source_absent"] = not os.path.exists(local)
        returned = os.path.join(cwd, "return")
        os.mkdir(returned)
        os.rename(os.path.join(cwd, "p.spil"), os.path.join(returned, "p.spil"))
        ret = run_cli(["open", "p.spil", "--read-only", "--verify", "--manufacturing-out", "out"], returned)
        record["spiling_return"] = ret
        if ret["ok"]:
            record["export_checks"] = audit.export_checks(os.path.join(returned, "out"))
            plan = json.load(open(os.path.join(returned, "out", "plan.json")))
            verification = json.load(open(os.path.join(returned, "out", "verification.json")))
            if "oracle" in case:
                record["oracle_checks"] = features.check(
                    case, plan, verification,
                    INTENT, json.load(open(os.path.join(cwd, "create.stdout.json"))))
    # PrusaSlicer slices the original source after Spiling, so its brim can
    # follow the brim Spiling planned.
    record["prusa"] = (prusa(src, cwd, brimmed=bool(plan and plan["adhesion"]["brim_paths"]))
                       if ARGS.reference == "prusa" else {"status": "not_requested"})
    if plan is not None:
        stdout = open(os.path.join(returned, "open.stdout.json")).read()
        m = re.search(r'"deposition_segments":\s*(\d+)', stdout)
        record["deposition_segments"] = int(m.group(1)) if m else None
        m = re.search(r'"filament_length_mm":\s*([0-9.]+)', stdout)
        record["filament_mm"] = float(m.group(1)) if m else None
        report = spiling_report(verification)
        record["spiling"] = report
        reference = record["prusa"]
        if reference.get("filament_mm") and report["total_filament_mm"]:
            record["filament_ratio"] = round(report["total_filament_mm"] / reference["filament_mm"], 4)
        if reference.get("estimated_seconds") and report.get("estimated_seconds"):
            record["time_ratio"] = round(report["estimated_seconds"] / reference["estimated_seconds"], 4)
        record["adhesion"] = plan["adhesion"]
        # Printers whose firmware takes a delivery package (Bambu) export it
        # beside the program.
        record["delivery_package"] = os.path.exists(
            os.path.join(returned, "out", "program.gcode.3mf"))
        layers = plan["layers"]
        spans = [l["region_coverage"]["unsupported_overhang_mm"] for l in layers]
        record["layers"] = len(layers)
        record["overhang_layers"] = sum(1 for v in spans if v > 0)
        record["max_overhang_mm"] = round(max(spans, default=0), 3)
    with LOCK:
        results.write(json.dumps(record) + "\n")
        results.flush()
    c = record["spiling_compile"]
    print(name, "prusa", record["prusa"].get("ok"), "| spiling", c["ok"],
          (c["error"] or {}).get("code"), "| return", (record.get("spiling_return") or {}).get("ok"),
          "| filament", record.get("filament_ratio"), "time", record.get("time_ratio"),
          "| package", record.get("delivery_package"), flush=True)
    return record



def main():
    global INTENT
    manifest = json.load(open(ARGS.manifest))
    cases = audit.select_cases(manifest, ARGS.prefixes, ARGS.cohort)
    if ARGS.list:
        print(json.dumps(cases, indent=2))
        return 0
    if not hasattr(os, "wait4") or not os.path.isdir("/proc"):
        raise ValueError("native corpus resource monitoring currently requires Linux /proc and wait4")
    audit.check_inputs(cases, os.path.join(ROOT, "step"))
    for path in (OUT, OUT + ".run.json", OUT + ".summary.json", WORK):
        if os.path.exists(path):
            raise ValueError(f"refusing to overwrite prior run: {path}; use a new corpus run directory")
    comparisons = {}
    if ARGS.coverage_control:
        path, corpus = ARGS.coverage_control
        comparisons["coverage_control"] = dict(panel_coverage.evidence_identity(path),
                                               corpus=str(Path(corpus).resolve()))
    if ARGS.prusa_off_results:
        comparisons["prusa_off"] = panel_coverage.evidence_identity(ARGS.prusa_off_results)
    os.mkdir(WORK)
    started = time.monotonic()
    receipt = {
        "started_utc": datetime.now(timezone.utc).isoformat(),
        "command": sys.argv, "profile_declared": ARGS.profile,
        "binaries": {name: {"path": str(Path(R, name).resolve()), "sha256": audit.digest(Path(R, name))}
                     for name in ("spiling-cli", "spiling-engine")},
        "runner_sha256": audit.digest(__file__), "audit_sha256": audit.digest(audit.__file__),
        "features_sha256": audit.digest(features.__file__),
        "reference_sha256": audit.digest(reference.__file__),
        "panel_coverage_sha256": audit.digest(panel_coverage.__file__),
        "located_spans_sha256": audit.digest(panel_coverage.located_spans.__file__),
        "comparisons": comparisons,
        "manifest_sha256": audit.digest(ARGS.manifest),
        "cohort": ARGS.cohort, "cases": cases,
        "settings": {"supports": SUPPORTS, "infill": INFILL, "fraction": FRACTION,
                     "reference": ARGS.reference, "reference_ini": PRUSA_INI,
                     "reference_ini_sha256": audit.digest(PRUSA_INI) if PRUSA_INI and PRUSA_INI != "from-intent" else None,
                     "intent_sha256": audit.digest(INTENT_SRC) if INTENT_SRC else None},
        "jobs": JOBS, "per_cli_timeout_seconds": TIMEOUT,
        "timing_scope": {"spiling_compile": "create/import/place/compile/save",
                         "spiling_return": "fresh process reopen/verify/export",
                         "cpu_seconds": "CLI and waited descendants, per-process wait4; not global concurrent-child deltas",
                         "prusa": "reference slice and mesh info",
                         "not_separately_measured": ["build", "import", "placement", "construction", "emission", "save"]},
    }
    if ARGS.reference == "prusa":
        reference_binary = shutil.which("prusa-slicer")
        if reference_binary is None:
            raise ValueError("requested Prusa reference is not installed; use --reference none for a labeled Spiling-only run")
        receipt["reference_binary"] = {"path": reference_binary, "sha256": audit.digest(reference_binary)}
        help_result = subprocess.run([reference_binary, "--help"], capture_output=True, text=True, timeout=TIMEOUT)
        receipt["reference_binary"]["version_banner"] = (help_result.stdout or help_result.stderr).splitlines()[0]
    diagnose = subprocess.run([os.path.join(R, "spiling-cli"), "diagnose", "--engine", os.path.join(R, "spiling-engine")],
                              cwd=WORK, capture_output=True, text=True, timeout=TIMEOUT)
    if diagnose.returncode:
        raise ValueError(f"matching CLI/engine readiness refused: {diagnose.stderr}")
    receipt["readiness"] = json.loads(diagnose.stdout)
    with open(OUT + ".run.json", "x") as output:
        json.dump(receipt, output, indent=2)
    intent = intent_file()
    if INTENT is None:
        INTENT = probe_intent()
    else:
        INTENT = json.load(open(intent))
    receipt["effective_intent"] = INTENT
    with open(OUT + ".run.json", "w") as output:
        json.dump(receipt, output, indent=2)
    records = []
    failure = None
    with open(OUT, "x") as results, ThreadPoolExecutor(JOBS) as pool:
        try:
            records = list(pool.map(lambda case: part(case, intent, results), cases))
        except Exception as error:
            failure = str(error)
    if failure is not None:
        records = [json.loads(line) for line in Path(OUT).read_text().splitlines() if line.strip()]
    summary = audit.verdict(cases, records, INTENT)
    summary["harness_error"] = failure
    summary["ok"] = summary["ok"] and failure is None
    summary["elapsed_seconds"] = round(time.monotonic() - started, 3)
    summary["reference"] = {"requested": ARGS.reference,
                            "sliced": sum(row.get("prusa", {}).get("ok") is True for row in records),
                            "refused": sum(row.get("prusa", {}).get("ok") is False for row in records)}
    summary["scaling"] = audit.scaling_observations(cases, records, summary["outcomes"])
    summary["coverage"] = panel_coverage.summary(
        cases, records, receipt, ROOT, ARGS.coverage_control, ARGS.prusa_off_results)
    summary["ok"] = summary["ok"] and summary["coverage"]["ok"]
    with open(OUT + ".summary.json", "x") as output:
        json.dump(summary, output, indent=2)
    print(json.dumps(summary, indent=2))
    return 0 if summary["ok"] else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, OSError) as error:
        PARSER.exit(1, f"corpus: {error}\n")
