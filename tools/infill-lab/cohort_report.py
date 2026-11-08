# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Complete-panel coupon Pareto charts and actual-deposition cutaway galleries."""

from __future__ import annotations
import argparse
import html
import json
import math
from pathlib import Path

from campaign import bounds
from model import digest, file_hash
from sweep import aggregate, geometric_score

COLORS = {
    "Alternating XY": "#dc9c36",
    "Grid": "#1aa69f",
    "Aligned X": "#8a83da",
    "Separator X": "#db6688",
    "Separator Y": "#5795d5",
    "Crown": "#9c8a72",
    "Solid": "#77828d",
}


def family(c):
    if c["family"] == "separator":
        return "Separator " + c["axis"].upper()
    if c["family"] == "crown":
        return "Crown"
    if c["fraction"] == 1:
        return "Solid"
    if c["pattern"] == "grid":
        return "Grid"
    return "Aligned X" if c["axis"] == "x" else "Alternating XY"


def setting(c):
    if c["family"] != "uniform":
        return f"{100 * c['fraction']:g}→{100 * c['region_fraction']:g}%"
    return f"{100 * c['fraction']:g}%"


def scenario_key(row):
    scenario = row["scenario"]
    base = bounds(row["specimen"])
    crop = scenario["bounds"]
    if any(
        not math.isclose(crop[k][a], base[k][a], abs_tol=1e-8)
        for k in (0, 1)
        for a in (0, 1)
    ):
        raise ValueError("unexpected source-relative XY coupon scope")
    shift = crop[0][2] - base[0][2]
    if not math.isclose(crop[1][2] - base[1][2], shift, abs_tol=1e-8):
        raise ValueError("coupon height differs from frozen panel")
    return {k: v for k, v in scenario.items() if k != "bounds"} | {"shift_z_mm": shift}


def physical_gate(row):
    result = row["mechanics"]
    if result["scope"] != "actual_interior_coupon":
        raise ValueError("result is not a finite actual-interior coupon")
    if {r["case"] for r in result["cases"]} != {
        "compression_x",
        "compression_y",
        "compression_z",
    }:
        raise ValueError("incomplete directional compression result")
    for case in result["cases"]:
        value = case["stiffness_n_mm"]
        if not math.isfinite(value) or value < 0:
            raise ValueError("invalid directional response")
        components = case["components"]
        if not components:
            if (
                value != 0
                or case["mechanism"] != "no_component_spans_fixture_faces"
                or case["spanning_components"] != 0
            ):
                raise ValueError("zero response lacks its nonspanning witness")
        elif not math.isclose(
            sum(c["stiffness_n_mm"] for c in components), value, rel_tol=1e-12
        ):
            raise ValueError("component force response differs")
        for component in components:
            for key, limit in (
                ("relative_residual", 2e-8),
                ("scaled_relative_residual", 2e-8),
                ("energy_error", 1e-6),
                ("force_balance", 1e-6),
            ):
                if (
                    not math.isfinite(component[key])
                    or not 0 <= component[key] <= limit
                ):
                    raise ValueError("physical gate failed: " + key)


