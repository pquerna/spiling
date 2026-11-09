# SPDX-FileCopyrightText: 2026 Spiling contributors
# SPDX-License-Identifier: OSL-3.0
# Licensed under the Open Software License version 3.0
"""Immutable research inputs; no project, kernel or manufacturing authority."""

from __future__ import annotations
from dataclasses import asdict, dataclass
import hashlib
import json
import math
from pathlib import Path

SCHEMA = 1


def digest(value) -> str:
    return hashlib.sha256(
        json.dumps(
            value, sort_keys=True, separators=(",", ":"), allow_nan=False
        ).encode()
    ).hexdigest()


def file_hash(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


@dataclass(frozen=True)
class Candidate:
    family: str = "uniform"
    fraction: float = 0.2
    axis: str = "alternating_xy"
    cut: float = 0.7
    region_fraction: float = 0.6
    plate: float = 0.2
    layer_height: float = 0.2
    combine_height: float = 0.0
    pattern: str = "rectilinear"
    roof_spacing_mm: float | None = None

    def __post_init__(self):
        if self.pattern not in {"rectilinear", "grid", "web_roof"}:
            raise ValueError("unknown production infill pattern")
        if self.pattern in {"grid", "web_roof"} and (
            self.family != "uniform" or self.combine_height
        ):
            raise ValueError("two-axis study requires uniform fine-cadence input")
        if self.pattern == "web_roof" and self.fraction == 1:
            raise ValueError("web + roof requires sparse base webs")
        if self.roof_spacing_mm is not None and (
            self.pattern != "web_roof"
            or type(self.roof_spacing_mm) not in (int, float)
            or not math.isfinite(self.roof_spacing_mm)
            or not 0.45 <= self.roof_spacing_mm <= 7.5
        ):
            raise ValueError(
                "roof spacing outside the captured 0.45–7.5 mm study envelope"
            )
        if self.family not in {"uniform", "crown", "flanges", "separator"}:
            raise ValueError("unknown construction family")
        if self.axis not in {"x", "y", "alternating_xy", "alternating_yx"}:
            raise ValueError("unknown hatch axis")
        if not all(
            math.isfinite(x)
            for x in (
                self.fraction,
                self.cut,
                self.region_fraction,
                self.plate,
                self.layer_height,
                self.combine_height,
            )
        ):
            raise ValueError("nonfinite candidate")
        if not (
            0 < self.fraction <= 1
            and 0 < self.region_fraction <= 1
            and 0.1 <= self.cut <= 0.9
            and 0.1 <= self.plate <= 0.8
        ):
            raise ValueError("candidate outside declared grammar")
        if self.layer_height not in (0.1, 0.2) or self.combine_height not in (0.0, 0.3):
            raise ValueError("unsupported study cadence")
        if self.family == "flanges" and self.cut <= 0.5:
            raise ValueError("flange bands must be disjoint")
        if self.family == "separator" and self.axis not in {"x", "y"}:
            raise ValueError("separator needs an explicit aligned hatch axis")

    def data(self):
        data = asdict(self)
        if self.roof_spacing_mm is None:
            data.pop("roof_spacing_mm")
        if self.family == "uniform":
            for key in ("cut", "region_fraction", "plate"):
                data.pop(key)
        elif self.family == "separator":
            data.pop("plate")
        return data

    def sparse_policy(self):
        """Inert production settings; the engine independently admits the recipe."""
        if self.combine_height:
            return {"kind": "combine_eligible", "max_height_mm": self.combine_height}
        if self.pattern == "web_roof":
            return {"kind": "web_roof", "roof_spacing_mm": self.roof_spacing_mm}
        return {"kind": {"grid": "grid", "rectilinear": "fine"}[self.pattern]}

    @property
    def identity(self):
        return digest(self.data())[:16]


@dataclass(frozen=True)
class Material:
    young_mpa: float = 2300.0
    poisson: float = 0.35
    density_g_mm3: float = 0.00124
    z_ratio: float = 1.0
    status: str = "assumed_elastic_not_strength_calibration"

    def __post_init__(self):
        if not (
            math.isfinite(self.young_mpa)
            and self.young_mpa > 0
            and -1 < self.poisson < 0.5
            and math.isfinite(self.poisson)
            and math.isfinite(self.density_g_mm3)
            and self.density_g_mm3 > 0
            and math.isfinite(self.z_ratio)
            and 0 < self.z_ratio <= 1.0
        ):
            raise ValueError("invalid linear elastic material")
