# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Independent library assembly, mode and external-response controls."""

import os
from pathlib import Path

import numpy as np
import pytest
from scipy import sparse
from skfem import Basis, BilinearForm, ElementTetP2, ElementVector, MeshTet, asm

from mechanics import elastic_tensor
from model import Material
from quadratic import prepare, solve_body
from reference import beam_mesh, solve_reference


def test_chunked_matrix_matches_independent_library_assembly_on_nonuniform_cells():
    m = beam_mesh(2, ny=2)
    m.points[0] = m.points[0] ** 1.2
    m.bounds = np.array([m.points.min(axis=1), m.points.max(axis=1)])
    m.volume = np.prod(m.bounds[1] - m.bounds[0])
    material = Material(z_ratio=0.25)
    body = prepare(m, material, chunk=1)
    p, t = body.mesh.points, body.mesh.cells[:4]
    mesh = MeshTet(p[:, : m.points.shape[1]], t, sort_t=False)
    basis = Basis(mesh, ElementVector(ElementTetP2()), intorder=2)
    tensor = elastic_tensor(material)

    @BilinearForm
    def form(u, v, w):
        def strain(q):
            g = q.grad
            return np.array(
                [
                    g[0, 0],
                    g[1, 1],
                    g[2, 2],
                    g[1, 2] + g[2, 1],
                    g[0, 2] + g[2, 0],
                    g[0, 1] + g[1, 0],
                ]
            )

        return np.einsum("i...,ij,j...->...", strain(v), tensor, strain(u))

    expected = asm(form, basis).tocsr()
    # Match the independent library's global edge numbering for this synthetic
    # box only. Production assembly never welds by physical coordinates.
    lookup = {tuple(np.round(q, 12)): i for i, q in enumerate(p.T)}
    ids = [
        3 * lookup[tuple(np.round(basis.doflocs[:, i], 12))] + i % 3
        for i in range(basis.N)
    ]
    mapping = sparse.coo_matrix(
        (np.ones(basis.N), (ids, np.arange(basis.N))), shape=(basis.N,) * 2
    ).tocsr()
    difference = body.stiffness.tocsr() - mapping @ expected @ mapping.T
    assert abs(difference.data).max() < abs(expected.data).max() * 1e-12
    # Affine rigid motions must carry no strain energy.
    for axis in range(3):
        u = np.cross(np.eye(3)[axis], p.T).ravel()
        assert (
            np.linalg.norm(body.stiffness @ u)
            < np.linalg.norm(body.stiffness.data) * 1e-10
        )


def test_quadratic_solved_bending_and_weak_z_axial_controls():
    mesh = beam_mesh(2)
    material = Material(z_ratio=0.25)
    body = prepare(mesh, material)
    result = solve_body(body, "compression_z", mesh.bounds, solver="superlu_mmd")
    assert result["stiffness_n_mm"] == pytest.approx(
        material.young_mpa * 0.25 * mesh.volume / 0.2**2, rel=1e-9
    )
    bending = solve_body(prepare(mesh, Material()), "pure_bending", mesh.bounds)
    assert bending["analytical_relative_error"] < 1e-7
    amg = solve_body(body, "compression_z", mesh.bounds, solver="amg")
    assert amg["stiffness_n_mm"] == pytest.approx(result["stiffness_n_mm"], rel=1e-7)


@pytest.mark.skipif(
    not os.environ.get("SPILING_CCX"), reason="external solver selected explicitly"
)
def test_quadratic_matches_external_c3d10_response(tmp_path):
    mesh = beam_mesh(3, ny=2, nz=2)
    material = Material(z_ratio=0.25)
    ref = solve_reference(
        mesh,
        tmp_path / "ccx",
        element="C3D10",
        material=material,
        case="compression_z",
        bounds=mesh.bounds,
        executable=Path(os.environ["SPILING_CCX"]).resolve(),
    )
    own = solve_body(
        prepare(mesh, material), "compression_z", mesh.bounds, solver="amg"
    )
    assert own["stiffness_n_mm"] == pytest.approx(ref["stiffness_n_mm"], rel=1e-6)


def test_rejected_external_result_cannot_seed_a_parity_study(tmp_path):
    import json
    from model import digest, file_hash
    from quadratic import JUDGE
    from quadratic_study import pack

    inputs, results, context = (tmp_path / n for n in ("inputs", "results", "context"))
    for p in (inputs, results, context):
        p.mkdir()
    for name in JUDGE:
        (context / name).write_text("frozen source")
    scenario = dict(element="C3D10", case="compression_z", refinement=dict(divisions=1))
    target = dict(judge={}, scenarios=[scenario])
    key = digest(target)
    (inputs / key).mkdir()
    (results / key).mkdir()
    path = inputs / key / "case.json"
    path.write_text(json.dumps(target))
    (inputs / "jobs.json").write_text(
        json.dumps(
            dict(judge={}, jobs=[dict(id=key, specimen="cube32", case=scenario)])
        )
    )
    (results / key / "trials.jsonl").write_text(
        json.dumps(
            dict(
                status="failed",
                scenario=scenario,
                judge={},
                input_sha256=file_hash(path),
            )
        )
        + "\n"
    )
    with pytest.raises(ValueError, match="rejected/mismatched"):
        pack(inputs, results, context, tmp_path / "out")


