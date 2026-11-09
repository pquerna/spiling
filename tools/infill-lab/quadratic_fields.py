# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Bounded solved-field diagnostics on the unchanged quadratic physical operator.

Energy localization is an observation, not a discretization-error estimator.
No smoothing, re-solving, node welding or new material/contact law is applied.
"""

from __future__ import annotations
import time
import numpy as np


def energy_fields(body, displacement, expected_energy, *, bins=16, chunk=1024):
    from quadratic import kernels

    if type(bins) is not int or not 2 <= bins <= 32:
        raise ValueError("field bins must be an integer between 2 and 32")
    if type(chunk) is not int or not 1 <= chunk <= 8192:
        raise ValueError("field chunks must be bounded")
    if body.mesh.cell_kind != "tet_p2":
        raise ValueError("quadratic fields require the retained TetP2 operator")
    u = np.asarray(displacement)
    if u.shape != (body.basis.N,) or not np.all(np.isfinite(u)):
        raise ValueError("field displacement inventory differs")
    if not np.isfinite(expected_energy) or expected_energy <= 0:
        raise ValueError("positive checked global energy required")
    template, _ = kernels(body.material)
    lo, hi = body.mesh.bounds
    extent = hi - lo
    if np.any(~np.isfinite(extent)) or np.any(extent <= 0):
        raise ValueError("positive field extents required")
    energy_grid = np.zeros((bins,) * 3)
    volume_grid = np.zeros_like(energy_grid)
    minimum_energy = float("inf")
    total = 0.0
    began = time.monotonic()
    for start in range(0, body.mesh.count, chunk):
        cells = body.mesh.cells[:, start : start + chunk].T
        corners = body.mesh.points[:, cells[:, :4]].transpose(1, 2, 0)
        jacobian = (corners[:, 1:] - corners[:, :1]).transpose(0, 2, 1)
        determinant = np.linalg.det(jacobian)
        inverse = np.linalg.inv(jacobian)
        scales = np.einsum("era,esb,e->erasb", inverse, inverse, determinant).reshape(
            -1, 81
        )
        values = (scales @ template).reshape(-1, 30, 30)
        indices = (3 * cells[:, :, None] + np.arange(3)).reshape(-1, 30)
        local = u[indices]
        energies = 0.5 * np.einsum("ei,eij,ej->e", local, values, local)
        if (
            not np.all(np.isfinite(energies))
            or np.min(energies) < -expected_energy * 1e-10
        ):
            raise ValueError("invalid localized quadratic energy")
        minimum_energy = min(minimum_energy, float(np.min(energies)))
        total += float(energies.sum())
        # Centroid assignment is explicit; these are integrated element bins,
        # not an interpolated stress image or an a posteriori error field.
        points = corners.mean(axis=1)
        ids = np.clip(((points - lo) / extent * bins).astype(int), 0, bins - 1)
        np.add.at(energy_grid, tuple(ids.T), energies)
        np.add.at(volume_grid, tuple(ids.T), determinant / 6)
    relative_error = abs(total / expected_energy - 1)
    if relative_error > 1e-7:
        raise ValueError(f"localized/global energy differs: {relative_error}")
    volume_error = abs(float(volume_grid.sum()) / body.mesh.volume - 1)
    if volume_error > 1e-9:
        raise ValueError("localized field volume differs")
    return dict(
        kind="integrated_element_centroid_bins_not_error_estimate",
        bounds_mm=body.mesh.bounds.tolist(),
        bins=bins,
        energy_n_mm=energy_grid.tolist(),
        volume_mm3=volume_grid.tolist(),
        total_energy_n_mm=total,
        global_energy_relative_error=relative_error,
        volume_relative_error=volume_error,
        minimum_element_energy_n_mm=minimum_energy,
        displacement_min_mm=u.reshape(-1, 3).min(axis=0).tolist(),
        displacement_max_mm=u.reshape(-1, 3).max(axis=0).tolist(),
        field_capture_s=time.monotonic() - began,
    )
