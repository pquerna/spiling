# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0

import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import audit


class CorpusAuditTests(unittest.TestCase):
    def setUp(self):
        self.case = {"file": "part.step", "sha256": hashlib.sha256(b"part").hexdigest()}
        self.intent = {"printer": {"build_envelope": {"min": [0, 0, 0], "max": [20, 20, 20]}}}
        self.record = {
            "part": self.case["file"], "sha256": self.case["sha256"],
            "spiling_compile": {"ok": True}, "spiling_return": {"ok": True},
            "source_absent": True,
            "export_checks": {name: True for name in ("manifest.json", "plan.json", "program.gcode",
                             "verification.json", "fresh_verified", "actual_deposition", "nonempty_layers")},
        }

    def test_complete_journey_requires_real_return_and_all_export_obligations(self):
        self.assertTrue(audit.verdict([self.case], [self.record], self.intent)["ok"])
        for key in ("source_absent", "spiling_return", "export_checks"):
            with self.subTest(key=key):
                edited = copy.deepcopy(self.record)
                del edited[key]
                self.assertFalse(audit.verdict([self.case], [edited], self.intent)["ok"])
        for key in self.record["export_checks"]:
            with self.subTest(key=key):
                edited = copy.deepcopy(self.record)
                edited["export_checks"][key] = False
                self.assertFalse(audit.verdict([self.case], [edited], self.intent)["ok"])

    def test_missing_duplicate_and_unexpected_cases_cannot_pass(self):
        self.assertFalse(audit.verdict([], [], self.intent)["ok"])
        self.assertFalse(audit.verdict([self.case, self.case], [self.record], self.intent)["ok"])
        unknown = dict(self.record, part="unrequested.step")
        for rows in ([], [self.record, self.record], [self.record, unknown]):
            with self.subTest(rows=len(rows)):
                self.assertFalse(audit.verdict([self.case], rows, self.intent)["ok"])

    def test_refusal_must_match_failure_class_not_just_a_failed_process(self):
        case = dict(self.case, expectation={"kind": "refusal", "code": "unsupported_geometry", "domain": "geometry"})
        record = dict(self.record, spiling_compile={"ok": False, "error": {"code": "unsupported_geometry", "domain": "geometry"}})
        self.assertEqual(audit.assess(case, record, self.intent)["status"], "expected_refusal")
        for error in ({"code": "timeout", "domain": "geometry"}, {"code": "unsupported_geometry", "domain": "transport"}):
            record["spiling_compile"]["error"] = error
            self.assertEqual(audit.assess(case, record, self.intent)["status"], "failed")
        self.assertEqual(audit.assess(case, self.record, self.intent)["status"], "failed")

    def test_input_identity_is_checked_before_native_work(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory, self.case["file"])
            with self.assertRaises(ValueError):
                audit.check_inputs([self.case], directory)
            path.write_bytes(b"part")
            audit.check_inputs([self.case], directory)
            path.write_bytes(b"changed")
            with self.assertRaises(ValueError):
                audit.check_inputs([self.case], directory)
        record = dict(self.record, sha256="different")
        self.assertFalse(audit.verdict([self.case], [record], self.intent)["ok"])

    def test_selection_is_nonempty_explicit_and_manifest_owned(self):
        manifest = {"parts": [self.case], "cohorts": {"canary": [self.case["file"]]}}
        self.assertEqual(audit.select_cases(manifest, ["part"], "canary"), [self.case])
        for prefixes, cohort in ((["missing"], "full"), ([], "missing")):
            with self.assertRaises(ValueError):
                audit.select_cases(manifest, prefixes, cohort)
        for parts in ([], [self.case, self.case], [dict(self.case, file="../outside.step")]):
            with self.assertRaises(ValueError):
                audit.select_cases({"parts": parts})

    def test_exports_require_fresh_verification_and_actual_material(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "manifest.json").write_text("{}")
            (root / "program.gcode").write_text("program")
            (root / "plan.json").write_text(json.dumps({"layers": [{}]}))
            report = {"verified": True, "deposition_segments": 3, "filament_length_mm": 1.0}
            (root / "verification.json").write_text(json.dumps(report))
            self.assertTrue(all(audit.export_checks(root).values()))
            for value in (0, float("nan"), None):
                report["filament_length_mm"] = value
                (root / "verification.json").write_text(json.dumps(report))
                self.assertFalse(audit.export_checks(root)["actual_deposition"])
            (root / "program.gcode").unlink()
            self.assertFalse(all(audit.export_checks(root).values()))

    def test_invalid_prusa_rotation_is_rejected_before_native_work(self):
        for rotations in (None, ["x", 90], {"w": 90}, {"x": True}, {"x": "90"}):
            with (self.subTest(rotations=rotations),
                  self.assertRaisesRegex(ValueError, "invalid Prusa rotations")):
                audit.select_cases({"parts": [dict(self.case, prusa_rotation_degrees=rotations)]})

    def test_invalid_expectations_are_rejected_before_native_work(self):
        for expectation in (None, False, "complete", [], {}, {"kind": "unknown"},
                            {"kind": "refusal"}, {"kind": "refusal", "code": "timeout", "domain": None}):
            with self.subTest(expectation=expectation), self.assertRaises(ValueError):
                audit.select_cases({"parts": [dict(self.case, expectation=expectation)]})

    def test_cohort2_requires_complete_source_absent_journeys(self):
        manifest = json.loads(Path(__file__).with_name("manifest-cohort2.json").read_text())
        cases = audit.select_cases(manifest)
        excluded = {
            "C2-07-SKR_E3_V3_Din_Mount.step": "#8735",
            "C2-08-Fly_Gemini_V2_Din_Mount_2pc.step": "#15946",
        }
        self.assertTrue(excluded.keys() <= {case["file"] for case in cases})
        for case in cases:
            with self.subTest(part=case["file"]):
                record = dict(self.record, part=case["file"], sha256=case["sha256"])
                if case["file"] in excluded:
                    expected_error = {"code": "unsupported_geometry", "domain": "geometry"}
                    self.assertEqual(case["expectation"], {
                        "kind": "refusal", **expected_error, "reason": case["expectation"]["reason"],
                    })
                    self.assertIn(excluded[case["file"]], case["expectation"]["reason"])
                    refused = dict(record, spiling_compile={"ok": False, "error": expected_error})
                    self.assertEqual(audit.assess(case, refused, self.intent)["status"], "expected_refusal")
                    self.assertEqual(audit.assess(case, record, self.intent)["status"], "failed")
                    continue
                self.assertEqual(case["expectation"], {"kind": "complete"})
                self.assertEqual(audit.assess(case, record, self.intent)["status"], "complete")
                for field in ("spiling_compile", "spiling_return"):
                    failed = dict(record, **{field: {"ok": False}})
                    self.assertEqual(audit.assess(case, failed, self.intent)["status"], "failed")


if __name__ == "__main__":
    unittest.main()
