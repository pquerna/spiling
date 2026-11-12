#!/usr/bin/env python3
"""log.py STATUS "description" [asi] : append the last run to log.jsonl (status keep|discard|crash|checks_failed|baseline).
Prints the confidence of the best improvement vs this segment's MAD (pi-autoresearch scoring)."""
import json, os, subprocess, sys, statistics, time
here = os.path.dirname(os.path.abspath(__file__))
run_path = os.path.join(here, "runs", "last.json")
recs = json.load(open(run_path))
metric = round(sum(r["s"] for r in recs), 1)
commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, cwd=here).stdout.strip()
entry = {"t": time.strftime("%H:%M"), "commit": commit, "status": sys.argv[1], "metric": metric,
         "parts": {r["part"]: r["s"] for r in recs}, "run": os.path.realpath(run_path).rsplit("/", 1)[1],
         "description": sys.argv[2], "asi": sys.argv[3] if len(sys.argv) > 3 else ""}
log = os.path.join(here, "log.jsonl")
prev = [json.loads(l) for l in open(log)] if os.path.exists(log) else []
open(log, "a").write(json.dumps(entry) + "\n")
seg = [e for e in prev + [entry] if e["run"].endswith(recs and entry["run"].rsplit("-", 1)[1])]
vals = [e["metric"] for e in seg]
base = next((e["metric"] for e in reversed(seg) if e["status"] == "baseline"), vals[0])
best = min((e["metric"] for e in seg if e["status"] in ("keep", "baseline")), default=metric)
mad = statistics.median([abs(v - statistics.median(vals)) for v in vals]) if len(vals) >= 3 else None
print(f"metric {metric}s  baseline {base}s  best kept {best}s  ({100*(base-best)/base:.1f}% faster)"
      + (f"  confidence {abs(base-best)/mad:.1f}x MAD" if mad else ""))
