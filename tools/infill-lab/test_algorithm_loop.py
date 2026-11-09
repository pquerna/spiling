# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Independent research admission negatives; no mechanical score goldens."""
from copy import deepcopy
import math
import pytest
from algorithm_judge import admit, context, accounting


@pytest.fixture
def baseline():
    rows = []
    for layer, z in enumerate((1.0, 1.2, 1.4, 12.0, 12.2)):
        for fixed in (0, 1):
            for c in (27.716815, 44.0, 60.283185):
                a, b = [20.901273, 20.901273, z], [67.098727, 67.098727, z]
                a[fixed] = b[fixed] = c
                rows.append(
                    dict(
                        start=a,
                        end=b,
                        width=0.45,
                        lower=z - 0.2,
                        upper=z,
                        kind=5,
                        owner=0,
                        layer=layer,
                        extrusion=1.0,
                    )
                )
    volume = sum(
        ((0.45 - 0.2) * 0.2 + math.pi * 0.2 * 0.2 / 4) * math.dist(r["start"], r["end"])
        for r in rows
    )
    return dict(roads=rows, report=dict(verified=True, deposited_volume_mm3=volume))


def test_identity_is_admitted_without_invented_native_status(baseline):
    rows = context(baseline)["sparse"]
    receipt = admit(baseline, rows)
    assert receipt["protected_roof_roads"] == 12
    assert receipt["protected_nodes"] > 0
    assert accounting(baseline, rows)["material_ratio"] == pytest.approx(1)


@pytest.mark.parametrize(
    "change,match",
    [
        (lambda rows: rows.pop(), "protected roof"),
        (lambda rows: rows.append(deepcopy(rows[0])), "duplicate"),
        (lambda rows: rows[0].update(width=0.46), "width"),
        (lambda rows: rows[0]["start"].__setitem__(0, float("nan")), "nonfinite"),
        (lambda rows: rows[0]["start"].__setitem__(1, 30.0), "pier"),
    ],
)
def test_bad_construction_cannot_score(baseline, change, match):
    rows = context(baseline)["sparse"]
    change(rows)
    with pytest.raises(ValueError, match=match):
        admit(baseline, rows)


def test_floating_reinforcement_does_not_gain_nominal_support(baseline):
    rows = context(baseline)["sparse"]
    r = deepcopy(rows[6])
    r["start"] = [32.0, 30.0, 1.2]
    r["end"] = [34.0, 30.0, 1.2]
    rows.append(r)
    with pytest.raises(ValueError, match="unsupported"):
        admit(baseline, rows)


@pytest.mark.parametrize("name", ["../escape", "/absolute", "has space", "a.b", ""])
def test_trial_names_cannot_escape_archive(name):
    from algorithm_loop import valid_name

    with pytest.raises(ValueError, match="trial name"):
        valid_name(name)


def test_frontier_keeps_small_gains_and_never_scores_failed_or_clipped_results():
    from algorithm_report import scores, pareto

    def row(name, material, stiffness, status="ok", excluded=False):
        return dict(
            name=name,
            volume_mm3=material,
            motion_proxy_s=10.0,
            stiffness_n_mm=dict(x=stiffness, z=stiffness, bend=stiffness),
            status=status,
            excluded=excluded,
        )

    rows = scores(
        [
            row("control", 10, 10),
            row("small_win", 10, 10.01),
            row("heavy", 11, 10),
            row("unknown", 1, 100, "failed"),
            row("clipped", 1, 100, excluded=True),
        ],
        "control",
    )
    assert pareto(rows) == ["small_win"]
    assert "geometric_mean_stiffness_ratio" not in rows[3]
    assert "geometric_mean_stiffness_ratio" not in rows[4]


@pytest.fixture
def rust_cohort(tmp_path, monkeypatch):
    import algorithm_loop as loop
    import subprocess

    repo = tmp_path / "source"
    repo.mkdir()
    for name in loop.EDITABLE:
        path = repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("// native generator\n")
    (repo / "verifier.rs").write_text("// independent verifier\n")
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "add", "."], cwd=repo, check=True)
    subprocess.run(
        [
            "git",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "frozen",
        ],
        cwd=repo,
        check=True,
    )
    monkeypatch.setattr(loop, "REPO", repo)
    original = subprocess.check_output

    def output(args, **kwargs):
        return (
            "rustc frozen test compiler\n"
            if args[0] == "rustc"
            else original(args, **kwargs)
        )

    monkeypatch.setattr(loop.subprocess, "check_output", output)
    candidates = tmp_path / "panel.jsonl"
    candidates.write_text('{"pattern":"web_roof","fraction":0.05}\n')
    root = tmp_path / "cohort"
    loop.freeze(root, candidates, ["cube48"])
    return loop, repo, root


def test_rust_trials_snapshot_edits_and_refuse_changed_independent_verifier(
    rust_cohort,
):
    loop, repo, root = rust_cohort
    control = loop.prepare(root, "control", None, "Native control")
    path = repo / loop.EDITABLE[0]
    path.write_text("// changed Rust construction\n")
    child = loop.prepare(root, "child", "control", "Change native geometry")
    assert child["parent"] == "control"
    assert child["rust_sources"] != control["rust_sources"]
    assert (
        root / "trials/child/source" / loop.EDITABLE[0]
    ).read_text() == path.read_text()
    (repo / "verifier.rs").write_text("// weakened independent verifier\n")
    with pytest.raises(ValueError, match="outside Rust generator"):
        loop.prepare(root, "invalid", "child", "Cannot change the judge")
    assert not (root / "trials/invalid").exists()


def test_failed_rust_build_keeps_source_and_never_claims_native_replay(
    rust_cohort, monkeypatch
):
    import lab, native, json

    loop, repo, root = rust_cohort
    loop.prepare(root, "broken", None, "Intentional compile failure")
    monkeypatch.setattr(lab, "bounded", lambda: None)

    def fail(*args, **kwargs):
        raise RuntimeError("Rust compiler rejected candidate")

    monkeypatch.setattr(native, "run", fail)
    result = loop.run(root, "broken")
    assert result["status"] == "failed" and not result["native_replayed"]
    assert "Rust compiler" in result["error"]
    assert (root / "trials/broken/source" / loop.EDITABLE[0]).exists()
    assert len((root / "events.jsonl").read_text().splitlines()) == 2
    with pytest.raises(ValueError, match="already evaluated"):
        loop.run(root, "broken")
    assert json.loads((root / "trials/broken/result.json").read_text()) == result


def test_python_generation_edits_and_unknown_parents_are_not_admitted(rust_cohort):
    loop, repo, root = rust_cohort
    with pytest.raises(ValueError, match="parent trial"):
        loop.prepare(root, "lost", "unknown", "Unknown lineage")
    (repo / "python_geometry.py").write_text("# alternate generator\n")
    with pytest.raises(ValueError, match="outside Rust generator"):
        loop.prepare(root, "python", None, "Cannot replace Rust generation")
    with pytest.raises(ValueError, match="committed baseline"):
        loop.freeze(root.parent / "dirty-freeze", root / "candidates.jsonl", ["cube48"])
    assert not (root.parent / "dirty-freeze").exists()
