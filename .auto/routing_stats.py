#!/usr/bin/env python3
"""routing_stats.py [RUN_DIR] : per-part routing seconds and modelled travel from SPILING_ROUTING_STATS lines."""
import json, os, sys
WT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
run = sys.argv[1] if len(sys.argv) > 1 else "/tmp/ar-run-" + os.path.basename(WT)
tot = {"route_s": 0., "travel_mm": 0., "travel_s": 0.}
for part in sorted(os.listdir(run)):
    f = os.path.join(run, part, "err.txt")
    if not os.path.exists(f): continue
    agg = {"route_s": 0., "travel_mm": 0., "travel_s": 0., "work": 0, "tasks": 0, "n": 0}
    for line in open(f):
        if '"routing_stats"' not in line: continue
        r = json.loads(line)
        for k in ["route_s", "travel_mm", "travel_s", "work", "tasks"]: agg[k] += r[k]
        agg["n"] += 1
    for k in tot: tot[k] += agg[k]
    print(f"{part} n={agg['n']} tasks={agg['tasks']} route_s={agg['route_s']:.2f} travel_s={agg['travel_s']:.3f} travel_mm={agg['travel_mm']:.1f} work={agg['work']}")
print(f"METRIC route_s={tot['route_s']:.2f}")
print(f"METRIC travel_s={tot['travel_s']:.3f}")
print(f"METRIC travel_mm={tot['travel_mm']:.1f}")
