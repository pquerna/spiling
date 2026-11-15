#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Sequential production-hatch measurements, checkpointed case receipts, no speed gate.

Supply the executable from Cargo's compiler-artifact JSON, not a stale guessed
binary. Use distinct OUTPUT directories per candidate. --reference compares
identical cases and output observations before computing a geometric speedup.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import statistics
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('binary', type=Path)
parser.add_argument('output', type=Path)
parser.add_argument('--backend', choices=['cpu', 'vulkan'], default='cpu')
parser.add_argument('--samples', type=int, default=3)
parser.add_argument('--layers', type=int, default=20)
parser.add_argument('--reference', type=Path)
parser.add_argument('--quick', action='store_true')
args = parser.parse_args()
if args.samples < 1 or args.layers < 1:
    parser.error('--samples and --layers must be positive')
binary = args.binary.resolve(strict=True)
with binary.open('rb') as f:
    binary_hash = hashlib.file_digest(f, 'sha256').hexdigest()
def source_receipt():
    untracked = subprocess.check_output(['git', 'ls-files', '--others', '--exclude-standard', '-z'])
    return {
        'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
        'tracked_diff_sha256': hashlib.sha256(subprocess.check_output(['git', 'diff', 'HEAD'])).hexdigest(),
        'untracked_sha256': {os.fsdecode(name): hashlib.sha256(Path(os.fsdecode(name)).read_bytes()).hexdigest()
                             for name in untracked.split(b'\0') if name},
    }

source = source_receipt()
args.output.mkdir(parents=True, exist_ok=True)
records_file = args.output / 'cases.jsonl'
if records_file.exists():
    parser.error('OUTPUT already contains cases; use a distinct directory')
reference = None
if args.reference:
    reference = {record['case']: record for record in json.loads(args.reference.read_text())['cases']}
patterns = ['rectilinear', 'grid', 'gyroid', 'cathedral']
cases = [(p, shape, size, density, 'core') for p in patterns
         for shape in ['rectangle', 'annulus']
         for size in ([48] if args.quick else [48, 192])
         for density in ([0.15] if args.quick else [0.05, 0.15, 0.30])]
if not args.quick:
    cases += [(p, shape, 48, 0.15, 'core') for p in patterns for shape in ['notch', 'islands']]
    cases += [('cathedral', shape, 48, density, 'roof') for shape in ['rectangle','annulus'] for density in [0.05,0.15,0.30]]
results = []
for pattern, shape, size, density, roof in cases:
    key = f'{pattern}/{shape}/{size}/{density}/{roof}'
    env = dict(os.environ, SPILING_INFILL_PATTERN=pattern,
               SPILING_INFILL_SHAPE=shape, SPILING_INFILL_SIZE=str(size),
               SPILING_INFILL_FRACTION=str(density), SPILING_INFILL_ROOF=roof,
               SPILING_INFILL_REPEATS=str(args.samples), SPILING_INFILL_LAYERS=str(args.layers),
               SPILING_INFILL_COMPUTE=args.backend, SPILING_INFILL_OBSERVE='1',
               RUST_MIN_STACK='8388608')
    run = subprocess.run([str(binary), 'measure_infill_generation', '--ignored', '--nocapture', '--test-threads=1'],
                         env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    rows = [json.loads(line.split('INFILL_PERF ',1)[1]) for line in run.stdout.splitlines() if 'INFILL_PERF ' in line]
    if run.returncode or len(rows) != args.samples:
        (args.output / 'failure.txt').write_text(f'{key}\n{run.stdout}')
        raise SystemExit(f'{key} failed; retained failure.txt and completed case receipts')
    semantic = ['paths', 'segments', 'length_mm', 'volume_mm3', 'signature']
    if any(any(row[k] != rows[0][k] for k in semantic) for row in rows):
        raise SystemExit(f'{key}: repeated output changed; no speed result admitted')
    timings = [r['generation_us']/1000 for r in rows]
    result = {'case': key, 'median_ms': statistics.median(timings),
              'min_ms': min(timings), 'max_ms': max(timings), 'samples': rows}
    if reference is not None:
        base = reference[key]
        if any(base['samples'][0][k] != rows[0][k] for k in semantic):
            raise SystemExit(f'{key}: candidate changed geometry/material observations')
        result['speedup'] = base['median_ms']/result['median_ms']
    with records_file.open('a') as f:
        f.write(json.dumps(result,sort_keys=True)+'\n')
    results.append(result)
    print(f'{key}: {result["median_ms"]:.3f} ms / {args.layers} layers; {rows[0]["segments"]} segments', flush=True)
summary = {'binary': str(binary), 'binary_sha256': binary_hash,
           'source': source, 'backend': args.backend, 'layers': args.layers, 'cases': results,
           'metric_scope': 'production hatch, not complete slicing latency; exact output observations frozen',
           'no_performance_admission_threshold': True}
if reference is not None:
    summary['geomean_speedup'] = math.exp(statistics.mean(math.log(r['speedup']) for r in results))
if source_receipt() != source:
    raise SystemExit('source changed during measurement; retain receipts but reject aggregate')
(args.output / 'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
