"""Compare native reported spans, not physical printability or located gaps."""

import json
import math
from pathlib import Path

import audit


def nonnegative(value):
    return type(value) in (int, float) and math.isfinite(value) and value >= 0


def span_layers(plan):
    intervals = {}
    for layer in plan["layers"]:
        lower, upper = layer["lower_z_mm"], layer["z_mm"]
        span = layer["region_coverage"]["unsupported_overhang_mm"]
        if not all(type(bound) in (int, float) and math.isfinite(bound) for bound in (lower, upper)) or not lower < upper:
            raise ValueError("invalid native coverage interval")
        if (lower, upper) in intervals or not nonnegative(span):
            raise ValueError("duplicate interval or invalid native span")
        intervals[lower, upper] = span
    if not intervals:
        raise ValueError("missing native coverage intervals")
    return intervals


def load_results(path):
    path = Path(path)
    rows = {}
    for line in path.read_text().splitlines():
        row = json.loads(line)
        part = row["part"]
        if part in rows:
            raise ValueError(f"duplicate coverage evidence: {part}")
        rows[part] = row
    receipt = json.loads(Path(str(path) + ".run.json").read_text())
    return rows, receipt


def evidence_identity(path):
    path = Path(path).resolve()
    receipt = Path(str(path) + ".run.json")
    return {"results": str(path), "results_sha256": audit.digest(path),
            "run_receipt_sha256": audit.digest(receipt)}


def distinct_settings(control, candidate, prusa_off):
    if not prusa_off:
        return [field for field in ("effective_intent", "settings", "profile_declared")
                if control[field] != candidate[field]]
    settings = lambda receipt: {key: value for key, value in receipt["settings"].items()
                                if key != "supports"}
    intent = lambda receipt: dict(receipt["effective_intent"], recipe={
        key: value for key, value in receipt["effective_intent"]["recipe"].items()
        if key != "supports"})
    differences = []
    if control["settings"]["supports"] != "disabled" or candidate["settings"]["supports"] != "breakaway":
        differences.append("support_policy")
    if settings(control) != settings(candidate) or intent(control) != intent(candidate):
        differences.append("reference_settings")
    if (not control.get("reference_binary") or not candidate.get("reference_binary")
            or control["reference_binary"]["sha256"] != candidate["reference_binary"]["sha256"]):
        differences.append("reference_binary")
    return differences


def matching_source(control, candidate):
    return control is not None and all(control.get(field) == candidate[field] for field in ("part", "sha256", "bytes"))


def fresh(row):
    return (row.get("source_absent") is True and row.get("spiling_compile", {}).get("ok") is True
            and row.get("spiling_return", {}).get("ok") is True
            and row.get("export_checks", {}).get("fresh_verified") is True)


def reported_losses(before, after):
    failures = []
    if max(after.values()) > max(before.values()):
        failures.append("max_span_increased")
    if sum(span > 0 for span in after.values()) > sum(span > 0 for span in before.values()):
        failures.append("unsupported_layers_increased")
    changes = []
    for interval, span in after.items():
        if span > 0 and (interval not in before or span > before[interval]):
            changes.append({"physical_z_mm": interval, "control_span_mm": before.get(interval),
                            "candidate_span_mm": span})
    if changes:
        failures.append("new_or_increased_local_spans")
    return failures, changes


def support_mm(row):
    value = row.get("spiling", {}).get("support_filament_length_mm")
    return value if nonnegative(value) else None


def prusa_mm(row):
    prusa = row.get("prusa", {})
    value = prusa.get("filament_mm")
    return value if prusa.get("ok") is True and audit.positive(value) else None


