# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0
import numpy as np
import pytest
from geometry import road_contains, mesh_from_voxels, voxelize_stadium
from mechanics import solve
from model import Candidate, Material


def road(a, b, width=0.45, lower=0.0, upper=0.2):
    return {
        "start": [*a, upper],
        "end": [*b, upper],
        "width": width,
        "lower": lower,
        "upper": upper,
    }


def test_global_contact_cannot_bond_a_remote_gap():
    a = road([0, 0], [10, 0])
    b = road([0, 0], [10, 0.625])
    # The roads genuinely meet at their origin, but the remote cell face is empty.
    assert road_contains(a, 0, 0, 0.1) and road_contains(b, 0, 0, 0.1)
    assert road_contains(a, 8, 0.2, 0.1) and road_contains(b, 8, 0.3125, 0.1)
    assert not road_contains(a, 8, 0.25625, 0.1)
    assert not road_contains(b, 8, 0.25625, 0.1)
    # Independent exact subset: every retained full cube must be inside one bead.
    g = voxelize_stadium([a, b], dx=0.1125, dz=0.1, phase=(0.03125, 0.03125))
    for cell in g.cells.T:
        xyz = g.points[:, cell]
        assert any(np.all(road_contains(r, *xyz)) for r in (a, b))


def test_edge_only_and_corner_only_contacts_do_not_share_dofs():
    body = np.zeros((2, 2, 2), dtype=bool)
    body[0, 0, 0] = body[1, 1, 1] = True
    g = mesh_from_voxels(body, [np.arange(3.0)] * 3)
    assert g.components == 2
    assert g.points.shape[1] == 16
    assert len(set(g.cells[:, 0]) & set(g.cells[:, 1])) == 0
    with pytest.raises(ValueError, match="disconnected"):
        solve(g)


def test_uniform_axial_units_and_energy():
    g = mesh_from_voxels(
        np.ones((2, 2, 4), bool),
        [np.linspace(0, 10, 3), np.linspace(0, 10, 3), np.linspace(0, 20, 5)],
    )
    r = solve(g)
    # E A delta / L = 2300 MPa * 100 mm2 * .02 mm / 20 mm = 230 N.
    assert r["reaction_n"] == pytest.approx(230.0, rel=2e-7)
    assert r["stiffness_n_mm"] == pytest.approx(11500.0, rel=2e-7)
    assert r["energy_error"] < 1e-7


def test_invalid_material_and_candidate_refuse():
    with pytest.raises(ValueError):
        Material(young_mpa=float("nan"))
    with pytest.raises(ValueError):
        Candidate(fraction=1.01)
    with pytest.raises(ValueError):
        Candidate(family="flanges", cut=0.3)
    with pytest.raises(ValueError):
        Candidate(family="separator", axis="alternating_xy")


def test_footprint_prisms_keep_real_gaps_and_phase_changes():
    from geometry import voxelize

    # Parallel disjoint footprints may never acquire a tie from global proximity.
    roads = [road([0, 0], [2, 0], width=0.4), road([0, 0.6], [2, 0.6], width=0.4)]
    for phase in [(0.0, 0.0), (0.037, 0.037)]:
        mesh = voxelize(roads, dx=0.1, dz=0.2, phase=phase)
        assert mesh.components == 2
        for cell in mesh.cells.T:
            y = mesh.points[1, cell]
            assert y.max() <= 0.2 + 1e-8 or y.min() >= 0.4 - 1e-8


def test_pareto_keeps_solid_only_when_its_response_earns_it():
    from lab import pareto

    def row(volume, seconds, k):
        return dict(
            status="ok",
            volume_mm3=volume,
            motion_s=seconds,
            mechanics={"stiffness_n_mm": k},
        )

    sparse = row(10, 10, 5)
    wasteful = row(20, 20, 4)
    solid = row(30, 30, 10)
    assert pareto([sparse, wasteful, solid, {"status": "refused_or_failed"}]) == [
        sparse,
        solid,
    ]
    assert pareto([sparse, dict(sparse)]) == [sparse, dict(sparse)]


def test_producer_identity_includes_checked_decoder_and_frozen_executables(tmp_path):
    from lab import identity

    for name in ("spiling-cli", "spiling-engine"):
        (tmp_path / name).write_text(name)
    value = identity(tmp_path)
    assert set(value) == {"judge", "binaries", "intent", "decoder"}
    assert set(value["decoder"]) == {"replay.ts", "display.ts", "generated.ts"}
    before = value["binaries"]["spiling-engine"]
    (tmp_path / "spiling-engine").write_text("different producer")
    assert identity(tmp_path)["binaries"]["spiling-engine"] != before


