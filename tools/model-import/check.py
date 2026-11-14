#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Real engine/CLI import, compile, source-absent saved return and export.

Run: python3 tools/model-import/check.py BIN_DIR WORK_DIR
WORK_DIR must not exist; contains exact inputs, logs, exports and results.json.
An optional third argument names a real STL (copied, never modified).
The final command observer needs Node. No compiler, source deletion outside
WORK_DIR, or external slicer is invoked.
"""
import json, os, shutil, subprocess, sys, time
from pathlib import Path
sys.dont_write_bytecode = True
from fixtures import generate, compatibility

BIN = Path(sys.argv[1]).resolve()
WORK = Path(sys.argv[2]).resolve()
WORK.mkdir(parents=True, exist_ok=False)
REPO = Path(__file__).resolve().parents[2]
generate(WORK / 'inputs')
compatibility(WORK / 'inputs')
base = json.loads((REPO/'fixtures/manufacturing/complete-sparse.intent.json').read_text())
base['recipe']['infill_fraction'] = .15
results = []

def call(args, name, cwd):
    started = time.monotonic()
    with (cwd/(name+'.json')).open('w') as out, (cwd/(name+'.log')).open('w') as err:
        result = subprocess.run([str(BIN/'spiling-cli'), 'project', *map(str,args), '--engine', str(BIN/'spiling-engine')], stdout=out, stderr=err,
                                env={**os.environ, 'RUST_MIN_STACK':'8388608'})
    if result.returncode:
        raise RuntimeError((cwd/(name+'.log')).read_text()[-6000:])
    return json.loads((cwd/(name+'.json')).read_text()), time.monotonic()-started

cases=[('box','box.stl',False), ('bore','bore.stl',False), ('cantilever','cantilever.stl',True), ('assembly','assembly.3mf',False)]
cases += [(name,name+suffix,False) for name,suffix in [('debris','.stl'),('crack','.stl'),('nested','.stl'),('negative-prusa','.3mf'),('negative-bambu','.3mf'),('negative-scoped','.3mf'),('modifier','.3mf'),('cura-namespace','.3mf'),('plates','.3mf')]]
if len(sys.argv)>3:
    real=Path(sys.argv[3]).resolve()
    shutil.copyfile(real,WORK/'inputs'/'real.stl')
    cases += [('real','real.stl',False),('real-supports','real.stl',True)]
for name, source, supports in cases:
    cwd=WORK/name; cwd.mkdir()
    local=cwd/source; shutil.copyfile(WORK/'inputs'/source,local)
    intent=json.loads(json.dumps(base))
    if not supports: intent['recipe']['supports']={'kind':'disabled'}
    (cwd/'intent.json').write_text(json.dumps(intent))
    record={'case':name}
    try:
        options=['--source-plate','2'] if name=='plates' else []
        created,record['create_seconds']=call(['create',cwd/'job.spil',*options,'--import',local,'--intent',cwd/'intent.json','--center-all-on-bed','--compile','--save','--section','0,0,3:0,0,1','--inspect-face','1','mesh:1'],'create',cwd)
        record['definitions']=len(created['definitions']);record['occurrences']=len(created['occurrences'])
        assert created['section'] is not None and len(created['face_inspections'])==1
        reports=[d['record']['provenance']['normalized'] for d in created['definitions']]
        findings=[f for r in reports for f in r['findings']]
        record['findings']=findings
        expected={'box':144.,'debris':144.,'crack':144.,'nested':128.,'negative-prusa':128.,'negative-bambu':128.,'negative-scoped':272.,'modifier':144.,'cura-namespace':144.,'plates':144.}.get(name)
        if expected is not None:
            measured=sum(row['area_mm2'] for row in created['section']['occurrences'])
            assert abs(measured-expected)<.001,(name,measured,expected)
            record['section_area_mm2']=measured
        codes={f['code'] for f in findings}
        required={'debris':'degenerate_facets','crack':'boundary_weld','modifier':'process_control','plates':'source_plate'}.get(name)
        if required:assert required in codes,(name,codes)
        if name.startswith('negative-'):assert 'negative_volumes' in codes
        if name=='modifier':assert any(f['code']=='process_control' and f['bounds_mm'] is not None for f in findings)
        if name=='plates':assert record['occurrences']==1 and any('2/2' in f['label'] for f in findings)
        local.unlink()
        reopened,record['return_seconds']=call(['open',cwd/'job.spil','--read-only','--verify','--manufacturing-out',cwd/'export'],'return',cwd)
        assert [d['record']['provenance']['normalized'] for d in reopened['definitions']]==reports
        assert reopened['project']['read_only'] and not reopened['project']['dirty']
        plan=json.loads((cwd/'export/plan.json').read_text())
        program=(cwd/'export/program.gcode').read_text()
        record['layers']=len(plan['layers'])
        record['arc_moves']=sum(s.startswith(('G2 ','G3 ')) for s in program.splitlines())
        kinds={path['kind'] for layer in plan['layers'] for path in layer['paths']}
        record['roles']=sorted(kinds)
        if name=='bore': assert record['arc_moves']>0
        if name=='cantilever': assert any('support' in role for role in kinds), kinds
        if name=='assembly': assert record['occurrences']==3 and record['definitions']==2
        record['ok']=True
    except Exception as exc:
        record.update(ok=False,error=str(exc))
    results.append(record)
    (WORK/'results.json').write_text(json.dumps(results,indent=2))
    print(json.dumps(record),flush=True)
if not all(row['ok'] for row in results):sys.exit(1)

subprocess.run(["node",str(REPO/"tools/model-import/void-witness.mjs"),str(WORK)],check=True)
