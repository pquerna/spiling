# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Freeze a Rust routing judge; snapshot, build and judge source-edit trials.

No source mutation, git reset, automatic promotion, Python geometry or external
agent. JSON is offline receipt metadata; route problems/results stay Rust/TSV.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import io
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import time
import tomllib

REPO = Path(__file__).resolve().parents[2]
CRATE = REPO / 'crates/deposition-routing'
EDITABLE = 'src/search.rs'
BUILD_ENV = ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_TARGET',
             'CARGO_PROFILE_RELEASE_OPT_LEVEL', 'CARGO_PROFILE_RELEASE_LTO',
             'CARGO_PROFILE_RELEASE_CODEGEN_UNITS', 'CARGO_INCREMENTAL',
             'RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER')



def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def write(path, data):
    with Path(path).open('x') as stream:
        json.dump(data, stream, indent=2, allow_nan=False)
        stream.write('\n')


def inventory(folder):
    paths = [p for p in folder.rglob('*') if p.is_file()]
    if any(p.is_symlink() for p in folder.rglob('*')):
        raise ValueError('symlinks are not admitted source')
    return {str(p.relative_to(folder)): sha(p) for p in sorted(paths)}


def verify_source(expected, actual):
    if expected.keys() != actual.keys() or any(
            actual[n] != h for n, h in expected.items() if n != EDITABLE):
        raise ValueError('frozen input/judge/library changed outside src/search.rs; start a new cohort')


def rows(text):
    parsed = list(csv.DictReader(io.StringIO(text), delimiter='\t'))
    keys = [(r['case'], r['algorithm']) for r in parsed]
    if not parsed or len(set(keys)) != len(keys):
        raise ValueError('empty or duplicated matrix inventory')
    for r in parsed:
        if r['algorithm'] not in ('baseline', 'candidate') or r['split'] not in ('discovery', 'holdout', 'negative'):
            raise ValueError('invalid matrix identity')
        if r['status'] != ('expected_refusal' if r['split'] == 'negative' else 'ok'):
            raise ValueError('required validity unknown/failed')
        for n in ('travel_mm', 'starts', 'deposition_mm', 'longest_chain_mm', 'waypoints', 'motion_lower_bound_s', 'work', 'planning_us'):
            r[n] = float(r[n])
            if not math.isfinite(r[n]) or r[n] < 0:
                raise ValueError('invalid finite metric')
        if r['status'] == 'ok' and (r['starts'] < 1 or r['deposition_mm'] <= 0):
            raise ValueError('invalid positive-case accounting')
    for r in parsed:
        pair = [p for p in parsed if p['case'] == r['case']]
        if {p['algorithm'] for p in pair} != {'baseline', 'candidate'} or len({p['split'] for p in pair}) != 1:
            raise ValueError('unpaired matrix inventory')
        if len({p['deposition_mm'] for p in pair}) != 1:
            raise ValueError('candidate changed deposition')
    return parsed


def grade(parsed):
    result = []
    for split in ('discovery', 'holdout', 'all'):
        panel = [r for r in parsed if r['algorithm'] == 'candidate' and r['status'] == 'ok'
                 and (split == 'all' or r['split'] == split)]
        if not panel:
            raise ValueError('missing discovery/holdout panel')
        scores = {}
        for metric in ('travel_mm', 'starts', 'motion_lower_bound_s'):
            comparisons={}
            for r in panel:
                base=next(p[metric] for p in parsed if p['case']==r['case'] and p['algorithm']=='baseline')
                comparisons[r['case']]=(r[metric]/base if base>0 else (1. if r[metric]==0 else None))
            values=list(comparisons.values())
            complete=all(v is not None for v in values)
            mean=(0. if 0. in values else math.exp(sum(math.log(v) for v in values)/len(values))) if complete else None
            scores[metric]=dict(geometric_mean_ratio=mean,worst_ratio=max(values) if complete else None,
                               relative_comparison_complete=complete,per_case=comparisons)
        result.append(dict(split=split, positive_cases=len(panel), metrics=scores))
    return result


def standalone_manifest(manifest, package):
    for name in ('version', 'edition', 'rust-version', 'license', 'repository'):
        manifest = re.sub(r'^'+re.escape(name)+r'\.workspace = true$',
                          name+' = '+json.dumps(package[name]), manifest, flags=re.MULTILINE)
    return manifest+'\n[workspace]\n'


