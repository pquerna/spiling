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


def test_metric_template_assembly_matches_library_on_nonuniform_cells():
    from mechanics import prepare
    from skfem import MeshHex, Basis, ElementHex1, ElementVector, asm, BilinearForm
    from skfem.helpers import sym_grad
    from mechanics import elastic_tensor

    g = mesh_from_voxels(
        np.ones((2, 2, 3), bool),
        [
            np.array([0.0, 0.2, 1.3]),
            np.array([0.0, 0.45, 2.0]),
            np.array([0.0, 0.2, 0.8, 1.0]),
        ],
    )
    basis = Basis(
        MeshHex(g.points, g.cells, sort_t=False),
        ElementVector(ElementHex1()),
        intorder=2,
    )
    for material in [Material(), Material(z_ratio=0.25)]:
        tensor = elastic_tensor(material)

        def strain(u):
            e = sym_grad(u)
            return np.array(
                [e[0, 0], e[1, 1], e[2, 2], 2 * e[1, 2], 2 * e[0, 2], 2 * e[0, 1]]
            )

        @BilinearForm
        def elasticity(u, v, w):
            return np.einsum("i...,ij,j...->...", strain(v), tensor, strain(u))

        expected = asm(elasticity, basis).tocsr()
        actual = prepare(g, material).stiffness
        assert np.max(abs((actual - expected).data)) < 1e-9
        assert np.max(abs((actual - actual.T).data)) < 1e-9


def test_prism_mesh_preserves_gaps_and_only_merges_identical_sections():
    from geometry import prism_mesh

    a = road([0, 0], [40, 0], width=0.4, lower=0, upper=0.2)
    b = road([0, 0.7], [40, 0.7], width=0.4, lower=0, upper=0.2)
    layers = [
        dict(r, lower=k * 0.2, upper=(k + 1) * 0.2) for k in range(10) for r in (a, b)
    ]
    g, info = prism_mesh(layers, max_xy=1, max_z=1, divisions=1)
    assert g.components == 2
    assert info["physical_slabs"] == 10 and info["mesh_slabs"] == 2
    assert info["retained_volume_fraction"] >= 0.97
    for cell in g.cells.T:
        y = g.points[1, cell]
        assert y.max() <= 0.2 + 1e-8 or y.min() >= 0.5 - 1e-8


def test_geometric_score_requires_complete_positive_weighted_results():
    from sweep import geometric_score

    assert geometric_score([0, 1], [0.5, 0.5]) == 0
    assert geometric_score([0.25, 1], [0.5, 0.5]) == pytest.approx(0.5)
    assert geometric_score([0.5, 0.5], [0.5, 0.5]) == pytest.approx(0.5)
    for values, weights in [
        ([-1, 1], [0.5, 0.5]),
        ([1], [0.5, 0.5]),
        ([1, 1], [0.4, 0.4]),
        ([float("nan")], [1]),
    ]:
        with pytest.raises(ValueError):
            geometric_score(values, weights)


def test_normalized_panel_does_not_reward_missing_failed_cases_or_hide_cost():
    from sweep import aggregate, frontier

    solid = Candidate(fraction=1).data()
    grid = Candidate(pattern="grid").data()

    def row(c, s, k, v):
        return {
            "candidate": c,
            "specimen": s,
            "status": "ok",
            "volume_mm3": v,
            "motion_s": v,
            "mechanics": {"case": "compression_z", "stiffness_n_mm": k},
        }

    tests = [("a", "compression_z", 0.5), ("b", "compression_z", 0.5)]
    rows = [
        row(solid, "a", 100, 100),
        row(solid, "b", 200, 200),
        row(grid, "a", 50, 40),
    ]
    result = aggregate(rows, tests=tests)
    assert len(result["points"]) == 1 and len(result["incomplete"]) == 1
    result = aggregate(rows + [row(grid, "b", 100, 80)], tests=tests)
    g = next(r for r in result["points"] if r["candidate"]["pattern"] == "grid")
    assert g["score"] == pytest.approx(0.5) and g["material_ratio"] == pytest.approx(
        0.4
    )
    assert len(frontier(result["points"])) == 2
    with pytest.raises(ValueError, match="duplicate"):
        aggregate(rows + [rows[0]], tests=tests)


