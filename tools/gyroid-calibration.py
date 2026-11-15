#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
"""Numerically integrate the Gyroid nodal field's bulk contour length.

Independent density calibration, not a mechanical model. No project imports or
third-party dependencies. Midpoint quadrature avoids sampling singular saddle
heights themselves. Increase both sample counts to inspect convergence.
"""
import argparse
import json
import math
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--z-samples", type=int, default=256)
parser.add_argument("--u-samples", type=int, default=1024)
args = parser.parse_args()
if min(args.z_samples, args.u_samples) < 1:
    parser.error("sample counts must be positive")
started = time.perf_counter()
lengths = []
for zi in range(args.z_samples):
    z = math.tau * (zi + 0.5) / args.z_samples
    sz, cz = math.sin(z), math.cos(z)
    along_x = abs(cz) >= abs(sz)
    length = 0.0
    for ui in range(args.u_samples):
        u = math.tau * (ui + 0.5) / args.u_samples
        su, cu = math.sin(u), math.cos(u)
        if along_x:
            phi = math.atan(su / cz)
            root = math.asin(max(-1, min(1, -sz * cu * math.copysign(1, cz) / math.hypot(su, cz))))
            roots = (root - phi, math.pi - root - phi)
        else:
            phi = math.atan(cu / sz)
            root = math.acos(max(-1, min(1, -cz * su * math.copysign(1, sz) / math.hypot(cu, sz))))
            roots = (phi + root, phi - root)
        for v in roots:
            x, y = (u, v) if along_x else (v, u)
            # Implicit differentiation, independent of the root derivative.
            fx = math.cos(x) * math.cos(y) - sz * math.sin(x)
            fy = -math.sin(x) * math.sin(y) + math.cos(y) * cz
            slope = -fx / fy if along_x else -fy / fx
            length += math.hypot(1, slope)
    lengths.append(length / (args.u_samples * math.tau))
mean = math.fsum(lengths) / args.z_samples
print(json.dumps({"z_samples": args.z_samples, "u_samples": args.u_samples,
                  "bulk_length_per_area_at_unit_frequency": mean,
                  "density_multiplier": 1 / mean,
                  "min_layer_length_per_area": min(lengths),
                  "max_layer_length_per_area": max(lengths),
                  "elapsed_s": time.perf_counter() - started}, indent=2))
