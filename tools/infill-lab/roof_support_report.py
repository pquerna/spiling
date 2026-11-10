"""Pareto report over complete frozen research trials, never across judge revisions."""
from __future__ import annotations

import argparse
import html
import json
from pathlib import Path


def dominates(first, second):
    metrics = ("volume_ratio", "motion_ratio", "starts_ratio")
    return all(first[key] <= second[key] + 1e-12 for key in metrics) and any(first[key] < second[key] - 1e-12 for key in metrics)


def report(root):
    frozen = json.loads((root / "freeze.json").read_text())
    from model import file_hash
    digest = file_hash(root / "freeze.json")
    rows = [json.loads(line) for line in (root / "log.jsonl").read_text().splitlines()]
    if any(row["freeze_sha256"] != digest for row in rows):
        raise ValueError("cannot pool different frozen judges")
    training = [row for row in rows if row["status"] == "ok" and row["split"] == "train"]
    baseline = dict(name="native-control", policy={"family": "control"}, volume_ratio=1.0, motion_ratio=1.0, starts_ratio=1.0, cases=[])
    candidates = [baseline, *training]
    front = [row for row in candidates if not any(dominates(other, row) for other in candidates)]
    summary = {"schema": 1, "scope": frozen["scope"], "freeze_sha256": digest,
               "training_trials": len(training), "failed_trials": [row["name"] for row in rows if row["status"] != "ok"],
               "frontier": [{key: row[key] for key in ("name", "volume_ratio", "motion_ratio", "starts_ratio", "policy")} for row in front],
               "final_audit": [row for row in rows if row["split"] == "holdout"],
               "all_trials": rows}
    (root / "summary.json").write_text(json.dumps(summary, indent=2, allow_nan=False) + "\n")
    colors = {name: index for index, name in enumerate(sorted({row["policy"]["family"] for row in candidates}))}
    minimum_x = min(row["volume_ratio"] for row in candidates) - 0.012
    minimum_y = min(row["motion_ratio"] for row in candidates) - 0.02
    maximum_y = max(1.015, max(row["motion_ratio"] for row in candidates) + 0.01)
    def point(row):
        return (65 + 580 * (row["volume_ratio"] - minimum_x) / (1.015 - minimum_x),
                350 - 290 * (row["motion_ratio"] - minimum_y) / (maximum_y - minimum_y))
    marks = []
    for index in range(6):
        value_x = minimum_x + (1.015 - minimum_x) * index / 5
        value_y = minimum_y + (maximum_y - minimum_y) * index / 5
        marks.append(f'<text x="{65 + 116 * index}" y="374" text-anchor="middle">{value_x * 100:.1f}%</text>')
        marks.append(f'<text x="55" y="{354 - 58 * index}" text-anchor="end">{value_y * 100:.1f}%</text>')
    for row in candidates:
        horizontal, vertical = point(row)
        family = row["policy"]["family"]
        strong = row in front
        color = f'var(--chart-{colors[family] % 6 + 1})'
        title = html.escape(f'{row["name"]}: material {row["volume_ratio"]:.5f}, modeled motion {row["motion_ratio"]:.5f}, starts {row["starts_ratio"]:.5f}')
        marks.append(f'<circle cx="{horizontal:.2f}" cy="{vertical:.2f}" r="{7 if strong else 4}" fill="{color}" opacity="{1 if strong else .4}" stroke="var(--foreground)" stroke-width="{2 if strong else 0}"><title>{title}</title></circle>')
    ordered = sorted(front, key=lambda row: row["volume_ratio"])
    if len(ordered) > 1:
        points = " ".join(f"{point(row)[0]:.2f},{point(row)[1]:.2f}" for row in ordered)
        marks.insert(0, f'<polyline points="{points}" fill="none" stroke="var(--primary)" stroke-width="1.5"/>')
    table = []
    seen = set()
    for row in sorted(front, key=lambda entry: (entry["volume_ratio"], entry["motion_ratio"])):
        identity = tuple(round(row[key], 10) for key in ("volume_ratio", "motion_ratio", "starts_ratio"))
        if identity in seen:
            continue
        seen.add(identity)
        table.append(f'<tr><td>{html.escape(row["name"])}</td><td>{100*(1-row["volume_ratio"]):.2f}%</td><td>{100*(1-row["motion_ratio"]):.2f}%</td><td>{100*(1-row["starts_ratio"]):.2f}%</td></tr>')
    audit = summary["final_audit"]
    details = []
    for row in audit:
        for case in row["cases"]:
            if "case" in case:
                details.append(f'<tr><td>{html.escape(row["name"])}</td><td>{html.escape(case["case"])}</td><td>{100*(1-case["volume_ratio"]):.2f}%</td><td>{100*(1-case["motion_ratio"]):.2f}%</td><td>{case["max_roof_bridge_mm"]:.3f} mm</td></tr>')
    page = '''<!doctype html><html><head><meta charset="utf-8"><style>
:root{font-family:var(--font-sans)}body{margin:0;color:var(--foreground)}svg{width:100%;height:420px}svg text{fill:var(--muted-foreground);font:12px var(--font-sans)}p{line-height:1.45}small{color:var(--muted-foreground)}table{border-collapse:collapse;width:100%;font-size:12px;margin:12px 0}td,th{text-align:left;padding:8px 4px;border-bottom:1px solid var(--border)}h3{font-size:16px;margin:18px 0 6px}.note{padding:16px;background:var(--secondary);border-radius:var(--radius);font-size:13px}.legend{display:flex;flex-wrap:wrap;gap:12px;font-size:12px}.key{width:9px;height:9px;border-radius:50%;display:inline-block;margin-right:5px}
</style></head><body>'''
    page += f'<h3>Cathedral roof-support research · {len(training)} measured trials</h3><p><strong>Lower-left is better.</strong> Complete-object material and ideal filtered-route motion, normalized per native case then geometrically averaged. Outlined points are nondominated in material, motion and starts.</p>'
    page += '<svg viewBox="0 0 710 420" role="img" aria-label="Pareto frontier of Cathedral material and ideal motion"><path d="M65 55V350H650" fill="none" stroke="var(--border)"/>' + "".join(marks) + '<text x="355" y="406" text-anchor="middle">Complete-object deposited material (% of native control)</text><text transform="translate(16 215) rotate(-90)" text-anchor="middle">Ideal modeled motion (% of control)</text></svg>'
    page += '<div class="legend">' + "".join(f'<span><i class="key" style="background:var(--chart-{index % 6 + 1})"></i>{html.escape(family)}</span>' for family, index in colors.items()) + '</div>'
    page += '<table><thead><tr><th>Pareto policy</th><th>Material saved</th><th>Modeled time saved</th><th>Starts saved</th></tr></thead><tbody>' + "".join(table) + '</tbody></table>'
    page += '<div class="note">Frozen hard gates: retain every web/seed/joint, shell and skin; analytic complete-stroke lower support at actual physical Z; no new unauthorized bridge process or worsened inherited warning. No strength or machine-time claim. Modified subsets are research candidates, <strong>not native-replayed production artifacts</strong>. Lifecycle, acceleration, pressure and future routing changes are excluded from the motion model.</div>'
    page += '<h3>Reserved-shape final audit</h3><table><thead><tr><th>Finalist</th><th>Reserved case</th><th>Material saved</th><th>Modeled time saved</th><th>Worst roof bridge</th></tr></thead><tbody>' + "".join(details) + '</tbody></table>'
    page += f'<small>Judge {digest[:16]} · {len(summary["failed_trials"])} retained failed trials · ' + html.escape(", ".join(summary["failed_trials"]) or "no candidate failures") + '</small></body></html>'
    (root / "frontier.html").write_text(page)
    print(json.dumps({"frontier": summary["frontier"], "training_trials": len(training), "audits": len(audit)}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    report(parser.parse_args().root.resolve())
