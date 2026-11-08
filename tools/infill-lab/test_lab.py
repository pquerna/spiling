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
