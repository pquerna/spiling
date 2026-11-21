"""Gate native located spans; report separate cross-slicer road observations."""

from collections import defaultdict
import json
import math
from pathlib import Path

import audit
import located_spans


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


def native_coverage(corpus, part, receipt):
    work = Path(corpus) / "work" / part
    plan_path = work / "return/out/plan.json"
    with plan_path.open(encoding="utf-8") as source:
        plan = json.load(source)
    recipe = receipt["effective_intent"]["recipe"]
    supports = recipe["supports"]
    gap = 0.0 if supports["kind"] == "disabled" else supports["z_gap_mm"]
    spans, measured = located_spans.native_observation(
        plan, plan_path, work / "located-spans.ndjson", gap, recipe["process"]["max_bridge_span_mm"])
    return spans, measured, report_warning(span_layers(plan), spans)


def report_warning(reported, spans):
    worst = {}
    for span in spans:
        interval = tuple(span["physical_z_mm"])
        if interval not in worst or span["length_mm"] > worst[interval]["length_mm"]:
            worst[interval] = span
    count, largest = 0, None
    for interval in sorted(reported.keys() | worst.keys()):
        located = worst.get(interval)
        declared = reported.get(interval, 0.0)
        observed = located["length_mm"] if located else 0.0
        difference = abs(declared - observed)
        if difference > located_spans.EPS:
            count += 1
            if largest is None or difference > largest["absolute_delta_mm"]:
                largest = {"physical_z_mm": interval, "reported_span_mm": declared,
                           "located_span_mm": observed, "absolute_delta_mm": difference,
                           "located_witness": located}
    return {"intervals_disagreeing": count, "largest": largest} if count else None


def segment_key(points):
    return tuple(sorted(map(tuple, points)))


class GapIndex:
    def __init__(self):
        self.segments = []
        self.exact = set()
        self.buckets = defaultdict(list)
        self.large = set()

    def add(self, points):
        segment = segment_key(points)
        if segment in self.exact:
            return
        self.exact.add(segment)
        ordinal = len(self.segments)
        self.segments.append(segment)
        footprint = located_spans.cells(located_spans.footprint_box(*segment, located_spans.EPS))
        if footprint is None:
            self.large.add(ordinal)
        else:
            for cell in footprint:
                self.buckets[cell].append(ordinal)

    def overlap(self, start, end, unit, length):
        footprint = located_spans.cells(located_spans.footprint_box(start, end, located_spans.EPS))
        if footprint is None:
            candidates = range(len(self.segments))
        else:
            candidates = self.large.copy()
            for cell in footprint:
                candidates.update(self.buckets.get(cell, ()))
        for ordinal in candidates:
            offsets = [(point[0] - start[0], point[1] - start[1]) for point in self.segments[ordinal]]
            if any(abs(dx * unit[1] - dy * unit[0]) > located_spans.EPS for dx, dy in offsets):
                continue
            left, right = sorted(dx * unit[0] + dy * unit[1] for dx, dy in offsets)
            if max(left, 0.0) < min(right, length):
                yield max(left, 0.0), min(right, length)


def located_losses(before, after):
    controls = defaultdict(GapIndex)
    for span in before:
        controls[tuple(span["physical_z_mm"])].add(span["points_mm"])
    changes = {}
    lost_spans = 0
    for span in after:
        interval = tuple(span["physical_z_mm"])
        control = controls.get(interval)
        if control is not None and segment_key(span["points_mm"]) in control.exact:
            continue
        start, end = span["points_mm"]
        length = math.dist(start, end)
        if length <= located_spans.EPS:
            continue
        unit = tuple((finish - begin) / length for begin, finish in zip(start, end))
        covered = control.overlap(start, end, unit, length) if control is not None else ()
        missing = max(located_spans.missing_intervals(covered, length),
                      key=lambda pair: pair[1] - pair[0], default=None)
        if missing is None:
            continue
        lost_spans += 1
        uncovered = missing[1] - missing[0]
        if interval not in changes or uncovered > changes[interval]["uncovered_span_mm"]:
            changes[interval] = {"physical_z_mm": interval, "candidate_span_mm": span["length_mm"],
                                 "candidate_layer": span["layer"], "candidate_road": span["road"],
                                 "candidate_role": span["role"], "uncovered_span_mm": uncovered,
                                 "uncovered_points_mm": [tuple(axis + distance * step
                                                                for axis, step in zip(start, unit))
                                                         for distance in missing]}
    return lost_spans, [changes[interval] for interval in sorted(changes)]


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


def prusa_span_comparison(corpus, part, row, receipt):
    settings = receipt["settings"]
    if settings["reference"] != "prusa" or settings["supports"] != "breakaway":
        return {"status": "not_requested"}
    if row.get("prusa", {}).get("ok") is not True:
        return {"status": "reference_unavailable"}
    if not fresh(row):
        return {"status": "fresh_spiling_unavailable"}
    work = Path(corpus) / "work" / part
    try:
        recipe = receipt["effective_intent"]["recipe"]
        supports = recipe["supports"]
        return located_spans.compare(work / "prusa.gcode", work / "return/out/plan.json",
                                     work / "located-spans.ndjson", supports["z_gap_mm"],
                                     recipe["process"]["max_bridge_span_mm"])
    except (OSError, ValueError, TypeError, KeyError, OverflowError) as error:
        return {"status": "unresolved", "detail": str(error)}


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

    results = []
    for case in cases:
        name = case["file"]
        row = current.get(name)
        baseline = control.get(name) if control is not None else None
        if row is None or not fresh(row):
            results.append({"part": name, "status": "no_fresh_candidate",
                            "prusa_span_comparison": {"status": "fresh_spiling_unavailable"},
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
                 "prusa_span_comparison": prusa_span_comparison(corpus, name, row, receipt),
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
                    before, control_observed, control_warning = native_coverage(control_corpus, name, control_receipt)
                    after, candidate_observed, candidate_warning = native_coverage(corpus, name, receipt)
                    lost, local = located_losses(before, after)
                except (OSError, ValueError, TypeError, KeyError, OverflowError) as error:
                    entry["failures"].append("missing_located_span_evidence")
                    entry["located_span_error"] = str(error)
                else:
                    if lost:
                        entry["failures"].append("new_or_increased_local_spans")
                    entry["span_metric"] = located_spans.METRIC
                    entry["located_spans_with_losses"] = lost
                    entry["local_span_losses"] = local
                    entry["span_report_warnings"] = {"control": control_warning, "candidate": candidate_warning}
                    for field in ("unsupported_layers", "max_span_mm"):
                        entry[f"reported_{field}"] = entry[field]
                        entry[f"control_{field}"] = control_observed[field]
                        entry[field] = candidate_observed[field]
        results.append(entry)
    return {"checked_against_control": control is not None,
            "ok": not issues and not any(row["failures"] for row in results),
            "control_mismatches": issues, "prusa_off_mismatches": off_issues, "cases": results}
