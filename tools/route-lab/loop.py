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
import secrets
import shutil
import subprocess
import time
import tomllib

REPO = Path(__file__).resolve().parents[2]
CRATE = REPO / 'crates/deposition-routing'
ROUTING = 'crates/deposition-routing/'
# Frozen local dependency closure of the routing crate. Its third-party
# packages come from the repository lockfile and the offline Cargo cache.
MEMBERS = ('crates/deposition-routing', 'crates/compute', 'crates/contracts')
SURFACE = 'workspace-route-rust-v5'
QUALITY = ('travel_mm', 'starts', 'machine_time_s')


def editable(name):
    """Workspace-relative algorithm files; everything else is frozen."""
    if not name.startswith(ROUTING):
        return False
    local=name[len(ROUTING):]
    return local == 'src/search.rs' or (local.startswith('src/search/') and Path(local).suffix == '.rs')


def overlay_algorithm(source, destination):
    # Replace the whole editable inventory, including helper-module deletions.
    for path in list(destination.rglob('*')):
        if path.is_file() and editable(str(path.relative_to(destination))):path.unlink()
    for name in workspace_inventory(source) if source==REPO else inventory(source):
        if editable(name):
            target=destination/name
            target.parent.mkdir(parents=True,exist_ok=True)
            shutil.copy2(source/name,target)

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


def sync_source(source, destination):
    """Update a fixed workspace with normal writes; let Cargo own build freshness.

    Snapshot copies may carry historical mtimes. Changed bytes must be written
    normally, not copied with those timestamps. Unchanged files retain their
    timestamps, so an identical trial can reuse every compiler artifact.
    """
    expected = inventory(source)
    destination.mkdir(parents=True, exist_ok=True)
    actual = inventory(destination)
    for name in actual.keys() - expected.keys():
        (destination/name).unlink()
    for name, digest in expected.items():
        if actual.get(name) != digest:
            target = destination/name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source/name, target)


def workspace_manifest(text):
    """The repository manifest, restricted to the frozen member closure."""
    text=re.sub(r'^members = \[.*\]$', 'members = '+json.dumps(list(MEMBERS)), text, count=1, flags=re.MULTILINE)
    text=re.sub(r'^(default-members|exclude) = \[.*\]\n', '', text, flags=re.MULTILINE)
    if 'members = '+json.dumps(list(MEMBERS)) not in text:raise ValueError('cannot restrict workspace members')
    return text


def workspace_inventory(repo):
    """Inventory of the frozen workspace as `assemble` would write it."""
    result={}
    for member in MEMBERS:
        for name,digest in inventory(repo/member).items():result[f'{member}/{name}']=digest
    result['Cargo.toml']=hashlib.sha256(workspace_manifest((repo/'Cargo.toml').read_text()).encode()).hexdigest()
    result['rust-toolchain.toml']=sha(repo/'rust-toolchain.toml')
    # The frozen lock is the repository lock pruned to this closure; bind both.
    result[REPOSITORY_LOCK]=sha(repo/'Cargo.lock')
    return dict(sorted(result.items()))


def assemble(repo, destination):
    destination.mkdir(parents=True)
    for member in MEMBERS:shutil.copytree(repo/member,destination/member)
    (destination/'Cargo.toml').write_text(workspace_manifest((repo/'Cargo.toml').read_text()))
    for name in ('Cargo.lock','rust-toolchain.toml'):shutil.copy2(repo/name,destination/name)


REPOSITORY_LOCK = 'repository:Cargo.lock'
def verify_source(expected, actual):
    protected=lambda values:{n:h for n,h in values.items() if not editable(n)}
    if protected(expected) != protected(actual) or ROUTING+'src/search.rs' not in actual:
        raise ValueError('frozen input/judge/library changed outside algorithm surface; start a new cohort')


