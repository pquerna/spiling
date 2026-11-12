#!/usr/bin/env python3
"""bench.py OUT.json PART... : supports-on compile of each corpus part through the real CLI/engine, in parallel.
Records wall seconds, compile elapsed, outcome, program hash and manufacturing summary. Standard library only."""
import json, os, subprocess, sys, time, hashlib, shutil, threading
from concurrent.futures import ThreadPoolExecutor
WT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("AR_BIN", os.path.join(WT, "target", "profiling"))
CORPUS = os.path.join(WT, ".cache", "realistic-corpus", "step")
INTENT = os.path.join(WT, ".auto", "intent.json")
RUN = os.environ.get("AR_RUN", "/tmp/ar-run-" + os.path.basename(WT))
TIMEOUT = int(os.environ.get("AR_TIMEOUT", "1800"))

def one(prefix):
    name = next(n for n in sorted(os.listdir(CORPUS)) if n.startswith(prefix))
    d = os.path.join(RUN, prefix); shutil.rmtree(d, ignore_errors=True); os.makedirs(d)
    shutil.copy(os.path.join(CORPUS, name), os.path.join(d, "source.step"))
    t = time.time()
    with open(os.path.join(d, "out.json"), "w") as out, open(os.path.join(d, "err.txt"), "w") as err:
        proc = subprocess.Popen([os.path.join(BIN, "spiling-cli"), "project", "create", "p.spil", "--import", "source.step",
                                 "--intent", INTENT, "--center-all-on-bed", "--compile", "--engine", os.path.join(BIN, "spiling-engine")],
                                cwd=d, stdout=out, stderr=err)
        timer = threading.Timer(TIMEOUT, proc.kill); timer.start()
        # wait4 reports the CLI's rusage including the engine it reaps: CPU seconds
        # resist contention from concurrent benchmark sets far better than wall time.
        _, status, usage = os.wait4(proc.pid, 0)
        timer.cancel(); proc.returncode = os.waitstatus_to_exitcode(status)
        rc = proc.returncode if proc.returncode >= 0 else "timeout"
    rec = {"part": prefix, "wall_s": round(time.time() - t, 2), "cpu_s": round(usage.ru_utime + usage.ru_stime, 2),
           "rc": rc, "ok": rc == 0}
    err_code = None
    for line in open(os.path.join(d, "err.txt")):
        try: r = json.loads(line)
        except ValueError: continue
        if r.get("level") == "error":
            e = r.get("error") or {}
            err_code = e.get("code") or r.get("event"); rec["error"] = (e.get("message") or r.get("message") or "")[:200]
    rec["code"] = "ok" if rc == 0 else (err_code or str(rc))
    if rc == 0:
        o = json.load(open(os.path.join(d, "out.json")))
        for op in o["operations"]:
            if op.get("operation") == "compile": rec["compile_s"] = round(op["elapsed_us"] / 1e6, 2)
        a = o["manufacturing"]["artifact"]; s = a["summary"]
        rec["program"] = a["program"]["hash"][:16]
        for k in ["deposition_segments", "filament_length_mm", "support_filament_length_mm", "support_deposition_segments", "layers", "paths", "deposited_volume_mm3"]:
            rec[k] = s.get(k)
    rec["s"] = rec["cpu_s"]
    return rec

parts = sys.argv[2:]
with ThreadPoolExecutor(len(parts)) as ex:
    recs = list(ex.map(one, parts))
json.dump(recs, open(sys.argv[1], "w"), indent=1)
for r in recs:
    print(f"{r['part']} {r['code']:<32} cpu {r['cpu_s']:8.1f}s wall {r['wall_s']:8.1f}s {r.get('program','')}")
print(f"METRIC total_s={sum(r['s'] for r in recs):.1f}")
print(f"METRIC wall_total_s={sum(r['wall_s'] for r in recs):.1f}")
print(f"METRIC max_s={max(r['s'] for r in recs):.1f}")
print(f"METRIC ok={sum(r['ok'] for r in recs)}")
