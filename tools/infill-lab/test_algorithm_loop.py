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


def test_prepare_preserves_trial_name_and_archives_the_only_editable_file(
    tmp_path, monkeypatch, baseline
):
    import algorithm_loop as loop

    source = tmp_path / "source"
    source.mkdir()
    (source / "algorithm_judge.py").write_text("frozen judge")
    (source / "roof_algorithm_judge.py").write_text("frozen roof judge")
    (source / "interior_algorithm.py").write_text(
        'def generate(context): return context["sparse"]\n'
    )
    root = tmp_path / "cohort"
    (root / "trials").mkdir(parents=True)
    (root / ".auto").mkdir()
    loop.write(root / "baseline.json", baseline)
    loop.write(
        root / "freeze.json",
        dict(
            judge={
                name: loop.sha(source / name)
                for name in ("algorithm_judge.py", "roof_algorithm_judge.py")
            }
        ),
    )
    monkeypatch.setattr(loop, "HERE", source)

    def generated(*args, **kwargs):
        folder = root / "trials" / "control"
        loop.write(folder / "sparse.json", context(baseline)["sparse"])

    monkeypatch.setattr(loop.subprocess, "run", generated)
    assert loop.prepare(root, "control", None, "identity")["status"] == "admitted"
    case = __import__("json").loads((root / "trials/control/case.json").read_text())
    assert case["name"] == "control"
    assert case["algorithm_sha256"] == loop.sha(source / "interior_algorithm.py")
    (source / "roof_algorithm_judge.py").write_text("changed")
    with pytest.raises(ValueError, match="judge source edited"):
        loop.prepare(root, "changed", None, "invalid judge")


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


def test_failed_generation_remains_a_named_source_snapshot(
    tmp_path, monkeypatch, baseline
):
    import algorithm_loop as loop
    import subprocess, json

    root = tmp_path / "cohort"
    (root / "trials").mkdir(parents=True)
    (root / ".auto").mkdir()
    source = tmp_path / "source"
    source.mkdir()
    (source / "algorithm_judge.py").write_text("judge")
    (source / "interior_algorithm.py").write_text('raise RuntimeError("experiment")\n')
    loop.write(
        root / "freeze.json",
        dict(judge={"algorithm_judge.py": loop.sha(source / "algorithm_judge.py")}),
    )
    loop.write(root / "baseline.json", baseline)
    monkeypatch.setattr(loop, "HERE", source)

    def failed(*args, **kwargs):
        raise subprocess.CalledProcessError(1, ["candidate"])

    monkeypatch.setattr(loop.subprocess, "run", failed)
    receipt = loop.prepare(root, "broken", None, "intentional generation failure")
    assert receipt["phase"] == "generation" and receipt["status"] == "failed"
    assert (root / "trials/broken/algorithm.py").read_text() == (
        source / "interior_algorithm.py"
    ).read_text()
    assert json.loads((root / ".auto/log.jsonl").read_text())["name"] == "broken"


def test_dispatch_reserves_full_simultaneous_memory_and_cpu():
    from algorithm_loop import capacity

    gib = 1024**3
    assert capacity(120 * gib, 20) == 2
    assert capacity(64 * gib, 20) == 1
    assert capacity(32 * gib, 20) == 0
    assert capacity(120 * gib, 3) == 1
    assert capacity(120 * gib, 2) == 0
