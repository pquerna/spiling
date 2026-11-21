import io
import json
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

    def test_located_gate_accepts_reversed_and_split_gaps_but_not_a_different_locus(self):
        def gap(start, end, layer=2, interval=(0.2, 0.4)):
            road = spans.Road(layer, 0, "top_skin", False, False, interval, [start, end], [0.4])
            return spans.describe_gap(road, road.points, abs(end[0] - start[0]), False, 10)

        before = [gap((0, 0), (2, 0)), gap((2, 0), (4, 0))]
        reversed_gap = gap((4, 0), (0, 0))
        self.assertEqual(panel_coverage.located_losses(before, [reversed_gap]), (0, []))
        shifted = gap((0, 1), (4, 1))
        same_xy_new_z = gap((0, 0), (4, 0), layer=3, interval=(0.4, 0.6))
        lost, changes = panel_coverage.located_losses(before, [shifted, same_xy_new_z])
        self.assertEqual(lost, 2)
        self.assertEqual([change["uncovered_span_mm"] for change in changes], [4, 4])

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
            with self.assertRaisesRegex(ValueError, "bind"):
                spans.native_observation(plan, plan_path, evidence, 0.2, 9)
            evidence.write_text("\n".join(evidence.read_text().splitlines()[:-1]) + "\n")
            with self.assertRaisesRegex(ValueError, "incomplete"):
                spans.native_observation(plan, plan_path, evidence, 0.2, 8)
