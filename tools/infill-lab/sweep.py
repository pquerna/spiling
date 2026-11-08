# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0
"""Fixed larger-interior sweeps and normalized, complete-case Pareto reporting."""

from __future__ import annotations
import argparse
import json
import math
from pathlib import Path
import sys
import time

from model import Candidate, digest

# Equal specimen and directional weight. Bending stays a distinct later gate;
# finite cut interiors cannot inherit a whole-part fixture interpretation.
from campaign import SHAPES, bounds

TESTS = [
    (s, "compression_" + axis, 1 / (3 * len(SHAPES))) for s in SHAPES for axis in "xyz"
]
FRACTIONS = (0.1, 0.2, 0.35, 0.5)


def controls():
    yield Candidate(fraction=1)
    for fraction in FRACTIONS:
        yield Candidate(fraction=fraction)
        yield Candidate(fraction=fraction, pattern="grid")
        yield Candidate(fraction=fraction, axis="x")


def geometric_score(values, weights):
    if (
        not values
        or len(values) != len(weights)
        or any(not math.isfinite(v) or v < 0 for v in values)
        or any(not math.isfinite(w) or w <= 0 for w in weights)
        or not math.isclose(sum(weights), 1)
    ):
        raise ValueError(
            "nonnegative complete responses and normalized weights required"
        )
    return (
        0.0
        if any(v == 0 for v in values)
        else math.exp(sum(w * math.log(v) for v, w in zip(values, weights)))
    )


def frontier(rows, y="material_ratio"):
    return [
        r
        for r in rows
        if not any(
            other["score"] >= r["score"]
            and other[y] <= r[y]
            and (other["score"] > r["score"] or other[y] < r[y])
            for other in rows
        )
    ]


def road_roles(data):
    """Nominal dose by independent replay association, not continuum mesh volume."""
    result = {"walls_mm3": 0.0, "skins_mm3": 0.0, "interior_mm3": 0.0}
    for r in data["roads"]:
        h, w = r["upper"] - r["lower"], r["width"]
        if not 0 < h <= w:
            raise ValueError("invalid nominal bead interval")
        v = ((w - h) * h + math.pi * h * h / 4) * math.dist(r["start"], r["end"])
        if r["kind"] == 1:
            role = "walls_mm3"
        elif r["kind"] in (2, 5, 13) or (r["kind"] == 8 and type(r["owner"]) is int):
            role = "interior_mm3"
        elif r["kind"] in (3, 4) or (r["kind"] == 8 and r["owner"] == "mandatory_skin"):
            role = "skins_mm3"
        else:
            raise ValueError(
                f"unclassified replay deposition: {r['kind']}, {r['owner']}"
            )
        result[role] += v
    actual = data["report"]["deposited_volume_mm3"]
    if not math.isclose(sum(result.values()), actual, rel_tol=1e-6, abs_tol=1e-5):
        raise ValueError("role dose does not reconcile with verified object deposition")
    return result


def responses(row):
    r = row["mechanics"]
    return {case["case"]: case["stiffness_n_mm"] for case in r.get("cases", [r])}


