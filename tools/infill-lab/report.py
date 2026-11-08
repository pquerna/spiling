# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0
"""Actual-road gallery and separate material/time/elastic-response Pareto axes."""

from __future__ import annotations
import argparse
import html
import json
from pathlib import Path
import numpy as np
import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
from mpl_toolkits.mplot3d.art3d import Line3DCollection
from lab import pareto


def label(row):
    c = row["candidate"]
    axis = {"x": "X", "y": "Y", "alternating_xy": "XY", "alternating_yx": "YX"}[
        c["axis"]
    ]
    cadence = " · combined" if c.get("combine_height") else ""
    return f"{c['family'].title()} {100 * c['fraction']:g}% · {axis}{cadence}"


def build(campaign: Path, output: Path):
    output.mkdir(parents=True, exist_ok=True)
    config = json.loads((campaign / "config.json").read_text())
    rows = [json.loads(x) for x in (campaign / "trials.jsonl").read_text().splitlines()]
    valid = [r for r in rows if r["status"] == "ok"]
    if not valid:
        raise ValueError("no successful actual constructions")
    root = campaign.parents[1]
    front = {r["id"] for r in pareto(rows)}
    plt.rcParams.update(
        {
            "font.family": "DejaVu Sans",
            "axes.spines.top": False,
            "axes.spines.right": False,
        }
    )
    fig, axes = plt.subplots(1, 2, figsize=(12, 4.6), layout="constrained")
    colors = {
        "x": "#007f83",
        "y": "#a56a1e",
        "alternating_xy": "#667787",
        "alternating_yx": "#9486af",
    }
    for r in valid:
        k = r["mechanics"]["stiffness_n_mm"]
        c = r["candidate"]
        color = colors[c["axis"]]
        marker = (
            "s"
            if c.get("combine_height")
            else ("D" if c["family"] != "uniform" else "o")
        )
        for ax, x in zip(axes, [r["mass_g"], r["motion_s"]]):
            ax.scatter(
                x,
                k,
                c=color,
                marker=marker,
                s=55,
                alpha=0.85,
                edgecolors="#111827" if r["id"] in front else "none",
                linewidths=1.2,
            )
    for ax in axes:
        ax.set_ylabel("Simulated elastic stiffness (N/mm)")
        ax.grid(alpha=0.15)
    axes[0].set_xlabel("Commanded deposited material (g)")
    axes[1].set_xlabel("Motion time lower bound (s)")
    fig.suptitle(
        f"{config['specimen']} / {config['case']} — same shell, skins and material model"
    )
    fig.savefig(output / "pareto.png", dpi=180)
    fig.savefig(output / "pareto.svg")
    plt.close(fig)
    # Gallery choices: explicit default, most efficient admissible result, stiffest and nonuniform.
    ordinary = next(
        (
            r
            for r in valid
            if r["candidate"]["family"] == "uniform"
            and r["candidate"]["axis"] == "alternating_xy"
            and r["candidate"]["fraction"] == 0.2
            and r["candidate"].get("layer_height", 0.2) == 0.2
        ),
        valid[0],
    )
    efficient = max(valid, key=lambda r: r["mechanics"]["stiffness_n_mm"] / r["mass_g"])
    stiffest = max(valid, key=lambda r: r["mechanics"]["stiffness_n_mm"])
    regional = max(
        (r for r in valid if r["candidate"]["family"] != "uniform"),
        key=lambda r: r["mechanics"]["stiffness_n_mm"] / r["mass_g"],
        default=efficient,
    )
    selected = list(
        {r["id"]: r for r in [ordinary, efficient, regional, stiffest]}.values()
    )
    gallery = []
    fig = plt.figure(figsize=(12, 3.7 * len(selected)), layout="constrained")
    for index, r in enumerate(selected):
        roads = json.loads((root / r["native"] / "roads.json").read_text())["roads"]
        points = np.array([p[k] for p in roads for k in ("start", "end")])
        lo = points.min(axis=0)
        hi = points.max(axis=0)
        center = (lo + hi) / 2
        segments = []
        tints = []
        for road in roads:
            a, b = np.array(road["start"]), np.array(road["end"])
            # A geometric cutaway removes the front half and upper roof; only decoded roads remain.
            if (
                road["upper"] > lo[2] + 0.7 * (hi[2] - lo[2])
                or min(a[1], b[1]) < lo[1] + 0.25
            ):
                continue
            segments.append([a - center, b - center])
            tints.append("#008d8e" if road["kind"] in (5, 13) else "#91a0af")
        ax = fig.add_subplot(len(selected), 1, index + 1, projection="3d")
        ax.add_collection3d(
            Line3DCollection(segments, colors=tints, linewidths=1.4, alpha=0.9)
        )
        size = hi - lo
        ax.set_xlim(-size[0] / 2, size[0] / 2)
        ax.set_ylim(-size[1] / 2, size[1] / 2)
        ax.set_zlim(-size[2] / 2, size[2] / 2)
        ax.set_box_aspect(size)
        ax.view_init(elev=28, azim=-65)
        ax.set_axis_off()
        ax.set_title(
            f"{label(r)}   |   {r['mass_g']:.3f} g   |   {r['mechanics']['stiffness_n_mm']:.0f} N/mm",
            loc="left",
            fontsize=12,
        )
        gallery.append({"label": label(r), "row": r, "roads": roads})
    fig.savefig(output / "gallery.png", dpi=180)
    plt.close(fig)
    brief = {
        "config": config,
        "trials": len(rows),
        "successes": len(valid),
        "failures": [r for r in rows if r["status"] != "ok"],
        "pareto_ids": sorted(front),
        "rows": rows,
        "selected": [g["row"]["id"] for g in gallery],
    }
    (output / "results.json").write_text(
        json.dumps(brief, sort_keys=True, allow_nan=False)
    )
    content = HTML.replace("__DATA__", json.dumps(gallery).replace("<", "\\u003c"))
    content = content.replace(
        "__META__",
        html.escape(
            f"{config['specimen']} · {config['case']} · {len(valid)}/{len(rows)} admitted · mesh {config['dx']:g} × {config['dx']:g} × ≤{config['dz']:g} mm"
        ),
    )
    import base64

    content = content.replace(
        "__CHART__", base64.b64encode((output / "pareto.png").read_bytes()).decode()
    )
    (output / "gallery.html").write_text(content)
    print(output / "gallery.html")