def test_replay_bridge_ownership_keeps_skin_cost_out_of_interior():
    from sweep import road_roles
    import math

    r = dict(road([0, 0], [1, 0]), kind=8, owner="mandatory_skin")
    dose = (0.45 - 0.2) * 0.2 + math.pi * 0.2 * 0.2 / 4
    data = {"roads": [r], "report": {"deposited_volume_mm3": dose}}
    assert road_roles(data)["skins_mm3"] == pytest.approx(dose)
    r["owner"] = 0
    assert road_roles(data)["interior_mm3"] == pytest.approx(dose)
    data["report"]["deposited_volume_mm3"] = dose * 2
    with pytest.raises(ValueError, match="reconcile"):
        road_roles(data)


def test_conforming_prism_solver_and_metric_assembly_match_independent_library():
    from geometry import mesh_from_prisms
    from mechanics import prepare, elastic_tensor, prism_quadrature
    from skfem import MeshWedge1, ElementWedge1, ElementVector, Basis, BilinearForm, asm
    from skfem.helpers import sym_grad

    xy = np.array([[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]])
    tri = np.array([[0, 1, 2], [0, 2, 3]])
    g = mesh_from_prisms(xy, tri, np.linspace(0, 20, 5), np.ones((2, 4), bool))
    assert (
        g.components == 1
        and g.volume == pytest.approx(2000)
        and g.points.shape[1] == 20
    )
    basis = Basis(
        MeshWedge1(g.points, g.cells, sort_t=False),
        ElementVector(ElementWedge1()),
        quadrature=prism_quadrature(4),
    )
    for material in [Material(), Material(z_ratio=0.25)]:
        body = prepare(g, material)
        tensor = elastic_tensor(material)

        def strain(u):
            e = sym_grad(u)
            return np.array(
                [e[0, 0], e[1, 1], e[2, 2], 2 * e[1, 2], 2 * e[0, 2], 2 * e[0, 1]]
            )

        @BilinearForm
        def elasticity(u, v, w):
            return np.einsum("i...,ij,j...->...", strain(v), tensor, strain(u))

        reference = asm(elasticity, basis).tocsr()
        assert np.max(abs((body.stiffness - reference).data)) < 1e-9
        r = solve(g, "compression_z", material, prepared=body)
        assert r["reaction_n"] == pytest.approx(230 * material.z_ratio, rel=1e-7)


def test_prism_point_contacts_never_tie_disconnected_bodies():
    from geometry import mesh_from_prisms

    xy = np.array([[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]])
    tri = np.array([[0, 1, 2], [0, 3, 4]])
    g = mesh_from_prisms(xy, tri, np.array([0.0, 0.2]), np.ones((2, 1), bool))
    assert g.components == 2 and g.points.shape[1] == 12
    assert not set(g.cells[:, 0]) & set(g.cells[:, 1])


def test_prism_quadrature_integrates_known_volume_and_mixed_moments():
    from mechanics import prism_quadrature

    for order in [2, 4]:
        (x, y, z), w = prism_quadrature(order)
        assert w.sum() == pytest.approx(0.5)
        assert np.all(x >= 0) and np.all(y >= 0) and np.all(x + y <= 1)
        assert (w * x * y * z).sum() == pytest.approx(1 / 48)
        assert (w * x * x * z * z).sum() == pytest.approx(1 / 36)


def test_prism_face_ownership_survives_nearby_boundaries_and_preserves_gaps():
    from geometry import prism_mesh

    outline = [([0, 0], [8, 0]), ([8, 0], [8, 8]), ([8, 8], [0, 8]), ([0, 8], [0, 0])]
    roads = []
    for k in range(2):
        roads.extend(
            road(
                a,
                b,
                width=0.4500625 if k == 0 else 0.45,
                lower=0.2 * k,
                upper=0.2 * (k + 1),
            )
            for a, b in outline
        )
        a, b = ([4, 0], [4, 8]) if k == 0 else ([0, 4], [8, 4])
        roads.append(road(a, b, lower=0.2 * k, upper=0.2 * (k + 1)))
    g, info = prism_mesh(roads, max_xy=2, max_z=1)
    assert g.components == 1
    assert info["retained_volume_fraction"] > 1 - 1e-6
    separated = [road([0, 0], [8, 0]), road([0, 0.7], [8, 0.7])]
    g, info = prism_mesh(separated, max_xy=2, max_z=1)
    assert g.components == 2


def test_nested_dissection_direct_reference_matches_analytic_and_iterative_solve():
    from mechanics import prepare

    g = mesh_from_voxels(
        np.ones((8, 2, 4), bool),
        [np.linspace(0, 12, 9), np.linspace(0, 10, 3), np.linspace(0, 20, 5)],
    )
    body = prepare(g, Material(z_ratio=0.25))
    order, _ = body.direct_order
    assert np.array_equal(np.sort(order), np.arange(body.basis.N))
    for case in ["compression_x", "compression_z", "bend_z"]:
        direct = solve(g, case, Material(z_ratio=0.25), prepared=body, solver="superlu")
        iterative = solve(g, case, Material(z_ratio=0.25), prepared=body)
        assert direct["stiffness_n_mm"] == pytest.approx(
            iterative["stiffness_n_mm"], rel=1e-7
        )


