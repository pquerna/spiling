# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Audited actual-road sections and a provisional finite-bay comparison."""

from __future__ import annotations

import argparse
import html
import json
import math
from pathlib import Path

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.collections import LineCollection
from matplotlib.patches import Polygon
from mpl_toolkits.mplot3d.art3d import Poly3DCollection
import numpy as np
import shapely

from cathedral import fixture
from vault_search import native_rows


def section(roads, y):
    boxes = []
    for r in roads:
        a, b = r["start"], r["end"]
        radius = r["width"] / 2
        if a[1] == b[1]:
            distance = abs(y - a[1])
            if distance >= radius:
                continue
            extension = math.sqrt(radius**2 - distance**2)
            lo, hi = min(a[0],b[0])-extension, max(a[0],b[0])+extension
        elif a[0] == b[0]:
            distance = max(min(a[1],b[1])-y, y-max(a[1],b[1]), 0.)
            if distance >= radius:
                continue
            extension = math.sqrt(radius**2-distance**2)
            lo, hi = a[0]-extension, a[0]+extension
        else:
            raise ValueError("section requires actual orthogonal roads")
        boxes.append(shapely.box(lo,r["lower"],hi,r["upper"]))
    return shapely.union_all(boxes, grid_size=1e-9)


def draw_shape(ax, shape):
    for polygon in shapely.get_parts(shape):
        if polygon.geom_type != "Polygon":
            raise ValueError("section has nonpolygon material")
        ax.add_patch(Polygon(np.asarray(polygon.exterior.coords), facecolor="#468ecf", linewidth=0))
        for ring in polygon.interiors:
            ax.add_patch(Polygon(np.asarray(ring.coords), facecolor="white", linewidth=0))


def bay_preview(ax, data, bounds):
    # Coalesce only identical actual XY/width strokes over touching physical
    # intervals. Transparency is display; no ideal wall replaces the capture.
    from shapely.geometry import LineString
    groups = {}
    for r in data["roads"]:
        if not isinstance(r["owner"],int) or r["kind"] not in (5,8):
            continue
        low, high = max(r["lower"],bounds[0][2]),min(r["upper"],bounds[1][2])
        if low >= high:
            continue
        key = (tuple(sorted((tuple(r["start"][:2]),tuple(r["end"][:2])))),r["width"])
        groups.setdefault(key,[]).append((low,high))
    faces = []
    window = shapely.box(bounds[0][0],bounds[0][1],bounds[1][0],bounds[1][1])
    for (points,width),intervals in groups.items():
        shape = LineString(points).buffer(width/2,quad_segs=8).intersection(window)
        if shape.is_empty:
            continue
        spans = []
        for low,high in sorted(intervals):
            if spans and abs(spans[-1][1]-low) < 1e-9:
                spans[-1][1] = high
            else:
                spans.append([low,high])
        for polygon in shapely.get_parts(shape):
            if polygon.geom_type != "Polygon":
                continue
            ring = np.asarray(polygon.exterior.coords)
            for low,high in spans:
                for a,b in zip(ring,ring[1:]):
                    faces.append([[*a,low],[*b,low],[*b,high],[*a,high]])
                for z in (low,high):
                    faces.append([[*point,z] for point in ring[:-1]])
    ax.add_collection3d(Poly3DCollection(faces,facecolor="#468ecf",edgecolor="none",alpha=.45))
    ax.set(xlim=(bounds[0][0],bounds[1][0]),ylim=(bounds[0][1],bounds[1][1]),
           zlim=(bounds[0][2],bounds[1][2]),xlabel="X",ylabel="Y",zlabel="Z")
    ax.set_box_aspect(np.asarray(bounds[1])-np.asarray(bounds[0]))
    ax.view_init(elev=22,azim=-57)