def test_auxiliary_space_keeps_quadratic_physical_response_and_symmetric_action():
    from quadratic import kernels
    from quadratic_preconditioner import auxiliary_preconditioner

    mesh = beam_mesh(3, ny=2, nz=2)
    body = prepare(mesh, Material(z_ratio=0.25))
    for case in ("compression_x", "compression_y", "compression_z"):
        expected = solve_body(body, case, mesh.bounds, solver="superlu_mmd")
        actual = solve_body(body, case, mesh.bounds, solver="p_amg")
        assert actual["stiffness_n_mm"] == pytest.approx(
            expected["stiffness_n_mm"], rel=1e-7
        )
    # The coarse solve is a preconditioner, not a replacement physical mesh.
    # Symmetric positive action is needed by the existing CG solver.
    matrix = (body.stiffness + 10 * sparse.eye(body.basis.N)).tobsr(blocksize=(3, 3))
    np.random.seed(0)
    action = auxiliary_preconditioner(body, kernels(body.material)[1])(
        matrix, np.ones(body.basis.N), np.array([], int)
    )
    x, y = np.random.default_rng(0).normal(size=(2, body.basis.N))
    assert x @ (action @ x) > 0
    assert x @ (action @ y) == pytest.approx(y @ (action @ x), rel=1e-9)


def test_incomplete_quadratic_runs_retain_resource_failure_and_finest_obligation(
    tmp_path,
):
    import json
    from quadratic_report import collect

    inputs, outputs = tmp_path / "inputs", tmp_path / "outputs"
    inputs.mkdir()
    jobs = []
    for division in (4, 8):
        key = str(division)
        jobs.append(
            dict(
                id=key,
                specimen="beam80",
                solver="p_amg",
                case=dict(refinement=dict(divisions=division)),
            )
        )
        folder = outputs / key
        folder.mkdir(parents=True)
        (folder / "launch.json").write_text(
            json.dumps(dict(failure="container_exit", removed=True, docker_exit=137))
        )
        (folder / "container.json").write_text(
            json.dumps([dict(State=dict(OOMKilled=True))])
        )
    (inputs / "jobs.json").write_text(json.dumps(dict(jobs=jobs, judge={})))
    report = collect(inputs, outputs)
    assert report["accepted"] == 0 and report["completed"] == 0
    assert report["missing"] == ["4", "8"]
    assert all(r["oom_killed"] and r["removed"] for r in report["incomplete"])
    beam = next(r for r in report["refinement_checks"] if r["specimen"] == "beam80")
    assert beam["coarse"] == 4 and beam["fine"] == 8
    assert not beam["passed"] and beam["reason"] == "missing_or_failed_required_mesh"


def test_optional_field_capture_preserves_solve_and_conserves_independent_energy():
    material = Material(z_ratio=0.25)
    body = prepare(beam_mesh(3, ny=2, nz=2), material)
    ordinary = solve_body(body, "compression_z", body.mesh.bounds, solver="superlu_mmd")
    captured = solve_body(
        body, "compression_z", body.mesh.bounds, solver="superlu_mmd", field_bins=4
    )
    assert "fields" not in ordinary
    assert captured["stiffness_n_mm"] == pytest.approx(
        ordinary["stiffness_n_mm"], rel=1e-12
    )
    fields = captured["fields"]
    energy = np.asarray(fields["energy_n_mm"])
    volume = np.asarray(fields["volume_mm3"])
    assert energy.shape == (4, 4, 4)
    assert energy.sum() == pytest.approx(captured["strain_energy_n_mm"], rel=1e-10)
    assert volume.sum() == pytest.approx(body.mesh.volume, rel=1e-10)
    # Uniform uniaxial response has constant energy density despite nonuniform
    # tetrahedron/bin counts. This checks localization against an analytic law.
    density = 0.5 * material.young_mpa * material.z_ratio * 0.001**2
    assert energy == pytest.approx(volume * density, rel=1e-7, abs=1e-15)
    assert fields["displacement_min_mm"][2] == pytest.approx(-0.0002)
    for bins in (True, 0, 33, 4.5):
        with pytest.raises(ValueError, match="field bins"):
            solve_body(body, "compression_z", body.mesh.bounds, field_bins=bins)


def test_field_capture_rejects_inconsistent_energy_and_displacement():
    from quadratic_fields import energy_fields

    body = prepare(beam_mesh(2))
    with pytest.raises(ValueError, match="displacement inventory"):
        energy_fields(body, np.zeros(body.basis.N - 1), 1.0)
    with pytest.raises(ValueError, match="localized/global energy"):
        energy_fields(body, np.zeros(body.basis.N), 1.0)