def test_grid_is_an_explicit_production_candidate_with_qualified_cadence():
    c = Candidate(pattern="grid", fraction=0.3)
    assert c.data()["pattern"] == "grid"
    with pytest.raises(ValueError):
        Candidate(pattern="gyroid")
    with pytest.raises(ValueError):
        Candidate(pattern="grid", combine_height=0.3)
    with pytest.raises(ValueError):
        Candidate(pattern="grid", family="separator", axis="x")


def test_source_fixture_datums_survive_an_inscribed_mesh():
    # The effective body can shrink with discretization; support/load positions
    # must still refer to the unchanged source specimen rather than its grid.
    g = mesh_from_voxels(
        np.ones((8, 2, 2), bool),
        [np.linspace(0.1, 11.9, 9), np.linspace(0, 5, 3), np.linspace(0, 4, 3)],
    )
    r = solve(g, "bend_z", fixture_bounds=[[0, 0, 0], [12, 5, 4]])
    assert r["fixture_bounds_mm"] == [[0.0, 0.0, 0.0], [12.0, 5.0, 4.0]]
    assert r["delta_mm"] == pytest.approx(0.004)
    assert r["energy_error"] < 1e-6


def test_one_elastic_body_handles_independent_loads_without_cross_material_reuse():
    from mechanics import prepare

    g = mesh_from_voxels(
        np.ones((2, 2, 4), bool),
        [np.linspace(0, 10, 3), np.linspace(0, 10, 3), np.linspace(0, 20, 5)],
    )
    body = prepare(g)
    for case, expected in [("compression_x", 46000.0), ("compression_z", 11500.0)]:
        r = solve(g, case, prepared=body)
        assert r["stiffness_n_mm"] == pytest.approx(expected, rel=1e-7)
        assert r["matrix_reused"] and r["assembly_wall_s"] == 0.0
    with pytest.raises(ValueError, match="another mesh/material"):
        solve(g, material=Material(young_mpa=1000.0), prepared=body)


def test_weak_z_material_is_reciprocal_positive_and_matches_axial_moduli():
    from mechanics import prepare, elastic_tensor

    material = Material(z_ratio=0.25)
    tensor = elastic_tensor(material)
    assert np.allclose(tensor, tensor.T)
    assert np.linalg.eigvalsh(tensor).min() > 0
    g = mesh_from_voxels(
        np.ones((2, 2, 4), bool),
        [np.linspace(0, 10, 3), np.linspace(0, 10, 3), np.linspace(0, 20, 5)],
    )
    body = prepare(g, material)
    assert solve(g, "compression_z", material, prepared=body)[
        "reaction_n"
    ] == pytest.approx(57.5, rel=1e-7)
    assert solve(g, "compression_x", material, prepared=body)[
        "stiffness_n_mm"
    ] == pytest.approx(46000.0, rel=1e-7)


def test_trial_timeout_stops_nested_work_in_the_owned_process_group(tmp_path):
    import sys
    from pathlib import Path
    from native import run

    worker = tmp_path / "worker.py"
    pid_file = tmp_path / "descendant.pid"
    worker.write_text(
        "import sys,subprocess\nfrom pathlib import Path\nsys.path.insert(0,"
        + repr(str(Path(__file__).resolve().parent))
        + ")\nfrom native import enter_trial_group,run\nenter_trial_group()\nrun([sys.executable,'-c',"
        + repr(
            "import os,time;from pathlib import Path;Path("
            + repr(str(pid_file))
            + ").write_text(str(os.getpid()));time.sleep(30)"
        )
        + "],timeout=20)\n"
    )
    with pytest.raises(TimeoutError):
        run([sys.executable, worker], timeout=0.5)
    pid = int(pid_file.read_text())
    stat = Path(f"/proc/{pid}/stat")
    # A killed grandchild can briefly await init's reap, but must not execute.
    assert not stat.exists() or stat.read_text().split()[2] == "Z"


def test_material_budget_proposals_use_only_measured_discrete_dose():
    from budget import refine_budget
    import math

    calls = []

    def measure(fraction):
        calls.append(fraction)
        return 100 + 10 * math.ceil(fraction * 10)

    rows = refine_budget(measure, 120, rounds=4)
    assert len(calls) == 4
    assert all(r["volume_mm3"] <= 120 for r in rows if r["within_budget"])

    def refused(_):
        raise RuntimeError("native authority unavailable")

    rows = refine_budget(refused, 120)
    assert len(rows) == 1 and rows[0]["status"] == "refused"


def test_summary_refuses_to_pool_changed_judges(tmp_path):
    import json
    from summary import read_study

    for i in (1, 2):
        p = tmp_path / "campaigns" / str(i)
        p.mkdir(parents=True)
        (p / "config.json").write_text(json.dumps({"identity": {"judge": i}}))
        (p / "trials.jsonl").write_text("")
    with pytest.raises(ValueError, match="one frozen judge"):
        read_study(tmp_path)
