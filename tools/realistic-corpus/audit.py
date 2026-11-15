# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0

import hashlib
import json
import math
from pathlib import Path
import features


def digest(path):
    value = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def select_cases(manifest, prefixes=(), cohort="full"):
    cases = manifest["parts"]
    names = [case["file"] for case in cases]
    if not names or len(names) != len(set(names)):
        raise ValueError("manifest must contain unique, nonempty case inventory")
    for name in names:
        if Path(name).name != name or name in (".", ".."):
            raise ValueError(f"case is not a source basename: {name}")
    if cohort != "full":
        selected = manifest.get("cohorts", {}).get(cohort)
        if not selected or len(selected) != len(set(selected)) or set(selected) - set(names):
            raise ValueError(f"unknown or invalid cohort: {cohort}")
        cases = [case for case in cases if case["file"] in selected]
    if prefixes:
        for prefix in prefixes:
            if not any(case["file"].startswith(prefix) for case in cases):
                raise ValueError(f"prefix matches no case in {cohort}: {prefix}")
        cases = [case for case in cases if any(case["file"].startswith(p) for p in prefixes)]
    if not cases:
        raise ValueError("selection is empty")
    return cases


def check_inputs(cases, directory):
    for case in cases:
        path = Path(directory) / case["file"]
        if not path.is_file() or digest(path) != case["sha256"]:
            raise ValueError(f"missing or changed source: {case['file']}")


def positive(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and value > 0


def export_checks(directory):
    root = Path(directory)
    checks = {}
    for name in ("manifest.json", "plan.json", "program.gcode", "verification.json"):
        path = root / name
        checks[name] = path.is_file() and path.stat().st_size > 0
    if not all(checks.values()):
        return checks
    report = json.loads((root / "verification.json").read_text())
    plan = json.loads((root / "plan.json").read_text())
    checks["fresh_verified"] = report.get("verified") is True
    checks["actual_deposition"] = positive(report.get("deposition_segments")) and positive(report.get("filament_length_mm"))
    checks["nonempty_layers"] = bool(plan.get("layers"))
    return checks


def envelope_refusal(error, intent):
    if error.get("code") != "build_envelope_exceeded" or error.get("domain") != "placement":
        return False
    details = error.get("details", {})
    placed = details.get("placed_bounds_mm", {})
    envelope = details.get("build_envelope")
    if envelope != intent["printer"]["build_envelope"]:
        return False
    try:
        minimum, maximum = placed["min"], placed["max"]
        values = [*minimum, *maximum, *envelope["min"], *envelope["max"]]
        return (len(minimum) == len(maximum) == 3
                and all(math.isfinite(value) for value in values)
                and all(minimum[axis] <= maximum[axis] for axis in range(3))
                and any(minimum[axis] < envelope["min"][axis]
                        or maximum[axis] > envelope["max"][axis] for axis in range(3)))
    except (KeyError, TypeError, ValueError, IndexError):
        return False


def assess(case, record, intent):
    failures = []
    if record.get("sha256") != case["sha256"]:
        failures.append("source_identity")
    expectation = case.get("expectation", {"kind": "complete"})
    compiled = record.get("spiling_compile", {})
    error = compiled.get("error") or {}
    if expectation["kind"] == "refusal":
        if compiled.get("ok") is not False or error.get("code") != expectation["code"] or error.get("domain") != expectation["domain"]:
            failures.append("expected_refusal")
        status = "expected_refusal"
    elif expectation["kind"] == "complete_or_envelope_refusal" and compiled.get("ok") is False:
        if not envelope_refusal(error, intent):
            failures.append("expected_envelope_refusal")
        status = "expected_refusal"
    elif expectation["kind"] in ("complete", "complete_or_envelope_refusal"):
        if compiled.get("ok") is not True:
            failures.append("compile")
        if record.get("spiling_return", {}).get("ok") is not True:
            failures.append("source_absent_return")
        if record.get("source_absent") is not True:
            failures.append("external_source_removed")
        required = ("manifest.json", "plan.json", "program.gcode", "verification.json",
                    "fresh_verified", "actual_deposition", "nonempty_layers")
        checks = record.get("export_checks", {})
        failures.extend(f"export:{name}" for name in required if checks.get(name) is not True)
        if "oracle" in case:
            observations = record.get("oracle_checks", {})
            failures.extend(f"feature:{name}" for name in sorted(features.required_checks(case, intent))
                            if observations.get(name) is not True)
        status = "complete"
    else:
        raise ValueError(f"unknown expectation: {expectation['kind']}")
    if record.get("harness_error"):
        failures.append("harness_error")
    return {"status": "failed" if failures else status, "failures": failures}


def verdict(cases, records, intent):
    expected = {case["file"]: case for case in cases}
    grouped = {}
    for record in records:
        grouped.setdefault(record["part"], []).append(record)
    missing = sorted(set(expected) - set(grouped))
    unexpected = sorted(set(grouped) - set(expected))
    duplicates = sorted(name for name, rows in grouped.items() if len(rows) != 1)
    outcomes = {name: assess(case, grouped[name][0], intent) for name, case in expected.items()
                if name in grouped and name not in duplicates}
    return {"ok": bool(expected) and len(expected) == len(cases) and not (missing or unexpected or duplicates)
            and len(outcomes) == len(expected)
            and all(row["status"] != "failed" for row in outcomes.values()),
            "expected_cases": len(expected), "recorded_cases": len(records),
            "missing": missing, "unexpected": unexpected, "duplicates": duplicates,
            "outcomes": outcomes}


def scaling_observations(cases, records, outcomes):
    indexed = {record["part"]: record for record in records}
    groups = {}
    for case in cases:
        parameters = case.get("parameters", {})
        axis = parameters.get("scale_axis")
        if axis is None:
            continue
        record = indexed.get(case["file"], {})
        compiled = record.get("spiling_compile", {})
        returned = record.get("spiling_return", {})
        groups.setdefault(axis, []).append({
            "part": case["file"], "input_scale": parameters["scale"],
            "source_bodies": case.get("oracle", {}).get("occurrences"),
            "compile_seconds": compiled.get("seconds"), "compile_cpu_seconds": compiled.get("cpu_seconds"),
            "return_seconds": returned.get("seconds"), "return_cpu_seconds": returned.get("cpu_seconds"),
            "engine_mib": compiled.get("engine_mib"), "layers": record.get("layers"),
            "deposition_segments": record.get("deposition_segments"),
            "complete": outcomes.get(case["file"], {}).get("status") == "complete",
        })
    return {axis: sorted(rows, key=lambda row: row["input_scale"]) for axis, rows in groups.items()}
