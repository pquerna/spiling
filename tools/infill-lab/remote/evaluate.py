# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Frozen FP64 sparse-system comparison; no geometry or material authority."""

from __future__ import annotations
import argparse
import hashlib
import importlib.metadata
import json
from pathlib import Path
import platform
import resource
import time

import numpy as np
from scipy import sparse
from scipy.sparse.linalg import spsolve


def file_hash(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def load_case(folder):
    manifest_path = folder / "case.json"
    if manifest_path.stat().st_size > 65536:
        raise ValueError("oversized case manifest")
    manifest = json.loads(manifest_path.read_text())
    files = manifest["files"]
    expected = {"matrix.npz", "vectors.npz"}
    if manifest.get("physical"):
        expected |= {"stiffness.npz", "fixture.npz"}
    if manifest["schema"] != 1 or set(files) != expected:
        raise ValueError("unknown sparse case schema or file inventory")
    for name, sha in files.items():
        path = folder / name
        if path.is_symlink() or file_hash(path) != sha:
            raise ValueError("case hash mismatch: " + name)
    matrix = sparse.load_npz(folder / "matrix.npz").tocsr()
    matrix.sum_duplicates()
    matrix.sort_indices()
    with np.load(folder / "vectors.npz", allow_pickle=False) as arrays:
        rhs, reference = arrays["rhs"], arrays["reference"]
    n = matrix.shape[0]
    if (
        n == 0
        or n > 10_000_000
        or matrix.shape != (n, n)
        or matrix.dtype != np.float64
        or any(a.shape != (n,) or a.dtype != np.float64 for a in (rhs, reference))
        or not all(np.all(np.isfinite(a)) for a in (matrix.data, rhs, reference))
        or np.any(matrix.diagonal() <= 0)
        or np.linalg.norm(rhs) == 0
    ):
        raise ValueError("invalid finite FP64 sparse system")
    difference = matrix - matrix.T
    if difference.nnz and np.max(abs(difference.data)) > 1e-12 * np.max(
        abs(matrix.data)
    ):
        raise ValueError("asymmetric sparse system")
    reference_residual = np.linalg.norm(matrix @ reference - rhs) / np.linalg.norm(rhs)
    if reference_residual > 2e-8:
        raise ValueError("invalid reference solution")
    return manifest, matrix, rhs, reference


def warp_solve(matrix, rhs):
    import warp as wp
    from warp import sparse as ws
    from warp.optim.linear import cg, preconditioner

    wp.config.kernel_cache_dir = "/cache/warp"
    wp.init()
    if not wp.is_cuda_available():
        raise RuntimeError("CUDA unavailable; GPU comparison refuses CPU fallback")
    if matrix.nnz > np.iinfo(np.int32).max:
        raise ValueError("Warp COO index limit exceeded")
    device = wp.get_device("cuda:0")
    started = time.monotonic()
    with wp.ScopedDevice(device):
        coo = matrix.tocoo()
        operator = ws.bsr_zeros(matrix.shape[0], matrix.shape[1], wp.float64)
        ws.bsr_set_from_triplets(
            operator,
            wp.array(coo.row.astype(np.int32), dtype=wp.int32),
            wp.array(coo.col.astype(np.int32), dtype=wp.int32),
            wp.array(coo.data, dtype=wp.float64),
        )
        b = wp.array(rhs, dtype=wp.float64)
        x = wp.zeros(len(rhs), dtype=wp.float64)
        conditioner = preconditioner(operator, "diag")
        wp.synchronize_device(device)
        setup_s = time.monotonic() - started
        started = time.monotonic()
        result = cg(operator, b, x, tol=1e-10, atol=0.0, maxiter=10000, M=conditioner)
        wp.synchronize_device(device)
        solve_s = time.monotonic() - started
        solution = x.numpy()
    return solution, {
        "device": str(device),
        "device_name": device.name,
        "setup_transfer_jit_s": setup_s,
        "solve_s": solve_s,
        "solver_result": [float(v) for v in result],
        "preconditioner": "diagonal",
    }


def physical_checks(folder, solution):
    stiffness = sparse.load_npz(folder / "stiffness.npz").tocsr()
    with np.load(folder / "fixture.npz", allow_pickle=False) as data:
        free, fixed, values, scaling, loaded = (
            data[k] for k in ("free", "fixed", "values", "scaling", "loaded")
        )
        delta = float(data["delta"])
    n = stiffness.shape[0]
    if (
        stiffness.shape != (n, n)
        or n % 3
        or delta <= 0
        or not np.isfinite(delta)
        or len(free) != len(solution)
        or scaling.shape != solution.shape
        or values.shape != fixed.shape
        or not np.array_equal(np.sort(np.r_[free, fixed]), np.arange(n))
        or not np.all(np.isin(loaded, fixed))
        or not all(np.all(np.isfinite(a)) for a in (values, scaling, stiffness.data))
        or np.any(scaling <= 0)
    ):
        raise ValueError("invalid captured physical fixture")
    boundary = np.zeros(n)
    boundary[fixed] = values
    u = boundary.copy()
    u[free] = scaling * solution
    force = stiffness @ u
    residual = np.linalg.norm(force[free]) / max(
        np.linalg.norm((stiffness @ boundary)[free]), 1e-30
    )
    reaction = -float(force[loaded].sum())
    energy = float(0.5 * u @ force)
    balance = np.linalg.norm(force.reshape(-1, 3).sum(axis=0)) / max(reaction, 1e-30)
    energy_error = abs(energy - 0.5 * reaction * delta) / max(abs(energy), 1e-30)
    if (
        not all(
            np.isfinite(v) for v in (reaction, energy, residual, balance, energy_error)
        )
        or reaction <= 0
        or energy <= 0
        or residual > 2e-8
        or balance > 1e-6
        or energy_error > 1e-6
    ):
        raise ValueError(
            f"physical replay gate: residual={residual}, balance={balance}, energy={energy_error}"
        )
    return {
        "relative_residual": float(residual),
        "reaction_n": reaction,
        "stiffness_n_mm": reaction / delta,
        "strain_energy_n_mm": energy,
        "force_balance": float(balance),
        "energy_error": float(energy_error),
    }


def evaluate(folder, backend):
    started = time.monotonic()
    manifest, matrix, rhs, reference = load_case(folder)
    if backend == "scipy":
        began = time.monotonic()
        solution = spsolve(matrix.tocsc(), rhs)
        timing = {"solve_s": time.monotonic() - began, "device": "cpu"}
    else:
        solution, timing = warp_solve(matrix, rhs)
    residual = float(np.linalg.norm(matrix @ solution - rhs) / np.linalg.norm(rhs))
    error = float(
        np.linalg.norm(solution - reference) / max(np.linalg.norm(reference), 1e-30)
    )
    energy = float(0.5 * solution @ (matrix @ solution))
    reference_energy = float(0.5 * reference @ (matrix @ reference))
    energy_error = abs(energy - reference_energy) / max(abs(reference_energy), 1e-30)
    if (
        not all(np.isfinite(v) for v in (residual, error, energy, energy_error))
        or residual > 2e-8
        or error > 1e-5
        or energy_error > 1e-6
    ):
        raise ValueError(
            f"solver gate: residual={residual}, solution={error}, energy={energy_error}"
        )
    return {
        "status": "ok",
        "scope": "frozen_sparse_backend_comparison_not_strength_qualification",
        "case_sha256": file_hash(folder / "case.json"),
        "provenance": manifest.get("provenance"),
        "backend": backend,
        "precision": "float64",
        "unknowns": matrix.shape[0],
        "nonzeros": matrix.nnz,
        "relative_residual": residual,
        "relative_reference_error": error,
        "relative_energy_error": energy_error,
        "physical": physical_checks(folder, solution)
        if manifest.get("physical")
        else None,
        "timing": timing,
        "wall_s": time.monotonic() - started,
        "peak_process_rss_kib": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
        "architecture": platform.machine(),
        "packages": {
            p: importlib.metadata.version(p) for p in ("numpy", "scipy", "warp-lang")
        },
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--input", type=Path, default=Path("/input"))
    p.add_argument("--output", type=Path, default=Path("/output"))
    p.add_argument(
        "--backend",
        choices=("scipy", "warp", "coupon", "reference", "quadratic"),
        required=True,
    )
    args = p.parse_args()
    try:
        if args.backend == "quadratic":
            from quadratic import evaluate as evaluate_quadratic

            result = evaluate_quadratic(args.input, args.output)
        elif args.backend == "reference":
            from reference import evaluate as evaluate_reference

            result = evaluate_reference(args.input, args.output)
        elif args.backend == "coupon":
            from coupon import evaluate as evaluate_coupon

            result = evaluate_coupon(args.input, args.output)
        else:
            result = evaluate(args.input, args.backend)
    except Exception as error:
        result = {"status": "failed", "backend": args.backend, "error": str(error)}
        (args.output / "result.json").write_text(
            json.dumps(result, indent=2, allow_nan=False) + "\n"
        )
        raise
    (args.output / "result.json").write_text(
        json.dumps(result, indent=2, allow_nan=False) + "\n"
    )
    print(json.dumps(result, allow_nan=False), flush=True)
    if args.backend in ("reference", "quadratic") and result["status"] != "ok":
        raise RuntimeError(
            "independent element scenarios failed; retained in trials.jsonl"
        )


if __name__ == "__main__":
    main()