def render(capture, archive_path, output):
    _, native = native_rows(capture)
    archive = json.loads(archive_path.read_text())
    rows = archive["rows"]
    mapping = {r["id"]:r for r in native}
    if {r["capture_id"] for r in rows} != set(mapping):
        raise ValueError("gallery must retain the complete native panel")
    output.mkdir(parents=True,exist_ok=False)
    loaded = {}
    for row in rows:
        native_row = mapping[row["capture_id"]]
        if native_row["status"] != "ok" or native_row["candidate"] != row["candidate"]:
            raise ValueError("unknown or unverified gallery construction")
        data = json.loads((capture/native_row["native"]/"roads.json").read_text())
        if data["report"]["deposited_volume_mm3"] != row["volume_mm3"]:
            raise ValueError("gallery dose differs")
        loaded[row["capture_id"]] = data
    flat = next(r for r in rows if r["name"] == "flat")
    baseline = loaded[flat["capture_id"]]
    bounds = fixture(baseline)
    y = (bounds[0][1]+bounds[1][1])/2
    fig, axes = plt.subplots(len(rows),2,figsize=(12,3.2*len(rows)),layout="constrained")
    for pair,row in zip(axes,rows):
        data = loaded[row["capture_id"]]
        ax,top = pair
        draw_shape(ax,section(data["roads"],y))
        ax.set(xlim=(bounds[0][0],bounds[1][0]),ylim=(0,14),aspect="equal",xlabel="X (mm)",ylabel="Z (mm)")
        ratio = row["volume_mm3"]/flat["volume_mm3"]-1
        ax.set_title(f"{row['name'].replace('_',' ').title()} · {ratio:+.2%} material",loc="left",weight="bold")
        target = min({r["upper"] for r in data["roads"]},key=lambda z:abs(z-3.))
        selected = [r for r in data["roads"] if r["upper"] == target]
        top.add_collection(LineCollection([[r["start"][:2],r["end"][:2]] for r in selected],
                         colors=["#468ecf" if isinstance(r["owner"],int) else "#aab3bb" for r in selected],
                         linewidths=1.2))
        top.set(xlim=(20,20+data["size_mm"][0]),ylim=(20,20+data["size_mm"][1]),aspect="equal",xlabel="X (mm)",ylabel="Y (mm)")
        top.set_title(f"Actual layer · Z {target:g} mm",loc="left")
    fig.suptitle("Cathedral · finite web bay and actual layers",fontsize=15)
    fig.savefig(output/"sections.png",dpi=160)
    fig.savefig(output/"sections.svg")
    plt.close(fig)
    fig=plt.figure(figsize=(12,8),layout="constrained")
    for index,row in enumerate(rows):
        ax=fig.add_subplot(2,2,index+1,projection="3d")
        bay_preview(ax,loaded[row["capture_id"]],bounds)
        ax.set_title(row["name"].replace('_',' ').title(),loc="left",weight="bold")
    fig.savefig(output/"bays.png",dpi=160)
    plt.close(fig)
    fig,ax = plt.subplots(figsize=(8,5),layout="constrained")
    scored = [r for r in rows if r.get("geometric_mean_ratio") is not None]
    for r in scored:
        x,ycost = r["geometric_mean_ratio"],r["volume_mm3"]/flat["volume_mm3"]
        ax.scatter([x],[ycost],s=65)
        ax.annotate(r["name"].replace('_',' '),(x,ycost),xytext=(7,7),textcoords="offset points")
    ax.axhline(1,color="#aaa",linewidth=.7)
    ax.axvline(1,color="#aaa",linewidth=.7)
    ax.set(xlabel="Geometric mean of three local stiffness ratios / flat",
           ylabel="Complete-object deposited material / flat",
           title="Finite web-bay screening · higher response, lower material")
    if not scored:
        ax.text(.5,.5,"No complete numerical score",transform=ax.transAxes,ha="center")
    fig.savefig(output/"comparison.png",dpi=160)
    fig.savefig(output/"comparison.svg")
    plt.close(fig)
    table = "".join(f"<tr><td>{html.escape(r['name'])}</td><td>{r['volume_mm3']:.2f}</td><td>{r['motion_s']:.2f}</td><td>{html.escape(r['status'])}</td><td>{html.escape(str(r.get('response_ratios',r.get('error',''))))}</td></tr>" for r in rows)
    page = f"""<!doctype html><html><meta charset="utf-8"><title>Cathedral experiment</title>
<style>body{{font:16px system-ui;max-width:1100px;margin:40px auto;padding:0 20px;color:#203040}}svg{{max-width:100%;height:auto}}table{{border-collapse:collapse;width:100%;font-size:13px}}td,th{{padding:10px;text-align:left;border-bottom:1px solid #ccd5df}}p{{max-width:80ch}}</style>
<h1>Cathedral: windows and pier ribs</h1><p>Four actual native constructions. Costs cover the complete object; numerical responses cover one finite cut web bay under assumed weak-Z elasticity. No global infill frontier, buckling or physical strength qualification. Motion is a decoded-feed lower bound.</p>
{(output/'comparison.svg').read_text()}<table><tr><th>Construction</th><th>Material (mm³)</th><th>Motion bound (s)</th><th>Numerics</th><th>Per-fixture ratios / flat</th></tr>{table}</table>
{(output/'sections.svg').read_text()}<img style="max-width:100%" src="bays.png" alt="Transparent nominal stroke prisms for each actual web bay"><p>Sections and transparent bays come from complete fresh actual-program replay. The opening and roof-root retention are constructed and checked by the engine, not by this renderer.</p></html>"""
    (output/"index.html").write_text(page)


if __name__ == "__main__":
    p=argparse.ArgumentParser(description=__doc__)
    for name in ("capture","archive","output"):
        p.add_argument("--"+name,type=Path,required=True)
    a=p.parse_args()
    render(a.capture,a.archive,a.output)
