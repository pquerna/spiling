# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import audit
import panel_coverage

RUNNER = Path(__file__).with_name("run.py")
CLI_DOUBLE = """#!/usr/bin/env python3
import json, os, pathlib, sys
args=sys.argv[1:]
intent={"printer":{"build_envelope":{"min":[0,0,0],"max":[30,30,30]}},
        "recipe":{"supports":{"kind":"disabled"},"process":{"max_bridge_span_mm":10},"bead_width_mm":0.45}}
print(json.dumps(["instrumented_phase", "trace", 1]),file=sys.stderr)
if args[0]=='diagnose':
    print(json.dumps({"command":"diagnose","hello":{"fixture":True}}))
elif args[1]=='create':
    pathlib.Path(args[2]).write_bytes(b'test container')
    print(json.dumps({"manufacturing":{"intent":intent}}))
elif args[1]=='open':
    if os.environ.get('CORPUS_DOUBLE_FAIL_RETURN'):
        print(json.dumps({"level":"error","domain":"manufacturing","error":{"code":"verification_failed"}}),file=sys.stderr)
        print(json.dumps(["instrumented_phase", "trace_end", 1]),file=sys.stderr)
        sys.exit(1)
    root=pathlib.Path('out');root.mkdir()
    (root/'manifest.json').write_text('{}')
    (root/'program.gcode').write_text('test program')
    reported=json.loads(os.environ['CORPUS_DOUBLE_SPANS'])
    lengths=(1, 20, float(os.environ['CORPUS_DOUBLE_LOCAL_LENGTH']))
    layers=[{"index":index,"lower_z_mm":index,"z_mm":index+1,
             "region_coverage":{"unsupported_overhang_mm":span,"web_roof_domains":[]},
             "paths":[{"kind":"top_skin","physical_z_mm":[index,index+1],
                       "points_mm":[[0,10*index,index+1],[length,10*index,index+1]],
                       "width":{"kind":"constant","width_mm":0.4}}]}
            for index,(length,span) in enumerate(zip(lengths,reported))]
    (root/'plan.json').write_text(json.dumps({"layers":layers,"adhesion":{"brim_paths":0}}))
    report={"verified":True,"deposition_segments":1,"filament_length_mm":1,"support_filament_length_mm":float(os.environ.get('CORPUS_DOUBLE_SUPPORT','100'))}
    (root/'verification.json').write_text(json.dumps(report))
    print(json.dumps(report))
"""


