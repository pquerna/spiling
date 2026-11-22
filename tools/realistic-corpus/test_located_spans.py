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


def road(layer, z, start, end, *, support=False, bridge=False):
    return spans.Road(layer, 0, "test", support, bridge, z, [start, end], [0.4])


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
        candidate = spans.Road(1, 0, "test", False, True, (0.2, 0.4),
                               [(0, 0), (10, 0), (10, 1), (0, 1)], [0.4] * 3)
        summary = spans.observe([candidate], 0.2, 20, list().append)
        self.assertEqual(summary["unsupported_spans"], 3)
        self.assertEqual(summary["tagged_bounded_bridges"], 0)
        self.assertEqual(summary["max_span_mm"], 10)

    @staticmethod
    def gap(start, end, interval=(1.0, 1.2), layer=8, road=0, role="top_skin"):
        path = spans.Road(layer, road, role, False, False, interval, [start, end], [0.45])
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

    def test_prusa_located_requires_referenced_placement_role_xy_and_both_z_bounds(self):
        candidate = self.gap((10, 1), (14, 1), interval=(1.0, 1.2), role="bottom_skin")
        frame = {"prusa_to_native_xy_mm": (10, 1)}
        cases = [
            ("Solid infill", (1.0005, 1.2), (4.5, 0), "acceptable"),
            ("Solid infill", (1.0, 1.2), (3.5, 0), "longer_than_prusa"),
            ("Solid infill", (1.0, 1.19), (4.5, 0), "no_common_observed_z"),
            ("Bridge infill", (1.0, 1.2), (4.5, 0), "no_registered_prusa_span"),
            ("Solid infill", (1.0, 1.2), (4.5, 1), "no_registered_prusa_span"),
        ]
        for role, interval, end, expected in cases:
            with self.subTest(expected=expected, end=end):
                control = self.gap((0, end[1]), end, interval=interval, role=role)
                losses = panel_coverage.located_losses([], [candidate], 0.275)
                result = panel_coverage.prusa_located_witnesses([control], [candidate], losses, frame, 0.275)
                self.assertEqual(result["screen_loss_counts"], {expected: 1})
                self.assertEqual(losses[0]["prusa_located"]["status"], expected)

    def test_prusa_located_does_not_choose_between_ambiguous_z_intervals(self):
        control = self.gap((0, 0), (4, 0), interval=(1.0, 1.2))
        native = [self.gap((0, 0), (4, 0), interval=(low, 1.2)) for low in (0.9995, 1.0005)]
        self.assertEqual(panel_coverage.common_prusa_intervals([control], native), {})

    def test_prusa_frame_requires_congruent_xy_source_bounds(self):
        with TemporaryDirectory() as directory:
            work = Path(directory)
            export = work / "return/out"
            export.mkdir(parents=True)
            step = work / "part.step"
            step.write_text("fixture")
            (export / "manifest.json").write_text(json.dumps({"scene": {"bounds_mm": {
                "min": [100, 98, 0], "max": [120, 118, 5]}}}))
            row = {"sha256": panel_coverage.audit.digest(step), "prusa": {
                "import": {"ok": True, "command": ["--info", str(step)], "size_x": 20, "size_y": 20},
                "slice": {"command": ["--center", "110,110", str(step)]}}}
            self.assertEqual(panel_coverage.prusa_frame(row, work, 0.275)["prusa_to_native_xy_mm"], (0, -2))
            row["prusa"]["import"]["size_x"] = 21
            with self.assertRaisesRegex(ValueError, "extents disagree"):
                panel_coverage.prusa_frame(row, work, 0.275)

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