def rows(text, require_valid=True, expected=None):
    parsed = list(csv.DictReader(io.StringIO(text), delimiter='\t'))
    keys = [(r['case'], r['algorithm']) for r in parsed]
    if not parsed or len(set(keys)) != len(keys):
        raise ValueError('empty or duplicated matrix inventory')
    for r in parsed:
        if r['algorithm'] not in ('baseline', 'candidate') or r['split'] not in ('discovery', 'holdout', 'negative'):
            raise ValueError('invalid matrix identity')
        if require_valid and r['status'] != ('expected_refusal' if r['split'] == 'negative' else 'ok'):
            raise ValueError('required validity unknown/failed')
        if r['status'] not in ('ok','expected_refusal','no_route','resource','cancelled','invalid_route','invalid_input','unexpected_success'):
            raise ValueError('unknown matrix status')
        for n in ('travel_mm', 'starts', 'deposition_mm', 'longest_chain_mm', 'waypoints', 'motion_lower_bound_s', 'machine_time_s', 'starts_bound', 'travel_bound_mm', 'work', 'planning_us'):
            r[n] = float(r[n])
            if not math.isfinite(r[n]) or r[n] < 0:
                raise ValueError('invalid finite metric')
        if r['status'] == 'ok' and (r['starts'] < 1 or r['deposition_mm'] <= 0):
            raise ValueError('invalid positive-case accounting')
        if r['status'] == 'ok' and (r['starts'] < r['starts_bound'] or r['travel_mm'] < r['travel_bound_mm']-1e-6
                                    or r['machine_time_s'] < r['motion_lower_bound_s']-1e-9):
            raise ValueError('route beats a lower bound; judge or bound is wrong')
    for r in parsed:
        pair = [p for p in parsed if p['case'] == r['case']]
        if {p['algorithm'] for p in pair} != {'baseline', 'candidate'} or len({p['split'] for p in pair}) != 1:
            raise ValueError('unpaired matrix inventory')
        if all(p['status']=='ok' for p in pair) and len({p['deposition_mm'] for p in pair}) != 1:
            raise ValueError('candidate changed deposition')
    if expected is not None and {(r['case'],r['split'],r['algorithm']) for r in parsed} != {
            (name,split,algorithm) for name,split in expected for algorithm in ('baseline','candidate')}:
        raise ValueError('candidate omitted or changed a frozen matrix case')
    return parsed


def grade(parsed):
    if any(r['status'] != ('expected_refusal' if r['split']=='negative' else 'ok') for r in parsed):
        raise ValueError('required validity unknown/failed; no subset fitness')
    result = []
    for split in ('discovery', 'holdout', 'all'):
        panel = [r for r in parsed if r['algorithm'] == 'candidate' and r['status'] == 'ok'
                 and (split == 'all' or r['split'] == split)]
        if not panel:
            raise ValueError('missing discovery/holdout panel')
        scores = {}
        for metric in (*QUALITY, 'motion_lower_bound_s'):
            comparisons={}
            for r in panel:
                base=next(p[metric] for p in parsed if p['case']==r['case'] and p['algorithm']=='baseline')
                comparisons[r['case']]=(r[metric]/base if base>0 else (1. if r[metric]==0 else None))
            values=list(comparisons.values())
            complete=all(v is not None for v in values)
            mean=(0. if 0. in values else math.exp(sum(math.log(v) for v in values)/len(values))) if complete else None
            scores[metric]=dict(geometric_mean_ratio=mean,worst_ratio=max(values) if complete else None,
                               relative_comparison_complete=complete,per_case=comparisons)
        # Absolute headroom: 1.0 is provably optimal; the bounds are relaxations,
        # so a larger value need not be reachable.
        ratios=lambda top,bottom:[r[top]/r[bottom] for r in panel if r[bottom]>0]
        mean=lambda values:math.exp(sum(math.log(v) for v in values)/len(values)) if values and min(values)>0 else None
        gap=dict(starts=mean(ratios('starts','starts_bound')),travel_mm=mean(ratios('travel_mm','travel_bound_mm')),
                 starts_at_bound=sum(r['starts']==r['starts_bound'] for r in panel))
        result.append(dict(split=split, positive_cases=len(panel), metrics=scores, gap_to_bound=gap))
    return result