@unittest.skipUnless(hasattr(os, "wait4") and Path("/proc").is_dir(), "Linux corpus process supervision")
class RunnerBoundaryTests(unittest.TestCase):
    def run_fixture(self, root, failure=False, invalid_expectation=False, control=None,
                    spans=(0, 20, 2), local_length=2, support=100):
        binaries = root / "bin"
        binaries.mkdir(parents=True)
        cli = binaries / "spiling-cli"
        cli.write_text(CLI_DOUBLE)
        cli.chmod(0o755)
        (binaries / "spiling-engine").write_text("explicit test double, not a native engine")
        corpus = root / "corpus"
        (corpus / "step").mkdir(parents=True)
        source = corpus / "step/fixture.step"
        source.write_text("input fixture, not geometry acceptance")
        manifest = root / "manifest.json"
        case = {"file": source.name, "sha256": audit.digest(source)}
        if invalid_expectation:
            case["expectation"] = None
        manifest.write_text(json.dumps({"parts": [case]}))
        environment = dict(os.environ, SPILING_CORPUS_JOBS="1", SPILING_CORPUS_SUPPORTS="disabled",
                           CORPUS_DOUBLE_SPANS=json.dumps(spans), CORPUS_DOUBLE_SUPPORT=str(support),
                           CORPUS_DOUBLE_LOCAL_LENGTH=str(local_length))
        if failure:
            environment["CORPUS_DOUBLE_FAIL_RETURN"] = "1"
        command = [sys.executable, str(RUNNER), str(corpus), str(binaries), "--manifest", str(manifest),
                   "--profile", "dev", "--reference", "none"]
        if control is not None:
            command += ["--coverage-control", str(control / "results.ndjson"), str(control)]
        result = subprocess.run(command, capture_output=True, text=True, env=environment, timeout=30)
        return result, corpus, command, environment

    def test_real_runner_exit_status_and_portable_return_boundary(self):
        with tempfile.TemporaryDirectory() as directory:
            result, root, command, environment = self.run_fixture(Path(directory))
            self.assertEqual(result.returncode, 0, result.stderr)
            summary = json.loads((root / "results.ndjson.summary.json").read_text())
            self.assertTrue(summary["ok"])
            self.assertFalse((root / "work/fixture.step/source.step").exists())
            self.assertFalse((root / "work/fixture.step/p.spil").exists())
            self.assertTrue((root / "work/fixture.step/return/p.spil").is_file())
            record = json.loads((root / "results.ndjson").read_text())
            self.assertGreater(record["spiling_compile"]["cpu_seconds"], 0)
            previous = (root / "results.ndjson").read_bytes()
            retry = subprocess.run(command, capture_output=True, text=True, env=environment, timeout=30)
            self.assertNotEqual(retry.returncode, 0)
            self.assertEqual((root / "results.ndjson").read_bytes(), previous)

    def test_recorded_replay_failure_fails_the_actual_runner_process(self):
        with tempfile.TemporaryDirectory() as directory:
            result, root, _, _ = self.run_fixture(Path(directory), failure=True)
            self.assertNotEqual(result.returncode, 0)
            summary = json.loads((root / "results.ndjson.summary.json").read_text())
            self.assertFalse(summary["ok"])
            self.assertIn("source_absent_return", summary["outcomes"]["fixture.step"]["failures"])
            record = json.loads((root / "results.ndjson").read_text())
            self.assertEqual(record["spiling_return"]["error"]["code"], "verification_failed")

    def test_invalid_expectation_refuses_before_creating_native_work(self):
        with tempfile.TemporaryDirectory() as directory:
            result, root, _, _ = self.run_fixture(Path(directory), invalid_expectation=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("invalid case expectation", result.stderr)
            self.assertFalse((root / "work").exists())
            self.assertFalse((root / "results.ndjson").exists())

    def test_native_control_reports_local_regression_without_failing_the_prusa_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            result, control, _, _ = self.run_fixture(path / "control")
            self.assertEqual(result.returncode, 0, result.stderr)
            result, candidate, _, _ = self.run_fixture(path / "worse", control=control, local_length=2.0001)
            self.assertEqual(result.returncode, 0, result.stderr)
            receipt = json.loads((candidate / "results.ndjson.summary.json").read_text())
            self.assertTrue(receipt["ok"])
            self.assertEqual(receipt["coverage"]["coverage_control"], "not_requested")
            coverage = receipt["coverage"]["cases"][0]
            self.assertEqual(coverage["failures"], [])
            self.assertEqual(coverage["control_max_span_mm"], coverage["max_span_mm"])
            self.assertEqual(coverage["control_unsupported_layers"], coverage["unsupported_layers"])
            self.assertEqual(coverage["located_spans_with_losses"], 1)
            loss, = coverage["local_span_losses"]
            self.assertEqual(loss["physical_z_mm"], [2, 3])
            self.assertEqual(coverage["span_registration_tolerance_mm"], 0.275)
            self.assertEqual(loss["candidate_points_mm"], [[0, 20], [2.0001, 20]])
            self.assertEqual(loss["control"]["points_mm"], [[0, 20], [2, 20]])
            self.assertAlmostEqual(loss["increase_mm"], 0.0001)
            meta = json.loads((candidate / "results.ndjson.run.json").read_text())
            self.assertEqual(meta["comparisons"]["coverage_control"]["results_sha256"],
                             audit.digest(control / "results.ndjson"))

    def test_report_disagreement_and_support_drop_warn_without_refusing(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            result, control, _, _ = self.run_fixture(path / "control", spans=(0, 20, 0))
            self.assertEqual(result.returncode, 0, result.stderr)
            for support, review in ((95, False), (94, True)):
                result, candidate, _, _ = self.run_fixture(
                    path / str(support), control=control, support=support)
                self.assertEqual(result.returncode, 0, result.stderr)
                coverage = json.loads((candidate / "results.ndjson.summary.json").read_text())["coverage"]
                self.assertEqual(coverage["cases"][0]["control_support_filament_mm"], 100.0)
                self.assertIs(coverage["cases"][0]["support_drop_review"], review)
                self.assertIsNone(coverage["cases"][0]["support_vs_prusa"])
                self.assertEqual(coverage["cases"][0]["located_spans_with_losses"], 0)
                warnings = coverage["cases"][0]["span_report_warnings"]
                self.assertIsNone(warnings["candidate"])
                self.assertEqual(warnings["control"]["largest"]["located_span_mm"], 2)


class ReferenceCoverageTests(unittest.TestCase):
    def row(self, enabled, filament, brim="0"):
        args = ["prusa-slicer", "--export-gcode"]
        if enabled:
            args += ["--support-material"]
        return {"part": "part.step", "sha256": "source", "bytes": 12,
                "prusa": {"ok": True, "filament_mm": filament, "slice": {
                    "command": args + ["--brim-width", brim, "-o", "out.gcode", "part.step"]}}}

    def test_reference_delta_checks_effective_slice_arguments(self):
        candidate = self.row(True, 120)
        self.assertEqual(panel_coverage.prusa_support_estimate(candidate, self.row(False, 100)),
                         (20, "matched_on_minus_off"))
        self.assertEqual(panel_coverage.prusa_support_estimate(candidate, self.row(False, 100, "5")),
                         (None, "reference_commands_differ"))
        self.assertEqual(panel_coverage.prusa_support_estimate(candidate, self.row(False, 130)),
                         (None, "reference_delta_negative"))


if __name__ == "__main__":
    unittest.main()