def collect(root, inputs, outputs):
    capture = json.loads((root / "capture.json").read_text())
    groups, seen, images, accepted, missing = {}, set(), set(), [], []
    for folder in inputs:
        manifest = json.loads((folder / "jobs.json").read_text())
        for job in manifest["jobs"]:
            spec = json.loads((folder / job["id"] / "case.json").read_text())
            input_hash = file_hash(folder / job["id"] / "case.json")
            result = outputs / job["id"]
            rows = (
                [
                    json.loads(line)
                    for line in (result / "trials.jsonl").read_text().splitlines()
                ]
                if (result / "trials.jsonl").exists()
                else []
            )
            by_scenario = {digest(r["scenario"]): r for r in rows}
            if len(by_scenario) != len(rows):
                raise ValueError("duplicate remote scenario")
            launch = (
                json.loads((result / "launch.json").read_text())
                if (result / "launch.json").exists()
                else None
            )
            for scenario in spec["scenarios"]:
                expected = (input_hash, digest(scenario))
                if expected in seen:
                    continue
                seen.add(expected)
                row = by_scenario.get(digest(scenario))
                if row is None:
                    row = {
                        "status": "missing_or_interrupted",
                        "candidate": spec["candidate"],
                        "specimen": spec["specimen"],
                        "scenario": scenario,
                        "input_sha256": input_hash,
                        "id": digest(expected),
                    }
                    missing.append(row)
                else:
                    for key in (
                        "candidate",
                        "specimen",
                        "source_sha256",
                        "replay_sha256",
                        "judge",
                    ):
                        if row[key] != spec[key]:
                            raise ValueError("remote result differs from input: " + key)
                    if row["input_sha256"] != input_hash or not launch:
                        raise ValueError("remote input/launch receipt missing")
                    images.add(launch["image"]["id"])
                    row["host"] = json.loads((result / "dispatch.json").read_text())[
                        "host"
                    ]
                    if row["status"] == "ok":
                        physical_gate(row)
                        accepted.append(row)
                key = digest(scenario_key(row))
                groups.setdefault(key, {"scenario": scenario_key(row), "rows": []})[
                    "rows"
                ].append(row)
    if len(images) > 1:
        raise ValueError("cannot pool different evaluator images")
    identities = {
        digest(
            {
                "judge": r["judge"],
                "binaries": r["bin_sha256"],
                "build": r["engine_build"],
            }
        )
        for r in accepted
    }
    if len(identities) > 1:
        raise ValueError("cannot pool different evaluator/producer identities")
    for group in groups.values():
        # Include failed native candidates, so an absent positive is visible.
        group["rows"] += [
            r
            for r in [
                json.loads(line)
                for line in (root / "captures.jsonl").read_text().splitlines()
            ]
            if r["status"] != "ok"
        ]
        shapes = capture["specimens"]
        tests = [
            (s, "compression_" + axis, 1 / (len(shapes) * 3))
            for s in shapes
            for axis in "xyz"
        ]
        try:
            group["aggregate"] = aggregate(group["rows"], tests=tests)
            for point in group["aggregate"]["points"]:
                point["group"] = family(point["candidate"])
                point["label"] = setting(point["candidate"])
                z = [t["ratio"] for t in point["tests"] if t["case"] == "compression_z"]
                point["z_score"] = geometric_score(z, [1 / len(z)] * len(z))
        except ValueError as error:
            group["error"] = str(error)
    return {
        "schema": 1,
        "capture": capture,
        "images": sorted(images),
        "groups": groups,
        "accepted_evaluations": len(accepted),
        "missing_evaluations": len(missing),
        "scope": "finite_nominal_actual_interior_elastic_stiffness_not_strength",
    }


def charts(report, output):
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    for key, group in report["groups"].items():
        if "aggregate" not in group:
            continue
        points = group["aggregate"]["points"]
        fig, axes = plt.subplots(1, 2, figsize=(13, 5.5), layout="constrained")
        for ax, metric, title in (
            (axes[0], "score", "XYZ geometric mean"),
            (axes[1], "z_score", "Z compression"),
        ):
            for name, color in COLORS.items():
                rs = sorted(
                    (r for r in points if r["group"] == name),
                    key=lambda r: r["material_ratio"],
                )
                if not rs:
                    continue
                ax.plot(
                    [100 * r[metric] for r in rs],
                    [100 * r["material_ratio"] for r in rs],
                    "o-",
                    color=color,
                    label=name,
                    linewidth=1.6,
                )
                for r in rs:
                    if name != "Solid":
                        ax.annotate(
                            r["label"],
                            (100 * r[metric], 100 * r["material_ratio"]),
                            xytext=(4, 5),
                            textcoords="offset points",
                            fontsize=7,
                        )
            ax.set(
                xlabel=f"{title} / same-source solid (%)",
                ylabel="Complete object material / solid (%)",
                xlim=left_zero(points, metric),
            )
            ax.grid(alpha=0.17)
            ax.legend(fontsize=8, loc="best")
        z = group["scenario"]["material"]["z_ratio"]
        fig.suptitle(
            f"Actual-interior coupons · {len(report['capture']['specimens'])} source shapes · assumed Eᵥ/Eₕ = {z:g}\nWalls + skins paid in full; stiffness is not strength",
            fontsize=12,
        )
        fig.savefig(output / (key[:12] + "-pareto.png"), dpi=180)
        fig.savefig(output / (key[:12] + "-pareto.svg"))
        plt.close(fig)
        (output / (key[:12] + "-interactive.html")).write_text(
            interactive(group, report["capture"]["specimens"])
        )


def left_zero(points, metric):
    return (0, max(1.05, max((r[metric] for r in points), default=1) * 1.1) * 100)


