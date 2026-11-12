#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Real engine/CLI import, compile, source-absent saved return and export.

Run: python3 tools/model-import/check.py BIN_DIR WORK_DIR
WORK_DIR must not exist; contains exact inputs, logs, exports and results.json.
No compiler, source deletion outside WORK_DIR, or external slicer is invoked.
"""
import json, os, shutil, subprocess, sys, time
from pathlib import Path
sys.dont_write_bytecode = True
from fixtures import generate

BIN = Path(sys.argv[1]).resolve()
WORK = Path(sys.argv[2]).resolve()
WORK.mkdir(parents=True, exist_ok=False)
REPO = Path(__file__).resolve().parents[2]
generate(WORK / 'inputs')
base = json.loads((REPO/'fixtures/manufacturing/complete-sparse.intent.json').read_text())
base['recipe']['infill_fraction'] = .15
base['printer']['workflow']['job']['park_position_mm'][2] = 210
results = []

def call(args, name, cwd):
    started = time.monotonic()
    with (cwd/(name+'.json')).open('w') as out, (cwd/(name+'.log')).open('w') as err:
        result = subprocess.run([str(BIN/'spiling-cli'), 'project', *map(str,args), '--engine', str(BIN/'spiling-engine')], stdout=out, stderr=err,
                                env={**os.environ, 'RUST_MIN_STACK':'8388608'})
    if result.returncode:
        raise RuntimeError((cwd/(name+'.log')).read_text()[-6000:])
    return json.loads((cwd/(name+'.json')).read_text()), time.monotonic()-started

for name, source, supports in [('box','box.stl',False), ('bore','bore.stl',False), ('cantilever','cantilever.stl',True), ('assembly','assembly.3mf',False)]:
    cwd=WORK/name; cwd.mkdir()
    local=cwd/source; shutil.copyfile(WORK/'inputs'/source,local)
    intent=json.loads(json.dumps(base))
    if not supports: intent['recipe']['supports']={'kind':'disabled'}
    (cwd/'intent.json').write_text(json.dumps(intent))
    record={'case':name}
    try:
        created,record['create_seconds']=call(['create',cwd/'job.spil','--import',local,'--intent',cwd/'intent.json','--center-all-on-bed','--compile','--save','--section','0,0,3:0,0,1','--inspect-face','1','mesh:1'],'create',cwd)
        record['definitions']=len(created['definitions']);record['occurrences']=len(created['occurrences'])
        assert created['section'] is not None and len(created['face_inspections'])==1
        local.unlink()
        reopened,record['return_seconds']=call(['open',cwd/'job.spil','--read-only','--verify','--manufacturing-out',cwd/'export'],'return',cwd)
        assert reopened['project']['read_only'] and not reopened['project']['dirty']
        plan=json.loads((cwd/'export/plan.json').read_text())
        program=(cwd/'export/program.gcode').read_text()
        record['layers']=len(plan['layers'])
        record['arc_moves']=sum(s.startswith(('G2 ','G3 ')) for s in program.splitlines())
        kinds={path['kind'] for layer in plan['layers'] for path in layer['paths']}
        record['roles']=sorted(kinds)
        if name=='bore': assert record['arc_moves']>0
        if supports: assert any('support' in role for role in kinds), kinds
        if name=='assembly': assert record['occurrences']==3 and record['definitions']==2
        record['ok']=True
    except Exception as exc:
        record.update(ok=False,error=str(exc))
    results.append(record)
    (WORK/'results.json').write_text(json.dumps(results,indent=2))
    print(json.dumps(record),flush=True)
if not all(row['ok'] for row in results):sys.exit(1)