def prusa_support_estimate(on_row, off_row):
    if not matching_source(off_row, on_row):
        return None, "missing_matched_reference_source"
    on, off = prusa_mm(on_row), prusa_mm(off_row)
    if on is None or off is None:
        return None, "reference_slice_unavailable"
    # The final three arguments are -o, its path, and the source path. Keep
    # dynamically chosen adhesion (not just the recipe) in the comparison.
    on_command = on_row["prusa"].get("slice", {}).get("command", [])
    off_command = off_row["prusa"].get("slice", {}).get("command", [])
    if (len(on_command) < 4 or len(off_command) < 4 or on_command[-3] != "-o"
            or off_command[-3] != "-o" or on_command[:-3].count("--support-material") != 1
            or [arg for arg in on_command[:-3] if arg != "--support-material"] != off_command[:-3]):
        return None, "reference_commands_differ"
    if on < off:
        return None, "reference_delta_negative"
    return on - off, "matched_on_minus_off"


def summary(cases, records, receipt, corpus, control_results=None, off_results=None):
    current = {row["part"]: row for row in records}
    control, control_corpus = None, None
    reference_off, off_issues = None, []
    if off_results is not None:
        reference_off, off_receipt = load_results(off_results)
        off_issues = distinct_settings(off_receipt, receipt, prusa_off=True)
    issues = []
    if control_results is not None:
        control_path, control_corpus = control_results
        control, control_receipt = load_results(control_path)
        issues = distinct_settings(control_receipt, receipt, prusa_off=False)

    def read_spans(root, part):
        plan = Path(root) / "work" / part / "return/out/plan.json"
        return span_layers(json.loads(plan.read_text()))

    results = []
    for case in cases:
        name = case["file"]
        row = current.get(name)
        baseline = control.get(name) if control is not None else None
        if row is None or not fresh(row):
            results.append({"part": name, "status": "no_fresh_candidate",
                            "failures": ["missing_candidate_coverage"]
                            if baseline is not None and fresh(baseline) else []})
            continue
        material = support_mm(row)
        reference = None if reference_off is None else reference_off.get(name)
        prusa_support, prusa_status = None, "not_requested"
        if off_results is not None:
            if off_issues:
                prusa_status = "reference_settings_differ"
            else:
                prusa_support, prusa_status = prusa_support_estimate(row, reference)
        entry = {"part": name, "unsupported_layers": row.get("overhang_layers"),
                 "max_span_mm": row.get("max_overhang_mm"), "support_filament_mm": material,
                 "prusa_support_estimate_mm": prusa_support,
                 "prusa_support_status": prusa_status,
                 "support_vs_prusa": material / prusa_support if material is not None and prusa_support else None,
                 "control_support_filament_mm": None, "support_drop_review": False,
                 "failures": []}
        if control is not None:
            if not matching_source(baseline, row) or not fresh(baseline):
                entry["failures"].append("missing_matched_control_coverage")
            else:
                baseline_support = support_mm(baseline)
                entry["control_support_filament_mm"] = baseline_support
                if baseline_support is None or material is None:
                    entry["failures"].append("missing_verified_support_filament")
                elif baseline_support > 0:
                    entry["support_vs_control"] = material / baseline_support
                    entry["support_drop_review"] = entry["support_vs_control"] < 0.95
                try:
                    before, after = read_spans(control_corpus, name), read_spans(corpus, name)
                except (OSError, ValueError, TypeError, KeyError, OverflowError):
                    entry["failures"].append("missing_native_span_evidence")
                else:
                    losses, local = reported_losses(before, after)
                    entry["failures"].extend(losses)
                    entry["local_span_losses"] = local
                    entry["control_unsupported_layers"] = sum(span > 0 for span in before.values())
                    entry["unsupported_layers"] = sum(span > 0 for span in after.values())
                    entry["control_max_span_mm"] = max(before.values())
                    entry["max_span_mm"] = max(after.values())
        results.append(entry)
    return {"checked_against_control": control is not None,
            "ok": not issues and not any(row["failures"] for row in results),
            "control_mismatches": issues, "prusa_off_mismatches": off_issues, "cases": results}
