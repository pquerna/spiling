# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0

from pathlib import Path
import re
import tempfile
import unittest

import audit
import features
import generate


class FeatureFamilyTests(unittest.TestCase):
    def test_section_oracle_respects_overlap_and_material_floors(self):
        boxes = [{"min": [0, 0, 0], "max": [4, 3, 1]},
                 {"min": [1, 0, 0], "max": [5, 3, 1]},
                 {"min": [0, 0, 1], "max": [1, 1, 3]}]
        self.assertEqual(features.section_area(boxes, 0.5), 15)
        self.assertEqual(features.section_area(boxes, 2), 1)
        self.assertEqual(features.section_area(boxes, 4), 0)

    def test_nominal_area_or_skirt_support_cannot_substitute_for_model_deposition(self):
        path = {"kind": "perimeter", "physical_z_mm": [0, 0.2],
                "width": {"kind": "constant", "width_mm": 0.4},
                "points_mm": [[0, 0, 0.2], [10, 0, 0.2]]}
        layers = [{"paths": [path]}]
        self.assertTrue(features.probe_covered(layers, [5, 0], 0.1, 0))
        self.assertFalse(features.probe_covered(layers, [5, 1], 0.1, 0))
        self.assertFalse(features.probe_covered(layers, [5, 0], 0.4, 0))
        for kind in ("support", "support_interface", "skirt", "brim"):
            path["kind"] = kind
            self.assertFalse(features.probe_covered(layers, [5, 0], 0.1, 0))

    def test_feature_gate_requires_each_promised_check(self):
        case = {"file": "shape.step", "sha256": "identity", "oracle": {"boxes": []}}
        intent = {"recipe": {"supports": {"kind": "disabled"}}}
        record = {"part": case["file"], "sha256": case["sha256"],
                  "spiling_compile": {"ok": True}, "spiling_return": {"ok": True}, "source_absent": True,
                  "export_checks": {name: True for name in ("manifest.json", "plan.json", "program.gcode",
                                    "verification.json", "fresh_verified", "actual_deposition", "nonempty_layers")},
                  "oracle_checks": {name: True for name in features.required_checks(case, intent)}}
        self.assertTrue(audit.verdict([case], [record], intent)["ok"])
        for name in list(record["oracle_checks"]):
            record["oracle_checks"][name] = False
            self.assertFalse(audit.verdict([case], [record], intent)["ok"])
            record["oracle_checks"][name] = True
        record["oracle_checks"] = {"unrelated_check": True}
        self.assertFalse(audit.verdict([case], [record], intent)["ok"])

    def test_parameterized_step_has_separate_body_namespaces_and_frozen_template_identity(self):
        data, boxes = generate.step([generate.box((4, 2, 0.8)), generate.box((2, 3, 0.8), (8, 0, 0), True)])
        text = data.decode()
        definitions = re.findall(r"(?m)^#(\d+)\s*=", text)
        self.assertEqual(len(definitions), len(set(definitions)))
        self.assertEqual(text.count("MANIFOLD_SOLID_BREP("), len(boxes))
        self.assertIn("'Context #1'", text)
        self.assertEqual(boxes[1], {"min": [5, 0, 0], "max": [8, 2, 0.8]})
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory, "family")
            manifest = generate.generate(destination, ["thin"], [1])
            self.assertEqual(manifest["template"]["sha256"], audit.digest(generate.TEMPLATE))
            audit.check_inputs(manifest["parts"], destination / "step")
            with self.assertRaises(FileExistsError):
                generate.generate(destination, ["thin"], [1])

    def test_scale_changes_one_declared_axis_without_enlarging_beads(self):
        rows = list(generate.cases("bodies", [1, 4, 16]))
        self.assertEqual([len(bodies) for _, bodies, _ in rows], [1, 4, 16])
        for _, bodies, _ in rows:
            for _, bounds in bodies:
                self.assertEqual([bounds["max"][axis] - bounds["min"][axis] for axis in range(3)], [2, 2, 0.8])
        for size in ((0, 2, 1), (-1, 2, 1), (float("inf"), 2, 1)):
            with self.assertRaises(ValueError):
                generate.box(size)

    def test_scaling_observations_cannot_turn_a_missing_return_into_a_completed_sample(self):
        cases = [{"file": "one.step", "parameters": {"scale_axis": "bodies", "scale": 1}},
                 {"file": "four.step", "parameters": {"scale_axis": "bodies", "scale": 4}},
                 {"file": "ordinary.step"}]
        records = [{"part": "one.step", "spiling_compile": {"ok": True, "seconds": 2}, "spiling_return": {"ok": True}},
                   {"part": "four.step", "spiling_compile": {"ok": True, "seconds": 3}}]
        rows = audit.scaling_observations(cases, records, {"one.step": {"status": "complete"}})["bodies"]
        self.assertEqual([row["input_scale"] for row in rows], [1, 4])
        self.assertTrue(rows[0]["complete"])
        self.assertFalse(rows[1]["complete"])
        self.assertIsNone(rows[0]["compile_cpu_seconds"])
        rows = audit.scaling_observations(cases, records, {"one.step": {"status": "failed"}})["bodies"]
        self.assertFalse(rows[0]["complete"])


if __name__ == "__main__":
    unittest.main()