HTML = """<!DOCTYPE html><html><head><meta charset="utf-8"><style>
*{box-sizing:border-box}body{margin:0;color:var(--foreground,#dbe4ef);font-family:var(--font-sans,system-ui)}
header{margin-bottom:16px}h2{font-size:21px;margin:0 0 7px}p{line-height:1.5;font-size:13px;color:var(--muted-foreground,#8696ab)}
img{width:100%;border-radius:var(--radius,10px)}.controls{display:flex;gap:8px;flex-wrap:wrap;margin:14px 0}
button{border:1px solid var(--border,#384657);background:var(--secondary,#202c39);color:inherit;border-radius:6px;padding:8px 10px;cursor:pointer}
button[aria-pressed=true]{border-color:var(--chart-2,#18b7b1);color:var(--chart-2,#18b7b1)}
canvas{width:100%;height:340px;touch-action:none}.view{background:var(--card,#16202c);padding:16px;border-radius:var(--radius,10px)}
.metrics{display:flex;gap:24px;flex-wrap:wrap}.metrics strong{display:block;font:22px var(--font-mono,monospace)}.metrics span{font-size:12px;color:var(--muted-foreground,#8696ab)}
label{display:flex;gap:12px;align-items:center;font-size:13px}input{flex:1}summary{cursor:pointer}details{font-size:13px;line-height:1.6;margin-top:14px}
</style></head><body><header><h2>Interior construction lab</h2><p>__META__</p></header>
<img alt="Material, motion time and simulated stiffness Pareto comparisons including solid fill" src="data:image/png;base64,__CHART__">
<div class="controls" id="choices"></div><div class="view"><div class="metrics" id="metrics"></div>
<canvas id="view" aria-label="Interactive view of actual independently replayed deposition paths"></canvas>
<label>Cutaway <input id="cut" type="range" min="15" max="100" value="70"><output id="height"></output></label>
<p>Drag to orbit. The roof and front wall are cut away to reveal actual paths.</p></div>
<details><summary>Model and limits</summary><p>BREP → native compile → emitted G-code → fresh independent replay → nominal footprint-prism mesh → full-integration Q1 elasticity. Assumed isotropic PLA: E 2300 MPa, ν 0.35, density 1.24 g/cm³. Nominal interfaces are perfectly bonded. These are stiffness estimates, not failure loads, crushing tests, calibration or machine readiness. Commanded material includes startup prime; FE volume remains separate. Time excludes acceleration, heating and firmware delays. Different densities are sampled, not an exhaustive or equal-mass optimization proof. Border rings mark the sampled three-objective Pareto frontier.</p></details>
<script>const items=__DATA__;let selected=0,yaw=-.8,pitch=.55,drag=null;const canvas=document.getElementById('view'),ctx=canvas.getContext('2d');
const colors=['#91a0af','#91a0af','#91a0af','#91a0af','#91a0af','#1ac6bb'];
function choose(i){selected=i;document.querySelectorAll('#choices button').forEach((b,j)=>b.setAttribute('aria-pressed',String(i===j)));const r=items[i].row;
document.getElementById('metrics').innerHTML=`<div><strong>${r.mass_g.toFixed(3)} g</strong><span>Commanded material</span></div><div><strong>${r.mechanics.stiffness_n_mm.toFixed(0)} N/mm</strong><span>Simulated elastic stiffness</span></div><div><strong>${r.motion_s.toFixed(1)} s</strong><span>Motion lower bound</span></div>`;draw()}
items.forEach((item,i)=>{let b=document.createElement('button');b.textContent=item.label;b.onclick=()=>choose(i);document.getElementById('choices').append(b)});
function draw(){const scale=window.devicePixelRatio||1;canvas.width=canvas.clientWidth*scale;canvas.height=340*scale;ctx.setTransform(scale,0,0,scale,0,0);ctx.clearRect(0,0,canvas.clientWidth,340);
const roads=items[selected].roads,points=roads.flatMap(r=>[r.start,r.end]),lo=[0,1,2].map(j=>Math.min(...points.map(p=>p[j]))),hi=[0,1,2].map(j=>Math.max(...points.map(p=>p[j]))),center=lo.map((v,j)=>(v+hi[j])/2),extent=hi.map((v,j)=>v-lo[j]);
const zoom=Math.min(canvas.clientWidth/(extent[0]+extent[1])*1.25,260/(extent[1]+extent[2]));const cut=lo[2]+(hi[2]-lo[2])*document.getElementById('cut').value/100;document.getElementById('height').textContent=(cut-lo[2]+.2).toFixed(1)+' mm';
function project(p){const [x,y,z]=p.map((v,j)=>v-center[j]);const u=Math.cos(yaw)*x-Math.sin(yaw)*y,v=Math.sin(yaw)*x+Math.cos(yaw)*y;return [canvas.clientWidth/2+zoom*u,170-zoom*(Math.cos(pitch)*z-Math.sin(pitch)*v),Math.sin(pitch)*z+Math.cos(pitch)*v]}
const ordered=roads.filter(r=>r.upper<=cut+1e-6&&Math.min(r.start[1],r.end[1])>=lo[1]+.25).map(r=>({r,a:project(r.start),b:project(r.end)})).sort((a,b)=>a.a[2]+a.b[2]-b.a[2]-b.b[2]);
for(const {r,a,b} of ordered){ctx.strokeStyle=colors[r.kind]||'#efad59';ctx.globalAlpha=r.kind===5||r.kind===13?.95:.4;ctx.lineWidth=Math.max(1,zoom*r.width*.65);ctx.lineCap='round';ctx.beginPath();ctx.moveTo(a[0],a[1]);ctx.lineTo(b[0],b[1]);ctx.stroke()}ctx.globalAlpha=1}
canvas.onpointerdown=e=>{drag=[e.clientX,e.clientY];canvas.setPointerCapture(e.pointerId)};canvas.onpointerup=()=>drag=null;canvas.onpointermove=e=>{if(!drag)return;yaw+=(e.clientX-drag[0])*.008;pitch=Math.max(-.2,Math.min(1.4,pitch+(e.clientY-drag[1])*.008));drag=[e.clientX,e.clientY];draw()};document.getElementById('cut').oninput=draw;window.addEventListener('resize',draw);choose(0);
</script></body></html>"""

if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--campaign", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    a = p.parse_args()
    build(a.campaign.resolve(), a.output.resolve())
