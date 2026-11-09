# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Cost reports must retain the native panel and reject changed evidence."""

import json
import math

import pytest

from model import digest, file_hash
from vault_report import audit


def panel(root):
    identity = {"binaries": {"spiling-cli": "cli", "spiling-engine": "engine"}}
    candidates = [
        dict(fraction=1, pattern="rectilinear"),
        dict(fraction=0.1, pattern="grid"),
        dict(fraction=0.1, pattern="web_roof"),
    ]
    capture = dict(
        schema=1, identity=identity, specimens=["test"], candidates=candidates
    )
    (root / "capture.json").write_text(json.dumps(capture))
    rows = []
    for c in candidates:
        key = digest(dict(candidate=c, specimen="test", identity=identity))
        relative = "native/" + key[:24]
        folder = root / relative
        folder.mkdir(parents=True)
        volume = ((0.45 - 0.2) * 0.2 + math.pi * 0.2**2 / 4) * 10
        data = dict(
            software_only=True,
            candidate=c,
            source_sha256="source",
            bin_sha256=identity["binaries"],
            motion_seconds_lower_bound=1.0,
            compile_wall_s=2.0,
            report=dict(verified=True, deposited_volume_mm3=volume),
            roads=[
                dict(
                    start=[0, 0, 0.2],
                    end=[10, 0, 0.2],
                    lower=0,
                    upper=0.2,
                    width=0.45,
                    kind=5,
                    owner=0,
                )
            ],
        )
        (folder / "roads.json").write_text(json.dumps(data))
        for name in ("actual.splr", "receipt.json", "project.spil"):
            (folder / name).write_bytes(b"test closure bytes")
        manifest = {p.name: file_hash(p) for p in folder.iterdir()}
        (folder / "manifest.json").write_text(json.dumps(manifest))
        rows.append(
            dict(
                id=key,
                specimen="test",
                candidate=c,
                status="ok",
                native=relative,
                source_sha256="source",
                roads_sha256=manifest["roads.json"],
                replay_sha256=manifest["actual.splr"],
                volume_mm3=volume,
            )
        )
    (root / "captures.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    return rows


def test_complete_matched_panel_keeps_cost_ties_without_strength_claim(tmp_path):
    panel(tmp_path)
    result = audit(tmp_path)
    assert result["mechanical_ranking"].startswith("blocked_")
    assert len(result["rows"]) == 3
    assert result["rows"][-1]["material_vs_grid"] == 1
    assert result["rows"][-1]["motion_vs_grid"] == 1


@pytest.mark.parametrize("change", ["missing", "duplicate", "changed_replay"])
def test_changed_or_incomplete_panel_cannot_become_comparison(tmp_path, change):
    rows = panel(tmp_path)
    path = tmp_path / "captures.jsonl"
    if change == "missing":
        path.write_text(json.dumps(rows[0]) + "\n")
    elif change == "duplicate":
        path.write_text(path.read_text() + json.dumps(rows[0]) + "\n")
    else:
        (tmp_path / rows[0]["native"] / "actual.splr").write_bytes(b"changed")
    with pytest.raises(ValueError):
        audit(tmp_path)