def run_logged(command, log, env, seconds):
    run=subprocess.run(command,cwd=REPO,env=env,capture_output=True,text=True,timeout=seconds)
    Path(log).write_text(run.stdout+run.stderr)
    return run


def cargo_commands(cohort, workspace, json_tests=False):
    cargo=['rustup','run',cohort['toolchain'],'cargo']
    manifest=['--offline','--locked','--manifest-path',str(workspace/'Cargo.toml'),'-p','spiling-deposition-routing']
    return [cargo+['fmt','--manifest-path',str(workspace/ROUTING/'Cargo.toml'),'--','--check'],
            cargo+['test',*manifest,'--all-targets','--message-format=json-render-diagnostics'],
            cargo+['build',*manifest,'--release','--example','route_matrix','--message-format=json-render-diagnostics']]


def freeze(root, seconds=1800):
    root.mkdir(parents=True, exist_ok=False)
    assemble(REPO, root/'frozen')
    shutil.copy2(__file__, root/'runner.py')
    toolchain = tomllib.loads((REPO/'rust-toolchain.toml').read_text())['toolchain']['channel']
    rustc = subprocess.check_output(['rustup', 'run', toolchain, 'rustc', '-Vv'], text=True)
    case_inventory=re.findall(r'\bcase\(\s*"([^"]+)",\s*"([^"]+)"', (CRATE/'examples/matrix/cases.rs').read_text())
    if not case_inventory or len(set(n for n,_ in case_inventory))!=len(case_inventory):
        raise ValueError('cannot freeze case inventory')
    repository=workspace_inventory(REPO)
    cohort=dict(surface=SURFACE, case_inventory=case_inventory, repository=repository, toolchain=toolchain,
                rustc=rustc, runner_sha256=sha(__file__), build_env={k:os.environ.get(k) for k in BUILD_ENV},
                compute_backend=os.environ.get('SPILING_COMPUTE_BACKEND','cpu'),
                public_base=subprocess.check_output(['git','rev-parse','HEAD'],cwd=REPO,text=True).strip())
    # A fixed workspace and persistent target use Cargo's ordinary cache.
    sync_source(root/'frozen',root/'build')
    target=Path(os.environ.get('CARGO_TARGET_DIR',root/'target')).resolve()
    cohort['build_target']=str(target)
    env=os.environ.copy();env.update(CARGO_TARGET_DIR=str(target),CARGO_BUILD_JOBS=env.get('CARGO_BUILD_JOBS','1'))
    started=time.monotonic()
    for i,command in enumerate(cargo_commands(cohort, root/'build')[1:]):
        # Unlocked once: Cargo prunes the repository lock to this closure
        # without changing any version. Trials then build --locked.
        command=[c for c in command if not c.startswith('--message-format') and c!='--locked']+(['--no-run'] if 'test' in command else [])
        if run_logged(command, root/f'seed-{i}.log', env, seconds).returncode:
            raise RuntimeError(f'seed build failed; see seed-{i}.log')
    cohort['seed_build_s']=time.monotonic()-started
    shutil.copy2(root/'build/Cargo.lock',root/'frozen/Cargo.lock')
    source=inventory(root/'frozen')
    if inventory(root/'build')!=source or {k:v for k,v in source.items() if k!='Cargo.lock'}!={k:v for k,v in repository.items() if k!=REPOSITORY_LOCK}:
        raise ValueError('assembled workspace differs from its inventory')
    cohort['source']=source
    write(root/'cohort.json', cohort)
    print(root/'cohort.json')