def aggregate(rows, *, tests=TESTS):
    """No survivors-only score: every candidate needs the complete frozen panel."""
    specimens = sorted({s for s, _, _ in tests})
    weights = [w for _, _, w in tests]
    by_candidate = {}
    for row in rows:
        key = digest(row["candidate"])
        bucket = by_candidate.setdefault(key, {})
        if row["specimen"] in bucket:
            raise ValueError(
                "duplicate candidate/specimen: select one coherent scenario"
            )
        bucket[row["specimen"]] = row
    solid = next(
        (
            b
            for b in by_candidate.values()
            if next(iter(b.values()))["candidate"] == Candidate(fraction=1).data()
        ),
        None,
    )
    if not solid or any(
        s not in solid or solid[s]["status"] != "ok" for s in specimens
    ):
        raise ValueError("complete verified solid references required")
    valid, incomplete = [], []
    for key, bucket in by_candidate.items():
        candidate = next(iter(bucket.values()))["candidate"]
        missing = [
            s for s in specimens if s not in bucket or bucket[s]["status"] != "ok"
        ]
        if missing:
            incomplete.append({"candidate": candidate, "missing_or_failed": missing})
            continue
        ratios = [
            responses(bucket[s])[case] / responses(solid[s])[case]
            for s, case, _ in tests
        ]
        score = geometric_score(ratios, weights)
        valid.append(
            {
                "candidate": candidate,
                "candidate_id": key,
                "score": score,
                "worst_ratio": min(ratios),
                "tests": [
                    {
                        "specimen": s,
                        "case": c,
                        "ratio": r,
                        "weight": w,
                        "stiffness_n_mm": responses(bucket[s])[c],
                        "solid_stiffness_n_mm": responses(solid[s])[c],
                    }
                    for (s, c, w), r in zip(tests, ratios)
                ],
                "material_ratio": sum(
                    bucket[s]["volume_mm3"] / solid[s]["volume_mm3"] for s in specimens
                )
                / len(specimens),
                "time_ratio": sum(
                    bucket[s]["motion_s"] / solid[s]["motion_s"] for s in specimens
                )
                / len(specimens),
                "specimens": {
                    s: {
                        k: bucket[s].get(k)
                        for k in (
                            "id",
                            "volume_mm3",
                            "mass_g",
                            "motion_s",
                            "native",
                            "mesh",
                            "peak_rss_kib",
                            "roles",
                            "replay_sha256",
                        )
                    }
                    for s in specimens
                },
            }
        )
    front = {r["candidate_id"] for r in frontier(valid)}
    for row in valid:
        row["pareto"] = row["candidate_id"] in front
    return {
        "tests": tests,
        "points": valid,
        "incomplete": incomplete,
        "metric": "assumed_linear_elastic_stiffness_not_strength",
        "mass": "equal-specimen mean object dose / same-source solid; startup prime separate; supports disabled",
    }


def run_sweep(args):
    from lab import bounded, identity, dump
    from native import run, ROOT

    bounded()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=True)
    specification = {
        "identity": identity(args.bin_dir.resolve()),
        "tests": TESTS,
        "candidates": [c.data() for c in controls()],
        "max_xy_mm": args.dx,
        "max_z_mm": args.dz,
        "divisions": args.divisions,
        "z_ratio": args.z_ratio,
        "roles": "nominal_stadium_dose_reconciled_to_actual_object_v1",
    }
    manifest = root / "sweep.json"
    if manifest.exists():
        if json.loads(manifest.read_text()) != json.loads(json.dumps(specification)):
            raise ValueError("sweep inputs changed; use a new output directory")
    else:
        dump(manifest, specification)
    candidates = root / "candidates.jsonl"
    if not candidates.exists():
        candidates.write_text("".join(json.dumps(c.data()) + "\n" for c in controls()))
    deadline = time.monotonic() + args.seconds
    for specimen in SHAPES:
        case = "compression_envelope"
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            break
        cmd = [
            sys.executable,
            ROOT / "tools/infill-lab/lab.py",
            "study",
            "--output",
            root,
            "--bin-dir",
            args.bin_dir.resolve(),
            "--specimen",
            specimen,
            "--case",
            case,
            "--mesh-method",
            "prisms",
            "--solver",
            "superlu_mmd",
            "--coupon-bounds",
            *[v for point in bounds(specimen) for v in point],
            "--divisions",
            args.divisions,
            "--dx",
            args.dx,
            "--dz",
            args.dz,
            "--z-ratio",
            args.z_ratio,
            "--candidates",
            candidates,
            "--controls-only",
            "--limit",
            13,
            "--seconds",
            remaining,
            "--trial-seconds",
            args.trial_seconds,
        ]
        print("START", specimen, flush=True)
        out, _ = run(cmd, timeout=remaining + 10, output=root / f"{specimen}-driver")
        print(out.decode(), flush=True)
    build(root, root / "report")


