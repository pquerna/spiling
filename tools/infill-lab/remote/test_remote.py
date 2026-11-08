# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Consumer gates for inert cases and bounded disposable jobs."""

import importlib.util
import json
from pathlib import Path
import numpy as np
import pytest
from scipy import sparse


def module(name):
    spec = importlib.util.spec_from_file_location(
        name, Path(__file__).parent / (name + ".py")
    )
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def fixture_case(folder):
    evaluator = module("evaluate")
    stiffness = sparse.kron(
        sparse.diags([-np.ones(2), [1, 2, 1], -np.ones(2)], [-1, 0, 1]), sparse.eye(3)
    ).tocsr()
    free = np.array([3, 4, 5])
    fixed = np.array([0, 1, 2, 6, 7, 8])
    values = np.array([0.0, 0.0, 0.0, 0.0, 0.0, -0.02])
    scaling = np.repeat(1 / np.sqrt(2), 3)
    matrix = sparse.eye(3, format="csr", dtype=np.float64)
    rhs = np.array([0.0, 0.0, -0.02 / np.sqrt(2)])
    sparse.save_npz(folder / "matrix.npz", matrix)
    sparse.save_npz(folder / "stiffness.npz", stiffness)
    np.savez(folder / "vectors.npz", rhs=rhs, reference=rhs)
    np.savez(
        folder / "fixture.npz",
        free=free,
        fixed=fixed,
        values=values,
        scaling=scaling,
        loaded=np.array([8]),
        delta=0.02,
    )
    files = {
        n: evaluator.file_hash(folder / n)
        for n in ("matrix.npz", "vectors.npz", "stiffness.npz", "fixture.npz")
    }
    (folder / "case.json").write_text(
        json.dumps({"schema": 1, "physical": True, "files": files})
    )
    return evaluator


def test_frozen_solver_retains_physical_units_energy_and_balance(tmp_path):
    evaluator = fixture_case(tmp_path)
    manifest, matrix, rhs, reference = evaluator.load_case(tmp_path)
    result = evaluator.physical_checks(tmp_path, reference)
    assert result["reaction_n"] == pytest.approx(0.01)
    assert result["stiffness_n_mm"] == pytest.approx(0.5)
    assert result["energy_error"] < 1e-12 and result["force_balance"] < 1e-12
    with pytest.raises(ValueError, match="physical replay gate"):
        evaluator.physical_checks(tmp_path, reference * 0.9)


def test_corrupted_input_and_invented_reference_refuse(tmp_path):
    evaluator = fixture_case(tmp_path)
    with (tmp_path / "vectors.npz").open("ab") as f:
        f.write(b"changed")
    with pytest.raises(ValueError, match="hash mismatch"):
        evaluator.load_case(tmp_path)
    evaluator = fixture_case(tmp_path)
    np.savez(tmp_path / "vectors.npz", rhs=np.ones(3), reference=np.zeros(3))
    manifest = json.loads((tmp_path / "case.json").read_text())
    manifest["files"]["vectors.npz"] = evaluator.file_hash(tmp_path / "vectors.npz")
    (tmp_path / "case.json").write_text(json.dumps(manifest))
    with pytest.raises(ValueError, match="reference solution"):
        evaluator.load_case(tmp_path)


def test_limits_follow_machine_and_current_available_memory():
    launcher = module("launch")
    machine = {
        "cpus": 20,
        "memory_available": 100 * 1024**3,
        "memory_total": 128 * 1024**3,
    }
    budget = launcher.limits(machine)
    assert budget["cpus"] == 15 and budget["memory_bytes"] == 75 * 1024**3
    assert launcher.limits(machine, 40, 8)["memory_bytes"] == 40 * 1024**3
    assert launcher.limits(machine, 94, 8)["memory_bytes"] == 94 * 1024**3
    constrained = {**machine, "memory_available": 50 * 1024**3}
    with pytest.raises(ValueError):
        launcher.limits(constrained, 48, 8)
    for memory, cpus in [(101, 8), (1, 21), (float("nan"), 8), (1, float("nan"))]:
        with pytest.raises((ValueError, OverflowError)):
            launcher.limits(machine, memory, cpus)


def test_blas_team_is_independent_of_cpu_allocation_and_bounded():
    launcher = module("launch")
    assert launcher.blas_threads(12, 1) == 1
    assert launcher.blas_threads(12) == 12
    assert launcher.blas_threads(0.5) == 1
    for count in (0, -1, 13, 1.5, True):
        with pytest.raises(ValueError, match="BLAS threads"):
            launcher.blas_threads(12, count)
