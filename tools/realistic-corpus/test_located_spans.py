import io
import json
import math
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

import located_spans as spans
import panel_coverage


PRUSA = """G90
M82
;Z:0.2
;HEIGHT:0.2
G1 X0 Y0 Z0.2
;TYPE:Perimeter
;WIDTH:0.4
G1 X1 E1
G1 E0
G92 E0
G1 X9
G1 E1
G1 X10 E2
;Z:0.4
;HEIGHT:0.2
G1 X0 Z0.4
;TYPE:Bridge infill
;HEIGHT:0.4
G1 X5 E3
G1 X10 E4
"""


def road(layer, z, start, end, *, support=False):
    return spans.Road(layer, 0, "test", support, z, [start, end], [0.4])


class LocatedSpanTests(unittest.TestCase):
    def test_retract_reset_keeps_debt_and_bridge_merges_across_commands(self):
        roads = spans.prusa_roads(io.StringIO(PRUSA))
        self.assertEqual([item.points for item in roads],
                         [[(0, 0), (1, 0)], [(9, 0), (10, 0)], [(0, 0), (5, 0), (10, 0)]])
        evidence = []
        summary = spans.observe(roads, 0.2, 8.0, evidence.append)
        self.assertEqual(summary["unsupported_spans"], 0)
        self.assertEqual(summary["tagged_bounded_bridges"], 1)
        self.assertEqual(evidence[0]["physical_z_mm"], (0.2, 0.4))
        self.assertAlmostEqual(evidence[0]["length_mm"], 7.6)
        self.assertEqual(evidence[0]["bounds_xy_mm"], [[1.2, 0], [8.8, 0]])
        self.assertEqual(spans.observe(roads, 0.2, 7.0, lambda _: None)["unsupported_spans"], 1)

    def test_adjacent_model_and_separated_support_differ_and_tangency_is_not_coverage(self):
        candidate = road(2, (0.4, 0.6), (0, 0), (10, 0))
        older = road(0, (0.0, 0.2), (0, 0), (10, 0))
        self.assertEqual(spans.observe([older, candidate], 0.2, 10, list().append)["max_span_mm"], 10)
        older.support = True
        self.assertEqual(spans.observe([older, candidate], 0.2, 10, list().append)["max_span_mm"], 0)
        later = road(1, (0.2, 0.4), (0, 0.2), (10, 0.2))
        self.assertEqual(spans.observe([later, candidate], 0.2, 10, list().append)["max_span_mm"], 10)

    def test_unsupported_reference_modes_are_not_reported_as_zero(self):
        for program in (PRUSA.replace("G1 X5 E3", "G2 X5 E3 I1"),
                        PRUSA.replace(";TYPE:Bridge infill", ";TYPE:Unknown"),
                        PRUSA.replace("G90", "G91")):
            with self.subTest(program=program), self.assertRaises(ValueError):
                spans.prusa_roads(io.StringIO(program))

    def test_an_unbacked_turn_ends_the_straight_span(self):
        candidate = spans.Road(1, 0, "bridge", False, (0.2, 0.4),
                               [(0, 0), (10, 0), (10, 1), (0, 1)], [0.4] * 3)
        summary = spans.observe([candidate], 0.2, 20, list().append)
        self.assertEqual(summary["unsupported_spans"], 3)
        self.assertEqual(summary["tagged_bounded_bridges"], 0)
        self.assertEqual(summary["max_span_mm"], 10)

    def test_endpoint_disks_anchor_each_run_without_filling_its_centerline(self):
        for role in ("bridge", "Bridge infill"):
            with self.subTest(role=role):
                candidate = spans.Road(1, 0, role, False, (0.2, 0.4),
                                       [(0, 0), (4, 0), (4, 5)], [0.4, 0.4])
                donors = [road(0, (0.0, 0.2), (0, -0.3), (4, -0.3))]
                bounded, one_ended = [gap for gap, _ in spans.road_gaps(candidate, spans.LowerEdges(donors), 4)]
                self.assertEqual(bounded["length_mm"], 4)
                self.assertTrue(bounded["bridge_within_limit"])
                self.assertEqual(one_ended["length_mm"], 5)
                self.assertFalse(one_ended["two_ended"])
                self.assertFalse(one_ended["bridge_within_limit"])
                donors[0].points = [(0, -0.4), (4, -0.4)]
                tangent, _ = next(spans.road_gaps(candidate, spans.LowerEdges(donors), 4))
                self.assertFalse(tangent["two_ended"])

    def test_wall_reports_do_not_exempt_other_runs_or_ambiguous_matches(self):
        wall = self.gap((0, 0), (4, 0), role="bridge")
        shorter = self.gap((0, 1), (3, 1), role="bridge")
        tied = self.gap((0, 2), (4, 2), road=1, role="bottom_skin")
        reports = {(1.0, 1.2): 4}
        plan = {"layers": [{"index": 8, "paths": [{"owner": {"kind": "wall"}}, {}]}]}
        panel_coverage.locate_wall_reports(plan, reports, [wall, shorter])
        self.assertEqual(wall["plan_reported_wall"], 4)
        self.assertNotIn("plan_reported_wall", shorter)
        panel_coverage.locate_wall_reports(plan, reports, [wall, shorter, tied])
        self.assertNotIn("plan_reported_wall", wall)

    @staticmethod
    def gap(start, end, interval=(1.0, 1.2), layer=8, road=0, role="top_skin"):
        path = spans.Road(layer, road, role, False, interval, [start, end], [0.45])
        return spans.describe_gap(path, path.points, math.dist(start, end), False, 10)

    def test_located_gate_accepts_moved_reversed_gaps_and_only_counts_length_increase(self):
        before = [self.gap((0, 0), (4, 0))]
        moved = self.gap((4.1, 0.275), (0.1, 0.275), layer=9, road=2)
        self.assertEqual(panel_coverage.located_losses(before, [moved], 0.275), [])
        longer = self.gap((4.1, 0.2), (-0.15, 0.2), layer=9, road=3)
        loss, = panel_coverage.located_losses(before, [longer], 0.275)
        self.assertEqual(loss["candidate_road"], 3)
        self.assertEqual(loss["increase_mm"], 0.25)
        self.assertEqual(loss["control"], before[0])

    def test_located_gate_keeps_spans_at_another_locus_role_or_z_new(self):
        before = [self.gap((0, 0), (4, 0))]
        after = [self.gap((0, 0.276), (4, 0.276)),
                 self.gap((4, 0), (0, 0), interval=(1.2, 1.4)),
                 self.gap((0, 0), (4, 0), role="bridge"),
                 self.gap((4.1, 0), (5, 0)), self.gap((2, -2), (2, 2)),
                 self.gap((0, 0), (4, 0.5))]
        changes = panel_coverage.located_losses(before, after, 0.275)
        self.assertEqual(len(changes), len(after))
        self.assertTrue(all(change["registration"] == "no_overlap" for change in changes))

    def test_located_gate_reserves_exact_span_and_never_reuses_control(self):
        before = [self.gap((0, 0), (4, 0))]
        adjacent = self.gap((0, 0.1), (4, 0.1), road=2)
        exact = self.gap((4, 0), (0, 0), road=3)
        loss, = panel_coverage.located_losses(before, [adjacent, exact], 0.275)
        self.assertEqual(loss["candidate_road"], 2)
        self.assertEqual(loss["registration"], "contended")

    def test_located_gate_reassigns_a_pair_instead_of_inventing_a_new_span(self):
        before = [self.gap((0, 0), (4, 0)), self.gap((0, 0.3), (4, 0.3))]
        after = [self.gap((0, 0.1), (4, 0.1)), self.gap((0, -0.2), (4, -0.2))]
        self.assertEqual(panel_coverage.located_losses(before, after, 0.275), [])

    def test_prusa_region_registers_shifted_transverse_bridge_without_matching_endpoints(self):
        candidate = self.gap((10, 1), (14, 1), interval=(1.0, 1.2), role="bottom_skin")
        bridge = road(4, (1.22, 1.42), (2, -1), (2, 1))
        bridge.role = "Bridge infill"
        witness, = spans.prusa_regions([bridge], [candidate], (10, 1), 0.2, 10)
        self.assertEqual(witness, {"status": "no_geometry"})
        bridge.z = (1.17, 1.42)
        witness, = spans.prusa_regions([bridge], [candidate], (10, 1), 0.2, 10)
        self.assertEqual(witness["status"], "unsupported")
        self.assertEqual(witness["assessment"], "partial_reference_coverage")
        self.assertAlmostEqual(witness["unsupported_span_mm"], 0.4)
        self.assertAlmostEqual(witness["reference_gap"]["length_mm"], 2)

    def test_prusa_region_distinguishes_supported_model_from_support_without_model(self):
        candidates = [self.gap((0, offset), (4, offset), interval=(0.9, 1.2), road=index)
                      for index, offset in enumerate((0, 2))]
        model = road(5, (1.31, 1.46), (-1, 0.1), (5, 0.1))
        donors = [road(4, (0.96, 1.12), (-1, offset), (5, offset), support=True)
                  for offset in (0.1, 2)]
        supported, absent = spans.prusa_regions([model, *donors], candidates, (0, 0), 0.2, 10)
        self.assertEqual(supported["status"], "supported")
        self.assertAlmostEqual(supported["model_overlap_mm"], 4)
        self.assertEqual(supported["unsupported_span_mm"], 0)
        self.assertEqual(absent, {"status": "no_geometry"})

    def test_prusa_regional_gaps_do_not_join_across_layers_or_count_bounded_bridges(self):
        candidate = self.gap((0, 0), (4, 0), interval=(1.0, 1.2))
        paths = [road(4, (0.89, 1.09), (0, 0), (2, 0)),
                 road(5, (1.11, 1.31), (2, 0), (4, 0))]
        witness, = spans.prusa_regions(paths, [candidate], (0, 0), 0.2, 10)
        self.assertEqual(witness["unsupported_span_mm"], 2)
        witness, = spans.prusa_regions(spans.prusa_roads(io.StringIO(PRUSA)),
                                     [self.gap((2, 0), (6, 0), interval=(0.23, 0.43))], (0, 0), 0.2, 8)
        self.assertEqual(witness["status"], "supported")

    def test_coverage_gate_uses_prusa_region_without_any_native_control(self):
        cases = [("longer", 8, 0, 4, True, "longer_registered_spans_than_prusa", "supported"),
                 ("native_bounded", 8, 0, 4, True, None, None),
                 ("native_one_ended", 8, 0, 4, True, "longer_registered_spans_than_prusa", "supported"),
                 ("reported_wall", 8, 0, 4, True, None, "supported"),
                 ("above", 8, 0, 2.001, True, "longer_registered_spans_than_prusa", "supported"),
                 ("equal", 8, 0, 2.0, True, None, "supported"),
                 ("below", 8, 0, 1.999, True, None, "supported"),
                 ("same", 4, 0, 4, True, None, "unsupported"),
                 ("absent", 4, 2, 4, True, None, "no_geometry"),
                 ("missing", 4, 0, 4, False, "missing_registered_prusa_coverage", None)]
        with TemporaryDirectory() as directory:
            corpus = Path(directory)
            step = corpus / "part.step"
            step.write_text("source")
            sha = panel_coverage.audit.digest(step)
            for case, limit, offset, length, reference, refusal, match in cases:
                with self.subTest(case=case):
                    work = corpus / case / "work/part.step"
                    export = work / "return/out"
                    export.mkdir(parents=True)
                    (work / "prusa.gcode").write_text(PRUSA)
                    (work / "create.stdout.json").write_text(json.dumps({"operations": [
                        {"operation": operation, "project": {}} for operation in
                        ("create", "import", "center_all_on_bed", "compile", "save")]}))
                    (export / "manifest.json").write_text(json.dumps({
                        "scene": {"bounds_mm": {"min": [0, 0, 0], "max": [10, 10, 1]}},
                        "definitions": [{"record": {"provenance": {"source_hash": sha}}}]}))
                    plan = {"layers": [{
                        "index": 1, "lower_z_mm": 0.2, "z_mm": 0.4,
                        "region_coverage": {"unsupported_overhang_mm": length}, "paths": [{
                            "kind": "bridge" if case.startswith("native_") else "bottom_skin",
                            "owner": {"kind": "wall"} if case == "reported_wall" else None,
                            "physical_z_mm": [0.2, 0.4],
                            "points_mm": [[2, offset, 0.4], [2 + length, offset, 0.4]],
                            "width": {"kind": "constant", "width_mm": 0.4}}]}]}
                    if case.startswith("native_"):
                        anchors = [(1.5, 2.2, 0)] if case == "native_one_ended" else [(1.5, 6.5, 0.3)]
                        plan["layers"].insert(0, {"index": 0, "lower_z_mm": 0.0, "z_mm": 0.2,
                            "region_coverage": {"unsupported_overhang_mm": 0}, "paths": [{
                                "kind": "perimeter", "physical_z_mm": [0.0, 0.2],
                                "points_mm": [[left, yy, 0.2], [right, yy, 0.2]],
                                "width": {"kind": "constant", "width_mm": 0.4}}
                                for left, right, yy in anchors]})
                    (export / "plan.json").write_text(json.dumps(plan))
                    row = {"part": "part.step", "sha256": sha, "source_absent": True,
                           "spiling_compile": {"ok": True}, "spiling_return": {"ok": True},
                           "export_checks": {"fresh_verified": True}, "prusa": {"ok": reference,
                               "import": {"ok": True, "command": ["--info", str(step)], "size_x": 10, "size_y": 10},
                               "slice": {"command": ["--center", "5,5", str(step)]}}}
                    receipt = {"settings": {"reference": "prusa", "supports": "breakaway"}, "effective_intent": {
                        "printer": {"build_envelope": {"min": [0, 0, 0], "max": [10, 10, 10]}},
                        "recipe": {"supports": {"kind": "breakaway", "z_gap_mm": 0.2},
                                   "process": {"max_bridge_span_mm": limit}}}}
                    summary = panel_coverage.summary([{"file": "part.step"}], [row], receipt, corpus / case)
                    self.assertEqual(summary["coverage_control"], "registered_prusa")
                    self.assertEqual(summary["ok"], refusal is None)
                    result, = summary["cases"]
                    self.assertEqual(result["failures"], [refusal] if refusal else [])
                    if case == "native_bounded":
                        self.assertEqual(result["prusa_located"]["status"], "no_native_spans")
                    if match:
                        comparison = result["prusa_located"]
                        self.assertEqual(comparison["span_counts"], {match: 1})
                        self.assertEqual(comparison["failing_spans"], int(bool(refusal)))
                        self.assertEqual(comparison["reported_wall_excess_spans"], int(case == "reported_wall"))
                        self.assertEqual(comparison["positive_excess_at_or_below_tolerance_spans"],
                                         int(case in {"equal", "below"}))
                        if refusal:
                            evidence = Path(comparison["failure_evidence"]["path"])
                            recorded, = map(json.loads, evidence.read_text().splitlines())
                            self.assertEqual(comparison["worst_longer_spans"], [recorded])
                            self.assertAlmostEqual(recorded["excess_mm"],
                                                   length - 0.4 if case == "native_one_ended" else length)
                            self.assertEqual(recorded["prusa_span_mm"], 0)
                            self.assertFalse(recorded["bridge_within_limit"])
                    if match == "no_geometry":
                        self.assertEqual(result["prusa_coverage_warning"], "no_full_span_comparisons")

    def test_prusa_frame_uses_recorded_bed_center_instead_of_scene_extents(self):
        with TemporaryDirectory() as directory:
            work = Path(directory)
            export = work / "return/out"
            export.mkdir(parents=True)
            step = work / "part.step"
            step.write_text("fixture")
            row = {"sha256": panel_coverage.audit.digest(step), "prusa": {
                "import": {"ok": True, "command": ["--info", str(step)], "size_x": 20, "size_y": 20},
                "slice": {"command": ["--center", "110,110", str(step)]}}}
            manifest = {"scene": {"bounds_mm": {"min": [100, 98, 0], "max": [120, 118, 5]}},
                        "definitions": [{"record": {"provenance": {"source_hash": row["sha256"]}}}]}
            (export / "manifest.json").write_text(json.dumps(manifest))
            receipt = {"effective_intent": {"printer": {"build_envelope": {
                "min": [0, 0, 0], "max": [220, 216, 220]}}}}
            creation = work / "create.stdout.json"
            creation.write_text(json.dumps({"operations": [{"operation": operation, "project": {}}
                                            for operation in ("create", "import", "center_all_on_bed", "compile", "save")]}))
            row["prusa"]["import"]["size_x"] = 17
            frame = panel_coverage.prusa_frame(row, work, receipt)
            self.assertEqual(frame["prusa_to_native_xy_mm"], (0, -2))
            self.assertEqual(frame["xy_extent_difference_mm"], (3, 0))
            manifest["definitions"][0]["record"]["provenance"]["source_hash"] = "different"
            (export / "manifest.json").write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "native placement does not bind"):
                panel_coverage.prusa_frame(row, work, receipt)
            step.write_text("different fixture")
            with self.assertRaisesRegex(ValueError, "does not bind the case source"):
                panel_coverage.prusa_frame(row, work, receipt)

    def test_panel_binds_complete_comparison_to_both_inputs(self):
        with TemporaryDirectory() as directory:
            corpus = Path(directory)
            work = corpus / "work/part.step"
            export = work / "return/out"
            export.mkdir(parents=True)
            (work / "prusa.gcode").write_text(PRUSA)
            (export / "plan.json").write_text(json.dumps({"layers": [{"index": 1, "paths": [{
                "kind": "bottom_skin", "physical_z_mm": [0.2, 0.4], "points_mm": [[0, 0, 0.4], [10, 0, 0.4]],
                "width": {"kind": "constant", "width_mm": 0.4},
            }]}]}))
            row = {"part": "part.step", "prusa": {"ok": True}, "source_absent": True,
                   "spiling_compile": {"ok": True}, "spiling_return": {"ok": True},
                   "export_checks": {"fresh_verified": True}}
            receipt = {"settings": {"reference": "prusa", "supports": "breakaway"}, "effective_intent": {"recipe": {
                "supports": {"z_gap_mm": 0.2}, "process": {"max_bridge_span_mm": 8},
            }}}
            result = panel_coverage.summary([{"file": "part.step"}], [row], receipt, corpus)
            comparison = result["cases"][0]["prusa_span_comparison"]
            self.assertEqual(comparison["status"], "observed_unregistered")
            self.assertEqual(comparison["part_max_span_delta_mm"], 10)
            evidence = Path(comparison["evidence"])
            rows = [json.loads(line) for line in evidence.read_text().splitlines()]
            self.assertEqual(rows[0]["inputs_sha256"].keys(), {"prusa", "spiling"})
            self.assertEqual(rows[-1]["event"], "complete")
            self.assertEqual(comparison["evidence_sha256"], panel_coverage.audit.digest(evidence))
            self.assertEqual(panel_coverage.prusa_span_comparison(corpus, "part.step", row, receipt)["status"],
                             "unresolved")
            plan_path = export / "plan.json"
            plan = json.loads(plan_path.read_text())
            native, measured = spans.native_observation(plan, plan_path, evidence, 0.2, 8)
            self.assertEqual(len(native), 1)
            self.assertEqual(measured["max_span_mm"], 10)
            prusa_path = work / "prusa.gcode"
            inputs = {"prusa": prusa_path, "spiling": plan_path}
            _, prusa_observed = spans.retained_observation(evidence, "prusa", inputs, 0.2, 8)
            self.assertEqual(prusa_observed["max_span_mm"], 0)
            prusa_path.write_text(PRUSA + "\n; different export\n")
            with self.assertRaisesRegex(ValueError, "bind"):
                spans.retained_observation(evidence, "prusa", inputs, 0.2, 8)
            with self.assertRaisesRegex(ValueError, "bind"):
                spans.native_observation(plan, plan_path, evidence, 0.2, 9)
            evidence.write_text("\n".join(evidence.read_text().splitlines()[:-1]) + "\n")
            with self.assertRaisesRegex(ValueError, "incomplete"):
                spans.native_observation(plan, plan_path, evidence, 0.2, 8)
