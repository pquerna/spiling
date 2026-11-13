#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Alternate frozen and candidate facade observers; compare complete retained output.

Build inspect_model_import with identical profiles before freezing each binary.
External resource controls belong to the caller. Native import-to-paint acceptance
uses tools/bench-open.mjs separately; these timings are not UI latency.
"""
import argparse
import hashlib
import json
import statistics
import subprocess
import time
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repeats', type=int, default=3)
    parser.add_argument('--display', action='store_true')
    parser.add_argument('sources', type=Path, nargs='+')
    args = parser.parse_args()
    if args.repeats < 1:
        parser.error('--repeats must be positive')
    args.output.mkdir(parents=True, exist_ok=False)
    identity = {'binaries': {}, 'sources': {}}
    for label, files in [('binaries', [args.baseline, args.candidate]),
                         ('sources', args.sources)]:
        for path in files:
            with path.open('rb') as data:
                identity[label][str(path)] = hashlib.file_digest(data, 'sha256').hexdigest()
    (args.output / 'identity.json').write_text(json.dumps(identity, indent=2) + '\n')
    rows = []
    # One unmeasured invocation per binary avoids charging first-use filesystem
    # effects to whichever side runs first. Receipts still capture all measured runs.
    for name, binary in [('baseline', args.baseline), ('candidate', args.candidate)]:
        result = subprocess.run([str(binary.resolve()), *(['--display'] if args.display else []),
                                 *map(str, args.sources)], text=True, capture_output=True)
        (args.output / f'warmup-{name}.stdout').write_text(result.stdout)
        (args.output / f'warmup-{name}.stderr').write_text(result.stderr)
        result.check_returncode()
    for trial in range(args.repeats):
        order = [('baseline', args.baseline), ('candidate', args.candidate)]
        if trial % 2:
            order.reverse()
        for name, binary in order:
            started = time.monotonic()
            result = subprocess.run([str(binary.resolve()), *(['--display'] if args.display else []), *map(str, args.sources)],
                                    text=True, capture_output=True, check=False)
            elapsed = time.monotonic() - started
            prefix = args.output / f'{trial}-{name}'
            prefix.with_suffix('.stdout').write_text(result.stdout)
            prefix.with_suffix('.stderr').write_text(result.stderr)
            result.check_returncode()
            values = [json.loads(line) for line in result.stdout.splitlines()]
            row = {'trial': trial, 'kind': name, 'process_seconds': elapsed, 'sources': values}
            rows.append(row)
            with (args.output / 'runs.jsonl').open('a') as out:
                out.write(json.dumps(row) + '\n')
            print(json.dumps({'trial': trial, 'kind': name, 'seconds': elapsed}), flush=True)
    expected = rows[0]['sources']
    semantic = ['path', 'facets', 'instances', 'packed_bytes', 'packed_sha256',
                'section_area_mm2', 'section_z_mm', 'findings']
    for row in rows:
        assert len(row['sources']) == len(expected), 'Changed source inventory'
        for before, after in zip(expected, row['sources']):
            assert {key: before[key] for key in semantic} == {key: after[key] for key in semantic}, \
                f"Changed geometry, section or findings: {after['path']}"
            if args.display:
                assert before['display']['chunks'] == after['display']['chunks'], 'Changed display chunks'
    timing = ['decode_seconds', 'pack_seconds', 'restore_seconds', 'section_seconds', 'total_seconds']
    summary = []
    for i, source in enumerate(expected):
        medians = {kind: {key: statistics.median(row['sources'][i][key] for row in rows
                                              if row['kind'] == kind) for key in timing}
                   for kind in ['baseline', 'candidate']}
        if args.display:
            for kind in medians:
                medians[kind]['display_seconds'] = statistics.median(
                    row['sources'][i]['display']['seconds'] for row in rows if row['kind'] == kind)
        summary.append({'source': source['path'], 'medians': medians})
    (args.output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')


if __name__ == '__main__':
    main()
