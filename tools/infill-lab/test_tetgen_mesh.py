# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Material and contact counterexamples for offline volume remeshing."""

import numpy as np
import pytest

from tetgen_mesh import (
    boundary_inventory,
    conserve_boundary,
    face_mesh,
    material_containment,
    oriented_cells,
)


def test_only_full_faces_join_displacements_not_a_shared_point_or_edge():
    p = np.array(
        [
            [0, 0, 0],
            [1, 0, 0],
            [0, 1, 0],
            [0, 0, 1],
            [-1, 0, 0],
            [0, -1, 0],
            [0, 0, -1],
        ],
        dtype=float,
    )
    point = face_mesh(p, np.array([[0, 1, 2, 3], [0, 4, 5, 6]]))
    assert point.components == 2
    assert point.points.shape[1] == 8
    edge = face_mesh(p, np.array([[0, 1, 2, 3], [0, 1, 5, 6]]))
    assert edge.components == 2
    assert edge.points.shape[1] == 8
    face = face_mesh(p, np.array([[0, 1, 2, 3], [0, 2, 1, 6]]))
    assert face.components == 1
    assert face.points.shape[1] == 5
    assert face.volume == pytest.approx(1 / 3)


def test_degenerate_duplicate_nonmanifold_and_changed_volume_refuse():
    p = np.array(
        [[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1], [0, 0, -1], [0, 0, 2]], dtype=float
    )
    for cells in (
        [[0, 1, 2, 2]],
        [[0, 1, 2, 3], [1, 0, 2, 3]],
        [[0, 1, 2, 3], [0, 1, 2, 4], [0, 1, 2, 5]],
    ):
        with pytest.raises(ValueError):
            face_mesh(p, np.array(cells))
    with pytest.raises(ValueError, match="volume"):
        face_mesh(p, np.array([[0, 1, 2, 3]]), expected_volume=1.0)


def test_containment_checks_complete_clipped_tets_not_centroids_or_volume_only():
    p = np.array([[0, 0, 0], [1, 0, 0], [0, 0.6, 0], [0, 0, 0.2]], dtype=float)
    cells = np.array([[0, 1, 2, 3]])
    roads = [
        dict(
            start=[-1.0, 0.0, 0.2], end=[2.0, 0.0, 0.2], width=0.5, lower=0.0, upper=0.2
        )
    ]
    bounds = [[-1.0, -1.0, 0.0], [2.0, 1.0, 0.2]]
    # The centroid is inside, but a finite toe enters void.
    assert abs(p.mean(axis=0)[1]) < 0.25
    with pytest.raises(ValueError, match="physical slab"):
        material_containment(p, cells, roads, bounds)
    wide = [dict(roads[0], width=2.0)]
    assert material_containment(p, cells, wide, bounds) == 1
    moving = [dict(roads[0], width=2.0, upper=0.1), dict(roads[0], lower=0.1)]
    with pytest.raises(ValueError, match="physical slab"):
        material_containment(p, cells, moving, bounds)


def test_same_side_overlapping_tetrahedra_cannot_share_a_face():
    p = np.array([[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1], [0, 0, 2]], dtype=float)
    with pytest.raises(ValueError, match="opposite orientation"):
        face_mesh(p, np.array([[0, 1, 2, 3], [0, 1, 2, 4]]))


def test_nonfinite_coordinates_and_invalid_connectivity_refuse():
    p = np.array([[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1]], dtype=float)
    for cells in (
        np.array([[0, 1, 2, -1]]),
        np.array([[0, 1, 2, 4]]),
        np.array([[0.0, 1.0, 2.0, 3.0]]),
    ):
        with pytest.raises(ValueError, match="invalid tetrahedral"):
            face_mesh(p, cells)
    for value in (float("nan"), float("inf")):
        bad = p.copy()
        bad[0, 0] = value
        with pytest.raises(ValueError, match="invalid tetrahedral"):
            face_mesh(bad, np.array([[0, 1, 2, 3]]))


def test_exact_oriented_boundary_rejects_equal_volume_relocation_and_reversal():
    p = np.array([[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1]], dtype=float)
    t = np.array([[0, 1, 2, 3]])
    boundary = boundary_inventory(t)
    conserve_boundary(p, boundary, p, t)
    conserve_boundary(p, boundary[:, [1, 2, 0]], p, t)  # cyclic rotation is identical
    moved = p.copy()
    moved[0, 0] = np.nextafter(0.0, 1.0)
    with pytest.raises(ValueError, match="coordinates"):
        conserve_boundary(p, boundary, moved, t)
    with pytest.raises(ValueError, match="oriented source boundary"):
        conserve_boundary(p, boundary[:, ::-1], p, t)
    extra = np.r_[p, p + 2]
    with pytest.raises(ValueError, match="oriented source boundary"):
        conserve_boundary(p, boundary, extra, t + 4)


def test_exact_boundary_survives_interior_subdivision():
    p = np.array([[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1]], dtype=float)
    t = np.array([[0, 1, 2, 3]])
    points = np.r_[p, p.mean(axis=0)[None]]
    cells, _ = oriented_cells(points, np.c_[boundary_inventory(t), np.full(4, 4)])
    conserve_boundary(p, boundary_inventory(t), points, cells)
    assert face_mesh(points, cells, expected_volume=1 / 6).components == 1


def test_source_prism_budget_is_explicit_and_does_not_relax_connectivity(
    tmp_path, monkeypatch
):
    import tetgen_mesh as t
    from types import SimpleNamespace

    monkeypatch.setattr(t.shutil, "which", lambda _: "/not-invoked")
    mesh = SimpleNamespace(cell_kind="wedge", count=306_882, components=1)
    with pytest.raises(ValueError, match="limit=250000"):
        t.remesh(mesh, [], [], tmp_path / "mesh")
    mesh.components = 2
    with pytest.raises(ValueError, match="components=2"):
        t.remesh(mesh, [], [], tmp_path / "mesh", max_source_prisms=400_000)
    for invalid in (True, 0, 1_000_001):
        with pytest.raises(ValueError, match="resource bound"):
            t.remesh(mesh, [], [], tmp_path / "mesh", max_source_prisms=invalid)