def interactive(group, shapes):
    data = json.dumps(
        {
            "points": group["aggregate"]["points"],
            "scenario": group["scenario"],
            "incomplete": group["aggregate"]["incomplete"],
        },
        allow_nan=False,
    ).replace("<", "\\u003c")
    # Inline SVG keeps the finished scientific comparison portable and local.
    return (
        """<!doctype html><html><head><meta charset="utf-8"><style>
:root{font-family:var(--font-sans,system-ui);color:var(--foreground,#253543)}body{margin:0}.controls{display:flex;gap:12px;align-items:center;flex-wrap:wrap}select{padding:6px;background:var(--card,#fff);color:inherit;border:1px solid var(--border,#cbd2d9);border-radius:6px}svg{width:100%;height:390px}svg text{fill:currentColor;font:11px system-ui}p{font-size:13px;color:var(--muted-foreground,#64717f)}table{border-collapse:collapse;width:100%;font-size:12px}td,th{text-align:left;padding:5px;border-bottom:1px solid var(--border,#ccd5db)}.details{padding:16px;background:var(--card,#f1f5f7);border-radius:var(--radius,8px)}</style></head><body>
<div class="controls"><b>Interior stiffness · complete object cost</b><select id="metric"><option value="score">XYZ geometric mean</option><option value="z_score">Z compression</option><option value="worst_ratio">Weakest response</option></select><select id="cost"><option value="material_ratio">Material</option><option value="time_ratio">Motion time</option></select></div><svg id="plot" viewBox="0 0 700 390" role="img" aria-label="Pareto chart of simulated interior stiffness and actual complete object material"></svg><div class="details" id="detail">Select a point for the actual directional results and costs.</div><p id="limits"></p><script>
const d=PAYLOAD,colors=COLORS,svg=document.getElementById('plot'),ns='http://www.w3.org/2000/svg',pct=x=>(100*x).toFixed(1)+'%';
function el(n,a,t){const e=document.createElementNS(ns,n);for(const[k,v]of Object.entries(a))e.setAttribute(k,v);if(t)e.textContent=t;svg.appendChild(e);return e}
function draw(){svg.replaceChildren();const m=document.getElementById('metric').value,cost=document.getElementById('cost').value,xmax=Math.max(1.05,...d.points.map(r=>r[m]*1.1)),ymax=Math.max(1.05,...d.points.map(r=>r[cost]*1.1)),X=v=>65+590*v/xmax,Y=v=>325-285*v/ymax;
for(let i=0;i<=4;i++){let x=xmax*i/4,y=ymax*i/4;el('line',{x1:X(x),x2:X(x),y1:Y(0),y2:Y(ymax),stroke:'var(--border,#d4dce0)'});el('line',{x1:X(0),x2:X(xmax),y1:Y(y),y2:Y(y),stroke:'var(--border,#d4dce0)'});el('text',{x:X(x),y:346,'text-anchor':'middle'},pct(x));el('text',{x:55,y:Y(y)+4,'text-anchor':'end'},pct(y))}
el('text',{x:350,y:375,'text-anchor':'middle'},document.getElementById('metric').selectedOptions[0].text+' / same-source solid');
for(const[name,color]of Object.entries(colors)){let rs=d.points.filter(r=>r.group===name).sort((a,b)=>a.material_ratio-b.material_ratio);el('polyline',{points:rs.map(r=>X(r[m])+','+Y(r[cost])).join(' '),stroke:color,fill:'none','stroke-width':2});for(const r of rs){const front=!d.points.some(t=>t[m]>=r[m]&&t[cost]<=r[cost]&&(t[m]>r[m]||t[cost]<r[cost]));const p=el('circle',{cx:X(r[m]),cy:Y(r[cost]),r:6,fill:color,stroke:front?'var(--foreground,#26333b)':'none','stroke-width':2,tabindex:0});p.appendChild(document.createElementNS(ns,'title')).textContent=name+' '+r.label;p.onclick=p.onfocus=()=>select(r);if(name!=='Solid')el('text',{x:X(r[m])+6,y:Y(r[cost])-8},r.label)}}}
function select(r){let s='<b>'+r.group+' · '+r.label+'</b> · XYZ '+pct(r.score)+' · Z '+pct(r.z_score)+' · material '+pct(r.material_ratio)+' · motion '+pct(r.time_ratio);s+='<table><tr><th>Source</th><th>X / solid</th><th>Y / solid</th><th>Z / solid</th><th>Object material</th></tr>';for(const [name,v]of Object.entries(r.specimens)){const t=r.tests.filter(t=>t.specimen===name);s+='<tr><td>'+name+'</td>'+t.map(t=>'<td>'+pct(t.ratio)+'</td>').join('')+'<td>'+v.mass_g.toFixed(2)+' g</td></tr>'}document.getElementById('detail').innerHTML=s+'</table>'}
document.getElementById('metric').onchange=draw;document.getElementById('cost').onchange=draw;document.getElementById('limits').textContent='Finite cut interiors from SHAPES. Assumed linear elasticity; Z/XY modulus '+d.scenario.material.z_ratio+'. No whole-part strength, buckling or bond-failure claim. Walls and skins remain fully charged. Motion excludes heat-up and acceleration. Lines join measured settings. Incomplete candidate panels: '+d.incomplete.length+'.';draw();
</script></body></html>""".replace("PAYLOAD", data)
        .replace("COLORS", json.dumps(COLORS))
        .replace("SHAPES", ", ".join(shapes))
    )


