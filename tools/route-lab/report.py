# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Standalone capability and quality plots from one frozen routing cohort."""
import argparse
import base64
import html
import json
import statistics
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from loop import rows, grade, sha


def render(root, output, control_name=None, selected_name=None):
    cohort=json.loads((root/'cohort.json').read_text());reports=[]
    for folder in sorted((root/'trials').iterdir()):
        receipt=json.loads((folder/'receipt.json').read_text())
        if receipt['cohort_sha256']!=sha(root/'cohort.json'):raise ValueError('mixed cohort')
        for path,expected in receipt['source'].items():
            if sha(folder/'source'/path)!=expected:raise ValueError('changed source snapshot')
        if receipt['status'] in ('valid','invalid'):
            parsed=rows((folder/'matrix-0.tsv').read_text(),require_valid=False,expected=cohort.get('case_inventory'))
            if parsed!=receipt['rows']:raise ValueError('changed retained matrix')
            totals=[]
            for repetition in range(3):
                repeat=rows((folder/f'matrix-{repetition}.tsv').read_text(),require_valid=False,expected=cohort.get('case_inventory'))
                stable=lambda panel:[{k:v for k,v in row.items() if k!='planning_us'} for row in panel]
                if stable(repeat)!=stable(parsed):raise ValueError('changed retained repetitions')
                totals.append(sum(row['planning_us'] for row in repeat if row['algorithm']=='candidate'))
            receipt['median_matrix_planning_us']=statistics.median(totals)
            if receipt['status']=='valid':receipt['grades']=grade(parsed)
        reports.append(receipt)
    valid=[r for r in reports if r['status']=='valid']
    if not valid:raise ValueError('no complete valid trial to plot')
    control=next(r for r in valid if r['name']==control_name) if control_name else valid[0]
    selected=next(r for r in valid if r['name']==selected_name) if selected_name else valid[-1]
    output.mkdir(parents=True,exist_ok=False)
    plt.rcParams.update({'font.size':10,'axes.spines.top':False,'axes.spines.right':False})
    fig=plt.figure(figsize=(10,8),layout='constrained');grid=fig.add_gridspec(2,2)
    coverage=fig.add_subplot(grid[0,0]);quality=fig.add_subplot(grid[0,1]);cost=fig.add_subplot(grid[1,:])
    counts=[sum(r['algorithm']=='candidate' and r['status']=='ok' for r in trial.get('rows',[])) for trial in reports]
    total=sum(split!='negative' for _,split in cohort['case_inventory'])
    coverage.barh([r['name'] for r in reports],counts,color=['#367c65' if r['status']=='valid' else '#b67c38' for r in reports])
    for i,(r,n) in enumerate(zip(reports,counts)):coverage.text(n+.15,i,f"{n}/{total} · {r['status']}",va='center',fontsize=9)
    coverage.set(xlim=(0,total+6),xlabel='Positive cases completed',title='Capability · failures retained without fitness')
    for i,r in enumerate(valid):
        m=r['grades'][-1]['metrics'];x=m['travel_mm']['geometric_mean_ratio'];y=m['starts']['geometric_mean_ratio']
        if x is None or y is None:continue
        quality.scatter(x,y,s=80,zorder=3)
        quality.annotate(r['name'],(x,y),xytext=(6,8+13*i),textcoords='offset points',fontsize=9)
    quality.set(xlabel='Geometric-mean travel / frozen reference',ylabel='Geometric-mean starts / frozen reference',title='Valid routes · lower is better')
    quality.margins(x=.25,y=.4);quality.grid(alpha=.16)
    costs=[r['median_matrix_planning_us']/1000. for r in valid]
    cost.barh([r['name'] for r in valid],costs,color='#466e98')
    for i,value in enumerate(costs):cost.text(value,i,f' {value:.2f}',va='center',fontsize=9)
    cost.set(xlim=(0,max(costs)*1.3),xlabel='Median total planning time (ms, 3 runs)',title='Search cost · complete valid matrix')
    fig.savefig(output/'pareto.png',dpi=160);fig.savefig(output/'pareto.svg');plt.close(fig)
    comparisons=[]
    for r in selected['rows']:
        if r['algorithm']!='candidate' or r['status']!='ok':continue
        previous=next(p for p in control['rows'] if (p['case'],p['algorithm'])==(r['case'],r['algorithm']))
        comparisons.append(dict(case=r['case'],split=r['split'],travel_before_mm=previous['travel_mm'],travel_after_mm=r['travel_mm'],starts_before=previous['starts'],starts_after=r['starts']))
    (output/'summary.json').write_text(json.dumps(dict(scope='one_frozen_declared_routing_panel_not_production_improvement',cohort_sha256=sha(root/'cohort.json'),trials=[{k:r[k] for k in ('name','status','algorithm_sha256','grades','parent_comparison','median_matrix_planning_us') if k in r} for r in reports],control=control['name'],selected=selected['name'],comparisons=comparisons),indent=2)+'\n')
    image='data:image/png;base64,'+base64.b64encode((output/'pareto.png').read_bytes()).decode()
    table=''.join(f"<tr><td>{html.escape(r['case'])}</td><td>{html.escape(r['split'])}</td><td>{r['travel_before_mm']:.2f} → {r['travel_after_mm']:.2f}</td><td>{r['starts_before']:.0f} → {r['starts_after']:.0f}</td></tr>" for r in comparisons)
    caption=html.escape(f"{total} positive declared routing cases plus {len(cohort['case_inventory'])-total} negative case(s). Per-case comparison: {control['name']} → {selected['name']}. Invalid trials receive no quality score.")
    (output/'report.html').write_text('''<!doctype html><html><head><style>body{margin:0;color:var(--foreground);font-family:var(--font-sans)}img{width:100%;height:auto}p{font-size:13px;line-height:1.5}table{border-collapse:collapse;width:100%;font-size:12px}td,th{padding:7px;border-bottom:1px solid var(--border);text-align:left}summary{cursor:pointer;font-weight:600}</style></head><body><img src="'''+image+'''" alt="Route algorithm capability and valid quality comparison"><p>'''+caption+'''</p><details><summary>Per-case travel and uninterrupted starts</summary><table><tr><th>Case</th><th>Panel</th><th>Travel (mm)</th><th>Starts</th></tr>'''+table+'''</table></details><p>Required deposition and independent judge stay frozen. Declared scheduling, travel and tool service; not collision proof, native manufacturing qualification or measured printer time.</p></body></html>''')


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--root',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--control');p.add_argument('--selected');a=p.parse_args();render(a.root,a.output,a.control,a.selected)
