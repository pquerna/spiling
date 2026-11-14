#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Fresh-user journey per shipped setup, with no hand-authored JSON.

For every shipped single-tool setup: list printers and materials, then
`project create --import PART --printer SETUP --material FAMILY --quality Q
--supports on|off --center-all-on-bed --compile --verify --manufacturing-out`,
and a source-absent read-only reopen + verify. Bambu setups must create and
plan, then refuse emission with unsupported_capability. Prints one line per
journey and exits nonzero on any unexpected outcome.

Usage: smoke-defaults.py BIN_DIR WORK_DIR [PART.step] [--material pla] [--quality standard]
"""
import json, os, shutil, subprocess, sys

args = sys.argv[1:]
def option(name, default):
    if name in args:
        i = args.index(name)
        value = args[i + 1]
        del args[i:i + 2]
        return value
    return default
MATERIAL = option("--material", "pla")
QUALITY = option("--quality", "standard")
BIN, WORK = (os.path.abspath(a) for a in args[:2])
PART = os.path.abspath(args[2]) if len(args) > 2 else os.path.join(
    os.path.dirname(__file__), "..", "fixtures", "geometry", "box-mm.step")
CLI = os.path.join(BIN, "spiling-cli")
ENGINE = os.path.join(BIN, "spiling-engine")


def cli(cwd, *argv):
    p = subprocess.run([CLI, "--engine", ENGINE, "project", *argv], cwd=cwd,
                       capture_output=True, text=True)
    out = json.loads(p.stdout) if p.returncode == 0 and p.stdout.strip() else None
    err = None
    if p.returncode != 0:
        for line in reversed(p.stderr.splitlines()):
            if '"cli_failure"' in line:
                err = json.loads(line)
                break
    return p.returncode, out, err


def main():
    os.makedirs(WORK, exist_ok=False)
    rc, listing, err = cli(WORK, "--list-printers", "--list-materials")
    if rc != 0:
        sys.exit(f"listing failed: {err}")
    setups = [r for r in listing["listings"][0]["shipped"] if r["one_step_defaults"]]
    failures = 0
    for setup in setups:
        bambu = setup["dialect"].startswith("bambu")
        for supports in ("off", "on"):
            name = f"{setup['id']}-{supports}"
            cwd = os.path.join(WORK, name)
            os.makedirs(cwd)
            shutil.copy(PART, os.path.join(cwd, "part.step"))
            rc, out, err = cli(cwd, "create", "p.spil", "--import", "part.step",
                               "--printer", setup["id"], "--material", MATERIAL,
                               "--quality", QUALITY, "--supports", supports,
                               "--center-all-on-bed", "--compile", "--verify",
                               "--manufacturing-out", "out")
            if bambu:
                text = json.dumps(err or {})
                ok = rc != 0 and "unsupported_capability" in text
                print(name, "refuses emission as designed" if ok else f"UNEXPECTED rc={rc} {text[:300]}")
                failures += 0 if ok else 1
                continue
            if rc != 0:
                print(name, "FAILED", json.dumps(err)[:400])
                failures += 1
                continue
            os.remove(os.path.join(cwd, "part.step"))
            rc2, _, err2 = cli(cwd, "open", "p.spil", "--read-only", "--verify",
                               "--manufacturing-out", "reopen")
            created = next(o for o in out["operations"] if o.get("step") == "defaults")["created"]
            report = json.load(open(os.path.join(cwd, "out", "verification.json")))
            ok = rc2 == 0 and report.get("verified")
            failures += 0 if ok else 1
            print(name, "ok" if ok else f"REOPEN FAILED {json.dumps(err2)[:300]}",
                  "| recipe", created["recipe"], "layer", created["layer_height_mm"],
                  "| supports", created["supports"]["kind"],
                  "| estimate %.0f s" % (report.get("estimated_seconds") or 0),
                  "| filament %.1f mm" % report["filament_length_mm"],
                  "| flow-limited", report.get("flow_limited_segments"),
                  "| cooling-slowed layers", report.get("cooling_slowed_layers"))
    sys.exit(1 if failures else 0)


main()