def test_finite_coupon_clipping_keeps_topology_and_reports_completed_zero():
    from geometry import prism_mesh
    from mechanics import solve_coupon
    from campaign import bounds

    roads = [road([0, 2, 1], [4, 2, 1], width=1, lower=0, upper=1)]
    coupon = [[0, 0, 0], [4, 4, 1]]
    mesh, _ = prism_mesh(roads, window=coupon, max_xy=1, max_z=0.5)
    assert mesh.bounds[0, 0] == 0 and mesh.bounds[1, 0] == 4
    result = solve_coupon(mesh, Material(z_ratio=0.25), coupon)
    cases = {r["case"]: r for r in result["cases"]}
    assert cases["compression_x"]["stiffness_n_mm"] > 0
    assert cases["compression_z"]["stiffness_n_mm"] > 0
    assert cases["compression_y"]["stiffness_n_mm"] == 0
    assert cases["compression_y"]["mechanism"] == "no_component_spans_fixture_faces"
    assert result["stiffness_n_mm"] == 0
    assert bounds("cube32") == [[22, 22, 12], [50, 50, 20]]


def test_reinforcement_queue_uses_admitted_single_interface_recipes():
    from campaign import candidates

    controls = list(candidates())
    assert len({c.identity for c in controls}) == len(controls)
    regional = [c for c in controls if c.family != "uniform"]
    assert len(regional) == 6
    assert {c.family for c in regional} == {"separator", "crown"}
    with pytest.raises(ValueError):
        Candidate(family="webs")


def test_coupon_report_keeps_physical_failures_and_cut_scenarios_separate():
    from cohort_report import physical_gate, scenario_key
    from campaign import bounds

    scenario = {
        "material": {"z_ratio": 0.25},
        "bounds": bounds("cube32"),
        "divisions": 1,
    }
    original = scenario_key({"specimen": "cube32", "scenario": scenario})
    shifted = {**scenario, "bounds": bounds("cube32", shift=1)}
    assert scenario_key({"specimen": "cube32", "scenario": shifted}) != original
    invented = {
        "mechanics": {
            "scope": "actual_interior_coupon",
            "cases": [
                {
                    "case": "compression_" + a,
                    "stiffness_n_mm": 0,
                    "components": [],
                    "mechanism": "solver_failed",
                    "spanning_components": 0,
                }
                for a in "xyz"
            ],
        }
    }
    with pytest.raises(ValueError, match="witness"):
        physical_gate(invented)


def test_coupon_subset_reports_only_its_selected_native_refusals(tmp_path):
    import json
    from campaign import bounds
    from cohort_report import collect

    root, inputs, outputs = (tmp_path / n for n in ("study", "inputs", "outputs"))
    for path in (root, inputs, outputs, inputs / "case"):
        path.mkdir(parents=True, exist_ok=True)
    (root / "capture.json").write_text(json.dumps({"specimens": ["cube32"]}))
    # There is deliberately no study-wide captures.jsonl: this panel's manifest
    # owns selected refusals, rather than unrelated candidates in the study.
    (inputs / "jobs.json").write_text(
        json.dumps({"jobs": [{"id": "case"}], "failed_captures": []})
    )
    (inputs / "case" / "case.json").write_text(json.dumps({
        "candidate": Candidate(fraction=1).data(),
        "specimen": "cube32",
        "scenarios": [{"bounds": bounds("cube32"), "material": {"z_ratio": .25}}],
    }))
    report = collect(root, [inputs], outputs)
    assert report["missing_evaluations"] == 1
    rows = next(iter(report["groups"].values()))["rows"]
    assert len(rows) == 1 and rows[0]["status"] == "missing_or_interrupted"


def test_finer_conforming_prisms_preserve_volume_and_face_connectivity():
    from geometry import prism_mesh

    roads = [{"start": [0, 0, .2], "end": [2, 0, .2],
              "lower": 0, "upper": .2, "width": .4}]
    meshes = [prism_mesh(roads, divisions=d)[0] for d in (1, 2, 4, 8)]
    for i, mesh in enumerate(meshes):
        assert mesh.volume == pytest.approx(meshes[0].volume, rel=1e-12)
        assert mesh.components == 1
        assert mesh.count == meshes[0].count * 4**i
