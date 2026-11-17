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

RUNNER = Path(__file__).with_name("run.py")
CLI_DOUBLE = """#!/usr/bin/env python3
import json, os, pathlib, sys
args=sys.argv[1:]
intent={"printer":{"build_envelope":{"min":[0,0,0],"max":[20,20,20]}},"recipe":{}}
if args[0]=='diagnose':
    print(json.dumps({"command":"diagnose","hello":{"fixture":True}}))
elif args[1]=='create':
    pathlib.Path(args[2]).write_bytes(b'test container')
    print(json.dumps({"manufacturing":{"intent":intent}}))
elif args[1]=='open':
    if os.environ.get('CORPUS_DOUBLE_FAIL_RETURN'):
        print(json.dumps({"level":"error","domain":"manufacturing","error":{"code":"verification_failed"}}),file=sys.stderr)
        sys.exit(1)
    root=pathlib.Path('out');root.mkdir()
    (root/'manifest.json').write_text('{}')
    (root/'program.gcode').write_text('test program')
    (root/'plan.json').write_text(json.dumps({"layers":[{"region_coverage":{"unsupported_overhang_mm":0,"web_roof_domains":[]}}],"adhesion":{"brim_paths":0}}))
    report={"verified":True,"deposition_segments":1,"filament_length_mm":1}
    (root/'verification.json').write_text(json.dumps(report))
    print(json.dumps(report))
"""


@unittest.skipUnless(hasattr(os, "wait4") and Path("/proc").is_dir(), "Linux corpus process supervision")
class RunnerBoundaryTests(unittest.TestCase):
    def run_fixture(self, root, failure=False, invalid_expectation=False):
        binaries = root / "bin"
        binaries.mkdir()
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
        environment = dict(os.environ, SPILING_CORPUS_JOBS="1")
        if failure:
            environment["CORPUS_DOUBLE_FAIL_RETURN"] = "1"
        command = [sys.executable, str(RUNNER), str(corpus), str(binaries), "--manifest", str(manifest),
                   "--profile", "dev", "--reference", "none"]
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

    def test_invalid_expectation_refuses_before_creating_native_work(self):
        with tempfile.TemporaryDirectory() as directory:
            result, root, _, _ = self.run_fixture(Path(directory), invalid_expectation=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("invalid case expectation", result.stderr)
            self.assertFalse((root / "work").exists())
            self.assertFalse((root / "results.ndjson").exists())


if __name__ == "__main__":
    unittest.main()