def read_rows(root):
    configs, rows = [], []
    for campaign in sorted((root / "campaigns").iterdir()):
        config = json.loads((campaign / "config.json").read_text())
        configs.append(config)
        for line in (campaign / "trials.jsonl").read_text().splitlines():
            row = json.loads(line)
            row.setdefault("specimen", config["specimen"])
            if row["status"] == "ok":
                data = json.loads((root / row["native"] / "roads.json").read_text())
                row["roles"] = road_roles(data)
            rows.append(row)
    # Specimen and load differ by design; material and numerical/producer identities cannot.
    if not configs:
        raise ValueError("no completed campaigns")
    frozen = (
        "identity",
        "mesh_method",
        "solver",
        "divisions",
        "dx",
        "dz",
        "phase",
        "material",
    )
    if len({digest({k: c[k] for k in frozen}) for c in configs}) != 1:
        raise ValueError(
            "cannot pool changed judge, producer, material or mesh scenario"
        )
    if any(c.get("coupon_bounds") != bounds(c["specimen"]) for c in configs):
        raise ValueError(
            "finite coupon scope differs from the frozen source-relative panel"
        )
    return rows, configs


def family(c):
    if c["fraction"] == 1:
        return "Solid"
    if c["pattern"] == "grid":
        return "Grid"
    return "Aligned X" if c["axis"] == "x" else "Alternating XY"


def build(root, output):
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    rows, configs = read_rows(root)
    data = aggregate(rows)
    data["configs"] = configs
    data["trials"] = rows
    output.mkdir(parents=True, exist_ok=True)
    (output / "results.json").write_text(
        json.dumps(data, indent=2, allow_nan=False) + "\n"
    )
    fig, ax = plt.subplots(figsize=(9, 6), layout="constrained")
    colors = {
        "Grid": "#007c91",
        "Alternating XY": "#c47a22",
        "Aligned X": "#8651b0",
        "Solid": "#4e5868",
    }
    for name, color in colors.items():
        group = sorted(
            [r for r in data["points"] if family(r["candidate"]) == name],
            key=lambda r: r["candidate"]["fraction"],
        )
        if not group:
            continue
        ax.plot(
            [100 * r["score"] for r in group],
            [100 * r["material_ratio"] for r in group],
            "o-",
            color=color,
            label=name,
        )
        for r in group:
            if name != "Solid":
                ax.annotate(
                    f"{100 * r['candidate']['fraction']:g}%",
                    (100 * r["score"], 100 * r["material_ratio"]),
                    xytext=(5, 4),
                    textcoords="offset points",
                    fontsize=8,
                )
    front = sorted(frontier(data["points"]), key=lambda r: r["score"])
    ax.plot(
        [100 * r["score"] for r in front],
        [100 * r["material_ratio"] for r in front],
        "--",
        color="#667777",
        alpha=0.5,
        label="Measured frontier",
    )
    ax.set(
        xlabel="Weighted geometric-mean stiffness / same-source solid (%)",
        ylabel="Mean actual object material / same-source solid (%)",
        title="Finite actual-interior sweep · simulated stiffness, not strength",
    )
    ax.grid(alpha=0.2)
    ax.legend()
    fig.savefig(output / "pareto.png", dpi=180)
    plt.close(fig)
    (output / "chart.html").write_text(chart_html(data))
    print(
        json.dumps(
            {
                "report": str(output),
                "complete_points": len(data["points"]),
                "incomplete": len(data["incomplete"]),
                "frontier": [
                    {
                        "family": family(r["candidate"]),
                        "fraction": r["candidate"]["fraction"],
                        "score": r["score"],
                        "material": r["material_ratio"],
                        "worst": r["worst_ratio"],
                    }
                    for r in front
                ],
            }
        ),
        flush=True,
    )


