# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Standalone plots from one frozen routing cohort; no geometry generation."""
import argparse
import base64
import json
import html
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from loop import rows, grade, sha


def render(root, output):
    cohort=json.loads((root/'cohort.json').read_text());reports=[]
    for folder in sorted((root/'trials').iterdir()):
        receipt=json.loads((folder/'receipt.json').read_text())
        if receipt['status']!='valid':continue
        if receipt['cohort_sha256']!=sha(root/'cohort.json'):raise ValueError('mixed cohort')
        for path,expected in receipt['source'].items():
            if sha(folder/'source'/path)!=expected:raise ValueError('changed source snapshot')
        parsed=rows((folder/'matrix-0.tsv').read_text())
        if parsed!=receipt['rows']:raise ValueError('changed retained matrix')
        receipt['grades']=grade(parsed);reports.append(receipt)
    control=next(r for r in reports if r['name']=='nearest')
    output.mkdir(parents=True,exist_ok=False)
    names=[r['name'] for r in reports]
    vectors=lambda r:[r['grades'][-1]['metrics'][m]['geometric_mean_ratio'] for m in ('travel_mm','starts','motion_lower_bound_s')]
    base=vectors(control)
    plt.rcParams.update({'font.size':10,'axes.spines.top':False,'axes.spines.right':False})
    fig,ax=plt.subplots(figsize=(8,4.8),layout='constrained')
    for i,r in enumerate(reports):
        v=vectors(r);x,y=v[0]/base[0],v[1]/base[1]
        ax.scatter(x,y,s=90,facecolors='none' if r['name']=='lookahead-half' else None,label=r['name'],zorder=3)
        ax.annotate(r['name'],(x,y),xytext=(5,10 if r['name']=='lookahead-half' else -16),textcoords='offset points',fontsize=9)
    ax.set(xlabel='Geometric-mean travel / nearest-entry control',ylabel='Geometric-mean starts / nearest-entry control',title='Routing source-edit trials · lower is better on both axes')
    ax.margins(x=.2,y=.35);ax.grid(alpha=.16)
    fig.savefig(output/'pareto.png',dpi=160);fig.savefig(output/'pareto.svg');plt.close(fig)
    selected=next(r for r in reports if r['name']=='tie-exit')
    comparisons=[]
    for r in selected['rows']:
        if r['algorithm']!='candidate' or r['status']!='ok':continue
        previous=next(p for p in control['rows'] if (p['case'],p['algorithm'])==(r['case'],r['algorithm']))
        comparisons.append(dict(case=r['case'],split=r['split'],travel_before_mm=previous['travel_mm'],travel_after_mm=r['travel_mm'],starts_before=previous['starts'],starts_after=r['starts']))
    (output/'summary.json').write_text(json.dumps(dict(scope='one_frozen_synthetic_routing_panel_not_production_improvement',cohort_sha256=sha(root/'cohort.json'),source=cohort['source'],trials=[dict(name=r['name'],algorithm_sha256=r['algorithm_sha256'],grades=r['grades'],parent_comparison=r.get('parent_comparison')) for r in reports],selected=selected['name'],comparisons=comparisons),indent=2)+'\n')
    image='data:image/png;base64,'+base64.b64encode((output/'pareto.png').read_bytes()).decode()
    table=''.join(f"<tr><td>{html.escape(r['case'])}</td><td>{html.escape(r['split'])}</td><td>{r['travel_before_mm']:.2f} → {r['travel_after_mm']:.2f}</td><td>{r['starts_before']:.0f} → {r['starts_after']:.0f}</td></tr>" for r in comparisons)
    (output/'report.html').write_text('''<!doctype html><html><head><style>body{margin:0;color:var(--foreground);font-family:var(--font-sans)}img{width:100%;height:auto}p{font-size:13px;line-height:1.5;color:var(--muted-foreground)}table{border-collapse:collapse;width:100%;font-size:12px}td,th{padding:7px 4px;border-bottom:1px solid var(--border);text-align:left}summary{cursor:pointer;font-weight:600}</style></head><body><img src="'''+image+'''" alt="Pareto comparison of four routing source variants"><p>13 positive cases and one sealed-cavity refusal; frozen Rust judge. The tie-break improves one discovery case and preserves the other twelve. Held-out cases are unchanged. This is a harness result, not a production-routing or printer-time claim.</p><details><summary>Per-case travel and uninterrupted starts</summary><table><tr><th>Case</th><th>Panel</th><th>Travel (mm)</th><th>Starts</th></tr>'''+table+'''</table></details><p>Required deposition stays fixed. Clearance uses declared conservative envelopes with permitted boundary contact. Scarf/helical construction and native manufacturing integration remain outside this increment.</p></body></html>''')


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--root',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();render(a.root,a.output)