def gallery(root, report, output):
    import numpy as np
    import matplotlib.pyplot as plt
    from mpl_toolkits.mplot3d.art3d import Line3DCollection

    captures = [
        json.loads(line) for line in (root / "captures.jsonl").read_text().splitlines()
    ]
    rows = [r for r in captures if r["status"] == "ok"]
    families = [
        ("Alternating XY", 0.2),
        ("Grid", 0.2),
        ("Aligned X", 0.2),
        ("Separator X", 0.1),
    ]
    for shape in report["capture"]["specimens"]:
        fig = plt.figure(figsize=(13, 4.3), layout="constrained")
        for i, (name, fraction) in enumerate(families):
            row = next(
                (
                    r
                    for r in rows
                    if r["specimen"] == shape
                    and family(r["candidate"]) == name
                    and r["candidate"]["fraction"] == fraction
                ),
                None,
            )
            ax = fig.add_subplot(1, 4, i + 1, projection="3d")
            if row is None:
                ax.set_axis_off()
                continue
            data = json.loads((root / row["native"] / "roads.json").read_text())
            cutoff = data["size_mm"][2] * 0.58
            for interior, color, alpha, lw in (
                (False, "#89939c", 0.12, 0.4),
                (True, COLORS[name], 0.75, 0.7),
            ):
                roads = [
                    r
                    for r in data["roads"]
                    if r["upper"] <= cutoff
                    and (
                        (
                            r["kind"] in (2, 5, 13)
                            or (r["kind"] == 8 and type(r["owner"]) is int)
                        )
                        == interior
                    )
                ]
                segs = np.array([[r["start"], r["end"]] for r in roads])
                if len(segs):
                    ax.add_collection3d(
                        Line3DCollection(segs, colors=color, alpha=alpha, linewidths=lw)
                    )
            x, y, z = data["size_mm"]
            ax.set(xlim=(20, 20 + x), ylim=(20, 20 + y), zlim=(0, z * 0.65))
            ax.set_box_aspect((x, y, z * 0.65))
            ax.view_init(25, -55)
            ax.set_axis_off()
            ax.set_title(
                name
                + " "
                + setting(row["candidate"])
                + "\n"
                + f"{data['report']['deposited_volume_mm3'] * 0.00124:.2f} g complete object",
                fontsize=10,
            )
        fig.suptitle(
            shape + " · actual nominal deposited roads · upper part cut away",
            fontsize=13,
        )
        fig.savefig(output / (shape + "-gallery.png"), dpi=180)
        plt.close(fig)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--study-root", type=Path, required=True)
    p.add_argument("--inputs", type=Path, nargs="+", required=True)
    p.add_argument("--results", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    report = collect(args.study_root, args.inputs, args.results)
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "results.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    charts(report, args.output)
    gallery(args.study_root, report, args.output)
    links = "".join(
        '<li><a href="' + html.escape(p.name) + '">' + html.escape(p.name) + "</a></li>"
        for p in sorted(args.output.glob("*"))
        if p.suffix in (".html", ".png") and p.name != "index.html"
    )
    (args.output / "index.html").write_text(
        '<!doctype html><meta charset="utf-8"><title>Actual-interior research</title><h1>Actual-interior research</h1><p>Finite coupon stiffness; complete object material. No physical strength claim.</p><ul>'
        + links
        + "</ul>"
    )
    print(
        json.dumps(
            {
                "accepted_evaluations": report["accepted_evaluations"],
                "missing_evaluations": report["missing_evaluations"],
                "panels": len(report["groups"]),
            }
        )
    )


if __name__ == "__main__":
    main()
