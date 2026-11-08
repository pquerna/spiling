# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Consumer gates for topology-preserving independent element experiments."""

import os
from pathlib import Path
import numpy as np
import pytest
from geometry import mesh_from_prisms
from mechanics import elastic_tensor
from model import Material
from reference import beam_mesh, deck, element_mesh, parse_dat, solve_reference


def test_quadratic_edges_share_only_existing_node_identity():
    # Two coordinate-coincident prisms with separate ownership remain separate.
    xy = np.array([[0, 0], [1, 0], [0, 1], [0, 0], [1, 0], [0, 1]], float)
    m = mesh_from_prisms(
        xy,
        np.array([[0, 1, 2], [3, 4, 5]]),
        np.array([0.0, 1.0]),
        np.ones((2, 1), bool),
    )
    p, c = element_mesh(m, "C3D15")
    assert len(p) == 30 and c.shape == (2, 15)
    assert not set(c[0]) & set(c[1])
    for cell in c:
        assert np.allclose(p[cell[6]], (p[cell[0]] + p[cell[1]]) / 2)
    # Face-neighbor prisms share quadratic face nodes as well as vertices.
    m = beam_mesh(1)
    p, c = element_mesh(m, "C3D15")
    assert len(set(c[0]) & set(c[1])) == 8


def test_reference_deck_keeps_orthotropic_tensor_and_bounded_numeric_fields():
    m = beam_mesh(2)
    material = Material(z_ratio=0.25)
    text, conditions, loaded, amplitude, _, _ = deck(
        m, "C3D15", material, "compression_z", m.bounds
    )
    lines = text.splitlines()
    start = lines.index("*ELASTIC,TYPE=ORTHO")
    constants = [
        float(v) for line in lines[start + 1 : start + 3] for v in line.split(",")
    ]
    c = elastic_tensor(material)
    assert np.allclose(
        constants,
        [
            c[0, 0],
            c[0, 1],
            c[1, 1],
            c[0, 2],
            c[1, 2],
            c[2, 2],
            c[5, 5],
            c[4, 4],
            c[3, 3],
        ],
        rtol=1e-12,
    )
    assert all(
        len(field) <= 20
        for line in lines
        if not line.startswith("*")
        for field in line.split(",")
    )
    assert loaded and amplitude > 0 and len(conditions) > len(loaded)


def test_reference_rejects_missing_nonfinite_and_duplicate_reactions():
    conditions = {(0, 2): 0.0, (1, 2): -0.1}
    text = " forces (fx,fy,fz) for set FIXTURE and time 1\n\n 1 0 0 2\n 2 0 0 -2\n\n total internal energy for set BODY\n\n .1\n\n total volume for set BODY\n\n 1.\n"
    forces, e, v = parse_dat(text, conditions)
    assert len(forces) == 2 and e == 0.1 and v == 1
    for broken in (
        text.replace(" 2 0 0 -2\n", ""),
        text.replace(" 2 0 0 -2", " 1 0 0 -2"),
        text.replace(" 2 0 0 -2", " 2 nan 0 -2"),
    ):
        with pytest.raises(ValueError):
            parse_dat(broken, conditions)


@pytest.mark.skipif(
    not os.environ.get("SPILING_CCX"),
    reason="independent native solver explicitly selected by integration runner",
)
def test_solved_bending_reference_exposes_linear_error_and_quadratic_remedy(tmp_path):
    results = {}
    for element in ("C3D6", "C3D15", "C3D10", "C3D8I"):
        m = beam_mesh(2, element=element)
        results[element] = solve_reference(
            m,
            tmp_path / element,
            element=element,
            material=Material(),
            case="pure_bending",
            bounds=m.bounds,
            executable=Path(os.environ["SPILING_CCX"]).resolve(),
        )
    assert results["C3D6"]["analytical_relative_error"] > 1
    assert results["C3D15"]["analytical_relative_error"] < 1e-5
    assert results["C3D8I"]["analytical_relative_error"] < 1e-5
    assert results["C3D10"]["analytical_relative_error"] < 1e-5