def chart_html(data):
    # Self-contained retained research output, not a product UI or geometry authority.
    payload = json.dumps(
        {k: data[k] for k in ("points", "incomplete", "tests", "metric")},
        allow_nan=False,
    ).replace("<", "\\u003c")
    return """<!doctype html><html><head><meta charset="utf-8"><style>
body{margin:0;font:14px var(--font-sans,system-ui);color:var(--foreground,#26323b)}h3{font-size:17px;margin:0 0 8px}p{font-size:12px;color:var(--muted-foreground,#617080)}svg{width:100%;height:380px;display:block}text{fill:currentColor;font-size:11px}button,select{font:inherit;padding:5px}#details{font-size:13px;line-height:1.6;min-height:100px}circle{cursor:pointer}circle:hover{r:7}.legend{display:flex;flex-wrap:wrap;gap:16px}i{display:inline-block;width:10px;height:10px;margin-right:5px}table{border-collapse:collapse;width:100%;font-size:12px}td,th{text-align:left;padding:4px;border-bottom:1px solid var(--border,#ccd5db)}
</style></head><body><h3>Larger-interior infill tradeoffs</h3><p>Actual verified deposition · assumed weak-Z PLA elasticity · no physical strength qualification</p><div class="legend" id="legend"></div><p>X axis: <select id="xaxis"><option value="overall">Overall stiffness</option><option value="z">Z compression stiffness</option><option value="worst">Weakest retained response</option></select> · Y axis: <select id="axis"><option value="material_ratio">Object material / solid</option><option value="time_ratio">Motion time lower bound / solid</option></select> · Best direction ↘</p><svg viewBox="0 0 700 380" id="chart" role="img" aria-label="Normalized infill material versus simulated stiffness"></svg><div id="details">Select a point to inspect its weakest load, actual costs and source/replay identities.</div><p id="limits"></p><script>
const data=PAYLOAD,colors={'Grid':'var(--chart-1,#007c91)','Alternating XY':'var(--chart-2,#c47a22)','Aligned X':'var(--chart-3,#8651b0)','Solid':'var(--muted-foreground,#4e5868)'};
function family(c){return c.fraction===1?'Solid':c.pattern==='grid'?'Grid':c.axis==='x'?'Aligned X':'Alternating XY'}
const fmt=x=>(100*x).toFixed(1)+'%';document.getElementById('legend').innerHTML=Object.entries(colors).map(([n,c])=>'<span><i style="background:'+c+'"></i>'+n+'</span>').join('');
function draw(){const svg=document.getElementById('chart'),axis=document.getElementById('axis').value,mode=document.getElementById('xaxis').value,score=r=>mode==='z'?r.tests.find(t=>t.case==='compression_z').ratio:mode==='worst'?r.worst_ratio:r.score;svg.replaceChildren();const ns='http://www.w3.org/2000/svg',xmax=Math.max(1.05,...data.points.map(r=>score(r)*1.08)),ymax=Math.max(1.05,...data.points.map(r=>r[axis]*1.08)),X=v=>65+570*v/xmax,Y=v=>325-280*v/ymax;
function el(n,a,t){const e=document.createElementNS(ns,n);Object.entries(a).forEach(([k,v])=>e.setAttribute(k,v));if(t)e.textContent=t;svg.appendChild(e);return e}
for(let i=0;i<=4;i++){const x=xmax*i/4,y=ymax*i/4;el('line',{x1:X(x),x2:X(x),y1:Y(0),y2:Y(ymax),stroke:'var(--border,#ccd5db)'});el('line',{x1:X(0),x2:X(xmax),y1:Y(y),y2:Y(y),stroke:'var(--border,#ccd5db)'});el('text',{x:X(x),y:345,'text-anchor':'middle'},fmt(x));el('text',{x:56,y:Y(y)+4,'text-anchor':'end'},fmt(y))}
el('text',{x:350,y:373,'text-anchor':'middle'},(mode==='overall'?'Weighted geometric-mean stiffness':mode==='z'?'Z compression stiffness':'Weakest retained stiffness')+' / same-source solid');
for(const[name,color]of Object.entries(colors)){const rows=data.points.filter(r=>family(r.candidate)===name).sort((a,b)=>a.candidate.fraction-b.candidate.fraction);el('polyline',{points:rows.map(r=>X(score(r))+','+Y(r[axis])).join(' '),stroke:color,fill:'none','stroke-width':2});for(const r of rows){const c=el('circle',{cx:X(score(r)),cy:Y(r[axis]),r:5,fill:color,tabindex:0,stroke:!data.points.some(t=>score(t)>=score(r)&&t[axis]<=r[axis]&&(score(t)>score(r)||t[axis]<r[axis]))?'var(--foreground,#26323b)':'none','stroke-width':2});c.appendChild(document.createElementNS(ns,'title')).textContent=name+' '+fmt(r.candidate.fraction)+' · stiffness '+fmt(r.score)+' · material '+fmt(r.material_ratio);c.onclick=c.onfocus=()=>select(r);if(name!=='Solid')el('text',{x:X(score(r))+6,y:Y(r[axis])-7},fmt(r.candidate.fraction));}}
}
function select(r){let s='<b>'+family(r.candidate)+' · '+fmt(r.candidate.fraction)+' setting</b><br>Stiffness '+fmt(r.score)+' · material '+fmt(r.material_ratio)+' · motion '+fmt(r.time_ratio)+' · weakest case '+fmt(r.worst_ratio)+' of solid';s+='<table><tr><th>Specimen / load</th><th>Stiffness retained</th></tr>'+r.tests.map(t=>'<tr><td>'+t.specimen+' / '+t.case+'</td><td>'+fmt(t.ratio)+'</td></tr>').join('')+'</table>';s+='<table><tr><th>Actual specimen</th><th>Object mass</th><th>Wall + skin dose</th><th>Motion lower bound</th></tr>'+Object.entries(r.specimens).map(([n,v])=>'<tr><td>'+n+'</td><td>'+v.mass_g.toFixed(2)+' g</td><td>'+fmt((v.roles.walls_mm3+v.roles.skins_mm3)/v.volume_mm3)+'</td><td>'+v.motion_s.toFixed(0)+' s</td></tr>').join('')+'</table>';document.getElementById('details').innerHTML=s}
document.getElementById('axis').onchange=draw;document.getElementById('xaxis').onchange=draw;document.getElementById('limits').textContent='Six source shapes: equal-weight independent XYZ compression of finite actual-interior coupons. This is not whole-part strength. Fixed walls/skins/nozzle. Object mass excludes fixed startup prime; supports disabled. Motion excludes heating and acceleration. Lines connect tested settings, not interpolated qualification. Incomplete candidate panels: '+data.incomplete.length+'.';draw();
</script></body></html>""".replace("PAYLOAD", payload)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="command", required=True)
    run = sub.add_parser("run")
    run.add_argument("--output", type=Path, required=True)
    run.add_argument("--bin-dir", type=Path, required=True)
    run.add_argument("--seconds", type=float, default=3600)
    run.add_argument("--trial-seconds", type=float, default=240)
    run.add_argument("--dx", type=float, default=4)
    run.add_argument("--dz", type=float, default=1)
    run.add_argument("--divisions", type=int, choices=[1, 2], default=1)
    run.add_argument("--z-ratio", type=float, default=0.25)
    report = sub.add_parser("report")
    report.add_argument("--study-root", type=Path, required=True)
    report.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    if args.command == "run":
        run_sweep(args)
    else:
        build(args.study_root.resolve(), args.output.resolve())


if __name__ == "__main__":
    main()