def trial(args):
    root=args.root.resolve();cohort=json.loads((root/'cohort.json').read_text())
    if cohort.get('surface')!=SURFACE:raise ValueError('different algorithm surface; use retained runner or start a new cohort')
    verify_source(cohort['repository'], workspace_inventory(REPO))
    if sha(__file__) != cohort['runner_sha256']:
        raise ValueError('frozen runner changed')
    if {k:os.environ.get(k) for k in BUILD_ENV} != cohort['build_env']:
        raise ValueError('frozen build environment changed')
    if os.environ.get('SPILING_COMPUTE_BACKEND','cpu') != cohort['compute_backend']:
        raise ValueError('compute backend differs from the cohort')
    if inventory(root/'frozen') != cohort['source']:
        raise ValueError('frozen source changed')
    if not re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}', args.name):
        raise ValueError('invalid trial name')
    out=root/'trials'/args.name;out.mkdir(parents=True,exist_ok=False)
    shutil.copytree(root/'frozen',out/'source')
    overlay_algorithm(REPO,out/'source')
    rustc=subprocess.check_output(['rustup','run',cohort['toolchain'],'rustc','-Vv'],text=True)
    if rustc != cohort['rustc']:
        raise ValueError('compiler changed')
    env=os.environ.copy()
    target=Path(cohort['build_target'])
    if 'CARGO_TARGET_DIR' in env and Path(env['CARGO_TARGET_DIR']).resolve()!=target:
        raise ValueError('build target differs from the cohort')
    env.update(CARGO_TARGET_DIR=str(target),CARGO_BUILD_JOBS=env.get('CARGO_BUILD_JOBS','1'),RUST_TEST_THREADS='1')
    build=root/'build'
    sync_source(out/'source',build)
    receipt=dict(name=args.name,parent=args.parent,hypothesis=args.hypothesis,source=inventory(out/'source'),
                 algorithm_source={n:h for n,h in inventory(out/'source').items() if editable(n)},
                 algorithm_sha256=hashlib.sha256(json.dumps({n:h for n,h in inventory(out/'source').items() if editable(n)},sort_keys=True).encode()).hexdigest(),cohort_sha256=sha(root/'cohort.json'),
                 status='unknown',commands=[],build_cache=[],build_target=str(target),scope='declared_deposition_routing_not_production_or_physical_qualification')
    start=time.monotonic()
    try:
        commands=cargo_commands(cohort, build)
        for i,command in enumerate(commands):
            receipt['commands'].append(command)
            command_started=time.monotonic()
            run=subprocess.run(command,cwd=REPO,env=env,capture_output=True,text=True,timeout=args.seconds)
            command_elapsed=time.monotonic()-command_started
            (out/f'command-{i}.log').write_text(run.stdout+run.stderr)
            if run.returncode:
                receipt.update(failure='build_or_test',exit_code=run.returncode)
                raise RuntimeError('build/test command failed')
            if i:
                artifacts=[json.loads(line) for line in run.stdout.splitlines() if line.startswith('{')]
                units=[a for a in artifacts if a.get('reason')=='compiler-artifact']
                receipt['build_cache'].append(dict(command=i,reused=sum(a.get('fresh',False) for a in units),
                    compiled=sum(not a.get('fresh',False) for a in units),wall_s=command_elapsed))
        executable=next(r['executable'] for r in artifacts if r.get('reason')=='compiler-artifact' and r.get('executable') and r['target']['name']=='route_matrix')
        shutil.copy2(executable,out/'route_matrix')
        executable=str(out/'route_matrix')
        receipt['binary_sha256']=sha(executable)
        observed=[]
        for i in range(3):
            command=[executable];receipt['commands'].append(command)
            run=subprocess.run(command,cwd=REPO,capture_output=True,text=True,timeout=args.seconds,
                               env=os.environ|{'SPILING_COMPUTE_BACKEND':cohort['compute_backend']})
            (out/f'matrix-{i}.tsv').write_text(run.stdout);(out/f'matrix-{i}.stderr').write_text(run.stderr)
            panel=rows(run.stdout,require_valid=False,expected=cohort['case_inventory'])
            valid=all(r['status']==('expected_refusal' if r['split']=='negative' else 'ok') for r in panel)
            if (run.returncode==0)!=valid:raise ValueError('matrix status/exit disagreement')
            observed.append(panel)
            device=[line[len('compute\t'):] for line in run.stderr.splitlines() if line.startswith('compute\t')]
            if len(device)!=1 or receipt.setdefault('compute_device',device[0])!=device[0]:
                raise ValueError('matrix did not report one stable compute device')
        # Repeat deterministic geometry/quality; timing is intentionally noisy.
        stable=lambda panel:[{k:v for k,v in r.items() if k!='planning_us'} for r in panel]
        if any(stable(panel)!=stable(observed[0]) for panel in observed[1:]):
            raise ValueError('nondeterministic route or matrix')
        parent=None
        if args.parent:
            parent=json.loads((root/'trials'/args.parent/'receipt.json').read_text())
            if parent['status'] not in ('valid','invalid') or parent['cohort_sha256']!=receipt['cohort_sha256']:
                raise ValueError('parent is not a complete matched trial')
            a={(r['case'],r['algorithm']):r for r in parent['rows']};b={(r['case'],r['algorithm']):r for r in observed[0]}
            if a.keys()!=b.keys():raise ValueError('candidate omitted a matrix case')
            if any({k:v for k,v in a[key].items() if k!='planning_us'} != {k:v for k,v in b[key].items() if k!='planning_us'} for key in a if key[1]=='baseline'):
                raise ValueError('frozen baseline changed')
        after=inventory(build)
        if after != receipt['source'] or inventory(out/'source') != receipt['source'] or inventory(root/'frozen') != cohort['source']:
            raise ValueError('trial modified frozen source during execution')
        # Least of three repetitions per case: the steadiest available cost signal.
        receipt['planning_us_min3']=sum(min(panel[i]['planning_us'] for panel in observed)
                                        for i,r in enumerate(observed[0]) if r['algorithm']=='candidate')
        receipt.update(status='valid' if valid else 'invalid',rows=observed[0])
        if valid:receipt['grades']=grade(observed[0])
        if parent and (not valid or parent['status']!='valid'):
            a={r['case']:r['status'] for r in parent['rows'] if r['algorithm']=='candidate'}
            b={r['case']:r['status'] for r in receipt['rows'] if r['algorithm']=='candidate'}
            receipt['parent_comparison']=dict(disposition='capability_only_no_quality_score',
                recovered=[n for n in a if a[n]!='ok' and b[n]=='ok'],
                lost=[n for n in a if a[n]=='ok' and b[n]!='ok'])
        elif parent:
            def vector(r):return [r['grades'][-1]['metrics'][n]['geometric_mean_ratio'] for n in QUALITY]
            a,b=vector(parent),vector(receipt)
            receipt['parent_comparison']=dict(parent=a,candidate=b,metrics=QUALITY,planning_ratio=receipt['planning_us_min3']/parent['planning_us_min3'] if parent.get('planning_us_min3') else None,
                disposition='zero_baseline_regression' if any(v is None for v in [*a,*b]) else
                'dominates_parent' if all(y<=x for x,y in zip(a,b)) and any(y<x for x,y in zip(a,b)) else
                'tradeoff' if any(y<x for x,y in zip(a,b)) else 'equal_or_worse')
    except (subprocess.TimeoutExpired,ValueError,StopIteration,RuntimeError,OSError) as exc:
        receipt['status']='unknown';receipt.setdefault('failure',type(exc).__name__);receipt['detail']=str(exc)
    finally:
        receipt['wall_s']=time.monotonic()-start
        write(out/'receipt.json',receipt)
        with (root/'trials.jsonl').open('a') as stream:stream.write(json.dumps({k:v for k,v in receipt.items() if k not in ('rows','source','commands')},allow_nan=False)+'\n')
        print(json.dumps({k:v for k,v in receipt.items() if k in ('name','status','failure','detail','parent_comparison','planning_us_min3')}|({'all':{'ratio_to_control':{n:receipt['grades'][-1]['metrics'][n]['geometric_mean_ratio'] for n in QUALITY},'gap_to_bound':receipt['grades'][-1]['gap_to_bound']}} if 'grades' in receipt else {}),allow_nan=False))
    if receipt['status']!='valid':raise SystemExit(1)