def freeze(root):
    root.mkdir(parents=True, exist_ok=False)
    shutil.copytree(CRATE, root/'frozen')
    shutil.copy2(__file__, root/'runner.py')
    package = tomllib.loads((REPO/'Cargo.toml').read_text())['workspace']['package']
    manifest = (root/'frozen/Cargo.toml').read_text()
    manifest = standalone_manifest(manifest, package)
    (root/'standalone.toml').write_text(manifest)
    toolchain = tomllib.loads((REPO/'rust-toolchain.toml').read_text())['toolchain']['channel']
    rustc = subprocess.check_output(['rustup', 'run', toolchain, 'rustc', '-Vv'], text=True)
    write(root/'cohort.json', dict(editable=EDITABLE, source=inventory(CRATE), toolchain=toolchain,
                                 rustc=rustc, runner_sha256=sha(__file__), build_env={k:os.environ.get(k) for k in BUILD_ENV}, standalone_sha256=sha(root/'standalone.toml'),
                                 public_base=subprocess.check_output(['git','rev-parse','HEAD'],cwd=REPO,text=True).strip()))
    print(root/'cohort.json')


def trial(args):
    root=args.root.resolve();cohort=json.loads((root/'cohort.json').read_text())
    verify_source(cohort['source'], inventory(CRATE))
    if sha(__file__) != cohort['runner_sha256']:
        raise ValueError('frozen runner changed')
    if {k:os.environ.get(k) for k in BUILD_ENV} != cohort['build_env']:
        raise ValueError('frozen build environment changed')
    if sha(root/'standalone.toml') != cohort['standalone_sha256']:
        raise ValueError('standalone build context changed')
    if inventory(root/'frozen') != cohort['source']:
        raise ValueError('frozen source changed')
    if not re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}', args.name):
        raise ValueError('invalid trial name')
    out=root/'trials'/args.name;out.mkdir(parents=True,exist_ok=False)
    shutil.copytree(root/'frozen',out/'source')
    shutil.copy2(CRATE/EDITABLE,out/'source'/EDITABLE)
    (out/'source/Cargo.toml').write_bytes((root/'standalone.toml').read_bytes())
    rustc=subprocess.check_output(['rustup','run',cohort['toolchain'],'rustc','-Vv'],text=True)
    if rustc != cohort['rustc']:
        raise ValueError('compiler changed')
    env=os.environ.copy()
    target=Path(env.get('CARGO_TARGET_DIR',root/'target')).resolve()
    env.update(CARGO_TARGET_DIR=str(target),CARGO_BUILD_JOBS=env.get('CARGO_BUILD_JOBS','1'),RUST_TEST_THREADS='1')
    cargo=['rustup','run',cohort['toolchain'],'cargo']
    manifest=['--offline','--manifest-path',str(out/'source/Cargo.toml')]
    receipt=dict(name=args.name,parent=args.parent,hypothesis=args.hypothesis,source=inventory(out/'source'),
                 algorithm_sha256=sha(out/'source'/EDITABLE),cohort_sha256=sha(root/'cohort.json'),
                 status='unknown',commands=[],scope='synthetic_declared_envelope_routing_not_native_or_physical_qualification')
    start=time.monotonic()
    try:
        commands=[cargo+['fmt','--manifest-path',str(out/'source/Cargo.toml'),'--','--check'],
                  cargo+['test',*manifest,'--all-targets'],
                  cargo+['build',*manifest,'--release','--example','route_matrix','--message-format=json-render-diagnostics']]
        for i,command in enumerate(commands):
            receipt['commands'].append(command)
            run=subprocess.run(command,cwd=REPO,env=env,capture_output=True,text=True,timeout=args.seconds)
            (out/f'command-{i}.log').write_text(run.stdout+run.stderr)
            if run.returncode:
                receipt.update(failure='build_or_test',exit_code=run.returncode)
                raise RuntimeError('build/test command failed')
        artifacts=[json.loads(line) for line in run.stdout.splitlines() if line.startswith('{')]
        executable=next(r['executable'] for r in artifacts if r.get('reason')=='compiler-artifact' and r.get('executable') and r['target']['name']=='route_matrix')
        shutil.copy2(executable,out/'route_matrix')
        executable=str(out/'route_matrix')
        receipt['binary_sha256']=sha(executable)
        observed=[]
        for i in range(3):
            command=[executable];receipt['commands'].append(command)
            run=subprocess.run(command,cwd=REPO,capture_output=True,text=True,timeout=args.seconds)
            (out/f'matrix-{i}.tsv').write_text(run.stdout);(out/f'matrix-{i}.stderr').write_text(run.stderr)
            if run.returncode:
                receipt.update(failure='validity_or_resource',exit_code=run.returncode)
                raise RuntimeError('matrix did not complete')
            observed.append(rows(run.stdout))
        # Repeat deterministic geometry/quality; timing is intentionally noisy.
        stable=lambda panel:[{k:v for k,v in r.items() if k!='planning_us'} for r in panel]
        if any(stable(panel)!=stable(observed[0]) for panel in observed[1:]):
            raise ValueError('nondeterministic route or matrix')
        parent=None
        if args.parent:
            parent=json.loads((root/'trials'/args.parent/'receipt.json').read_text())
            if parent['status']!='valid' or parent['cohort_sha256']!=receipt['cohort_sha256']:
                raise ValueError('parent is not a valid matched trial')
            a={(r['case'],r['algorithm']):r for r in parent['rows']};b={(r['case'],r['algorithm']):r for r in observed[0]}
            if a.keys()!=b.keys():raise ValueError('candidate omitted a matrix case')
            if any({k:v for k,v in a[key].items() if k!='planning_us'} != {k:v for k,v in b[key].items() if k!='planning_us'} for key in a if key[1]=='baseline'):
                raise ValueError('frozen baseline changed')
        after=inventory(out/'source')
        after.pop('Cargo.lock',None)
        if after != receipt['source'] or inventory(root/'frozen') != cohort['source']:
            raise ValueError('trial modified frozen source during execution')
        receipt.update(status='valid',rows=observed[0],grades=grade(observed[0]),
                       standalone_lock_sha256=sha(out/'source/Cargo.lock'))
        if parent:
            def vector(r):return [r['grades'][-1]['metrics'][n]['geometric_mean_ratio'] for n in ('travel_mm','starts','motion_lower_bound_s')]
            a,b=vector(parent),vector(receipt)
            receipt['parent_comparison']=dict(parent=a,candidate=b,
                disposition='zero_baseline_regression' if any(v is None for v in [*a,*b]) else
                'dominates_parent' if all(y<=x for x,y in zip(a,b)) and any(y<x for x,y in zip(a,b)) else
                'tradeoff' if any(y<x for x,y in zip(a,b)) else 'equal_or_worse')
    except (subprocess.TimeoutExpired,ValueError,StopIteration,RuntimeError,OSError) as exc:
        receipt['status']='unknown';receipt.setdefault('failure',type(exc).__name__);receipt['detail']=str(exc)
    finally:
        receipt['wall_s']=time.monotonic()-start
        write(out/'receipt.json',receipt)
        with (root/'trials.jsonl').open('a') as stream:stream.write(json.dumps({k:v for k,v in receipt.items() if k not in ('rows','source','commands')},allow_nan=False)+'\n')
        print(json.dumps({k:v for k,v in receipt.items() if k in ('name','status','failure','parent_comparison','grades')},allow_nan=False))
    if receipt['status']!='valid':raise SystemExit(1)


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--root',type=Path,required=True)
    sub=p.add_subparsers(dest='action',required=True);sub.add_parser('freeze')
    q=sub.add_parser('trial');q.add_argument('name');q.add_argument('--parent');q.add_argument('--hypothesis',required=True);q.add_argument('--seconds',type=int,default=120)
    args=p.parse_args()
    if args.action=='freeze':freeze(args.root.resolve())
    else:
        # Exclusive session ownership; crashes leave an explicit lock to inspect.
        lock=args.root.resolve()/'trial.lock'
        descriptor=os.open(lock,os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600)
        try:
            os.write(descriptor,str(os.getpid()).encode())
            trial(args)
        finally:
            os.close(descriptor)
            lock.unlink()


if __name__=='__main__':main()
