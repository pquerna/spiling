// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A normal identity for an explicitly selected native STEP sphere carrier.
//! This grants no endpoint attachment, pole fan or material degree. Those are
//! independently proved from native edgeuses and the complete boundary atlas.
//! A bare native sphere keeps its actual rational NURBS evaluation law.

use super::*;
use solidcore::surface::surface::{Surface, p2::SurfaceKind};

pub(super) struct SphereChart {
    center: [Interval; 3],
    cofactors: [[Interval; 3]; 3],
    determinant: Interval,
    reversed: bool,
    outward: bool,
}
fn explicit_step_sphere(surface: &Surface) -> Option<&solidcore::surface::sphere::Sphere> {
    let mut carrier = surface;
    let mut explicit = false;
    for _ in 0..32 {
        match carrier.kind() {
            SurfaceKind::CurveBounded(bound) => carrier = bound.basis(),
            SurfaceKind::Step(step) => {
                carrier = step.surface()?;
                explicit = true;
            }
            SurfaceKind::Sphere(sphere) if explicit => return Some(sphere),
            _ => return None,
        }
    }
    None
}
impl SphereChart {
    pub(super) fn new(
        surface: &(impl SurfaceEvaluator + ?Sized),
        domain: &PreparedFaceTrimDomain<'_>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        work.charge(1, cancel)?;
        let selected = domain.surface_operand();
        let (Some(candidate), Some(owner)) = (surface.native_surface(), selected.native_surface())
        else {
            return Ok(None);
        };
        if !domain.is_native_material() || !std::ptr::eq(candidate, owner) {
            return Err(GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "native normal identity is not bound to its borrowed material carrier",
            ));
        }
        let Some(sphere) = explicit_step_sphere(candidate) else {
            return Ok(None);
        };
        let placement = sphere.base().position();
        let p = placement.origin();
        let x = placement.x_axis();
        let y = placement.y_axis();
        let z = placement.z_axis();
        let center = [p.x, p.y, p.z];
        let axes = [[x.x, x.y, x.z], [y.x, y.y, y.z], [z.x, z.y, z.z]];
        let radius = sphere.radius();
        if !radius.is_finite()
            || radius <= 0.0
            || center
                .into_iter()
                .chain(axes.into_iter().flatten())
                .any(|v| !v.is_finite())
        {
            return Ok(None);
        }
        let columns =
            axes.map(|axis| axis.map(|v| Interval::point(v).mul(Interval::point(radius))));
        let cofactors = [
            cross(columns[1], columns[2]),
            cross(columns[2], columns[0]),
            cross(columns[0], columns[1]),
        ];
        let determinant = dot(columns[0], cofactors[0]);
        if !determinant.lo.is_finite() || !determinant.hi.is_finite() || determinant.contains_zero()
        {
            return Ok(None);
        }
        let reversed = !domain.orientation();
        Ok(Some(Self {
            center: center.map(Interval::point),
            cofactors,
            determinant,
            reversed,
            outward: (determinant.lo > 0.0) != reversed,
        }))
    }
    pub(super) fn normal(&self, position: [Interval; 3]) -> Result<[Interval; 3], GeometryError> {
        let relative = std::array::from_fn(|i| position[i].sub(self.center[i]));
        let mut local = [Interval::point(0.0); 3];
        for (i, value) in local.iter_mut().enumerate() {
            *value = dot(self.cofactors[i], relative).div(self.determinant)?;
        }
        let n: [Interval; 3] = std::array::from_fn(|row| {
            (0..3).fold(Interval::point(0.0), |sum, column| {
                sum.add(self.cofactors[column][row].mul(local[column]))
            })
        });
        Ok(if self.reversed {
            n.map(Interval::neg)
        } else {
            n
        })
    }
    pub(super) fn convex(&self) -> bool {
        self.outward
    }
}
