// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Whole-domain plane tolerance classification, not exact coincident-material
//! authority. Face-use orientation is always supplied by the topology owner.
use super::{Interval, analytic_eval::surface_order};
use crate::{GeometryError, GeometryWorkBudget};
use solidcore::surface::{surface::Surface, surface::p2::SurfaceKind};
use spiling_contracts::geometry::{GeometryErrorCode, PlaneMm};
use std::sync::atomic::AtomicBool;
pub const COPLANE_ANGULAR_TOLERANCE: f64 = 1e-10;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoplaneClassification {
    Unresolved,
    Transverse,
    ParallelDisjoint,
    CoplanarSameNormal,
    CoplanarOppositeNormal,
}
impl CoplaneClassification {
    pub fn is_coplanar(self) -> bool {
        matches!(
            self,
            Self::CoplanarSameNormal | Self::CoplanarOppositeNormal
        )
    }
    pub fn is_parallel(self) -> bool {
        matches!(
            self,
            Self::ParallelDisjoint | Self::CoplanarSameNormal | Self::CoplanarOppositeNormal
        )
    }
}
pub fn classify_face_coplane(
    face_orientation: bool,
    surface: &Surface,
    plane: &PlaneMm,
    angular_tolerance: f64,
    distance_tolerance: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<CoplaneClassification, GeometryError> {
    work.charge(1, cancel)?;
    plane.validate()?;
    if !angular_tolerance.is_finite()
        || !(0.0..1.0).contains(&angular_tolerance)
        || !distance_tolerance.is_finite()
        || distance_tolerance < 0.0
    {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "invalid plane classification tolerances",
        ));
    }
    let SurfaceKind::Plane(p) = surface.kind() else {
        return Ok(CoplaneClassification::Unresolved);
    };
    let domain = surface.natural_uv_domain();
    let lo = domain.min();
    let hi = domain.max();
    let j = surface_order(surface, [[lo.x, hi.x], [lo.y, hi.y]], 1, work, cancel)?;
    let n = plane.normal.map(Interval::point);
    let length = n
        .into_iter()
        .fold(Interval::point(0.0), |s, a| s.add(a.square()))
        .sqrt()?;
    let normal = super::cross(j.du, j.dv);
    let normal_length = normal
        .into_iter()
        .fold(Interval::point(0.0), |s, a| s.add(a.square()))
        .sqrt()?;
    if normal_length.lo <= 0.0 {
        return Err(GeometryError::new(
            GeometryErrorCode::SingularGeometry,
            "degenerate affine surface normal",
        ));
    }
    let cross = super::cross(normal, n);
    let sine = cross
        .into_iter()
        .fold(Interval::point(0.0), |s, a| s.add(a.square()))
        .sqrt()?
        .div(length.mul(normal_length))?;
    if sine.lo > angular_tolerance {
        return Ok(CoplaneClassification::Transverse);
    }
    if sine.hi > angular_tolerance {
        return Ok(CoplaneClassification::Unresolved);
    }
    let distance = (0..3)
        .fold(Interval::point(0.0), |s, i| {
            s.add(n[i].mul(j.position[i].sub(Interval::point(plane.origin_mm[i]))))
        })
        .div(length)?;
    if distance.abs_lower() > distance_tolerance {
        return Ok(CoplaneClassification::ParallelDisjoint);
    }
    if distance.abs_upper() > distance_tolerance {
        return Ok(CoplaneClassification::Unresolved);
    }
    let sign = if face_orientation { 1.0 } else { -1.0 };
    // The normal is the actual chart cross product, not placement Z: signed UV
    // scales can reverse it without changing the geometric plane.
    let dot = (0..3)
        .fold(Interval::point(0.0), |s, i| s.add(normal[i].mul(n[i])))
        .mul(Interval::point(sign));
    let _ = p;
    if dot.lo > 0.0 {
        Ok(CoplaneClassification::CoplanarSameNormal)
    } else if dot.hi < 0.0 {
        Ok(CoplaneClassification::CoplanarOppositeNormal)
    } else {
        Ok(CoplaneClassification::Unresolved)
    }
}