def sealed(args):
    """Promotion check on a regenerated, transformed panel no editor has seen."""
    root=args.root.resolve();cohort=json.loads((root/'cohort.json').read_text())
    seed=args.seed if args.seed is not None else secrets.randbits(32)|1
    if seed<1:raise ValueError('seed 0 is the visible panel')
    out=root/'sealed'/f'{seed}.json'
    out.parent.mkdir(exist_ok=True)
    report=dict(seed=seed,cohort_sha256=sha(root/'cohort.json'),trials={},
                scope='same judge on seeded families and an isometry of every case; not native or physical qualification')
    for name in args.trials:
        receipt=json.loads((root/'trials'/name/'receipt.json').read_text())
        binary=root/'trials'/name/'route_matrix'
        if receipt['cohort_sha256']!=report['cohort_sha256'] or sha(binary)!=receipt.get('binary_sha256'):
            raise ValueError('trial is not a retained binary of this cohort')
        run=subprocess.run([str(binary),'--sealed',str(seed)],cwd=REPO,capture_output=True,text=True,timeout=args.seconds,
                           env=os.environ|{'SPILING_COMPUTE_BACKEND':cohort['compute_backend']})
        panel=rows(run.stdout,require_valid=False,expected=cohort['case_inventory'])
        valid=all(r['status']==('expected_refusal' if r['split']=='negative' else 'ok') for r in panel)
        entry=dict(status='valid' if valid else 'invalid',failed=[r['case'] for r in panel if r['algorithm']=='candidate' and r['status'] not in ('ok','expected_refusal')])
        if valid:
            entry['grades']=grade(panel)
            # An isometry must not change route quality; seeded families are excluded.
            visible={r['case']:r for r in receipt['rows'] if r['algorithm']=='candidate' and r['status']=='ok'}
            drift={r['case']:r['travel_mm']/visible[r['case']]['travel_mm'] for r in panel
                   if r['algorithm']=='candidate' and r['status']=='ok' and r['case'] in visible
                   and not r['case'].startswith('lattice') and visible[r['case']]['travel_mm']>0}
            entry['isometry_travel_drift']=dict(worst=max(drift.values()),best=min(drift.values()),
                changed=sorted(n for n,v in drift.items() if abs(v-1)>1e-6))
        report['trials'][name]=entry
    write(out,report)
    print(json.dumps({'seed':seed,'trials':{n:{'status':e['status'],'failed':e['failed'],**({'ratio_to_control':{m:e['grades'][-1]['metrics'][m]['geometric_mean_ratio'] for m in QUALITY},'isometry_travel_drift':e['isometry_travel_drift']} if 'grades' in e else {})} for n,e in report['trials'].items()}},allow_nan=False))
    if any(e['status']!='valid' for e in report['trials'].values()):raise SystemExit(1)


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--root',type=Path,required=True)
    sub=p.add_subparsers(dest='action',required=True);sub.add_parser('freeze').add_argument('--seconds',type=int,default=1800)
    q=sub.add_parser('trial');q.add_argument('name');q.add_argument('--parent');q.add_argument('--hypothesis',required=True);q.add_argument('--seconds',type=int,default=120)
    q=sub.add_parser('sealed');q.add_argument('trials',nargs='+');q.add_argument('--seed',type=int);q.add_argument('--seconds',type=int,default=120)
    args=p.parse_args()
    if args.action=='freeze':freeze(args.root.resolve(),args.seconds)
    else:
        # Exclusive session ownership; crashes leave an explicit lock to inspect.
        lock=args.root.resolve()/'trial.lock'
        descriptor=os.open(lock,os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600)
        try:
            os.write(descriptor,str(os.getpid()).encode())
            (sealed if args.action=='sealed' else trial)(args)
        finally:
            os.close(descriptor)
            lock.unlink()


if __name__=='__main__':main()