def test_quadratic_tetrahedra_preserve_volume_and_shared_faces():
    mesh = beam_mesh(2, ny=2, nz=2)
    p, cells = element_mesh(mesh, "C3D10")
    corners = p[cells[:, :4]]
    det = np.einsum(
        "ij,ij->i",
        np.cross(corners[:, 1] - corners[:, 0], corners[:, 2] - corners[:, 0]),
        corners[:, 3] - corners[:, 0],
    )
    assert np.all(det > 0)
    assert det.sum() / 6 == pytest.approx(mesh.volume, rel=1e-12)
    # Every unmatched corner facet lies on the original box boundary.
    faces = np.sort(
        np.concatenate(
            [cells[:, ids] for ids in ((0, 1, 2), (0, 1, 3), (0, 2, 3), (1, 2, 3))]
        ),
        axis=1,
    )
    unique, counts = np.unique(faces, axis=0, return_counts=True)
    assert np.all(counts <= 2)
    for face in unique[counts == 1]:
        coords = p[face]
        assert any(
            np.allclose(coords[:, axis], value)
            for axis in range(3)
            for value in mesh.bounds[:, axis]
        )


def test_element_gate_requires_complete_positive_converged_responses():
    from element_report import comparison

    assert comparison(100.0, 104.0)["passed"]
    assert not comparison(100.0, 116.0)["passed"]
    for a, b in ((0.0, 1.0), (1.0, float("nan")), (1.0, -1.0)):
        with pytest.raises(ValueError):
            comparison(a, b)


def test_element_gate_cannot_promote_a_subset_without_analytical_controls():
    from element_report import analytical_gate

    assert not analytical_gate([])
    assert not analytical_gate(
        [dict(element="C3D15", case="pure_bending", required=True, passed=True)]
    )
    cases = [
        dict(
            element=e, case=c, required=c != "pure_bending" or e != "C3D6", passed=True
        )
        for e in ("C3D6", "C3D15", "C3D10", "C3D8I")
        for c in ("pure_bending", "compression_x", "compression_y", "compression_z")
    ]
    assert analytical_gate(cases)
    cases[-1]["passed"] = False
    assert not analytical_gate(cases)


def reference_panel(tmp_path, entries):
    """Bind a tiny retained panel as the collector's real file consumer does."""
    import json
    from model import digest, file_hash

    inputs, outputs = tmp_path / "inputs", tmp_path / "outputs"
    inputs.mkdir()
    outputs.mkdir()
    jobs = []
    judge = {"frozen": "judge"}
    for element, division, threads, status in entries:
        scenario = dict(
            element=element,
            case="compression_z",
            refinement=dict(divisions=division, max_z=1.0),
        )
        spec = dict(judge=judge, kind_source="actual_roads", scenarios=[scenario])
        key = digest(spec)
        folder, result = inputs / key, outputs / key
        folder.mkdir()
        result.mkdir()
        (folder / "case.json").write_text(json.dumps(spec))
        row = dict(
            scenario=scenario,
            judge=judge,
            input_sha256=file_hash(folder / "case.json"),
            status=status,
        )
        if status == "ok":
            row.update(
                stiffness_n_mm=100.0,
                components=[
                    dict(force_balance=0.0, energy_error=0.0, volume_relative_error=0.0)
                ],
            )
        (result / "trials.jsonl").write_text(json.dumps(row) + "\n")
        (result / "launch.json").write_text(
            json.dumps(
                dict(
                    removed=True,
                    backend="reference",
                    image=dict(id="frozen-image"),
                    blas_threads=threads,
                )
            )
        )
        jobs.append(dict(id=key, specimen="cube32", case=scenario))
    (inputs / "jobs.json").write_text(json.dumps(dict(judge=judge, jobs=jobs)))
    return inputs, outputs


def test_element_report_refuses_pooled_thread_profiles(tmp_path):
    from element_report import collect

    inputs, outputs = reference_panel(
        tmp_path, [("C3D15", 1, 1, "ok"), ("C3D15", 2, 4, "ok")]
    )
    with pytest.raises(ValueError, match="thread profiles"):
        collect(inputs, outputs)


def test_rejected_finest_required_mesh_cannot_promote_coarser_successes(tmp_path):
    from element_report import collect

    inputs, outputs = reference_panel(
        tmp_path,
        [("C3D15", 1, 1, "ok"), ("C3D15", 2, 1, "ok"), ("C3D15", 4, 1, "error")],
    )
    report = collect(inputs, outputs)
    check = next(
        c
        for c in report["response_checks"]
        if c["specimen"] == "cube32" and c["check"] == "quadratic_wedge_planar"
    )
    assert check["coarse"] == ("C3D15", 2, 1.0)
    assert check["fine"] == ("C3D15", 4, 1.0)
    assert not check["passed"] and report["status"] == "element_gate_open"
