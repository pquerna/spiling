#!/usr/bin/env python3
"""check.py BASELINE.json RUN.json : semantic equivalence gate for speed experiments.
Fails when any part changes outcome class (ok->refusal, or a different refusal code) or when an ok part's
manufacturing summary drifts beyond tolerance. refusal->ok is reported as an improvement needing review."""
import json, sys
base = {r["part"]: r for r in json.load(open(sys.argv[1]))}
run = {r["part"]: r for r in json.load(open(sys.argv[2]))}
TOL = {"filament_length_mm": 0.01, "support_filament_length_mm": 0.03, "deposition_segments": 0.05,
       "support_deposition_segments": 0.08, "deposited_volume_mm3": 0.01}
bad, notes, same = [], [], 0
for p, b in base.items():
    r = run.get(p)
    if r is None: continue
    if b["code"] != r["code"]:
        (notes if r["ok"] else bad).append(f"{p}: outcome {b['code']} -> {r['code']} {r.get('error','')[:160]}")
        continue
    if not b["ok"]: continue
    if r.get("layers") != b.get("layers"): bad.append(f"{p}: layers {b.get('layers')} -> {r.get('layers')}")
    if r.get("program") == b.get("program"): same += 1; continue
    for k, tol in TOL.items():
        x, y = b.get(k) or 0, r.get(k) or 0
        rel = abs(y - x) / max(abs(x), 1e-9)
        if rel > tol: bad.append(f"{p}: {k} {x:.6g} -> {y:.6g} ({100*rel:.2f}% > {100*tol:.0f}%)")
        elif rel > 0: notes.append(f"{p}: {k} {100*rel:+.3f}%")
print(f"identical programs: {same}/{sum(1 for p in base if base[p]['ok'] and p in run)}")
for n in notes: print("note:", n)
for b in bad: print("FAIL:", b)
sys.exit(1 if bad else 0)
